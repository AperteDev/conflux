//! `conflux-helper`: what the conflux GTK app does as root, once unlocked.
//!
//! The app starts it with `pkexec` (one password prompt per unlock), then sends
//! newline-delimited JSON requests on its stdin and reads the answers on its
//! stdout (see `conflux_core::helper`). It only writes the system config, acts
//! on the system daemons' units, passes requests to their sockets, and deletes
//! a daemon's state, each checked; nothing else. It exits when the app closes
//! its stdin (on Lock, or when the app goes away) or after
//! [`IDLE_SECS`] without a request, which locks the app again.
//!
//! For testing without root, `CONFLUX_TEST_SYSTEM_ROOT=<dir>` makes it work on a
//! scratch "system" under `<dir>` (`config.toml`, `run/`, `state/`) and refuse
//! to touch units. It is honoured only when not running as root, so it can
//! never point a privileged helper elsewhere.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::mpsc;
use std::time::Duration;

use conflux_core::config_edit::{check_replacement, write_atomic};
use conflux_core::helper::{HelperRequest, HelperResponse, UnitAction, IDLE_SECS};
use conflux_core::ipc::{client, Request, Response};
use conflux_core::paths::{
    unit_name, validate_profile, Paths, RunMode, DEFAULT_PROFILE, SYSTEM_CONFIG,
};

/// How long a daemon may take to reload.
const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the helper works: the real system, or a scratch one for tests.
struct Ctx {
    config: PathBuf,
    /// The scratch system's root, in test mode.
    test_root: Option<PathBuf>,
}

impl Ctx {
    fn system() -> Self {
        Ctx {
            config: PathBuf::from(SYSTEM_CONFIG),
            test_root: None,
        }
    }

    fn test(root: PathBuf) -> Self {
        Ctx {
            config: root.join("config.toml"),
            test_root: Some(root),
        }
    }

    /// The socket and state dir of `profile`'s daemon.
    fn paths(&self, profile: &str) -> Result<Paths, String> {
        validate_profile(profile).map_err(|e| e.to_string())?;
        match &self.test_root {
            None => Paths::resolve(RunMode::System, Some(profile)).map_err(|e| e.to_string()),
            Some(root) => {
                let default = profile == DEFAULT_PROFILE;
                Ok(Paths {
                    config: self.config.clone(),
                    state: if default {
                        root.join("state")
                    } else {
                        root.join("state").join(profile)
                    },
                    socket: root.join("run").join(if default {
                        "conflux.sock".to_string()
                    } else {
                        format!("conflux-{profile}.sock")
                    }),
                })
            }
        }
    }
}

fn main() -> ExitCode {
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let root = unsafe { libc::geteuid() } == 0;
    let ctx = match std::env::var_os("CONFLUX_TEST_SYSTEM_ROOT") {
        _ if root => Ctx::system(),
        Some(dir) => Ctx::test(PathBuf::from(dir)),
        None => {
            eprintln!(
                "conflux-helper changes the system's conflux setup as root; \
                 the conflux GTK app starts it through pkexec"
            );
            return ExitCode::FAILURE;
        }
    };

    // Lines come from a thread, so waiting for one can time out.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut out = io::stdout().lock();
    while let Ok(Ok(line)) = rx.recv_timeout(Duration::from_secs(IDLE_SECS)) {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<HelperRequest>(&line) {
            Ok(request) => handle(&ctx, request),
            Err(e) => HelperResponse::Error(format!("bad request: {e}")),
        };
        let Ok(json) = serde_json::to_string(&response) else {
            break;
        };
        if writeln!(out, "{json}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

fn handle(ctx: &Ctx, request: HelperRequest) -> HelperResponse {
    let result = match request {
        HelperRequest::Hello => Ok(Vec::new()),
        HelperRequest::WriteConfig {
            expected,
            text,
            reload,
        } => return write_config(ctx, expected, &text, &reload),
        HelperRequest::Unit { action, profile } => unit(ctx, action, &profile),
        HelperRequest::Ipc { profile, request } => {
            return match ctx.paths(&profile) {
                Ok(paths) => match client::send(&paths.socket, &request, None) {
                    Ok(response) => HelperResponse::Ipc(response),
                    Err(e) => HelperResponse::Error(format!("{profile} daemon: {e}")),
                },
                Err(e) => HelperResponse::Error(e),
            };
        }
        HelperRequest::RemoveState { profile } => remove_state(ctx, &profile),
    };
    match result {
        Ok(problems) => HelperResponse::Done { problems },
        Err(e) => HelperResponse::Error(e),
    }
}

fn write_config(
    ctx: &Ctx,
    expected: Option<String>,
    text: &str,
    reload: &[String],
) -> HelperResponse {
    let current = match std::fs::read_to_string(&ctx.config) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            return HelperResponse::Error(format!("{}: {e}", ctx.config.display()));
        }
    };
    if current != expected {
        return HelperResponse::Changed;
    }
    if let Err(e) = check_replacement(current.as_deref(), text) {
        return HelperResponse::Error(e.short());
    }
    if current.is_none() {
        if let Some(dir) = ctx.config.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return HelperResponse::Error(format!("{}: {e}", dir.display()));
            }
        }
    }
    if let Err(e) = write_atomic(&ctx.config, text) {
        return HelperResponse::Error(format!("{}: {e}", ctx.config.display()));
    }
    let problems = reload
        .iter()
        .filter_map(|profile| {
            let socket = match ctx.paths(profile) {
                Ok(paths) => paths.socket,
                Err(e) => return Some(e),
            };
            match client::send(&socket, &Request::Reload, Some(RELOAD_TIMEOUT)) {
                Ok(Response::Reloaded { ok: true, .. }) => None,
                Ok(Response::Reloaded { message, .. }) | Ok(Response::Error(message)) => {
                    Some(format!("{profile} daemon: {message}"))
                }
                Ok(_) => Some(format!("{profile} daemon: unexpected answer to a reload")),
                Err(e) if client::is_not_running(&e) => None,
                Err(e) => Some(format!("{profile} daemon: {e}")),
            }
        })
        .collect();
    HelperResponse::Done { problems }
}

