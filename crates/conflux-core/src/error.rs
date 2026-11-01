//! Error and result types for `conflux-core`.

use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by the core library.
#[derive(Debug, Error)]
pub enum Error {
    /// The config file could not be read from disk.
    #[error("failed to read config at {path}: {source}")]
    ConfigRead {
        /// Path that was attempted.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The config file was not valid TOML or did not match the schema.
    #[error("failed to parse config: {0}")]
    ConfigParse(#[from] toml::de::Error),

    /// The config parsed but failed semantic validation.
    #[error("invalid config: {0}")]
    Validation(String),

    /// An edit to the config file could not be made (see `config_edit`).
    #[error("cannot edit config: {0}")]
    ConfigEdit(String),

    /// A required base directory could not be determined.
    #[error("could not determine {0} directory")]
    MissingDir(&'static str),

    /// A backend (local/webdav/git) operation failed.
    #[error("backend error: {0}")]
    Backend(String),

    /// The sync cannot run until something outside conflux changes: a `git_repo`
    /// that is not a clone, a detached HEAD, a rebase left in progress, a conflict
    /// awaiting resolution. Retrying cannot help, so the daemon reports it once
    /// rather than working through its retry backoff. The message is written for
    /// the user and needs no prefix.
    #[error("{0}")]
    Precondition(String),

    /// The on-disk index could not be (de)serialized.
    #[error("index error: {0}")]
    Index(#[from] serde_json::Error),

    /// A generic I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The message alone, for a UI: a TOML parse error without the excerpt of
    /// the file it otherwise spans several lines with.
    pub fn short(&self) -> String {
        match self {
            Error::ConfigParse(e) => format!("failed to parse config: {}", e.message()),
            other => other.to_string(),
        }
    }
}

/// Convenience result alias for the core library.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a sync failed, in a few words, from its error message: for a UI or a
/// notification, next to the whole message. Errors it knows read as their kind
/// ("Can't reach the server"); any other as its first clause, without the
/// "backend error:" in front.
pub fn reason(message: &str) -> String {
    let lower = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    let known = if has(&["password_command"]) {
        Some("The password command failed")
    } else if has(&["certificate", "unknownissuer"]) {
        Some("The server's certificate is not trusted")
    } else if has(&[
        "401",
        "unauthorized",
        "authentication required",
        "authentication replays",
        "no more ssh credentials",
        "permission denied (publickey",
    ]) {
        Some("The server refused the login")
    } else if has(&["403 forbidden"]) {
        Some("The server denied access")
    } else if has(&["507", "insufficient storage"]) {
        Some("The server is out of space")
    } else if has(&["no space left on device"]) {
        Some("The disk is full")
    } else if has(&[
        "connection refused",
        "failed to lookup address",
        "name or service not known",
        "failed to resolve address",
        "could not resolve",
        "temporary failure in name resolution",
        "network is unreachable",
        "no route to host",
        "timed out",
        "failed to connect",
        "error sending request",
    ]) {
        Some("Can't reach the server")
    } else if has(&["404 not found", "status code: 404", "repository not found"]) {
        Some("Not found on the server")
    } else {
        None
    };
    if let Some(reason) = known {
        return reason.to_string();
    }
    let rest = message.strip_prefix("backend error: ").unwrap_or(message);
    let first = rest.split("; ").next().unwrap_or(rest).trim();
    let mut chars = first.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_their_reason() {
        for (message, expected) in [
            (
                "backend error: webdav request failed: error sending request for url \
                 (http://127.0.0.1:9/dav/): Connection refused (os error 111)",
                "Can't reach the server",
            ),
            (
                "backend error: git fetch failed: failed to resolve address for host: Name \
                 or service not known; class=Net (12)",
                "Can't reach the server",
            ),
            (
                "backend error: PROPFIND failed: 401 Unauthorized",
                "The server refused the login",
            ),
            (
                "backend error: webdav request failed: error sending request for url \
                 (https://x/): invalid peer certificate: UnknownIssuer",
                "The server's certificate is not trusted",
            ),
            (
                "backend error: password_command exited with exit status: 1",
                "The password command failed",
            ),
            (
                "branch `master` has no upstream to sync with, so conflux has nothing to \
                 fetch from or push to; set one with `git push -u origin master`",
                "Branch `master` has no upstream to sync with, so conflux has nothing to \
                 fetch from or push to",
            ),
            (
                "backend error: PUT a.txt failed: 500",
                "PUT a.txt failed: 500",
            ),
        ] {
            assert_eq!(reason(message), expected, "{message}");
        }
    }
}
