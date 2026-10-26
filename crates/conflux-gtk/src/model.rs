//! What the window shows for one scope, loaded off the main thread: the config
//! file and what its daemons answer. No GTK here; systemd's view of the units is
//! added on the main thread (`systemd.rs`).

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use conflux_core::config::{Config, Sync};
use conflux_core::engine::{group_label, SyncSummary};
use conflux_core::ipc::{client, DaemonInfo, GroupStatus, Request, Response};
use conflux_core::paths::{validate_profile, Paths, RunMode, DEFAULT_PROFILE, SYSTEM_CONFIG};

/// How long a status query may take before the daemon counts as stuck.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Whose config and daemons: the user's own or the system-wide ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::System => "system",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "user" => Some(Scope::User),
            "system" => Some(Scope::System),
            _ => None,
        }
    }

    fn mode(self) -> RunMode {
        match self {
            Scope::User => RunMode::User,
            Scope::System => RunMode::System,
        }
    }
}

/// The systemd unit running `profile`'s daemon.
pub fn unit_name(profile: &str) -> String {
    if profile == DEFAULT_PROFILE {
        "conflux.service".to_string()
    } else {
        format!("conflux@{profile}.service")
    }
}

/// One scope's state at one moment.
#[derive(Debug)]
pub struct Snapshot {
    pub config_path: PathBuf,
    pub config: ConfigState,
    /// One per profile with a daemon or a mention in the config; `default` first.
    pub daemons: Vec<Daemon>,
}

#[derive(Debug)]
pub enum ConfigState {
    /// No file at the path.
    Missing,
    /// The file is there but cannot be read (permissions).
    Unreadable(String),
    /// Not TOML, or not a conflux config: nothing to show.
    Invalid(String),
    Loaded(Box<Loaded>),
}

#[derive(Debug)]
pub struct Loaded {
    /// The file's text, to tell whether it changed since the last look.
    pub text: String,
    /// The config as written.
    pub raw: Config,
    /// The config resolved as the daemon does, also when the daemon would
    /// refuse it, unless it cannot even be resolved.
    pub prepared: Option<Config>,
    /// Why the daemon would refuse the config, if it would.
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

impl Loaded {
    fn parse(text: String) -> Result<Self, String> {
        let raw = Config::parse_raw(&text).map_err(|e| e.to_string())?;
        let error = Config::from_toml_str(&text).err().map(|e| e.to_string());
        let prepared = Config::parse_unvalidated(&text).ok();
        let warnings = match (&error, &prepared) {
            (None, Some(config)) => config.warnings(),
            _ => Vec::new(),
        };
        Ok(Loaded {
            text,
            raw,
            prepared,
            error,
            warnings,
        })
    }

    /// The label the daemon reports the sync at `index` under.
    pub fn label(&self, index: usize) -> String {
        group_label(self.resolved_sync(index).unwrap_or(&self.raw.syncs[index]))
    }

