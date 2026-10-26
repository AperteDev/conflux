//! The Syncs and Remotes pages of one scope.
//!
//! Rows are rebuilt only when the config or the set of daemons changes; every
//! other look updates them in place, so an open menu or the scroll position
//! survives polling.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use conflux_core::helper::{HelperRequest, HelperResponse, UnitAction};
use conflux_core::ipc::{client, Request, Response, SyncTarget};
use gtk::{gio, glib};

use crate::editor::{self, Edit, EditError};
use crate::helper::Helper;
use crate::model::{self, ConfigState, Daemon, DaemonState, Loaded, Scope, Snapshot};
use crate::systemd::{Systemd, Unit};
use crate::window::Ctx;
use crate::{daemons, detail, dryrun, text, ui};

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
    /// Bumped whenever the syncs or remotes may have moved in the file (it
    /// changed on disk, or one was removed here): pages built before then refuse
    /// edits, as their positions may point elsewhere now.
    generation: Cell<u64>,
    /// The text this app wrote last, until a look at the file confirms it.
    pending: RefCell<Option<String>>,
    /// Edits waiting to be written, one at a time.
    queue: RefCell<VecDeque<Job>>,
    editing: Cell<bool>,
    /// Banners of open pages, shown when the file changes on disk under them.
    banners: RefCell<Vec<glib::WeakRef<adw::Banner>>>,
    monitor: RefCell<Option<(PathBuf, gio::FileMonitor)>>,
    /// The privileged helper, while the system scope is unlocked.
    helper: RefCell<Option<Rc<Helper>>>,
    unlocking: Cell<bool>,
}

struct Job {
    generation: u64,
    edit: Edit,
    done: Box<dyn FnOnce(Result<(), String>)>,
}

#[derive(Default)]
struct State {
    snapshot: Option<Rc<Snapshot>>,
    /// `None` when systemd could not be asked.
    systemd: Option<Systemd>,
    /// What the rows were built from.
    built_from: Option<String>,
    sync_rows: Vec<SyncRow>,
    daemon_rows: Vec<DaemonRow>,
    linger: Option<(adw::SwitchRow, Rc<Cell<bool>>)>,
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
    run: gtk::Button,
    enable: gtk::Switch,
    /// Set while the switch follows systemd, so that is not taken for a click.
    quiet: Rc<Cell<bool>>,
    actions: gio::SimpleActionGroup,
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
            generation: Cell::new(0),
            pending: RefCell::default(),
            queue: RefCell::default(),
            editing: Cell::new(false),
            banners: RefCell::default(),
            monitor: RefCell::default(),
            helper: RefCell::default(),
            unlocking: Cell::new(false),
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

    pub fn snapshot(&self) -> Option<Rc<Snapshot>> {
        self.state.borrow().snapshot.clone()
    }

    /// What systemd last said about the scope's daemons.
    pub fn systemd(&self) -> Option<Systemd> {
        self.state.borrow().systemd.clone()
    }

    pub fn ctx(&self) -> &Rc<Ctx> {
        &self.ctx
    }

    /// Whether this app may change the scope's config and daemons: the user's
    /// always, the system's once unlocked.
    pub fn editable(&self) -> bool {
        self.scope == Scope::User || self.helper.borrow().is_some()
    }

    /// Whether this is the system scope, locked.
    pub fn locked(&self) -> bool {
        self.scope == Scope::System && self.helper.borrow().is_none()
    }

    /// The helper, while unlocked.
    pub fn helper(&self) -> Option<Rc<Helper>> {
        self.helper.borrow().clone()
    }

