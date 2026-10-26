//! The Syncs and Remotes pages of one scope.
//!
//! Rows are rebuilt only when the config or the set of daemons changes; every
//! other look updates them in place, so an open menu or the scroll position
//! survives polling.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use conflux_core::ipc::{client, Request, Response, SyncTarget};
use gtk::{gio, glib};

use crate::model::{self, ConfigState, Daemon, DaemonState, Loaded, Scope, Snapshot};
use crate::systemd::Unit;
use crate::window::Ctx;
use crate::{detail, dryrun, text, ui};

pub struct ScopeView {
    pub scope: Scope,
    /// Whether a load of this scope is running.
    pub loading: Cell<bool>,
    ctx: Rc<Ctx>,
    syncs_outer: gtk::Stack,
    remotes_outer: gtk::Stack,
    syncs: gtk::Stack,
    remotes: gtk::Stack,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    snapshot: Option<Rc<Snapshot>>,
    /// `None` when systemd could not be asked.
    units: Option<HashMap<String, Unit>>,
    /// What the rows were built from.
    built_from: Option<String>,
    sync_rows: Vec<SyncRow>,
    daemon_rows: Vec<DaemonRow>,
    sync_all: Option<gtk::Button>,
    /// Labels of the syncs started here and still running.
    busy: HashSet<String>,
}

struct SyncRow {
    index: usize,
    label: String,
    id: Option<String>,
    title: String,
    row: adw::ActionRow,
    spinner: adw::Spinner,
    error: gtk::Image,
    sync: gtk::Button,
    actions: gio::SimpleActionGroup,
}

struct DaemonRow {
    profile: String,
    row: adw::ActionRow,
    dot: gtk::Image,
}

impl ScopeView {
    pub fn new(
        scope: Scope,
        ctx: Rc<Ctx>,
        syncs_outer: &gtk::Stack,
        remotes_outer: &gtk::Stack,
    ) -> Rc<Self> {
        let view = Rc::new(ScopeView {
            scope,
            loading: Cell::new(false),
            ctx,
            syncs_outer: syncs_outer.clone(),
            remotes_outer: remotes_outer.clone(),
            syncs: gtk::Stack::new(),
            remotes: gtk::Stack::new(),
            state: RefCell::default(),
        });
        ui::replace_child(&view.syncs, "status", &ui::loading("Loading"));
        ui::replace_child(&view.remotes, "status", &ui::loading("Loading"));
        syncs_outer.add_named(&view.syncs, Some(scope.name()));
        remotes_outer.add_named(&view.remotes, Some(scope.name()));
        view
    }

    /// Show this scope's pages.
    pub fn present(&self) {
        self.syncs_outer.set_visible_child_name(self.scope.name());
        self.remotes_outer.set_visible_child_name(self.scope.name());
    }

    /// Why the daemon would refuse the config, if it would.
    pub fn problem(&self) -> Option<String> {
        let state = self.state.borrow();
        match &state.snapshot.as_ref()?.config {
            ConfigState::Loaded(loaded) => loaded.error.clone(),
            _ => None,
        }
    }

    pub fn config_path(&self) -> Option<PathBuf> {
        let state = self.state.borrow();
        Some(state.snapshot.as_ref()?.config_path.clone())
    }

    /// Show a fresh look at the scope.
    pub fn show(
        self: &Rc<Self>,
        snapshot: model::Snapshot,
        units: Result<HashMap<String, Unit>, glib::Error>,
    ) {
        let snapshot = Rc::new(snapshot);
        let key = structure_key(&snapshot);
        let had_problem = self.problem();
        let rebuild = {
            let mut state = self.state.borrow_mut();
            state.snapshot = Some(snapshot.clone());
            state.units = units.ok();
            state.built_from.as_deref() != Some(key.as_str())
        };
        if rebuild {
            self.rebuild(&snapshot);
            self.state.borrow_mut().built_from = Some(key);
        }
        self.update();
        if self.problem() != had_problem {
            self.ctx.problem_changed();
        }
    }

