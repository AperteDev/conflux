//! Wire types for CLI <-> daemon IPC over the Unix socket.
//!
//! Messages are newline-delimited JSON: the client writes one [`Request`] line
//! and reads one [`Response`] line. The synchronous client used by the CLI and
//! the GUI is [`client`]; the async server lives in the daemon.
//!
//! New fields are added with `#[serde(default)]` and new requests as new
//! variants, so an older CLI or GUI keeps working against a newer daemon and the
//! other way round (an older daemon answers an unknown request with
//! [`Response::Error`]).

pub mod client;

use crate::engine::{SyncPlan, SyncSummary};
use serde::{Deserialize, Serialize};

/// A command sent from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Ask for the status of every active group.
    Status,
    /// Run a sync now for the given target.
    Sync(SyncTarget),
    /// Compute, without applying, what a sync of the target would do (dry run).
    Plan(SyncTarget),
    /// Re-read and apply the configuration file.
    Reload,
    /// Describe the daemon itself: version, profile, and config file.
    Info,
}

impl Request {
    /// Whether the request only reads state. Read-only requests are open to any
    /// local user on a system daemon's socket; the others need the daemon's
    /// owner, root, or a member of its group.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Request::Status | Request::Info)
    }
}

/// Which groups a [`Request::Sync`] applies to. Profile selection happens at the
/// daemon level (each daemon serves one profile), so this only distinguishes
/// "everything this daemon runs" from a single named group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncTarget {
    /// Every active group.
    All,
    /// A single group, selected by its `id` or its `remote:remote_path` label.
    Group(String),
}

/// The daemon's reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Status of each active group.
    Status(Vec<GroupStatus>),
    /// Outcome of a sync request, one entry per group run.
    Synced(Vec<GroupOutcome>),
    /// Result of a dry-run request, one entry per targeted group.
    Planned(Vec<GroupPlan>),
    /// Answer to [`Request::Info`].
    Info(DaemonInfo),
    /// Result of a reload.
    Reloaded {
        /// Whether the reload succeeded.
        ok: bool,
        /// Human-readable detail.
        message: String,
    },
    /// The request could not be served.
    Error(String),
}

/// What a daemon reports about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// The daemon's version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// The profile it runs (`"default"` unless set).
    pub profile: String,
    /// The config file it reads.
    pub config: String,
    /// Its process id.
    pub pid: u32,
}

/// A snapshot of one group's runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupStatus {
    /// Group label (`remote:remote_path`).
    pub label: String,
    /// The group's `id`, when the config sets one.
    #[serde(default)]
    pub id: Option<String>,
    /// Remote name.
    pub remote: String,
    /// Local root.
    pub root: String,
    /// Trigger kind (`manual`/`timer`/`watch`).
    pub trigger: String,
    /// Epoch seconds of the last run, if any.
    pub last_run: Option<u64>,
    /// Compact summary of the last successful run.
    pub last_summary: Option<SyncSummary>,
    /// Last error message, if the last run failed.
    pub last_error: Option<String>,
    /// Whether a sync of the group is running right now.
    #[serde(default)]
    pub running: bool,
}

/// The outcome of running a single group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupOutcome {
    /// Group label (`remote:remote_path`).
    pub label: String,
    /// Summary on success.
    pub summary: Option<SyncSummary>,
    /// Error message on failure.
    pub error: Option<String>,
}

/// The dry-run result for a single group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupPlan {
    /// Group label (`remote:remote_path`).
    pub label: String,
    /// The computed plan on success.
    pub plan: Option<SyncPlan>,
    /// Error message on failure.
    pub error: Option<String>,
}