    /// Start the helper, which asks for authentication; editing is allowed once
    /// it answers.
    pub fn unlock(self: &Rc<Self>) {
        if self.scope != Scope::System || self.helper().is_some() || self.unlocking.replace(true) {
            return;
        }
        let view = self.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let started = Helper::start(move || {
                if let Some(view) = weak.upgrade() {
                    // Gone on its own: idle too long, or killed. (After Lock it
                    // is taken already.)
                    let helper = view.helper.borrow_mut().take();
                    if helper.is_some() {
                        view.ctx.toast("Locked again");
                        view.lock_changed();
                    }
                }
            })
            .await;
            view.unlocking.set(false);
            match started {
                Ok(helper) => {
                    *view.helper.borrow_mut() = Some(helper);
                    view.lock_changed();
                }
                Err(e) => view.ctx.toast(&e),
            }
        });
    }

    /// Let the helper go: the system scope is read-only again.
    pub fn lock(&self) {
        // Taken first: the borrow must end before the UI looks at the lock again.
        let helper = self.helper.borrow_mut().take();
        if let Some(helper) = helper {
            helper.stop();
            self.lock_changed();
        }
    }

    /// Rows and pages were built for the old lock state: build them again.
    fn lock_changed(&self) {
        self.state.borrow_mut().built_from = None;
        self.ctx.nav.pop_to_tag("main");
        self.ctx.changed();
        self.ctx.refresh();
    }

    /// Ask `profile`'s daemon (on `socket`) something: through the helper while
    /// unlocked, so a system daemon takes it from root; directly otherwise.
    pub async fn ask(
        &self,
        socket: PathBuf,
        profile: String,
        request: Request,
    ) -> Result<Response, String> {
        if let Some(helper) = self.helper() {
            return match helper
                .call(&HelperRequest::Ipc { profile, request })
                .await?
            {
                HelperResponse::Ipc(response) => Ok(response),
                HelperResponse::Error(e) => Err(e),
                _ => Err("unexpected answer from the helper".to_string()),
            };
        }
        gio::spawn_blocking(move || client::send(&socket, &request, None))
            .await
            .expect("the daemon request panicked")
            .map_err(|e| format!("Could not reach the daemon: {e}"))
    }

    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Pages built before now must not edit by position any more.
    pub fn bump_generation(&self) {
        self.generation.set(self.generation.get() + 1);
    }

    /// Show `banner` when the file changes on disk while its page is open.
    pub fn watch_page(&self, banner: &adw::Banner) {
        let mut banners = self.banners.borrow_mut();
        banners.retain(|b| b.upgrade().is_some());
        banners.push(banner.downgrade());
    }

    /// Write `edit` to the config, after the edits queued before it. `done`
    /// learns whether it was written, or why not. `generation` is the one the
    /// edit's page was built in.
    pub fn edit(
        self: &Rc<Self>,
        generation: u64,
        edit: Edit,
        done: impl FnOnce(Result<(), String>) + 'static,
    ) {
        self.queue.borrow_mut().push_back(Job {
            generation,
            edit,
            done: Box::new(done),
        });
        self.pump();
    }

    fn pump(self: &Rc<Self>) {
        if self.editing.get() {
            return;
        }
        let Some(job) = self.queue.borrow_mut().pop_front() else {
            return;
        };
        let Job {
            generation,
            edit,
            done,
        } = job;
        if generation != self.generation.get() {
            done(Err(EditError::Changed.to_string()));
            return self.pump();
        }
        let target = {
            let state = self.state.borrow();
            state.snapshot.as_ref().and_then(|s| match &s.config {
                ConfigState::Loaded(loaded) => Some((
                    s.config_path.clone(),
                    self.pending
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| loaded.text.clone()),
                    running(s),
                )),
                _ => None,
            })
        };
        let Some((path, base, daemons)) = target else {
            done(Err("There is no config file to change".to_string()));
            return self.pump();
        };
        let helper = match self.scope {
            Scope::User => None,
            Scope::System => match self.helper() {
                Some(helper) => Some(helper),
                None => {
                    done(Err("Unlock to change the system configuration".to_string()));
                    return self.pump();
                }
            },
        };
        self.editing.set(true);
        let view = self.clone();
        glib::spawn_future_local(async move {
            let result = match helper {
                // The user's own file: written here.
                None => {
                    let sockets = daemons
                        .into_iter()
                        .map(|(socket, _)| socket)
                        .collect::<Vec<_>>();
                    gio::spawn_blocking(move || editor::apply(&path, &base, edit, &sockets))
                        .await
                        .expect("writing the config panicked")
                }
                // The system's: made here, written by the helper.
                Some(helper) => {
                    let base2 = base.clone();
                    let prepared =
                        gio::spawn_blocking(move || editor::prepare(&path, &base2, edit))
                            .await
                            .expect("editing the config panicked");
                    match prepared {
                        Ok(None) => Ok(editor::Applied {
                            text: base,
                            reload_problems: Vec::new(),
                        }),
                        Ok(Some(new)) => {
                            let request = HelperRequest::WriteConfig {
                                expected: Some(base),
                                text: new.clone(),
                                reload: daemons.into_iter().map(|(_, profile)| profile).collect(),
                            };
                            match helper.call(&request).await {
                                Ok(HelperResponse::Done { problems }) => Ok(editor::Applied {
                                    text: new,
                                    reload_problems: problems,
                                }),
                                Ok(HelperResponse::Changed) => Err(EditError::Changed),
                                Ok(HelperResponse::Error(e)) => Err(EditError::Refused(e)),
                                Ok(_) => Err(EditError::Failed(
                                    "unexpected answer from the helper".to_string(),
                                )),
                                Err(e) => Err(EditError::Failed(e)),
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            view.editing.set(false);
            match result {
                Ok(applied) => {
                    *view.pending.borrow_mut() = Some(applied.text);
                    for problem in applied.reload_problems {
                        view.ctx
                            .toast(&format!("A daemon did not take the change: {problem}"));
                    }
                    done(Ok(()));
                    view.ctx.refresh();
                }
                Err(e @ EditError::Changed) => {
                    done(Err(e.to_string()));
                    view.ctx.refresh();
                }
                Err(e) => done(Err(e.to_string())),
            }
            view.pump();
        });
    }

    /// Start the scope's config from the example.
    fn create_config(self: &Rc<Self>) {
        let Some(path) = self.config_path() else {
            return;
        };
        let view = self.clone();
        let helper = self.helper();
        glib::spawn_future_local(async move {
            let result = match helper {
                None => gio::spawn_blocking(move || editor::create(&path))
                    .await
                    .expect("creating the config panicked"),
                Some(helper) => {
                    let request = HelperRequest::WriteConfig {
                        expected: None,
                        text: editor::EXAMPLE.to_string(),
                        reload: Vec::new(),
                    };
                    match helper.call(&request).await {
                        Ok(HelperResponse::Done { .. }) => Ok(()),
                        Ok(HelperResponse::Changed) => Err("there is one already".to_string()),
                        Ok(HelperResponse::Error(e)) | Err(e) => Err(e),
                        Ok(_) => Err("unexpected answer from the helper".to_string()),
                    }
                }
            };
            match result {
                Ok(()) => view.ctx.toast("Created the config file"),
                Err(e) => view
                    .ctx
                    .toast(&format!("Could not create the config file: {e}")),
            }
            view.ctx.refresh();
        });
    }

    /// Tell whether the file changed on disk by someone else, and follow it.
    fn note_change(self: &Rc<Self>, old: Option<&str>, new: Option<&str>) {
        let mut pending = self.pending.borrow_mut();
        if new.is_some() && new == pending.as_deref() {
            // Our own edit, now on disk.
            *pending = None;
        } else if new == old {
            // Nothing new, or a look taken before our last edit was written.
        } else {
            *pending = None;
            if old.is_some() {
                self.bump_generation();
                for banner in self.banners.borrow().iter().filter_map(|b| b.upgrade()) {
                    banner.set_revealed(true);
                }
            }
        }
    }

    /// Look again as soon as the config file changes, not only when polling.
    fn watch_file(&self, path: &std::path::Path) {
        let mut monitor = self.monitor.borrow_mut();
        if monitor.as_ref().is_some_and(|(p, _)| p == path) {
            return;
        }
        *monitor = gio::File::for_path(path)
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .ok()
            .map(|m| {
                let ctx = Rc::downgrade(&self.ctx);
                m.connect_changed(move |_, _, _, _| {
                    if let Some(ctx) = ctx.upgrade() {
                        ctx.refresh();
                    }
                });
                (path.to_path_buf(), m)
            });
    }

    /// Show a fresh look at the scope.
    pub fn show(self: &Rc<Self>, snapshot: model::Snapshot, systemd: Result<Systemd, glib::Error>) {
        let snapshot = Rc::new(snapshot);
        let key = structure_key(&snapshot);
        let old_text = self
            .state
            .borrow()
            .snapshot
            .as_ref()
            .and_then(|s| loaded_text(s));
        self.note_change(old_text.as_deref(), loaded_text(&snapshot).as_deref());
        self.watch_file(&snapshot.config_path);
        let rebuild = {
            let mut state = self.state.borrow_mut();
            state.snapshot = Some(snapshot.clone());
            state.systemd = systemd.ok();
            state.built_from.as_deref() != Some(key.as_str())
        };
        if rebuild {
            self.rebuild(&snapshot);
            self.state.borrow_mut().built_from = Some(key);
        }
        self.update();
        self.ctx.changed();
    }

    fn rebuild(self: &Rc<Self>, snapshot: &Rc<Snapshot>) {
        {
            let mut state = self.state.borrow_mut();
            state.sync_rows.clear();
            state.daemon_rows.clear();
            state.linger = None;
            state.sync_all = None;
        }
        let loaded = match &snapshot.config {
            ConfigState::Loaded(loaded) => loaded,
            other => {
                let page = self.config_status(&snapshot.config_path, other);
                ui::replace_child(&self.syncs, "status", &page);
                let page = self.config_status(&snapshot.config_path, other);
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

    fn daemons_group(self: &Rc<Self>, snapshot: &Rc<Snapshot>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Daemons")
            .description("One daemon runs each profile")
            .build();
        if self.editable() {
            let add = gtk::Button::builder()
                .label("Add")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let weak = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    daemons::add(&view);
                }
            });
            group.set_header_suffix(Some(&add));
        }
        let mut rows = Vec::new();
        for daemon in &snapshot.daemons {
            let profile = daemon.profile.clone();
            let row = ui::row(&profile, "");
            row.set_activatable(true);
            let dot = gtk::Image::from_icon_name("media-record-symbolic");
            row.add_prefix(&dot);

            let run = gtk::Button::builder()
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .visible(false)
                .build();
            let enable = gtk::Switch::builder()
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            let menu = gio::Menu::new();
            menu.append(Some("Restart"), Some("daemon.restart"));
            menu.append(Some("View Log"), Some("daemon.log"));
            if profile != conflux_core::paths::DEFAULT_PROFILE {
                menu.append(Some("Remove"), Some("daemon.remove"));
            }
            let more = gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("More")
                .menu_model(&menu)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&run);
            row.add_suffix(&enable);
            row.add_suffix(&more);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

            let actions = gio::SimpleActionGroup::new();
            let action = |name: &str, run: fn(&Rc<ScopeView>, &str)| {
                let (weak, profile) = (Rc::downgrade(self), profile.clone());
                gio::ActionEntry::builder(name)
                    .activate(move |_: &gio::SimpleActionGroup, _, _| {
                        if let Some(view) = weak.upgrade() {
                            run(&view, &profile);
                        }
                    })
                    .build()
            };
            actions.add_action_entries([
                action("restart", |view, p| {
                    daemons::control(view, p, UnitAction::Restart)
                }),
                action("log", daemons::show_log),
                action("remove", daemons::remove),
            ]);
            row.insert_action_group("daemon", Some(&actions));

            {
                let (weak, profile) = (Rc::downgrade(self), profile.clone());
                run.connect_clicked(move |button| {
                    if let Some(view) = weak.upgrade() {
                        let action = if button.icon_name().as_deref()
                            == Some("media-playback-stop-symbolic")
                        {
                            UnitAction::Stop
                        } else {
                            UnitAction::Start
                        };
                        daemons::control(&view, &profile, action);
                    }
                });
            }
            let quiet = Rc::new(Cell::new(false));
            {
                let (weak, profile, quiet) = (Rc::downgrade(self), profile.clone(), quiet.clone());
                enable.connect_active_notify(move |switch| {
                    if quiet.get() {
                        return;
                    }
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let (switch2, quiet) = (switch.clone(), quiet.clone());
                    daemons::set_enabled(&view, &profile, switch.is_active(), move || {
                        quiet.set(true);
                        switch2.set_active(!switch2.is_active());
                        quiet.set(false);
                    });
                });
            }
            {
                let (weak, profile, snapshot) =
                    (Rc::downgrade(self), profile.clone(), snapshot.clone());
                row.connect_activated(move |_| {
                    if let Some(view) = weak.upgrade() {
                        let page = daemons::page(&view, &snapshot, &profile);
                        view.ctx.nav.push(&page);
                    }
                });
            }
            group.add(&row);
            rows.push(DaemonRow {
                profile,
                row,
                dot,
                run,
                enable,
                quiet,
                actions,
            });
        }

        let mut linger = None;
        if self.scope == Scope::User {
            let row = adw::SwitchRow::builder()
                .title("Keep running after logout")
                .subtitle("Without it, your daemons stop when you log out")
                .sensitive(self.editable())
                .build();
            let quiet = Rc::new(Cell::new(false));
            let (weak, quiet2) = (Rc::downgrade(self), quiet.clone());
            row.connect_active_notify(move |row| {
                if quiet2.get() {
                    return;
                }
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let (row2, quiet) = (row.clone(), quiet2.clone());
                daemons::set_linger(&view, row.is_active(), move || {
                    quiet.set(true);
                    row2.set_active(!row2.is_active());
                    quiet.set(false);
                });
            });
            group.add(&row);
            linger = Some((row, quiet));
        }
        let mut state = self.state.borrow_mut();
        state.daemon_rows = rows;
        state.linger = linger;
        group
    }

    fn syncs_group(
        self: &Rc<Self>,
        snapshot: &Rc<Snapshot>,
        loaded: &Loaded,
    ) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder().title("Syncs").build();
        if loaded.raw.syncs.is_empty() {
            group.set_description(Some(if self.editable() {
                "No syncs yet: add one with the + button"
            } else {
                "No syncs yet"
            }));
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
                    let page = detail::sync_page(&view, &snapshot, index);
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
            group.set_description(Some(if self.editable() {
                "No remotes yet: add one with the + button"
            } else {
                "No remotes yet"
            }));
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
                    view.ctx
                        .nav
                        .push(&detail::remote_page(&view, &snapshot, index));
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
                (
                    text::path(self.scope, dir),
                    "Folder",
                    "drive-harddisk-symbolic",
                )
            } else if let Some(repo) = &sync.git_repo {
                (
                    text::path(self.scope, repo),
                    "Git clone",
                    "network-server-symbolic",
                )
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
                    view.ctx
                        .nav
                        .push(&detail::sync_page(&view, &snapshot, index));
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

        let editable = self.editable();
        for row in &state.daemon_rows {
            let Some(daemon) = snapshot.daemons.iter().find(|d| d.profile == row.profile) else {
                continue;
            };
            let unit_name = model::unit_name(&daemon.profile);
            let unit = state.systemd.as_ref().and_then(|s| s.unit(&unit_name));
            let (summary, class) = daemon_text(self.scope, daemon, unit, state.systemd.is_some());
            row.row.set_subtitle(&summary);
            for c in ["success", "warning", "error", "dim-label"] {
                row.dot.remove_css_class(c);
            }
            row.dot.add_css_class(class);
            row.row.set_tooltip_text(daemon_tooltip(daemon).as_deref());

            // Controls for a daemon systemd runs; one started by hand has none.
            let installed = unit.is_some_and(|u| u.installed);
            let active = unit.is_some_and(Unit::is_active);
            let outside = daemons::runs_outside(snapshot, &daemon.profile, active);
            row.run.set_visible(installed);
            row.run.set_sensitive(editable && !outside);
            row.run.set_icon_name(if active {
                "media-playback-stop-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
            row.run.set_tooltip_text(Some(if outside {
                "Running outside systemd; stop it where it was started"
            } else if active {
                "Stop"
            } else {
                "Start"
            }));
            row.enable.set_visible(installed);
            row.enable.set_sensitive(editable);
            row.enable.set_tooltip_text(Some(match self.scope {
                Scope::User => "Start at login",
                Scope::System => "Start at boot",
            }));
            let enabled = unit.is_some_and(|u| u.enabled);
            if row.enable.is_active() != enabled {
                row.quiet.set(true);
                row.enable.set_active(enabled);
                row.quiet.set(false);
            }
            for (name, on) in [
                ("restart", editable && installed && active),
                ("log", installed),
                ("remove", editable),
            ] {
                if let Some(action) = row.actions.lookup_action(name) {
                    action
                        .downcast::<gio::SimpleAction>()
                        .expect("a simple action")
                        .set_enabled(on);
                }
            }
        }
        if let (Some((row, quiet)), Some(systemd)) = (&state.linger, &state.systemd) {
            if row.is_active() != systemd.linger {
                quiet.set(true);
                row.set_active(systemd.linger);
                quiet.set(false);
            }
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
                text::mapping(self.scope, sync),
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

    /// The daemon running the sync at `index` (its socket and profile), and
    /// the sync's label and title.
    fn runner(&self, index: usize) -> Option<(PathBuf, String, String, String)> {
        let state = self.state.borrow();
        let snapshot = state.snapshot.as_ref()?;
        let row = state.sync_rows.iter().find(|r| r.index == index)?;
        let (daemon, _) = snapshot.status_of(&row.label, row.id.as_deref())?;
        Some((
            daemon.socket.clone(),
            daemon.profile.clone(),
            row.label.clone(),
            row.title.clone(),
        ))
    }

    fn sync_now(self: &Rc<Self>, index: usize) {
        let Some((socket, profile, label, title)) = self.runner(index) else {
            return;
        };
        self.state.borrow_mut().busy.insert(label.clone());
        self.update();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let request = Request::Sync(SyncTarget::Group(label.clone()));
            let result = view.ask(socket, profile, request).await;
            view.state.borrow_mut().busy.remove(&label);
            view.ctx.toast(&sync_outcome(&title, result));
            view.update();
            view.ctx.refresh();
        });
    }

    fn sync_all(self: &Rc<Self>) {
        let (daemons, labels) = {
            let state = self.state.borrow();
            let Some(snapshot) = &state.snapshot else {
                return;
            };
            let labels: Vec<String> = state
                .sync_rows
                .iter()
                .filter(|r| snapshot.status_of(&r.label, r.id.as_deref()).is_some())
                .map(|r| r.label.clone())
                .collect();
            (running(snapshot), labels)
        };
        self.state.borrow_mut().busy.extend(labels.iter().cloned());
        self.update();
        let view = self.clone();
        glib::spawn_future_local(async move {
            let mut failed = Vec::new();
            for (socket, profile) in daemons {
                match view
                    .ask(socket, profile, Request::Sync(SyncTarget::All))
                    .await
                {
                    Ok(Response::Synced(outcomes)) => failed.extend(
                        outcomes
                            .into_iter()
                            .filter_map(|o| o.error.map(|e| format!("{}: {e}", o.label))),
                    ),
                    Ok(Response::Error(e)) | Err(e) => failed.push(e),
                    Ok(_) => {}
                }
            }
            {
                let mut state = view.state.borrow_mut();
                for label in &labels {
                    state.busy.remove(label);
                }
            }
            view.ctx.toast(&match failed.as_slice() {
                [] => "Synced all".to_string(),
                [one] => format!("A sync failed: {one}"),
                many => format!("{} syncs failed; see each sync", many.len()),
            });
            view.update();
            view.ctx.refresh();
        });
    }

    fn dry_run(self: &Rc<Self>, index: usize) {
        if let Some((socket, profile, label, title)) = self.runner(index) {
            dryrun::show(self, socket, profile, label, &title);
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

/// The text of the snapshot's config file, when it could be read.
fn loaded_text(snapshot: &Snapshot) -> Option<String> {
    match &snapshot.config {
        ConfigState::Loaded(loaded) => Some(loaded.text.clone()),
        _ => None,
    }
}

/// The socket and profile of each of the snapshot's running daemons.
fn running(snapshot: &Snapshot) -> Vec<(PathBuf, String)> {
    snapshot
        .daemons
        .iter()
        .filter(|d| matches!(d.state, DaemonState::Running { .. }))
        .map(|d| (d.socket.clone(), d.profile.clone()))
        .collect()
}

impl ScopeView {
    /// The page shown instead of the lists when the config cannot be shown.
    fn config_status(
        self: &Rc<Self>,
        path: &std::path::Path,
        state: &ConfigState,
    ) -> adw::StatusPage {
        let path = text::path(self.scope, path);
        let (icon, title, description) = match state {
            ConfigState::Missing => (
                "document-new-symbolic",
                "No Configuration",
                match self.scope {
                    Scope::User => format!(
                        "Your conflux configuration goes in {path}. Start one from the example, \
                     which lists every option with its default."
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
        } else if matches!(state, ConfigState::Missing) && self.editable() {
            let button = gtk::Button::builder()
                .label("Create Config File")
                .halign(gtk::Align::Center)
                .css_classes(["pill", "suggested-action"])
                .build();
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.create_config();
                }
            });
            page.set_child(Some(&button));
        }
        page
    }
}

/// A daemon's state in a few words, and the style class of its dot.
pub fn daemon_text(
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

fn sync_outcome(title: &str, result: Result<Response, String>) -> String {
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
        Err(e) => format!("{title}: {e}"),
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