    fn rebuild(self: &Rc<Self>, snapshot: &Rc<Snapshot>) {
        {
            let mut state = self.state.borrow_mut();
            state.sync_rows.clear();
            state.daemon_rows.clear();
            state.sync_all = None;
        }
        let loaded = match &snapshot.config {
            ConfigState::Loaded(loaded) => loaded,
            other => {
                let page = config_status(self.scope, &snapshot.config_path, other);
                ui::replace_child(&self.syncs, "status", &page);
                let page = config_status(self.scope, &snapshot.config_path, other);
                ui::replace_child(&self.remotes, "status", &page);
                return;
            }
        };

        let page = adw::PreferencesPage::new();
        if !loaded.warnings.is_empty() {
            let group = adw::PreferencesGroup::builder().title("Warnings").build();
            for warning in &loaded.warnings {
                let row = ui::row(warning, "");
                row.set_title_selectable(true);
                row.add_prefix(&ui::state_icon("dialog-warning-symbolic", "warning"));
                group.add(&row);
            }
            page.add(&group);
        }
        page.add(&self.daemons_group(snapshot));
        page.add(&self.syncs_group(snapshot, loaded));
        ui::replace_child(&self.syncs, "list", &page);
        ui::replace_child(&self.remotes, "list", &self.remotes_page(snapshot, loaded));
    }