    /// The resolved form of the sync at `index`, when it can be resolved.
    pub fn resolved_sync(&self, index: usize) -> Option<&Sync> {
        self.prepared.as_ref()?.syncs.get(index)
    }
}

/// A daemon instance: one profile of the scope.
#[derive(Debug)]
pub struct Daemon {
    pub profile: String,
    pub socket: PathBuf,
    pub state: DaemonState,
}

#[derive(Debug)]
pub enum DaemonState {
    /// Nothing listens on the socket.
    NotRunning,
    /// The socket is there, but this user may not open it.
    NoAccess,
    /// It did not answer properly.
    Failed(String),
    /// `info` is `None` for a daemon older than the `Info` request.
    Running {
        info: Option<DaemonInfo>,
        groups: Vec<GroupStatus>,
    },
}

impl Snapshot {
    /// The running daemon reporting the group with `id` (when it has one) or
    /// `label`, with its status.
    pub fn status_of(&self, label: &str, id: Option<&str>) -> Option<(&Daemon, &GroupStatus)> {
        self.daemons.iter().find_map(|d| match &d.state {
            DaemonState::Running { groups, .. } => groups
                .iter()
                .find(|g| g.label == label || (id.is_some() && g.id.as_deref() == id))
                .map(|g| (d, g)),
            _ => None,
        })
    }
}

/// Read the config and ask every daemon of `scope` for its state.
pub fn load(scope: Scope) -> Snapshot {
    let base = Paths::resolve(scope.mode(), None).ok();
    let mut config_path = match scope {
        // Like the CLI: `$CONFLUX_CONFIG` applies to the user's own daemon.
        Scope::User => base.as_ref().map(|p| p.config.clone()).unwrap_or_default(),
        // The system daemon does not see this user's environment.
        Scope::System => PathBuf::from(SYSTEM_CONFIG),
    };
    let mut config = read_config(&config_path);

    let socket_dir = base
        .as_ref()
        .and_then(|p| p.socket.parent().map(Path::to_path_buf));
    let mut profiles = BTreeSet::new();
    if let ConfigState::Loaded(loaded) = &config {
        profiles.extend(config_profiles(&loaded.raw));
    }
    if let Some(dir) = &socket_dir {
        profiles.extend(socket_profiles(dir));
    }
    profiles.remove(DEFAULT_PROFILE);
    let daemons: Vec<Daemon> = std::iter::once(DEFAULT_PROFILE.to_string())
        .chain(profiles)
        .filter_map(|profile| {
            let socket = Paths::resolve(scope.mode(), Some(&profile)).ok()?.socket;
            let state = query(&socket);
            Some(Daemon {
                profile,
                socket,
                state,
            })
        })
        .collect();

    // A daemon started with `--config` reads another file: show what it runs.
    let in_use = daemons.iter().find_map(|d| match &d.state {
        DaemonState::Running {
            info: Some(info), ..
        } => Some(PathBuf::from(&info.config)),
        _ => None,
    });
    if let Some(path) = in_use.filter(|p| *p != config_path) {
        config = read_config(&path);
        config_path = path;
    }

    Snapshot {
        config_path,
        config,
        daemons,
    }
}

fn read_config(path: &Path) -> ConfigState {
    match std::fs::read_to_string(path) {
        Ok(text) => match Loaded::parse(text) {
            Ok(loaded) => ConfigState::Loaded(Box::new(loaded)),
            Err(e) => ConfigState::Invalid(e),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => ConfigState::Missing,
        Err(e) => ConfigState::Unreadable(e.to_string()),
    }
}

/// Every profile the config's syncs belong to.
fn config_profiles(config: &Config) -> BTreeSet<String> {
    config
        .syncs
        .iter()
        .flat_map(|s| s.profiles.iter().cloned())
        .filter(|p| validate_profile(p).is_ok())
        .collect()
}

/// The profiles with a control socket in `dir`: `conflux.sock` for the default
/// one, `conflux-<profile>.sock` for the others.
fn socket_profiles(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|name| {
            if name == "conflux.sock" {
                return Some(DEFAULT_PROFILE.to_string());
            }
            let profile = name.strip_prefix("conflux-")?.strip_suffix(".sock")?;
            validate_profile(profile).ok()?;
            Some(profile.to_string())
        })
        .collect()
}

/// Ask the daemon on `socket` who it is and how its groups are doing.
fn query(socket: &Path) -> DaemonState {
    let ask = |request| client::send(socket, &request, Some(QUERY_TIMEOUT));
    let failed = |e: io::Error| {
        if client::is_not_running(&e) {
            DaemonState::NotRunning
        } else if e.kind() == io::ErrorKind::PermissionDenied {
            DaemonState::NoAccess
        } else {
            DaemonState::Failed(e.to_string())
        }
    };
    let info = match ask(Request::Info) {
        Ok(Response::Info(info)) => Some(info),
        // A daemon from before `Info`.
        Ok(_) => None,
        Err(e) => return failed(e),
    };
    match ask(Request::Status) {
        Ok(Response::Status(groups)) => DaemonState::Running { info, groups },
        Ok(Response::Error(e)) => DaemonState::Failed(e),
        Ok(_) => DaemonState::Failed("unexpected answer to a status request".into()),
        Err(e) => failed(e),
    }
}

/// "just now", "5 min ago", "3 h ago", or a date for anything older than a day.
pub fn ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => conflux_core::timefmt::local(then)
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// "2 added, 1 removed", or "no changes".
pub fn summary_text(s: &SyncSummary) -> String {
    let parts: Vec<String> = [
        (s.added, "added"),
        (s.modified, "changed"),
        (s.removed, "removed"),
        (s.conflicted, "in conflict"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    if parts.is_empty() {
        "no changes".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_come_from_sockets_and_syncs() {
        let dir = std::env::temp_dir().join(format!("conflux-gtk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "conflux.sock",
            "conflux-desktop.sock",
            "conflux-.x.sock",
            "other.sock",
        ] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let found = socket_profiles(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            ["default", "desktop"]
        );

        let config = Config::parse_raw(
            "[[sync]]\nremote_fs = \"/b\"\nlocal = \"/a\"\nprofiles = [\"laptop\", \"bad/name\"]\n\n\
             [[sync]]\nremote_fs = \"/d\"\nlocal = \"/c\"\n",
        )
        .unwrap();
        assert_eq!(
            config_profiles(&config).into_iter().collect::<Vec<_>>(),
            ["default", "laptop"]
        );
    }

    #[test]
    fn a_missing_socket_is_a_daemon_not_running() {
        assert!(matches!(
            query(Path::new("/nonexistent/conflux.sock")),
            DaemonState::NotRunning
        ));
    }

    #[test]
    fn invalid_configs_keep_what_can_be_shown() {
        // Valid TOML and schema, but `watch-both` needs a filesystem remote.
        let loaded = Loaded::parse(
            "[[remote]]\nid = \"r\"\nbackend = \"webdav\"\nurl = \"https://x/\"\n\n\
             [[sync]]\nremote = \"r\"\nlocal = \"/a\"\ntrigger = \"watch-both\"\n"
                .to_string(),
        )
        .unwrap();
        assert!(loaded.error.is_some());
        assert_eq!(loaded.label(0), "r");
        // A shortcut is still named as the daemon names it.
        let loaded = Loaded::parse(
            "[[sync]]\nremote_fs = \"/b\"\nlocal = \"/a\"\ntrigger = \"watch-both\"\n\
             include = [\"[\"]\n"
                .to_string(),
        )
        .unwrap();
        assert!(loaded.error.is_some());
        assert_eq!(loaded.label(0), "fs:/b");
        assert!(Loaded::parse("nonsense = 1\n".to_string()).is_err());
    }

    #[test]
    fn times_and_summaries_read_naturally() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(1000, 1000 + 5 * 60), "5 min ago");
        assert_eq!(ago(1000, 1000 + 3 * 3600 + 5), "3 h ago");
        assert_eq!(
            summary_text(&SyncSummary {
                added: 2,
                removed: 1,
                modified: 0,
                conflicted: 0
            }),
            "2 added, 1 removed"
        );
        assert_eq!(summary_text(&SyncSummary::default()), "no changes");
        assert_eq!(unit_name("default"), "conflux.service");
        assert_eq!(unit_name("desktop"), "conflux@desktop.service");
    }
}
