//! Preferences: the `[daemon]` table of the shown scope, the defaults every sync
//! inherits unless it (or its remote) sets its own.

use std::rc::Rc;

use adw::prelude::*;
use conflux_core::config::DEFAULT_EXCLUDES;
use conflux_core::config_edit::Section;
use conflux_core::model::{Deletions, EmptyDirMode};

use crate::detail::{group, options};
use crate::form::{self, Form, Kind, Toaster};
use crate::model::ConfigState;
use crate::scope::ScopeView;
use crate::text;

pub fn show(view: &Rc<ScopeView>) {
    let Some(snapshot) = view.snapshot() else {
        return;
    };
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        view.ctx()
            .toast("There is no config file to set preferences in");
        return;
    };
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .search_enabled(false)
        .build();
    let weak = dialog.downgrade();
    let toast: Toaster = Rc::new(move |message: &str| {
        if let Some(dialog) = weak.upgrade() {
            let toast = adw::Toast::new(message);
            toast.set_use_markup(false);
            dialog.add_toast(toast);
        }
    });
    let form = Form::new(view, Section::Daemon, toast);
    let doc = &loaded.doc;
    let w = |key| form::written(doc, Section::Daemon, key);
    let daemon = &loaded.raw.daemon;

    let page = adw::PreferencesPage::builder()
        .title("Daemon")
        .icon_name("emblem-system-symbolic")
        .build();
    if !form.editable() {
        page.set_description("Unlock to change the system configuration");
    }
    page.add(&group(
        "Daemon",
        &[
            form.choice(
                "Log level",
                "log_level",
                options(&[
                    (Some("error"), "Errors"),
                    (Some("warn"), "Warnings"),
                    (Some("info"), "Information"),
                    (Some("debug"), "Debugging"),
                    (Some("trace"), "Everything"),
                ]),
                w("log_level"),
                Some("info"),
            )
            .upcast_ref(),
            form.entry(
                "Profile run by the plain unit",
                "profile",
                w("profile"),
                Kind::Text,
                Some("default"),
                false,
            )
            .upcast_ref(),
        ],
    ));

    let defaults = group(
        "Defaults for Syncs",
        &[
            form.entry(
                "Debounce",
                "debounce",
                w("debounce"),
                Kind::Duration,
                Some("5s"),
                false,
            )
            .upcast_ref(),
            form.entry(
                "Timer interval",
                "interval",
                w("interval"),
                Kind::Duration,
                Some("1h"),
                false,
            )
            .upcast_ref(),
            form.entry(
                "Pull interval",
                "pull_interval",
                w("pull_interval"),
                Kind::Duration,
                Some("off, 5m for git"),
                false,
            )
            .upcast_ref(),
            form.choice(
                "Empty folders",
                "empty_dirs",
                options(&[
                    (Some("ignore"), text::empty_dirs(EmptyDirMode::Ignore)),
                    (Some("prune"), text::empty_dirs(EmptyDirMode::Prune)),
                    (Some("mirror"), text::empty_dirs(EmptyDirMode::Mirror)),
                ]),
                w("empty_dirs"),
                Some("ignore"),
            )
            .upcast_ref(),
            form.choice(
                "Deletions",
                "deletions",
                options(&[
                    (Some("deny"), text::deletions(Deletions::Deny)),
                    (Some("allow"), text::deletions(Deletions::Allow)),
                ]),
                w("deletions"),
                Some("deny"),
            )
            .upcast_ref(),
            form.entry(
                "Largest file",
                "max_file_size",
                w("max_file_size"),
                Kind::Size,
                Some("100MB"),
                false,
            )
            .upcast_ref(),
        ],
    );
    defaults.set_description(Some("Each sync, or the remote it uses, can set its own"));
    page.add(&defaults);
    page.add(&form.list(
        "Excluded Everywhere",
        "Never synced by any sync; the default skips version control and editor files",
        "exclude",
        daemon.exclude.clone(),
        DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
        "Add a path or glob",
    ));
    dialog.add(&page);
    dialog.present(Some(&view.ctx().window));
}
