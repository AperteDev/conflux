//! The main window: the scope switch, the pages of both scopes, polling, and the
//! window's actions.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::model::{self, Scope};
use crate::scope::ScopeView;
use crate::ui::{self, builder};
use crate::{add, prefs, systemd};

/// How often the shown scope is looked at again while the window is visible.
const POLL_SECONDS: u32 = 3;

/// What the pages share: where to show dialogs, toasts and detail pages, and how
/// to ask for a fresh look.
pub struct Ctx {
    pub window: adw::ApplicationWindow,
    pub nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    win: RefCell<Weak<Window>>,
}

impl Ctx {
    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        self.toasts.add_toast(toast);
    }

    /// Look at the shown scope again now.
    pub fn refresh(&self) {
        if let Some(win) = self.win.borrow().upgrade() {
            win.refresh();
        }
    }

    /// A scope was looked at again: the banner and the actions may change.
    pub fn changed(&self) {
        if let Some(win) = self.win.borrow().upgrade() {
            win.show_state();
        }
    }
}

pub struct Window {
    ctx: Rc<Ctx>,
    closed: Cell<bool>,
    scope_toggle: adw::ToggleGroup,
    banner: adw::Banner,
    lock_banner: adw::Banner,
    user: Rc<ScopeView>,
    system: Rc<ScopeView>,
}

pub fn open(app: &adw::Application) {
    let b = builder!("window");
    let window: adw::ApplicationWindow = ui::get(&b, "window");
    window.set_application(Some(app));
    let ctx = Rc::new(Ctx {
        window: window.clone(),
        nav: ui::get(&b, "nav"),
        toasts: ui::get(&b, "toasts"),
        win: RefCell::new(Weak::new()),
    });
    let syncs: gtk::Stack = ui::get(&b, "syncs_stack");
    let remotes: gtk::Stack = ui::get(&b, "remotes_stack");
    let win = Rc::new(Window {
        user: ScopeView::new(Scope::User, ctx.clone(), &syncs, &remotes),
        system: ScopeView::new(Scope::System, ctx.clone(), &syncs, &remotes),
        scope_toggle: ui::get(&b, "scope"),
        banner: ui::get(&b, "banner"),
        lock_banner: ui::get(&b, "lock_banner"),
        closed: Cell::new(false),
        ctx,
    });
    *win.ctx.win.borrow_mut() = Rc::downgrade(&win);

    let weak = Rc::downgrade(&win);
    win.scope_toggle.connect_active_name_notify(move |_| {
        if let Some(win) = weak.upgrade() {
            win.scope_changed();
        }
    });
    let weak = Rc::downgrade(&win);
    win.lock_banner.connect_button_clicked(move |_| {
        if let Some(win) = weak.upgrade() {
            let view = win.view(Scope::System);
            if view.locked() {
                view.unlock();
            } else {
                view.lock();
            }
        }
    });
    let weak = Rc::downgrade(&win);
    win.banner.connect_button_clicked(move |_| {
        if let Some(win) = weak.upgrade() {
            win.show_problem_details();
        }
    });
    add_actions(&win);

    let weak = Rc::downgrade(&win);
    window.connect_destroy(move |_| {
        if let Some(win) = weak.upgrade() {
            win.closed.set(true);
            // Unlocked lasts as long as the window.
            win.system.lock();
        }
    });

    // Poll while the window can be seen; a minimized one is left alone. The
    // timer owns the window's state, until the window is gone.
    let owner = win.clone();
    glib::timeout_add_seconds_local(POLL_SECONDS, move || {
        if owner.closed.get() {
            return glib::ControlFlow::Break;
        }
        if owner.ctx.window.is_visible() && !owner.ctx.window.is_suspended() {
            owner.refresh();
        }
        glib::ControlFlow::Continue
    });

    win.scope_changed();
    window.present();
}

impl Window {
    fn scope(&self) -> Scope {
        self.scope_toggle
            .active_name()
            .and_then(|name| Scope::from_name(&name))
            .unwrap_or(Scope::User)
    }

    fn view(&self, scope: Scope) -> &Rc<ScopeView> {
        match scope {
            Scope::User => &self.user,
            Scope::System => &self.system,
        }
    }

