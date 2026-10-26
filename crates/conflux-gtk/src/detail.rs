//! Read-only detail pages of a sync and a remote: every setting, and where an
//! unset one inherits its value from.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use conflux_core::config::{format_byte_size, Config, Remote, DEFAULT_GIT_PULL_INTERVAL};
use conflux_core::model::RemoteKind;

use crate::model::{self, ConfigState, Snapshot};
use crate::{text, ui};

/// Where an inherited value comes from.
enum From<'a> {
    Remote(&'a str),
    Daemon,
    Default(&'static str),
}

fn origin(value: String, from: From) -> String {
    match from {
        From::Remote(id) => format!("{value}, from remote {id}"),
        From::Daemon => format!("{value}, from [daemon]"),
        From::Default(why) => format!("{value}, {why}"),
    }
}

fn size(bytes: u64) -> String {
    if bytes == 0 {
        "Unlimited".to_string()
    } else {
        format_byte_size(bytes)
    }
}

fn not_set() -> String {
    "Not set".to_string()
}

fn group(title: &str, rows: Vec<adw::ActionRow>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    for row in rows {
        group.add(&row);
    }
    group
}

fn page(title: &str, groups: Vec<adw::PreferencesGroup>) -> adw::NavigationPage {
    let prefs = adw::PreferencesPage::new();
    for g in groups {
        prefs.add(&g);
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&prefs));
    adw::NavigationPage::builder()
        .title(title)
        .child(&toolbar)
        .build()
}

/// The sync at `index` of the snapshot's config.
pub fn sync_page(snapshot: &Rc<Snapshot>, index: usize) -> adw::NavigationPage {
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        return page("Sync", Vec::new());
    };
    let config = &loaded.raw;
    let sync = &config.syncs[index];
    let label = loaded.label(index);
    let remote = sync.remote.as_deref().and_then(|id| config.remote(id));
    let git = sync.git_repo.is_some() || remote.is_some_and(|r| r.backend == RemoteKind::Git);
    let daemon = &config.daemon;
    let mut groups = Vec::new();

    if let Some((runner, status)) = snapshot.status_of(&label, sync.id.as_deref()) {
        let last = match status.last_run {
            Some(t) => conflux_core::timefmt::local(t),
            None => "Never".to_string(),
        };
        let result = match (&status.last_error, &status.last_summary) {
            (Some(e), _) => format!("Failed: {e}"),
            (None, Some(s)) => model::summary_text(s),
            (None, None) => "None yet".to_string(),
        };
        groups.push(group(
            "Status",
            vec![
                ui::property("Run by", &format!("The {} daemon", runner.profile)),
                ui::property("Last sync", &last),
                ui::property("Result", &result),
            ],
        ));
    }

    groups.push(group(
        "Identity",
        vec![
            ui::property("Id", sync.id.as_deref().unwrap_or("Not set")),
            ui::property("Label", sync.label.as_deref().unwrap_or("Not set")),
            ui::property("Name in the daemon", &label),
        ],
    ));

    let mut mapping = Vec::new();
    if let Some(repo) = &sync.git_repo {
        mapping.push(ui::property("Git clone", &text::path(repo)));
    } else {
        mapping.push(ui::property("Local folder", &text::path(&sync.local)));
        match (&sync.remote_fs, remote) {
            (Some(dir), _) => mapping.push(ui::property("Folder", &text::path(dir))),
            (None, Some(r)) => mapping.push(ui::property(
                "Remote",
                &format!("{} ({})", r.id, text::backend(r.backend)),
            )),
            (None, None) => mapping.push(ui::property(
                "Remote",
                sync.remote.as_deref().unwrap_or("Not set"),
            )),
        }
        let path = sync.remote_path.trim_matches('/');
        mapping.push(ui::property(
            "Path on the remote",
            if path.is_empty() {
                "The remote's root"
            } else {
                path
            },
        ));
    }
    groups.push(group("Mapping", mapping));

    if sync.git_repo.is_none() {
        let own_exclude = text::list(&sync.exclude, "None of its own");
        groups.push(group(
            "What Is Synced",
            vec![
                ui::property("Include", &text::list(&sync.include, "Nothing")),
                ui::property(
                    "Exclude",
                    &format!("{own_exclude}; plus {} from [daemon]", daemon.exclude.len()),
                ),
                ui::property(
                    "Scope",
                    &match sync.scope {
                        Some(s) => text::scope(s).to_string(),
                        None => origin(
                            text::scope(Default::default()).to_string(),
                            From::Default("the default"),
                        ),
                    },
                ),
                ui::property(
                    "Deletions",
                    &match sync.deletions {
                        Some(d) => text::deletions(d).to_string(),
                        None => origin(text::deletions(daemon.deletions).to_string(), From::Daemon),
                    },
                ),
            ],
        ));
    }

    let mut when = vec![ui::property("Trigger", &text::trigger(sync, daemon))];
    when.push(ui::property(
        "Debounce",
        &match sync.debounce {
            Some(d) => text::duration(d),
            None => origin(text::duration(daemon.debounce), From::Daemon),
        },
    ));
    when.push(ui::property(
        "Pull interval",
        &pull_interval(config, index, git),
    ));
    groups.push(group("When It Syncs", when));

    let mut other = Vec::new();
    if sync.git_repo.is_none() {
        other.push(ui::property(
            "Empty folders",
            &match sync.empty_dirs {
                Some(m) => text::empty_dirs(m).to_string(),
                None => origin(
                    text::empty_dirs(daemon.empty_dirs).to_string(),
                    From::Daemon,
                ),
            },
        ));
        other.push(ui::property(
            "Largest file",
            &match (sync.max_file_size, remote.and_then(|r| r.max_file_size)) {
                (Some(n), _) => size(n),
                (None, Some(n)) => origin(size(n), From::Remote(&remote.unwrap().id)),
                (None, None) => origin(size(daemon.max_file_size), From::Daemon),
            },
        ));
    } else {
        other.push(ui::property(
            "Commit message",
            &match &sync.commit_msg_command {
                Some(cmd) => format!("From command: {cmd}"),
                None => "conflux sync from <hostname>".to_string(),
            },
        ));
    }
    other.push(ui::property(
        "Profiles",
        &text::list(&sync.profiles, "None, so it never runs"),
    ));
    groups.push(group("Other", other));

    page(&text::sync_title(sync, &label), groups)
}

