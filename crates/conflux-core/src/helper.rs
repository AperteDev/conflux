//! Wire types for the privileged helper (`conflux-helper`) the GUI starts with
//! `pkexec` to change the system-wide config and daemons.
//!
//! Like the daemon's IPC, it is newline-delimited JSON, here over the helper's
//! stdin and stdout: the GUI writes one [`HelperRequest`] line and reads one
//! [`HelperResponse`] line, in order. The helper does only these few things,
//! each checked, so being unlocked never means running anything else as root.

use serde::{Deserialize, Serialize};

use crate::ipc::{Request, Response};

/// How long the helper waits for a request before it exits, locking the GUI
/// again.
pub const IDLE_SECS: u64 = 5 * 60;

/// A request to the helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HelperRequest {
    /// Answer [`HelperResponse::Done`]; tells the GUI the helper is up.
    Hello,
    /// Replace the system config with `text`, if it still reads `expected`
    /// (`None`: there is no file yet), then ask the daemons of `reload` (by
    /// profile) to reload it. Refused like an edit in the GUI: when it would
    /// turn a config the daemon accepts into one it refuses.
    WriteConfig {
        /// The text the change was made on.
        expected: Option<String>,
        /// The new text.
        text: String,
        /// Profiles whose daemons should reload.
        reload: Vec<String>,
    },
    /// Act on the unit of `profile`'s system daemon.
    Unit {
        /// What to do.
        action: UnitAction,
        /// Whose daemon.
        profile: String,
    },
    /// Send `request` to `profile`'s system daemon, as root.
    Ipc {
        /// Whose daemon.
        profile: String,
        /// What to ask it.
        request: Request,
    },
    /// Delete the state of `profile`'s system daemon (not `default`'s).
    RemoveState {
        /// Whose state.
        profile: String,
    },
}

/// What [`HelperRequest::Unit`] does, as `systemctl` would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitAction {
    /// Start it now.
    Start,
    /// Stop it now.
    Stop,
    /// Restart it now.
    Restart,
    /// Start it at boot.
    Enable,
    /// Stop starting it at boot.
    Disable,
}

impl UnitAction {
    /// The `systemctl` verb.
    pub fn verb(self) -> &'static str {
        match self {
            UnitAction::Start => "start",
            UnitAction::Stop => "stop",
            UnitAction::Restart => "restart",
            UnitAction::Enable => "enable",
            UnitAction::Disable => "disable",
        }
    }
}

/// The helper's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HelperResponse {
    /// Done; `problems` lists what went wrong on the side, e.g. a daemon that
    /// did not take a new config.
    Done {
        /// Side problems, if any.
        problems: Vec<String>,
    },
    /// The daemon's answer to [`HelperRequest::Ipc`].
    Ipc(Response),
    /// The config changed on disk since the GUI read it.
    Changed,
    /// It could not be done, and why.
    Error(String),
}