fn unit(ctx: &Ctx, action: UnitAction, profile: &str) -> Result<Vec<String>, String> {
    validate_profile(profile).map_err(|e| e.to_string())?;
    if ctx.test_root.is_some() {
        return Err("the test system has no units".to_string());
    }
    let unit = unit_name(profile);
    let output = Command::new("systemctl")
        .args([action.verb(), &unit])
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if output.status.success() {
        Ok(Vec::new())
    } else {
        Err(format!(
            "systemctl {} {unit}: {}",
            action.verb(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn remove_state(ctx: &Ctx, profile: &str) -> Result<Vec<String>, String> {
    if profile == DEFAULT_PROFILE {
        // Its state is the base dir, which holds every other profile's too.
        return Err("the default daemon's state is not removed".to_string());
    }
    let dir = ctx.paths(profile)?.state;
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(format!("{}: {e}", dir.display())),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "[[sync]]\nremote_fs = \"/b\"\nlocal = \"/a\"\n";

    fn scratch() -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx::test(dir.path().to_path_buf());
        (dir, ctx)
    }

    fn write(ctx: &Ctx, expected: Option<&str>, text: &str) -> HelperResponse {
        handle(
            ctx,
            HelperRequest::WriteConfig {
                expected: expected.map(str::to_string),
                text: text.to_string(),
                reload: vec!["default".to_string()],
            },
        )
    }

    #[test]
    fn writes_a_new_config_then_changes_it() {
        let (_dir, ctx) = scratch();
        assert!(
            matches!(write(&ctx, None, GOOD), HelperResponse::Done { problems } if problems.is_empty())
        );
        let changed = GOOD.replace("/a", "/c");
        assert!(matches!(
            write(&ctx, Some(GOOD), &changed),
            HelperResponse::Done { .. }
        ));
        assert_eq!(std::fs::read_to_string(&ctx.config).unwrap(), changed);
    }

    #[test]
    fn refuses_a_config_changed_since_it_was_read() {
        let (_dir, ctx) = scratch();
        std::fs::write(&ctx.config, GOOD).unwrap();
        assert!(matches!(
            write(&ctx, Some("older"), GOOD),
            HelperResponse::Changed
        ));
        assert!(matches!(write(&ctx, None, GOOD), HelperResponse::Changed));
    }

    #[test]
    fn refuses_to_break_an_accepted_config() {
        let (_dir, ctx) = scratch();
        std::fs::write(&ctx.config, GOOD).unwrap();
        let broken = "[[sync]]\nlocal = \"/a\"\n";
        assert!(matches!(
            write(&ctx, Some(GOOD), broken),
            HelperResponse::Error(_)
        ));
        assert_eq!(std::fs::read_to_string(&ctx.config).unwrap(), GOOD);
    }

    #[test]
    fn rejects_profiles_that_could_escape_their_paths() {
        let (_dir, ctx) = scratch();
        for request in [
            HelperRequest::RemoveState {
                profile: "../etc".into(),
            },
            HelperRequest::Unit {
                action: UnitAction::Start,
                profile: "a b".into(),
            },
            HelperRequest::Ipc {
                profile: "/x".into(),
                request: Request::Status,
            },
        ] {
            assert!(matches!(handle(&ctx, request), HelperResponse::Error(_)));
        }
    }

    #[test]
    fn removes_a_profiles_state_but_never_the_base() {
        let (dir, ctx) = scratch();
        let state = dir.path().join("state").join("laptop");
        std::fs::create_dir_all(&state).unwrap();
        let remove = |p: &str| {
            handle(
                &ctx,
                HelperRequest::RemoveState {
                    profile: p.to_string(),
                },
            )
        };
        assert!(matches!(remove("laptop"), HelperResponse::Done { .. }));
        assert!(!state.exists());
        assert!(matches!(remove("default"), HelperResponse::Error(_)));
        assert!(dir.path().join("state").exists());
    }

    #[test]
    fn the_test_system_has_no_units_and_daemons_may_be_down() {
        let (_dir, ctx) = scratch();
        let unit = HelperRequest::Unit {
            action: UnitAction::Start,
            profile: "default".into(),
        };
        assert!(matches!(handle(&ctx, unit), HelperResponse::Error(_)));
        let ipc = HelperRequest::Ipc {
            profile: "default".into(),
            request: Request::Status,
        };
        assert!(matches!(handle(&ctx, ipc), HelperResponse::Error(_)));
        assert!(matches!(
            handle(&ctx, HelperRequest::Hello),
            HelperResponse::Done { .. }
        ));
    }
}
