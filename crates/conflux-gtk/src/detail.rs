//! Detail pages of a sync and a remote: every setting, editable when the scope
//! is, and what an unset one inherits.

use std::rc::Rc;

use adw::prelude::*;
use conflux_core::config::{format_byte_size, Config, DEFAULT_GIT_PULL_INTERVAL};
use conflux_core::config_edit::Section;
use conflux_core::model::{Deletions, EmptyDirMode, RemoteKind};

use crate::editor::Edit;
use crate::form::{self, Form, Kind, Toaster};
use crate::model::{self, ConfigState, Loaded, Snapshot};
use crate::scope::ScopeView;
use crate::{text, ui};

fn size(bytes: u64) -> String {
    if bytes == 0 {
        "unlimited".to_string()
    } else {
        format_byte_size(bytes)
    }
}

/// Owned `(value, label)` options for a combo.
pub fn options(list: &[(Option<&str>, &str)]) -> Vec<(Option<String>, String)> {
    list.iter()
        .map(|(v, l)| (v.map(str::to_string), l.to_string()))
        .collect()
}

pub fn group(title: &str, rows: &[&gtk::Widget]) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    for row in rows {
        group.add(*row);
    }
    group
}

fn toaster(view: &ScopeView) -> Toaster {
    let ctx = view.ctx().clone();
    Rc::new(move |text: &str| ctx.toast(text))
}

/// A Delete button: what the confirmation says, and what it runs.
struct Delete {
    heading: String,
    body: String,
    run: Rc<dyn Fn()>,
}

/// A page with a banner for when the file changes on disk under it, and an
/// optional Delete button.
fn page(
    view: &Rc<ScopeView>,
    title: &str,
    groups: Vec<adw::PreferencesGroup>,
    delete: Option<Delete>,
) -> adw::NavigationPage {
    let button = delete
        .filter(|_| view.editable())
        .map(|delete| delete_button(view, delete).upcast());
    page_with(view, title, groups, button)
}

/// A page with a banner for when the file changes on disk under it, and
/// `button` at the end of its header bar.
pub fn page_with(
    view: &Rc<ScopeView>,
    title: &str,
    groups: Vec<adw::PreferencesGroup>,
    button: Option<gtk::Widget>,
) -> adw::NavigationPage {
    let prefs = adw::PreferencesPage::new();
    for g in groups {
        prefs.add(&g);
    }
    let header = adw::HeaderBar::new();
    if let Some(button) = button {
        header.pack_end(&button);
    }
    let banner = adw::Banner::builder()
        .title("The config file changed on disk; this page is out of date")
        .button_label("Back")
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&prefs));
    let nav_page = adw::NavigationPage::builder()
        .title(title)
        .child(&toolbar)
        .build();
    view.watch_page(&banner);
    let nav = view.ctx().nav.clone();
    banner.connect_button_clicked(move |_| {
        nav.pop();
    });
    nav_page
}

/// A header button that runs `delete` once the user confirms it.
fn delete_button(view: &ScopeView, delete: Delete) -> gtk::Button {
    let Delete { heading, body, run } = delete;
    let button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(heading.as_str())
        .build();
    let window = view.ctx().window.clone();
    button.connect_clicked(move |_| {
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let run = run.clone();
        dialog.choose(
            Some(&window),
            gtk::gio::Cancellable::NONE,
            move |response| {
                if response == "delete" {
                    run();
                }
            },
        );
    });
    button
}

/// Write `edit` removing an entry, then leave its page.
fn remove(form: &Rc<Form>, edit: Edit, done: &'static str) -> Rc<dyn Fn()> {
    let form = form.clone();
    let edit = std::cell::RefCell::new(Some(edit));
    Rc::new(move || {
        let Some(edit) = edit.borrow_mut().take() else {
            return;
        };
        let view = form.view().clone();
        form.commit(edit, move |ok| {
            if ok {
                // Positions after the removed entry moved up.
                view.bump_generation();
                view.ctx().nav.pop();
                view.ctx().toast(done);
            }
        });
    })
}

/// What the sync at `index` inherits for its pull interval, without its own.
fn inherited_pull(config: &Config, index: usize) -> String {
    let sync = &config.syncs[index];
    let remote = sync.remote.as_deref().and_then(|id| config.remote(id));
    let git = sync.git_repo.is_some() || remote.is_some_and(|r| r.backend == RemoteKind::Git);
    if let Some((r, d)) = remote.and_then(|r| r.pull_interval.map(|d| (r, d))) {
        format!("{} from remote {}", text::duration(d), r.id)
    } else if let Some(d) = config.daemon.pull_interval {
        format!("{} from [daemon]", text::duration(d))
    } else if git {
        format!(
            "{}, the default for git",
            text::duration(DEFAULT_GIT_PULL_INTERVAL)
        )
    } else {
        "off".to_string()
    }
}

