//! Writing the config file: one edit at a time, on top of the text the app last
//! read, checked the way the daemon checks it, then reloaded by the daemons
//! running it. No GTK here; it runs off the main thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use conflux_core::config_edit::{write_atomic, ConfigDoc};
use conflux_core::ipc::{client, Request, Response};
use conflux_core::Config;

/// A change to the config, made on its parsed document.
pub type Edit = Box<dyn FnOnce(&mut ConfigDoc) -> conflux_core::Result<()> + Send>;

/// How long a daemon may take to reload.
const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// The starter config: every option, commented out at its default.
pub const EXAMPLE: &str = include_str!("../../../config.example.toml");

#[derive(Debug)]
pub enum EditError {
    /// The file is no longer what the edit was made on.
    Changed,
    /// The edit would make a valid config one the daemon refuses, or cannot be
    /// made at all; the message says why.
    Refused(String),
    /// The file could not be read or written.
    Failed(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Changed => f.write_str(
                "The config file changed on disk since this was opened; open it again to edit",
            ),
            EditError::Refused(e) | EditError::Failed(e) => f.write_str(e),
        }
    }
}

/// The result of a written edit.
#[derive(Debug)]
pub struct Applied {
    /// The file's new text.
    pub text: String,
    /// Daemons that would not take the new config, and why.
    pub reload_problems: Vec<String>,
}

/// Apply `edit` to the config at `path`, which must still read `base`, then ask
/// the daemons on `sockets` to reload it.
///
/// An edit that turns a config the daemon accepts into one it refuses is not
/// written. One made to a config the daemon already refuses is, so that it can
/// be fixed step by step.
pub fn apply(
    path: &Path,
    base: &str,
    edit: Edit,
    sockets: &[PathBuf],
) -> Result<Applied, EditError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| EditError::Failed(format!("{}: {e}", path.display())))?;
    if text != base {
        return Err(EditError::Changed);
    }
    let mut doc = ConfigDoc::parse(&text).map_err(|e| EditError::Refused(e.short()))?;
    edit(&mut doc).map_err(|e| EditError::Refused(e.short()))?;
    let new = doc.to_string();
    if new == text {
        return Ok(Applied {
            text,
            reload_problems: Vec::new(),
        });
    }
    if let Err(e) = Config::from_toml_str(&new) {
        if Config::from_toml_str(&text).is_ok() {
            return Err(EditError::Refused(e.short()));
        }
    }
    write_atomic(path, &new).map_err(|e| EditError::Failed(format!("{}: {e}", path.display())))?;
    Ok(Applied {
        text: new,
        reload_problems: reload(sockets),
    })
}

/// Ask each daemon to reload its config; returns what went wrong.
pub fn reload(sockets: &[PathBuf]) -> Vec<String> {
    sockets
        .iter()
        .filter_map(
            |socket| match client::send(socket, &Request::Reload, Some(RELOAD_TIMEOUT)) {
                Ok(Response::Reloaded { ok: true, .. }) => None,
                Ok(Response::Reloaded { message, .. }) | Ok(Response::Error(message)) => {
                    Some(message)
                }
                Ok(_) => Some("unexpected answer to a reload".to_string()),
                Err(e) if client::is_not_running(&e) => None,
                Err(e) => Some(e.to_string()),
            },
        )
        .collect()
}

/// Start a config at `path` from the example, unless there is one already.
pub fn create(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, EXAMPLE.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conflux_core::config_edit::Section;

    const VALID: &str =
        "# mine\n[daemon]\ndebounce = \"60m\"\n\n[[sync]]\nremote_fs = \"/b\"\nlocal = \"/a\"\n";

    fn file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn set(key: &'static str, value: &'static str) -> Edit {
        Box::new(move |doc| doc.set(Section::Sync(0), key, value))
    }

    #[test]
    fn writes_the_edit_and_keeps_the_rest() {
        let (_dir, path) = file(VALID);
        let applied = apply(&path, VALID, set("trigger", "manual"), &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, applied.text);
        assert_eq!(text, format!("{VALID}trigger = \"manual\"\n"));
    }

    #[test]
    fn refuses_an_edit_on_a_file_changed_since_it_was_read() {
        let (_dir, path) = file(VALID);
        let err = apply(&path, "something older", set("trigger", "manual"), &[]).unwrap_err();
        assert!(matches!(err, EditError::Changed));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID);
    }

    #[test]
    fn refuses_to_break_a_valid_config() {
        let (_dir, path) = file(VALID);
        let err = apply(&path, VALID, set("trigger", "sometimes"), &[]).unwrap_err();
        assert!(matches!(err, EditError::Refused(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), VALID);
    }

    #[test]
    fn edits_a_config_that_is_already_refused() {
        let broken = VALID.replace("local = \"/a\"\n", "local = \"/a\"\ninclude = [\"[\"]\n");
        let (_dir, path) = file(&broken);
        apply(&path, &broken, set("trigger", "manual"), &[]).unwrap();
        let fix: Edit = Box::new(|doc| doc.unset(Section::Sync(0), "include").map(|_| ()));
        let text = std::fs::read_to_string(&path).unwrap();
        apply(&path, &text, fix, &[]).unwrap();
        Config::load(&path).unwrap();
    }

    #[test]
    fn creates_a_config_from_the_example_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conflux").join("config.toml");
        create(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE);
        assert!(create(&path).is_err());
        Config::load(&path).unwrap();
    }

    #[test]
    fn a_daemon_that_is_not_running_needs_no_reload() {
        assert!(reload(&[PathBuf::from("/nonexistent/conflux.sock")]).is_empty());
    }
}