    fn daemons_group(&self, snapshot: &Snapshot) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Daemons")
            .description("One daemon runs each profile")
            .build();
        let mut rows = Vec::new();
        for daemon in &snapshot.daemons {
            let row = ui::row(&daemon.profile, "");
            let dot = gtk::Image::from_icon_name("media-record-symbolic");
            row.add_prefix(&dot);
            group.add(&row);
            rows.push(DaemonRow {
                profile: daemon.profile.clone(),
                row,
                dot,
            });
        }
        self.state.borrow_mut().daemon_rows = rows;
        group
    }

    fn syncs_group(
        self: &Rc<Self>,
        snapshot: &Rc<Snapshot>,
        loaded: &Loaded,
    ) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder().title("Syncs").build();
        if loaded.raw.syncs.is_empty() {
            group.set_description(Some(
                "No syncs yet: add a [[sync]] to the config file to sync a folder",
            ));
            return group;
        }

        let sync_all = gtk::Button::builder()
            .label("Sync All")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let weak = Rc::downgrade(self);
        sync_all.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.sync_all();
            }
        });
        group.set_header_suffix(Some(&sync_all));

        let mut rows = Vec::new();
        for (index, sync) in loaded.raw.syncs.iter().enumerate() {
            let label = loaded.label(index);
            let title = text::sync_title(sync, &label);
            let row = ui::row(&title, "");
            row.set_activatable(true);
            // Mapping, then state; a long error is shown whole on the detail page.
            row.set_subtitle_lines(3);

            let spinner = adw::Spinner::new();
            spinner.set_visible(false);
            let error = ui::state_icon("dialog-error-symbolic", "error");
            error.set_visible(false);
            let sync_button = gtk::Button::builder()
                .icon_name("view-refresh-symbolic")
                .tooltip_text("Sync Now")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let menu = gio::Menu::new();
            menu.append(Some("Dry Run"), Some("row.dry-run"));
            menu.append(Some("Open Local Folder"), Some("row.open-folder"));
            let more = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("More")
                .menu_model(&menu)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&spinner);
            row.add_suffix(&error);
            row.add_suffix(&sync_button);
            row.add_suffix(&more);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

            let actions = gio::SimpleActionGroup::new();
            let weak = Rc::downgrade(self);
            let dry_run = gio::ActionEntry::builder("dry-run")
                .activate(move |_: &gio::SimpleActionGroup, _, _| {
                    if let Some(view) = weak.upgrade() {
                        view.dry_run(index);
                    }
                })
                .build();
            let local = loaded
                .resolved_sync(index)
                .map(|s| s.local.clone())
                .unwrap_or_else(|| sync.local.clone());
            let ctx = self.ctx.clone();
            let open_folder = gio::ActionEntry::builder("open-folder")
                .activate(move |_: &gio::SimpleActionGroup, _, _| open_folder(&ctx, &local))
                .build();
            actions.add_action_entries([dry_run, open_folder]);
            row.insert_action_group("row", Some(&actions));

            let weak = Rc::downgrade(self);
            sync_button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.sync_now(index);
                }
            });
            let weak = Rc::downgrade(self);
            let snapshot = snapshot.clone();
            row.connect_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    let page = detail::sync_page(&snapshot, index);
                    view.ctx.nav.push(&page);
                }
            });

            group.add(&row);
            rows.push(SyncRow {
                index,
                label,
                id: sync.id.clone(),
                title,
                row,
                spinner,
                error,
                sync: sync_button,
                actions,
            });
        }
        let mut state = self.state.borrow_mut();
        state.sync_rows = rows;
        state.sync_all = Some(sync_all);
        group
    }

    fn remotes_page(
        self: &Rc<Self>,
        snapshot: &Rc<Snapshot>,
        loaded: &Loaded,
    ) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder().title("Remotes").build();
        if loaded.raw.remotes.is_empty() {
            group.set_description(Some("No remotes: add a [[remote]] to the config file"));
        }
        for (index, remote) in loaded.raw.remotes.iter().enumerate() {
            let title = remote.label.clone().unwrap_or_else(|| remote.id.clone());
            let row = ui::row(
                &title,
                &format!("{} · {}", text::backend(remote.backend), remote.url),
            );
            row.add_prefix(&gtk::Image::from_icon_name(text::backend_icon(
                remote.backend,
            )));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.set_activatable(true);
            let weak = Rc::downgrade(self);
            let snapshot = snapshot.clone();
            row.connect_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.ctx.nav.push(&detail::remote_page(&snapshot, index));
                }
            });
            group.add(&row);
        }
        page.add(&group);

        // Syncs that name their target themselves, without a [[remote]].
        let direct = adw::PreferencesGroup::builder()
            .title("Other Targets")
            .description("Folders and git clones that syncs use directly")
            .build();
        let mut any = false;
        for (index, sync) in loaded.raw.syncs.iter().enumerate() {
            let (title, kind, icon) = if let Some(dir) = &sync.remote_fs {
                (text::path(dir), "Folder", "drive-harddisk-symbolic")
            } else if let Some(repo) = &sync.git_repo {
                (text::path(repo), "Git clone", "network-server-symbolic")
            } else {
                continue;
            };
            any = true;
            let used_by = text::sync_title(sync, &loaded.label(index));
            let row = ui::row(&title, &format!("{kind} · used by {used_by}"));
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.set_activatable(true);
            let weak = Rc::downgrade(self);
            let snapshot = snapshot.clone();
            row.connect_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.ctx.nav.push(&detail::sync_page(&snapshot, index));
                }
            });
            direct.add(&row);
        }
        if any {
            page.add(&direct);
        }
        page
    }

    /// Bring the rows up to date with the latest look.
    fn update(&self) {
        let state = self.state.borrow();
        let Some(snapshot) = &state.snapshot else {
            return;
        };
        let ConfigState::Loaded(loaded) = &snapshot.config else {
            return;
        };

        for row in &state.daemon_rows {
            let Some(daemon) = snapshot.daemons.iter().find(|d| d.profile == row.profile) else {
                continue;
            };
            let unit_name = model::unit_name(&daemon.profile);
            let unit = state.units.as_ref().map(|u| u.get(&unit_name));
            let (summary, class) = daemon_text(self.scope, daemon, unit.flatten(), unit.is_some());
            row.row.set_subtitle(&summary);
            for c in ["success", "warning", "error", "dim-label"] {
                row.dot.remove_css_class(c);
            }
            row.dot.add_css_class(class);
            row.row.set_tooltip_text(daemon_tooltip(daemon).as_deref());
        }

        let now = model::now_secs();
        let mut any_running = false;
        for row in &state.sync_rows {
            let sync = &loaded.raw.syncs[row.index];
            let status = snapshot.status_of(&row.label, row.id.as_deref());
            let busy = state.busy.contains(&row.label) || status.is_some_and(|(_, g)| g.running);
            any_running |= status.is_some();
            let line = match status {
                None => not_running_text(sync),
                Some(_) if busy => "Syncing…".to_string(),
                Some((_, g)) => match (&g.last_error, g.last_run, &g.last_summary) {
                    (Some(e), Some(t), _) => format!("Failed {}: {e}", model::ago(t, now)),
                    (Some(e), None, _) => format!("Failed: {e}"),
                    (None, Some(t), Some(s)) => {
                        format!("Synced {}, {}", model::ago(t, now), model::summary_text(s))
                    }
                    (None, Some(t), None) => format!("Synced {}", model::ago(t, now)),
                    (None, None, _) => "Not synced yet".to_string(),
                },
            };
            row.row.set_subtitle(&format!(
                "{}\n{} · {line}",
                text::mapping(sync),
                text::trigger(sync, &loaded.raw.daemon)
            ));
            let failed = status.is_some_and(|(_, g)| g.last_error.is_some()) && !busy;
            row.error.set_visible(failed);
            row.spinner.set_visible(busy);
            row.sync.set_sensitive(status.is_some() && !busy);
            row.sync.set_tooltip_text(Some(if status.is_some() {
                "Sync Now"
            } else {
                "No running daemon runs this sync"
            }));
            if let Some(action) = row.actions.lookup_action("dry-run") {
                action
                    .downcast::<gio::SimpleAction>()
                    .expect("a simple action")
                    .set_enabled(status.is_some());
            }
            if status.is_some() {
                row.row.remove_css_class("dim-label");
            } else {
                row.row.add_css_class("dim-label");
            }
        }
        if let Some(button) = &state.sync_all {
            button.set_sensitive(any_running);
        }
    }

    /// The daemon running the sync at `index`: its socket and the sync's label.
    fn runner(&self, index: usize) -> Option<(PathBuf, String, String)> {
        let state = self.state.borrow();
        let snapshot = state.snapshot.as_ref()?;
        let row = state.sync_rows.iter().find(|r| r.index == index)?;
        let (daemon, _) = snapshot.status_of(&row.label, row.id.as_deref())?;
        Some((daemon.socket.clone(), row.label.clone(), row.title.clone()))
    }

    fn sync_now(self: &Rc<Self>, index: usize) {
        let Some((socket, label, title)) = self.runner(index) else {
            return;
        };
        self.state.borrow_mut().busy.insert(label.clone());
        self.update();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let target = label.clone();
            let result = gio::spawn_blocking(move || {
                client::send(&socket, &Request::Sync(SyncTarget::Group(target)), None)
            })
            .await
            .expect("the sync request panicked");
            view.state.borrow_mut().busy.remove(&label);
            view.ctx.toast(&sync_outcome(&title, result));
            view.update();
            view.ctx.refresh();
        });
    }

    fn sync_all(self: &Rc<Self>) {
        let (sockets, labels) = {
            let state = self.state.borrow();
            let Some(snapshot) = &state.snapshot else {
                return;
            };
            let sockets: Vec<PathBuf> = snapshot
                .daemons
                .iter()
                .filter(|d| matches!(d.state, DaemonState::Running { .. }))
                .map(|d| d.socket.clone())
                .collect();
            let labels: Vec<String> = state
                .sync_rows
                .iter()
                .filter(|r| snapshot.status_of(&r.label, r.id.as_deref()).is_some())
                .map(|r| r.label.clone())
                .collect();
            (sockets, labels)
        };
        self.state.borrow_mut().busy.extend(labels.iter().cloned());
        self.update();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let results = gio::spawn_blocking(move || {
                sockets
                    .iter()
                    .map(|s| client::send(s, &Request::Sync(SyncTarget::All), None))
                    .collect::<Vec<_>>()
            })
            .await
            .expect("the sync requests panicked");
            {
                let mut state = view.state.borrow_mut();
                for label in &labels {
                    state.busy.remove(label);
                }
            }
            let failed: Vec<String> = results
                .into_iter()
                .flat_map(|r| match r {
                    Ok(Response::Synced(outcomes)) => outcomes
                        .into_iter()
                        .filter_map(|o| o.error.map(|e| format!("{}: {e}", o.label)))
                        .collect(),
                    Ok(Response::Error(e)) => vec![e],
                    Ok(_) => Vec::new(),
                    Err(e) => vec![e.to_string()],
                })
                .collect();
            view.ctx.toast(&match failed.as_slice() {
                [] => "Synced all".to_string(),
                [one] => format!("A sync failed: {one}"),
                many => format!("{} syncs failed; see each sync", many.len()),
            });
            view.update();
            view.ctx.refresh();
        });
    }

    fn dry_run(&self, index: usize) {
        if let Some((socket, label, title)) = self.runner(index) {
            dryrun::show(&self.ctx, socket, label, &title);
        }
    }
}