fn loaded(snapshot: &Snapshot) -> Option<&Loaded> {
    match &snapshot.config {
        ConfigState::Loaded(loaded) => Some(loaded),
        _ => None,
    }
}

/// The sync at `index` of the snapshot's config.
pub fn sync_page(
    view: &Rc<ScopeView>,
    snapshot: &Rc<Snapshot>,
    index: usize,
) -> adw::NavigationPage {
    let Some(loaded) = loaded(snapshot) else {
        return page(view, "Sync", Vec::new(), None);
    };
    let config = &loaded.raw;
    let sync = &config.syncs[index];
    let daemon = &config.daemon;
    let label = loaded.label(index);
    let title = text::sync_title(sync, &label);
    let section = Section::Sync(index);
    let form = Form::new(view, section, toaster(view));
    let doc = &loaded.doc;
    let w = |key| form::written(doc, section, key);
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
            &[
                ui::property("Run by", &format!("The {} daemon", runner.profile)).upcast_ref(),
                ui::property("Last sync", &last).upcast_ref(),
                ui::property("Result", &result).upcast_ref(),
            ],
        ));
    }

    groups.push(group(
        "Identity",
        &[
            form.entry("Id", "id", w("id"), Kind::Text, Some("none"), false)
                .upcast_ref(),
            form.entry(
                "Label",
                "label",
                w("label"),
                Kind::Text,
                Some("none"),
                false,
            )
            .upcast_ref(),
            ui::property("Name in the daemon", &label).upcast_ref(),
        ],
    ));

    let mut mapping: Vec<gtk::Widget> = Vec::new();
    if sync.git_repo.is_some() {
        mapping.push(ui::property("Kind", "A git clone, synced through git").upcast());
        mapping.push(
            form.entry(
                "Git clone",
                "git_repo",
                w("git_repo"),
                Kind::Folder,
                None,
                true,
            )
            .upcast(),
        );
    } else {
        mapping.push(
            form.entry(
                "Local folder",
                "local",
                w("local"),
                Kind::Folder,
                None,
                true,
            )
            .upcast(),
        );
        if sync.remote_fs.is_some() {
            mapping.push(
                form.entry(
                    "Folder",
                    "remote_fs",
                    w("remote_fs"),
                    Kind::Folder,
                    None,
                    true,
                )
                .upcast(),
            );
        } else {
            let mut remotes: Vec<(Option<String>, String)> = config
                .remotes
                .iter()
                .map(|r| {
                    let name = match &r.label {
                        Some(l) => format!("{l} ({})", r.id),
                        None => r.id.clone(),
                    };
                    (Some(r.id.clone()), name)
                })
                .collect();
            // A remote the config does not define still shows as is.
            if let Some(id) = sync
                .remote
                .as_ref()
                .filter(|id| config.remote(id).is_none())
            {
                remotes.push((Some(id.clone()), format!("{id} (not defined)")));
            }
            mapping.push(
                form.choice("Remote", "remote", remotes, sync.remote.clone(), None)
                    .upcast(),
            );
        }
        mapping.push(
            form.entry(
                "Path on the remote",
                "remote_path",
                w("remote_path"),
                Kind::Text,
                Some("the remote's root"),
                false,
            )
            .upcast(),
        );
    }
    let mapping: Vec<&gtk::Widget> = mapping.iter().collect();
    groups.push(group("Mapping", &mapping));

    if sync.git_repo.is_none() {
        groups.push(group(
            "How It Syncs",
            &[
                form.choice(
                    "Scope",
                    "scope",
                    options(&[
                        (Some("include"), text::scope(Default::default())),
                        (
                            Some("remote"),
                            text::scope(conflux_core::model::Scope::Remote),
                        ),
                        (
                            Some("local"),
                            text::scope(conflux_core::model::Scope::Local),
                        ),
                        (
                            Some("mirror"),
                            text::scope(conflux_core::model::Scope::Mirror),
                        ),
                    ]),
                    w("scope"),
                    Some("include"),
                )
                .upcast_ref(),
                form.choice(
                    "Deletions",
                    "deletions",
                    options(&[
                        (
                            None,
                            &format!("Inherit: {}", text::deletions(daemon.deletions)),
                        ),
                        (Some("deny"), text::deletions(Deletions::Deny)),
                        (Some("allow"), text::deletions(Deletions::Allow)),
                    ]),
                    w("deletions"),
                    None,
                )
                .upcast_ref(),
            ],
        ));
        groups.push(form.list(
            "Include",
            "Paths and globs synced both ways, relative to the local folder; ** is everything",
            "include",
            sync.include.clone(),
            Vec::new(),
            "Add a path or glob",
        ));
        groups.push(form.list(
            "Exclude",
            &format!(
                "Never synced, on top of the {} patterns excluded in [daemon]",
                daemon.exclude.len()
            ),
            "exclude",
            sync.exclude.clone(),
            Vec::new(),
            "Add a path or glob",
        ));
    }

    groups.push(group(
        "When It Syncs",
        &[
            form.choice(
                "Trigger",
                "trigger",
                options(&[
                    (Some("manual"), "Manual"),
                    (Some("timer"), "On a timer"),
                    (Some("watch"), "On local changes"),
                    (Some("watch-both"), "On changes on either side"),
                ]),
                w("trigger"),
                Some("watch"),
            )
            .upcast_ref(),
            form.entry(
                "Timer interval",
                "interval",
                w("interval"),
                Kind::Duration,
                Some(&format!(
                    "{} from [daemon]",
                    text::duration(daemon.interval)
                )),
                false,
            )
            .upcast_ref(),
            form.entry(
                "Debounce",
                "debounce",
                w("debounce"),
                Kind::Duration,
                Some(&format!(
                    "{} from [daemon]",
                    text::duration(daemon.debounce)
                )),
                false,
            )
            .upcast_ref(),
            form.entry(
                "Pull interval",
                "pull_interval",
                w("pull_interval"),
                Kind::Duration,
                Some(&inherited_pull(config, index)),
                false,
            )
            .upcast_ref(),
        ],
    ));

    let remote = sync.remote.as_deref().and_then(|id| config.remote(id));
    let mut other: Vec<gtk::Widget> = Vec::new();
    if sync.git_repo.is_none() {
        other.push(
            form.choice(
                "Empty folders",
                "empty_dirs",
                options(&[
                    (
                        None,
                        &format!("Inherit: {}", text::empty_dirs(daemon.empty_dirs)),
                    ),
                    (Some("ignore"), text::empty_dirs(EmptyDirMode::Ignore)),
                    (Some("prune"), text::empty_dirs(EmptyDirMode::Prune)),
                    (Some("mirror"), text::empty_dirs(EmptyDirMode::Mirror)),
                ]),
                w("empty_dirs"),
                None,
            )
            .upcast(),
        );
        let inherited = match remote.and_then(|r| r.max_file_size.map(|n| (r, n))) {
            Some((r, n)) => format!("{} from remote {}", size(n), r.id),
            None => format!("{} from [daemon]", size(daemon.max_file_size)),
        };
        other.push(
            form.entry(
                "Largest file",
                "max_file_size",
                w("max_file_size"),
                Kind::Size,
                Some(&inherited),
                false,
            )
            .upcast(),
        );
    } else {
        other.push(
            form.entry(
                "Commit message command",
                "commit_msg_command",
                w("commit_msg_command"),
                Kind::Text,
                Some("\"conflux sync from <hostname>\""),
                false,
            )
            .upcast(),
        );
    }
    let other: Vec<&gtk::Widget> = other.iter().collect();
    groups.push(group("Limits", &other));

    // Profiles with a daemon, and those other syncs use, to pick from.
    let mut known: Vec<String> = snapshot.daemons.iter().map(|d| d.profile.clone()).collect();
    for p in config.syncs.iter().flat_map(|s| s.profiles.iter()) {
        if !known.contains(p) {
            known.push(p.clone());
        }
    }
    groups.push(form.profiles(known, sync.profiles.clone()));

    let delete = remove(
        &form,
        Box::new(move |doc| doc.remove_sync(index)),
        "Removed the sync",
    );
    page(
        view,
        &title,
        groups,
        Some(Delete {
            heading: "Delete Sync".to_string(),
            body: format!(
                "{title} is removed from the config file. Its files stay where they are."
            ),
            run: delete,
        }),
    )
}