/// The periodic pull of the sync at `index` and where it is set, resolved the
/// way the daemon does: the sync, its remote, `[daemon]`, then the git default.
fn pull_interval(config: &Config, index: usize, git: bool) -> String {
    let sync = &config.syncs[index];
    let remote = sync.remote.as_deref().and_then(|id| config.remote(id));
    let shown = |d: Duration| text::duration(d);
    if let Some(d) = sync.pull_interval {
        shown(d)
    } else if let Some((r, d)) = remote.and_then(|r| r.pull_interval.map(|d| (r, d))) {
        origin(shown(d), From::Remote(&r.id))
    } else if let Some(d) = config.daemon.pull_interval {
        origin(shown(d), From::Daemon)
    } else if git {
        origin(
            shown(DEFAULT_GIT_PULL_INTERVAL),
            From::Default("the default for git"),
        )
    } else {
        "Off".to_string()
    }
}

/// The `[[remote]]` at `index` of the snapshot's config.
pub fn remote_page(snapshot: &Rc<Snapshot>, index: usize) -> adw::NavigationPage {
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        return page("Remote", Vec::new());
    };
    let config = &loaded.raw;
    let remote: &Remote = &config.remotes[index];
    let mut groups = vec![group(
        "Remote",
        vec![
            ui::property("Id", &remote.id),
            ui::property("Label", remote.label.as_deref().unwrap_or("Not set")),
            ui::property("Backend", text::backend(remote.backend)),
            ui::property(
                if remote.backend == RemoteKind::Filesystem {
                    "Folder"
                } else {
                    "URL"
                },
                &remote.url,
            ),
        ],
    )];

    if remote.backend != RemoteKind::Filesystem {
        let password = match (&remote.password_command, &remote.password) {
            (Some(cmd), _) => format!("From command: {cmd}"),
            (None, Some(_)) => "Written in the config file".to_string(),
            (None, None) => not_set(),
        };
        let mut rows = vec![
            ui::property("Username", remote.username.as_deref().unwrap_or("Not set")),
            ui::property("Password", &password),
        ];
        if remote.backend == RemoteKind::Git {
            rows.push(ui::property(
                "SSH key",
                &match (&remote.identity_file, &remote.identity_file_command) {
                    (Some(file), _) => file.clone(),
                    (None, Some(cmd)) => format!("From command: {cmd}"),
                    (None, None) => "From ~/.ssh/config, ssh-agent or the default keys".to_string(),
                },
            ));
            rows.push(ui::property(
                "Branch",
                remote
                    .branch
                    .as_deref()
                    .unwrap_or("The remote's default branch"),
            ));
            rows.push(ui::property(
                "Commit message",
                &match &remote.commit_msg_command {
                    Some(cmd) => format!("From command: {cmd}"),
                    None => "conflux sync from <hostname>".to_string(),
                },
            ));
        }
        groups.push(group("Connection", rows));
    }

    let daemon = &config.daemon;
    groups.push(group(
        "Defaults for Its Syncs",
        vec![
            ui::property(
                "Pull interval",
                &match (remote.pull_interval, daemon.pull_interval) {
                    (Some(d), _) => text::duration(d),
                    (None, Some(d)) => origin(text::duration(d), From::Daemon),
                    (None, None) if remote.backend == RemoteKind::Git => origin(
                        text::duration(DEFAULT_GIT_PULL_INTERVAL),
                        From::Default("the default for git"),
                    ),
                    (None, None) => "Off".to_string(),
                },
            ),
            ui::property(
                "Largest file",
                &match remote.max_file_size {
                    Some(n) => size(n),
                    None => origin(size(daemon.max_file_size), From::Daemon),
                },
            ),
        ],
    ));

    let users: Vec<adw::ActionRow> = config
        .syncs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.remote.as_deref() == Some(remote.id.as_str()))
        .map(|(i, s)| ui::row(&text::sync_title(s, &loaded.label(i)), &text::mapping(s)))
        .collect();
    let used = group("Used By", users);
    if config
        .syncs
        .iter()
        .all(|s| s.remote.as_deref() != Some(remote.id.as_str()))
    {
        used.set_description(Some("No sync uses this remote"));
    }
    groups.push(used);

    page(remote.label.as_deref().unwrap_or(&remote.id), groups)
}