/// What decides whether the rows must be rebuilt.
fn structure_key(snapshot: &Snapshot) -> String {
    let config = match &snapshot.config {
        ConfigState::Missing => "missing".to_string(),
        ConfigState::Unreadable(e) => format!("unreadable {e}"),
        ConfigState::Invalid(e) => format!("invalid {e}"),
        ConfigState::Loaded(loaded) => loaded.text.clone(),
    };
    let profiles: Vec<&str> = snapshot
        .daemons
        .iter()
        .map(|d| d.profile.as_str())
        .collect();
    format!(
        "{}\n{}\n{config}",
        snapshot.config_path.display(),
        profiles.join(",")
    )
}

/// The page shown instead of the lists when the config cannot be shown.
fn config_status(scope: Scope, path: &std::path::Path, state: &ConfigState) -> adw::StatusPage {
    let path = text::path(path);
    let (icon, title, description) = match state {
        ConfigState::Missing => (
            "document-new-symbolic",
            "No Configuration",
            match scope {
                Scope::User => format!(
                    "Your conflux configuration goes in {path}. The README shows how to start one."
                ),
                Scope::System => format!("There is no system-wide configuration at {path}."),
            },
        ),
        ConfigState::Unreadable(e) => (
            "dialog-password-symbolic",
            "Can't Read the Configuration",
            format!("{path}: {e}"),
        ),
        ConfigState::Invalid(e) => (
            "dialog-error-symbolic",
            "The Configuration Has Errors",
            format!("{path}: {e}"),
        ),
        ConfigState::Loaded(_) => unreachable!("a loaded config is shown as lists"),
    };
    let page = adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(glib::markup_escape_text(&description))
        .build();
    if matches!(state, ConfigState::Invalid(_)) {
        let button = gtk::Button::builder()
            .label("Open Config File")
            .action_name("win.open-config")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        page.set_child(Some(&button));
    }
    page
}

