//! The privileged helper as the app sees it: started with `pkexec` on Unlock,
//! asked one request at a time, and gone on Lock or with the window (its stdin
//! closes). See `conflux_core::helper`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

use conflux_core::helper::{HelperRequest, HelperResponse};
use futures_channel::oneshot;
use gtk::{gio, glib, prelude::*};

/// Where the helper is installed: the polkit policy names this path. The
/// Makefile builds the app with its own (`CONFLUX_HELPER_PATH`).
const INSTALLED: &str = match option_env!("CONFLUX_HELPER_PATH") {
    Some(path) => path,
    None => "/usr/lib/conflux/conflux-helper",
};

type Reply = oneshot::Sender<Result<HelperResponse, String>>;

pub struct Helper {
    process: gio::Subprocess,
    input: gio::OutputStream,
    /// Answers come in the order of the requests.
    pending: RefCell<VecDeque<Reply>>,
}

/// How to start the helper: `pkexec` with the helper next to this app (a build
/// from source) or the installed one. `CONFLUX_HELPER_COMMAND` replaces it, for
/// testing without root.
fn command() -> Vec<OsString> {
    if let Some(cmd) = std::env::var("CONFLUX_HELPER_COMMAND")
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        return cmd.split_whitespace().map(OsString::from).collect();
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("conflux-helper")))
        .filter(|path| path.exists());
    let helper = beside.unwrap_or_else(|| PathBuf::from(INSTALLED));
    vec!["pkexec".into(), helper.into()]
}

impl Helper {
    /// Start the helper, which asks for authentication, and wait until it
    /// answers. `on_exit` runs when it is gone again, however that happens.
    pub async fn start(on_exit: impl FnOnce() + 'static) -> Result<Rc<Helper>, String> {
        let argv = command();
        let argv: Vec<&std::ffi::OsStr> = argv.iter().map(OsString::as_os_str).collect();
        let process = gio::Subprocess::newv(
            &argv,
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        )
        .map_err(|e| format!("Could not start the helper: {}", e.message()))?;
        let (Some(input), Some(output)) = (process.stdin_pipe(), process.stdout_pipe()) else {
            return Err("Could not talk to the helper".to_string());
        };
        let helper = Rc::new(Helper {
            process: process.clone(),
            input,
            pending: RefCell::default(),
        });

        let lines = gio::DataInputStream::new(&output);
        let weak = Rc::downgrade(&helper);
        glib::spawn_future_local(async move {
            while let Ok(Some(line)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
                let Some(helper) = weak.upgrade() else {
                    return;
                };
                let reply = helper.pending.borrow_mut().pop_front();
                if let Some(reply) = reply {
                    let _ = reply.send(serde_json::from_str(&line).map_err(|e| e.to_string()));
                }
            }
            if let Some(helper) = weak.upgrade() {
                for reply in helper.pending.borrow_mut().drain(..) {
                    let _ = reply.send(Err("The helper is gone; unlock again".to_string()));
                }
            }
            on_exit();
        });

        match helper.call(&HelperRequest::Hello).await {
            Ok(_) => Ok(helper),
            Err(_) => {
                let _ = process.wait_future().await;
                // pkexec: 126 when the prompt was dismissed, 127 when it failed.
                Err(match process.exit_status() {
                    126 => "Unlocking was cancelled".to_string(),
                    127 => "Could not authenticate".to_string(),
                    status => format!("The helper could not start (exit status {status})"),
                })
            }
        }
    }

    /// Ask the helper something and wait for its answer.
    pub async fn call(&self, request: &HelperRequest) -> Result<HelperResponse, String> {
        let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        line.push('\n');
        let (reply, answer) = oneshot::channel();
        self.pending.borrow_mut().push_back(reply);
        // A whole line at once, so requests never interleave; the helper reads
        // all the time, so this does not wait.
        if let Err(e) = self
            .input
            .write_all(line.as_bytes(), gio::Cancellable::NONE)
            .and_then(|_| self.input.flush(gio::Cancellable::NONE))
        {
            self.pending.borrow_mut().pop_back();
            return Err(format!("Could not talk to the helper: {}", e.message()));
        }
        answer
            .await
            .unwrap_or_else(|_| Err("The helper is gone; unlock again".to_string()))
    }

    /// Let the helper go: it exits once its stdin closes.
    pub fn stop(&self) {
        if self.input.close(gio::Cancellable::NONE).is_err() {
            self.process.force_exit();
        }
    }
}
