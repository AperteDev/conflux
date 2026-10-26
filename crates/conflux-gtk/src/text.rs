//! How syncs and remotes read in the UI. No GTK here.

use std::path::Path;
use std::time::Duration;

use conflux_core::config::{DaemonConfig, Sync};
use conflux_core::model::{Deletions, EmptyDirMode, RemoteKind, Scope as SyncScope, Trigger};

use crate::model::Scope;

/// `"5s"`, `"1h 30m"`, or `"off"` for zero.
pub fn duration(d: Duration) -> String {
    if d.is_zero() {
        "off".to_string()
    } else {
        humantime::format_duration(d).to_string()
    }
}

/// A path as it reads in `scope`: in the user's, with the home folder shortened
/// to `~`, as people write it; in the system's, whole, as `~` there would mean
/// the system daemon's home, not this user's.
pub fn path(scope: Scope, path: &Path) -> String {
    if scope == Scope::System {
        return path.display().to_string();
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// What a sync is called: its label, else its id, else the daemon's label.
pub fn sync_title(sync: &Sync, label: &str) -> String {
    sync.label
        .clone()
        .or_else(|| sync.id.clone())
        .unwrap_or_else(|| label.to_string())
}

/// Where a sync's files go: `remote:path`, a folder, or a git clone.
pub fn target(scope: Scope, sync: &Sync) -> String {
    if let Some(repo) = &sync.git_repo {
        return format!("Git clone {}", self::path(scope, repo));
    }
    let path = sync.remote_path.trim_matches('/');
    match (&sync.remote_fs, &sync.remote) {
        (Some(dir), _) if path.is_empty() => self::path(scope, dir),
        (Some(dir), _) => format!("{}/{path}", self::path(scope, dir)),
        (None, Some(remote)) if path.is_empty() => remote.clone(),
        (None, Some(remote)) => format!("{remote}:{path}"),
        (None, None) => String::new(),
    }
}

/// `local ⇄ target`, or just the clone for a `git_repo` sync.
pub fn mapping(scope: Scope, sync: &Sync) -> String {
    if sync.git_repo.is_some() {
        target(scope, sync)
    } else {
        format!("{} ⇄ {}", path(scope, &sync.local), target(scope, sync))
    }
}

/// When a sync runs, e.g. "On local changes" or "Every 15m".
pub fn trigger(sync: &Sync, daemon: &DaemonConfig) -> String {
    match sync.trigger {
        Trigger::Manual => "Manual".to_string(),
        Trigger::Timer => format!(
            "Every {}",
            duration(sync.interval.unwrap_or(daemon.interval))
        ),
        Trigger::Watch => "On local changes".to_string(),
        Trigger::WatchBoth => "On changes on either side".to_string(),
    }
}

pub fn backend(kind: RemoteKind) -> &'static str {
    match kind {
        RemoteKind::Webdav => "WebDAV",
        RemoteKind::Git => "Git",
        RemoteKind::Filesystem => "Folder",
    }
}

pub fn backend_icon(kind: RemoteKind) -> &'static str {
    match kind {
        RemoteKind::Webdav => "folder-remote-symbolic",
        RemoteKind::Git => "network-server-symbolic",
        RemoteKind::Filesystem => "drive-harddisk-symbolic",
    }
}

pub fn scope(scope: SyncScope) -> &'static str {
    match scope {
        SyncScope::Include => "Only the included paths",
        SyncScope::Remote => "Included paths both ways, everything else pulled from the remote",
        SyncScope::Local => "Included paths both ways, everything else pushed to the remote",
        SyncScope::Mirror => "The whole tree, both ways",
    }
}

pub fn deletions(d: Deletions) -> &'static str {
    match d {
        Deletions::Deny => "Never delete",
        Deletions::Allow => "Propagate deletions",
    }
}

pub fn empty_dirs(mode: EmptyDirMode) -> &'static str {
    match mode {
        EmptyDirMode::Ignore => "Ignore",
        EmptyDirMode::Prune => "Remove after syncing",
        EmptyDirMode::Mirror => "Mirror",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conflux_core::Config;

    fn sync(toml: &str) -> Sync {
        Config::parse_raw(&format!("[[sync]]\n{toml}"))
            .unwrap()
            .syncs
            .remove(0)
    }

    #[test]
    fn mappings_follow_the_kind_of_sync() {
        let s = sync("remote = \"nas\"\nlocal = \"~/Documents\"\nremote_path = \"/docs/\"");
        assert_eq!(mapping(Scope::User, &s), "~/Documents ⇄ nas:docs");
        let s = sync("remote_fs = \"/mnt/b\"\nlocal = \"~/a\"");
        assert_eq!(mapping(Scope::User, &s), "~/a ⇄ /mnt/b");
        let s = sync("git_repo = \"~/notes\"");
        assert_eq!(mapping(Scope::User, &s), "Git clone ~/notes");
    }

    #[test]
    fn paths_under_home_use_a_tilde() {
        let home = std::env::var("HOME").unwrap();
        let docs = format!("{home}/Documents");
        assert_eq!(path(Scope::User, Path::new(&docs)), "~/Documents");
        assert_eq!(path(Scope::User, Path::new(&home)), "~");
        assert_eq!(path(Scope::User, Path::new("/mnt/backup")), "/mnt/backup");
        // The system daemon's ~ is not this user's.
        assert_eq!(path(Scope::System, Path::new(&docs)), docs);
    }

    #[test]
    fn triggers_name_their_interval() {
        let daemon = DaemonConfig::default();
        let s = sync("remote = \"r\"\nlocal = \"/a\"\ntrigger = \"timer\"");
        assert_eq!(trigger(&s, &daemon), "Every 1h");
        let s = sync("remote = \"r\"\nlocal = \"/a\"\ntrigger = \"timer\"\ninterval = \"15m\"");
        assert_eq!(trigger(&s, &daemon), "Every 15m");
        assert_eq!(duration(Duration::ZERO), "off");
    }

    #[test]
    fn titles_prefer_label_then_id() {
        let s = sync("id = \"docs\"\nremote = \"r\"\nlocal = \"/a\"");
        assert_eq!(sync_title(&s, "r"), "docs");
        let s = sync("remote = \"r\"\nlocal = \"/a\"");
        assert_eq!(sync_title(&s, "r"), "r");
    }
}