    fn scope_changed(&self) {
        let scope = self.scope();
        self.view(scope).present();
        self.show_state();
        self.refresh();
    }

    /// Follow the shown scope: its config's problem in the banner, and what can
    /// be added or set.
    fn show_state(&self) {
        self.show_problem();
        let view = self.view(self.scope());
        // Like Settings' Printers: the system's settings are locked until unlocked.
        if view.scope == Scope::System {
            if view.locked() {
                self.lock_banner.set_title("Some settings are locked");
                self.lock_banner.set_button_label(Some("Unlock…"));
            } else {
                self.lock_banner
                    .set_title("Unlocked: changes apply to the whole system");
                self.lock_banner.set_button_label(Some("Lock"));
            }
            self.lock_banner.set_revealed(true);
        } else {
            self.lock_banner.set_revealed(false);
        }
        let can_edit = view.editable()
            && view
                .snapshot()
                .is_some_and(|s| matches!(s.config, model::ConfigState::Loaded(_)));
        for name in ["add-sync", "add-remote", "preferences"] {
            if let Some(action) = self.ctx.window.lookup_action(name) {
                action
                    .downcast::<gio::SimpleAction>()
                    .expect("a simple action")
                    .set_enabled(can_edit || name == "preferences");
            }
        }
    }

    /// Load the shown scope again, unless a load is still running.
    fn refresh(&self) {
        let view = self.view(self.scope()).clone();
        if view.loading.replace(true) {
            return;
        }
        let scope = view.scope;
        glib::spawn_future_local(async move {
            let snapshot = gio::spawn_blocking(move || model::load(scope))
                .await
                .expect("loading the config and daemon state panicked");
            let names: Vec<String> = snapshot
                .daemons
                .iter()
                .map(|d| model::unit_name(&d.profile))
                .collect();
            let systemd = systemd::load(scope, &names).await;
            view.show(snapshot, systemd);
            view.loading.set(false);
        });
    }

    fn show_problem(&self) {
        match self.view(self.scope()).problem() {
            Some(_) => {
                self.banner.set_title(
                    "This configuration has an error; the daemon keeps running the last valid one",
                );
                self.banner.set_button_label(Some("Details"));
                self.banner.set_revealed(true);
            }
            None => self.banner.set_revealed(false),
        }
    }

    fn show_problem_details(&self) {
        let Some(problem) = self.view(self.scope()).problem() else {
            return;
        };
        let dialog = adw::AlertDialog::new(Some("Configuration Error"), Some(&problem));
        dialog.add_response("close", "Close");
        dialog.present(Some(&self.ctx.window));
    }

    fn open_config(&self) {
        let Some(path) = self.view(self.scope()).config_path() else {
            return;
        };
        if !path.exists() {
            self.ctx
                .toast(&format!("There is no config file at {}", path.display()));
            return;
        }
        let ctx = self.ctx.clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(
            Some(&self.ctx.window),
            gio::Cancellable::NONE,
            move |result| {
                if let Err(e) = result {
                    ctx.toast(&format!("Could not open the config file: {}", e.message()));
                }
            },
        );
    }
}

fn add_actions(win: &Rc<Window>) {
    let weak = Rc::downgrade(win);
    let refresh = gio::ActionEntry::builder("refresh")
        .activate(move |_: &adw::ApplicationWindow, _, _| {
            if let Some(win) = weak.upgrade() {
                win.refresh();
            }
        })
        .build();
    let weak = Rc::downgrade(win);
    let open_config = gio::ActionEntry::builder("open-config")
        .activate(move |_: &adw::ApplicationWindow, _, _| {
            if let Some(win) = weak.upgrade() {
                win.open_config();
            }
        })
        .build();
    let entry = |name: &str, run: fn(&Rc<ScopeView>)| {
        let weak = Rc::downgrade(win);
        gio::ActionEntry::builder(name)
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(win) = weak.upgrade() {
                    run(win.view(win.scope()));
                }
            })
            .build()
    };
    let add_sync = entry("add-sync", add::add_sync);
    let add_remote = entry("add-remote", add::add_remote);
    let preferences = entry("preferences", prefs::show);
    win.ctx
        .window
        .add_action_entries([refresh, open_config, add_sync, add_remote, preferences]);
}