/// The `[[remote]]` at `index` of the snapshot's config.
pub fn remote_page(
    view: &Rc<ScopeView>,
    snapshot: &Rc<Snapshot>,
    index: usize,
) -> adw::NavigationPage {
    let Some(loaded) = loaded(snapshot) else {
        return page(view, "Remote", Vec::new(), None);
    };
    let config = &loaded.raw;
    let remote = &config.remotes[index];
    let daemon = &config.daemon;
    let section = Section::Remote(index);
    let form = Form::new(view, section, toaster(view));
    let doc = &loaded.doc;
    let w = |key| form::written(doc, section, key);
    let folder = remote.backend == RemoteKind::Filesystem;

    let id = form.custom_entry("Id", Some(remote.id.clone()), move |section, new| {
        if new.is_empty() {
            return Err("A remote needs an id".to_string());
        }
        let Section::Remote(i) = section else {
            unreachable!("a remote's form edits a remote")
        };
        Ok(Box::new(move |doc| doc.rename_remote(i, &new)))
    });
    let mut groups = vec![group(
        "Remote",
        &[
            id.upcast_ref(),
            form.entry(
                "Label",
                "label",
                w("label"),
                Kind::Text,
                Some("none"),
                false,
            )
            .upcast_ref(),
            ui::property("Backend", text::backend(remote.backend)).upcast_ref(),
            form.entry(
                if folder { "Folder" } else { "URL" },
                "url",
                w("url"),
                if folder { Kind::Folder } else { Kind::Text },
                None,
                true,
            )
            .upcast_ref(),
        ],
    )];

    if !folder {
        let mut rows: Vec<gtk::Widget> = vec![
            form.entry(
                "Username",
                "username",
                w("username"),
                Kind::Text,
                Some("none"),
                false,
            )
            .upcast(),
            form.entry(
                "Password command",
                "password_command",
                w("password_command"),
                Kind::Text,
                Some("none"),
                false,
            )
            .upcast(),
            form.password(
                "Password, kept as plain text in the config",
                "password",
                w("password"),
            )
            .upcast(),
        ];
        if remote.backend == RemoteKind::Git {
            rows.push(
                form.entry(
                    "SSH key file",
                    "identity_file",
                    w("identity_file"),
                    Kind::File,
                    Some("~/.ssh/config, ssh-agent or the default keys"),
                    false,
                )
                .upcast(),
            );
            rows.push(
                form.entry(
                    "SSH key command",
                    "identity_file_command",
                    w("identity_file_command"),
                    Kind::Text,
                    Some("none"),
                    false,
                )
                .upcast(),
            );
            rows.push(
                form.entry(
                    "Branch",
                    "branch",
                    w("branch"),
                    Kind::Text,
                    Some("the remote's default branch"),
                    false,
                )
                .upcast(),
            );
            rows.push(
                form.entry(
                    "Commit message command",
                    "commit_msg_command",
                    w("commit_msg_command"),
                    Kind::Text,
                    Some("\"conflux sync from <hostname>\""),
                    false,
                )
                .upcast(),
            );
        }
        let rows: Vec<&gtk::Widget> = rows.iter().collect();
        groups.push(group("Connection", &rows));
    }

    let pull = match daemon.pull_interval {
        Some(d) => format!("{} from [daemon]", text::duration(d)),
        None if remote.backend == RemoteKind::Git => format!(
            "{}, the default for git",
            text::duration(DEFAULT_GIT_PULL_INTERVAL)
        ),
        None => "off".to_string(),
    };
    groups.push(group(
        "Defaults for Its Syncs",
        &[
            form.entry(
                "Pull interval",
                "pull_interval",
                w("pull_interval"),
                Kind::Duration,
                Some(&pull),
                false,
            )
            .upcast_ref(),
            form.entry(
                "Largest file",
                "max_file_size",
                w("max_file_size"),
                Kind::Size,
                Some(&format!("{} from [daemon]", size(daemon.max_file_size))),
                false,
            )
            .upcast_ref(),
        ],
    ));

    let used = adw::PreferencesGroup::builder().title("Used By").build();
    let mut any = false;
    for (i, s) in config.syncs.iter().enumerate() {
        if s.remote.as_deref() != Some(remote.id.as_str()) {
            continue;
        }
        any = true;
        let row = ui::row(&text::sync_title(s, &loaded.label(i)), &text::mapping(s));
        row.set_activatable(true);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (view, snapshot) = (view.clone(), snapshot.clone());
        row.connect_activated(move |_| {
            view.ctx().nav.push(&sync_page(&view, &snapshot, i));
        });
        used.add(&row);
    }
    if !any {
        used.set_description(Some("No sync uses this remote"));
    }
    groups.push(used);

    let title = remote.label.clone().unwrap_or_else(|| remote.id.clone());
    let delete = remove(
        &form,
        Box::new(move |doc| doc.remove_remote(index)),
        "Removed the remote",
    );
    page(
        view,
        &title,
        groups,
        Some(Delete {
            heading: "Delete Remote".to_string(),
            body: format!(
                "{title} is removed from the config file. Nothing on the remote is touched."
            ),
            run: delete,
        }),
    )
}
