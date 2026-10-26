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