/// A daemon's state in a few words, and the style class of its dot.
fn daemon_text(
    scope: Scope,
    daemon: &Daemon,
    unit: Option<&Unit>,
    systemd_known: bool,
) -> (String, &'static str) {
    let unit_name = model::unit_name(&daemon.profile);
    let unit_active = unit.is_some_and(Unit::is_active);
    let (state, class) = match &daemon.state {
        DaemonState::Running { info, .. } => {
            let mut s = if systemd_known && !unit_active {
                "Running, started outside systemd".to_string()
            } else {
                "Running".to_string()
            };
            if let Some(info) = info.as_ref().filter(|i| i.profile != daemon.profile) {
                s.push_str(&format!(", runs profile {}", info.profile));
            }
            (s, "success")
        }
        DaemonState::NoAccess => (
            "Running; its status is not open to this user".to_string(),
            "warning",
        ),
        DaemonState::Failed(e) => (format!("Not answering: {e}"), "error"),
        DaemonState::NotRunning => match unit.map(|u| u.active_state.as_str()) {
            Some("failed") => ("Failed".to_string(), "error"),
            Some("activating") => ("Starting".to_string(), "warning"),
            _ => ("Not running".to_string(), "dim-label"),
        },
    };
    let mut parts = vec![state];
    match unit {
        Some(u) if u.installed => {
            parts.insert(0, unit_name);
            if u.enabled {
                parts.push(
                    match scope {
                        Scope::User => "Starts at login",
                        Scope::System => "Starts at boot",
                    }
                    .to_string(),
                );
            }
        }
        Some(_) => parts.push("No systemd unit".to_string()),
        None => {}
    }
    (parts.join(" · "), class)
}

fn daemon_tooltip(daemon: &Daemon) -> Option<String> {
    match &daemon.state {
        DaemonState::Running {
            info: Some(info), ..
        } => Some(format!(
            "conflux {}, process {}\nConfig: {}\nSocket: {}",
            info.version,
            info.pid,
            info.config,
            daemon.socket.display()
        )),
        _ => Some(format!("Socket: {}", daemon.socket.display())),
    }
}

fn not_running_text(sync: &conflux_core::config::Sync) -> String {
    let default = sync.profiles.len() == 1 && sync.profiles[0] == "default";
    if default {
        "Not run by a running daemon".to_string()
    } else if sync.profiles.is_empty() {
        "In no profile, so never run".to_string()
    } else {
        format!(
            "Not run by a running daemon (profiles: {})",
            sync.profiles.join(", ")
        )
    }
}

fn sync_outcome(title: &str, result: std::io::Result<Response>) -> String {
    match result {
        Ok(Response::Synced(outcomes)) => match outcomes.first() {
            Some(o) => match (&o.summary, &o.error) {
                (_, Some(e)) => format!("{title} failed: {e}"),
                (Some(s), None) => format!("Synced {title}: {}", model::summary_text(s)),
                (None, None) => format!("Synced {title}"),
            },
            None => format!("{title} was not synced"),
        },
        Ok(Response::Error(e)) => format!("{title}: {e}"),
        Ok(_) => format!("{title}: unexpected answer from the daemon"),
        Err(e) => format!("Could not reach the daemon: {e}"),
    }
}

fn open_folder(ctx: &Rc<Ctx>, path: &std::path::Path) {
    let ctx2 = ctx.clone();
    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(
        Some(&ctx.window),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(e) = result {
                ctx2.toast(&format!("Could not open the folder: {}", e.message()));
            }
        },
    );
}
