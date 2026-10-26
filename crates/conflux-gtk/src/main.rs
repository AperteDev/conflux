//! Conflux: a GTK app to view the conflux configuration and the daemons running
//! it, for the user and the whole system. Optional: the config file stays the
//! source of truth and everything works without this app.

mod add;
mod detail;
mod dryrun;
mod editor;
mod form;
mod model;
mod prefs;
mod scope;
mod systemd;
mod text;
mod ui;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "dev.aperte.Conflux";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => window::open(app),
    });

    let about = gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| about(app))
        .build();
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    app.add_action_entries([about, quit]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("win.refresh", &["<Control>r", "F5"]);
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);

    app.run()
}

fn about(app: &adw::Application) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Conflux")
        .application_icon("emblem-synchronizing-symbolic")
        .developer_name("SelfRef")
        .version(env!("CARGO_PKG_VERSION"))
        .website(env!("CARGO_PKG_REPOSITORY"))
        .issue_url(format!("{}/issues", env!("CARGO_PKG_REPOSITORY")))
        .license_type(gtk::License::Gpl30)
        .comments("View the conflux configuration and the daemons running it")
        .build();
    dialog.present(app.active_window().as_ref());
}
