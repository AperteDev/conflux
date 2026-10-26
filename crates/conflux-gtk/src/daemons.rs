//! Managing a scope's daemons: one per profile, each run by a systemd unit
//! (`conflux.service` for `default`, `conflux@<profile>.service` otherwise).
//! Starting, stopping and enabling go through systemd; which syncs a daemon
//! runs is the `profiles` of each sync in the config.

use std::cell::Cell;
use std::ffi::OsStr;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use conflux_core::config_edit::Entry;
use conflux_core::paths::{validate_profile, DEFAULT_PROFILE};
use gtk::{gio, glib};

use crate::detail::{group, page_with};
use crate::form::{profiles_of, reason, set_profiles};
use crate::model::{unit_name, ConfigState, DaemonState, Scope, Snapshot};
use crate::scope::ScopeView;
use crate::systemd::{self, Action};
use crate::{text, ui};
use conflux_core::helper::{HelperRequest, HelperResponse, UnitAction};

/// Look again now, and once more when systemd has likely carried the job out.
fn refresh_soon(view: &Rc<ScopeView>) {
    view.ctx().refresh();
    let ctx = Rc::downgrade(view.ctx());
    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
        if let Some(ctx) = ctx.upgrade() {
            ctx.refresh();
        }
    });
}

/// Act on `profile`'s unit: through the user's systemd manager for the user's
/// daemons, through the helper (as root) for the system's.
async fn act(view: &ScopeView, profile: &str, action: UnitAction) -> Result<(), String> {
    let unit = unit_name(profile);
    let result = match view.scope {
        Scope::User => {
            let done = match action {
                UnitAction::Start => systemd::control(view.scope, &unit, Action::Start).await,
                UnitAction::Stop => systemd::control(view.scope, &unit, Action::Stop).await,
                UnitAction::Restart => systemd::control(view.scope, &unit, Action::Restart).await,
                UnitAction::Enable => systemd::set_enabled(view.scope, &unit, true).await,
                UnitAction::Disable => systemd::set_enabled(view.scope, &unit, false).await,
            };
            done.map_err(|e| e.message().to_string())
        }
        Scope::System => match view.helper() {
            None => Err("unlock to change the system's daemons".to_string()),
            Some(helper) => {
                let request = HelperRequest::Unit {
                    action,
                    profile: profile.to_string(),
                };
                match helper.call(&request).await? {
                    HelperResponse::Done { .. } => Ok(()),
                    HelperResponse::Error(e) => Err(e),
                    _ => Err("unexpected answer from the helper".to_string()),
                }
            }
        },
    };
    result.map_err(|e| format!("Could not change {unit}: {e}"))
}

/// Start, stop or restart `profile`'s daemon.
pub fn control(view: &Rc<ScopeView>, profile: &str, action: UnitAction) {
    let (view, profile) = (view.clone(), profile.to_string());
    glib::spawn_future_local(async move {
        if let Err(e) = act(&view, &profile, action).await {
            view.ctx().toast(&e);
        }
        refresh_soon(&view);
    });
}

/// Make `profile`'s daemon start at login (or boot), or not; `failed` runs when
/// it could not be done.
pub fn set_enabled(
    view: &Rc<ScopeView>,
    profile: &str,
    enabled: bool,
    failed: impl FnOnce() + 'static,
) {
    let (view, profile) = (view.clone(), profile.to_string());
    let action = if enabled {
        UnitAction::Enable
    } else {
        UnitAction::Disable
    };
    glib::spawn_future_local(async move {
        if let Err(e) = act(&view, &profile, action).await {
            view.ctx().toast(&e);
            failed();
        }
        refresh_soon(&view);
    });
}

/// Keep the user's daemons running after logout, or not.
pub fn set_linger(view: &Rc<ScopeView>, enabled: bool, failed: impl FnOnce() + 'static) {
    let view = view.clone();
    glib::spawn_future_local(async move {
        if let Err(e) = systemd::set_linger(enabled).await {
            view.ctx()
                .toast(&format!("Could not change it: {}", e.message()));
            failed();
        }
        refresh_soon(&view);
    });
}

fn state_dir(scope: Scope, profile: &str) -> Option<std::path::PathBuf> {
    crate::model::paths(scope, Some(profile)).map(|p| p.state)
}

/// The page of `profile`'s daemon: what and where it is, its controls, and the
/// syncs it runs.
pub fn page(view: &Rc<ScopeView>, snapshot: &Rc<Snapshot>, profile: &str) -> adw::NavigationPage {
    let editable = view.editable();
    let unit = unit_name(profile);
    let daemon = snapshot.daemons.iter().find(|d| d.profile == profile);
    let systemd = view.systemd();
    let state = systemd.as_ref().and_then(|s| s.unit(&unit)).cloned();
    let installed = state.as_ref().is_some_and(|u| u.installed);
    let active = state.as_ref().is_some_and(|u| u.is_active());
    let mut groups = Vec::new();

    let mut about: Vec<gtk::Widget> = vec![
        ui::property("Profile", profile).upcast(),
        ui::property(
            "Unit",
            &if installed {
                unit.clone()
            } else {
                format!("{unit} (not installed)")
            },
        )
        .upcast(),
    ];
    if let Some(daemon) = daemon {
        let (summary, _) =
            crate::scope::daemon_text(view.scope, daemon, state.as_ref(), systemd.is_some());
        // The unit is named in its own row.
        let summary = summary
            .strip_prefix(&format!("{unit} · "))
            .unwrap_or(&summary);
        about.push(ui::property("State", summary).upcast());
        if let DaemonState::Running {
            info: Some(info), ..
        } = &daemon.state
        {
            about.push(
                ui::property(
                    "Version",
                    &format!("conflux {}, process {}", info.version, info.pid),
                )
                .upcast(),
            );
            if info.profile != profile {
                about.push(ui::property("Runs the profile", &info.profile).upcast());
            }
            about.push(ui::property("Config file", &info.config).upcast());
        }
        about.push(ui::property("Control socket", &daemon.socket.display().to_string()).upcast());
    }
    if let Some(dir) = state_dir(view.scope, profile) {
        about.push(ui::property("State folder", &text::path(view.scope, &dir)).upcast());
    }
    let about: Vec<&gtk::Widget> = about.iter().collect();
    groups.push(group("Daemon", &about));

    if installed {
        let running = adw::SwitchRow::builder()
            .title("Running")
            .active(active)
            .sensitive(editable)
            .build();
        let start_at = adw::SwitchRow::builder()
            .title(match view.scope {
                Scope::User => "Start at login",
                Scope::System => "Start at boot",
            })
            .active(state.as_ref().is_some_and(|u| u.enabled))
            .sensitive(editable)
            .build();
        let outside =
            daemon.is_some_and(|d| matches!(d.state, DaemonState::Running { .. })) && !active;
        if outside {
            running.set_subtitle("Running outside systemd; stop it where it was started");
            running.set_sensitive(false);
        }
        {
            let (view, profile) = (view.clone(), profile.to_string());
            running.connect_active_notify(move |row| {
                let action = if row.is_active() {
                    UnitAction::Start
                } else {
                    UnitAction::Stop
                };
                control(&view, &profile, action);
            });
        }
        {
            let (view, profile) = (view.clone(), profile.to_string());
            let quiet = Rc::new(Cell::new(false));
            start_at.connect_active_notify(move |row| {
                if quiet.get() {
                    return;
                }
                let (row2, quiet) = (row.clone(), quiet.clone());
                set_enabled(&view, &profile, row.is_active(), move || {
                    quiet.set(true);
                    row2.set_active(!row2.is_active());
                    quiet.set(false);
                });
            });
        }
        let restart = button_row("Restart", "view-refresh-symbolic", editable && active);
        {
            let (view, profile) = (view.clone(), profile.to_string());
            restart.connect_activated(move |_| control(&view, &profile, UnitAction::Restart));
        }
        let log = button_row("View Log", "text-x-generic-symbolic", true);
        {
            let (view, profile) = (view.clone(), profile.to_string());
            log.connect_activated(move |_| show_log(&view, &profile));
        }
        groups.push(group(
            "Control",
            &[
                running.upcast_ref(),
                start_at.upcast_ref(),
                restart.upcast_ref(),
                log.upcast_ref(),
            ],
        ));
    } else {
        let note = adw::PreferencesGroup::builder()
            .title("Control")
            .description(if profile == DEFAULT_PROFILE {
                "conflux.service is not installed, so this daemon can only be run by hand \
                 (conflux daemon)"
            } else {
                "conflux@.service is not installed, so this daemon can only be run by hand \
                 (conflux --profile <name> daemon)"
            })
            .build();
        groups.push(note);
    }

    if let ConfigState::Loaded(loaded) = &snapshot.config {
        let syncs = adw::PreferencesGroup::builder()
            .title("Syncs It Runs")
            .description("The syncs whose profiles include this one")
            .build();
        if loaded.raw.syncs.is_empty() {
            syncs.set_description(Some("There are no syncs yet"));
        }
        let generation = view.generation();
        for (index, sync) in loaded.raw.syncs.iter().enumerate() {
            let member = profiles_of(&loaded.doc, index).iter().any(|p| p == profile);
            let row = adw::SwitchRow::builder()
                .title(text::sync_title(sync, &loaded.label(index)))
                .subtitle(text::mapping(view.scope, sync))
                .use_markup(false)
                .active(member)
                .sensitive(editable)
                .build();
            let (view, profile) = (view.clone(), profile.to_string());
            let quiet = Rc::new(Cell::new(false));
            row.connect_active_notify(move |row| {
                if quiet.get() {
                    return;
                }
                let (on, name) = (row.is_active(), profile.clone());
                let (row2, quiet, ctx) = (row.clone(), quiet.clone(), view.ctx().clone());
                view.edit(
                    generation,
                    Box::new(move |doc| {
                        let mut list = profiles_of(doc, index);
                        list.retain(|p| *p != name);
                        if on {
                            list.push(name);
                        }
                        set_profiles(doc, index, &list)
                    }),
                    move |result| {
                        if let Err(e) = result {
                            ctx.toast(&e);
                            quiet.set(true);
                            row2.set_active(!on);
                            quiet.set(false);
                        }
                    },
                );
            });
            syncs.add(&row);
        }
        groups.push(syncs);
    }

    let remove = (editable && profile != DEFAULT_PROFILE).then(|| {
        let button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove Daemon")
            .build();
        let (view, profile) = (view.clone(), profile.to_string());
        button.connect_clicked(move |_| remove(&view, &profile));
        button.upcast()
    });
    page_with(view, &format!("{profile} Daemon"), groups, remove)
}

fn button_row(title: &str, icon: &str, sensitive: bool) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .sensitive(sensitive)
        .build();
    row.add_suffix(&gtk::Image::from_icon_name(icon));
    row
}

/// Ask, then remove `profile`'s daemon: stop it, stop it starting at login, and
/// optionally take the profile out of the syncs and delete the daemon's state.
pub fn remove(view: &Rc<ScopeView>, profile: &str) {
    let in_syncs = gtk::CheckButton::builder()
        .label("Also take it out of the syncs' profiles")
        .active(true)
        .build();
    let state = state_dir(view.scope, profile);
    let delete_state = gtk::CheckButton::builder()
        .label(match &state {
            Some(dir) => format!(
                "Delete what it remembers of past syncs ({})",
                text::path(view.scope, dir)
            ),
            None => "Delete what it remembers of past syncs".to_string(),
        })
        .build();
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 6);
    extra.append(&in_syncs);
    extra.append(&delete_state);
    let dialog = adw::AlertDialog::new(
        Some(&format!("Remove the {profile} Daemon?")),
        Some(
            "Its unit is stopped and no longer starts at login. Syncs left in no profile \
             stop running; their files stay where they are.",
        ),
    );
    dialog.set_extra_child(Some(&extra));
    dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    let (view, profile) = (view.clone(), profile.to_string());
    let window = view.ctx().window.clone();
    dialog.choose(Some(&window), gio::Cancellable::NONE, move |response| {
        if response != "remove" {
            return;
        }
        let (in_syncs, delete_state) = (in_syncs.is_active(), delete_state.is_active());
        glib::spawn_future_local(async move {
            // Neither fails the removal: the unit may be stopped or disabled already.
            let _ = act(&view, &profile, UnitAction::Stop).await;
            let _ = act(&view, &profile, UnitAction::Disable).await;
            if in_syncs {
                let name = profile.clone();
                let ctx = view.ctx().clone();
                view.edit(
                    view.generation(),
                    Box::new(move |doc| {
                        for i in 0..doc.len(Entry::Sync) {
                            let mut list = profiles_of(doc, i);
                            if list.contains(&name) {
                                list.retain(|p| *p != name);
                                set_profiles(doc, i, &list)?;
                            }
                        }
                        Ok(())
                    }),
                    move |result| {
                        if let Err(e) = result {
                            ctx.toast(&e);
                        }
                    },
                );
            }
            if let Some(dir) = state.filter(|_| delete_state) {
                let removed = match view.helper() {
                    // The system daemon's state belongs to root.
                    Some(helper) => {
                        let request = HelperRequest::RemoveState {
                            profile: profile.clone(),
                        };
                        match helper.call(&request).await {
                            Ok(HelperResponse::Done { .. }) => Ok(()),
                            Ok(HelperResponse::Error(e)) | Err(e) => Err(e),
                            Ok(_) => Err("unexpected answer from the helper".to_string()),
                        }
                    }
                    None => gio::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                        _ => Ok(()),
                    })
                    .await
                    .expect("removing the state folder panicked"),
                };
                if let Err(e) = removed {
                    view.ctx()
                        .toast(&format!("Could not delete its state: {e}"));
                }
            }
            view.ctx().nav.pop_to_tag("main");
            view.ctx().toast(&format!("Removed the {profile} daemon"));
            refresh_soon(&view);
        });
    });
}

/// Ask for a profile and the syncs it runs, then give it a daemon.
pub fn add(view: &Rc<ScopeView>) {
    let Some(snapshot) = view.snapshot() else {
        return;
    };
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        return;
    };
    let template = view.systemd().is_some_and(|s| s.template);
    let name = adw::EntryRow::builder()
        .title("Profile name, e.g. the host's")
        .use_markup(false)
        .build();
    let start_now = adw::SwitchRow::builder()
        .title("Start now")
        .active(template)
        .sensitive(template)
        .build();
    let start_at = adw::SwitchRow::builder()
        .title(match view.scope {
            Scope::User => "Start at login",
            Scope::System => "Start at boot",
        })
        .active(template)
        .sensitive(template)
        .build();
    let general = adw::PreferencesGroup::new();
    general.add(&name);
    general.add(&start_now);
    general.add(&start_at);
    if !template {
        general.set_description(Some(
            "conflux@.service is not installed: the daemon has to be run by hand \
             (conflux --profile <name> daemon)",
        ));
    }
    let syncs = adw::PreferencesGroup::builder()
        .title("Syncs It Runs")
        .description("Adds the profile to each chosen sync")
        .build();
    let mut picks = Vec::new();
    for (index, sync) in loaded.raw.syncs.iter().enumerate() {
        let row = adw::SwitchRow::builder()
            .title(text::sync_title(sync, &loaded.label(index)))
            .subtitle(text::mapping(view.scope, sync))
            .use_markup(false)
            .build();
        syncs.add(&row);
        picks.push((index, row));
    }
    let page = adw::PreferencesPage::new();
    page.add(&general);
    page.add(&syncs);

    let (dialog, add_button, banner) = crate::add::dialog("Add Daemon", &page);
    let taken: Vec<String> = snapshot.daemons.iter().map(|d| d.profile.clone()).collect();
    dialog.present(Some(&view.ctx().window));
    let view = view.clone();
    let weak = dialog.downgrade();
    add_button.connect_clicked(move |_| {
        let Some(dialog) = weak.upgrade() else {
            return;
        };
        let profile = name.text().trim().to_string();
        let refuse = |why: &str| {
            banner.set_title(why);
            banner.set_revealed(true);
        };
        if let Err(e) = validate_profile(&profile) {
            return refuse(&reason(e));
        }
        if taken.contains(&profile) {
            return refuse(&format!("There is a {profile} daemon already"));
        }
        let chosen: Vec<usize> = picks
            .iter()
            .filter(|(_, row)| row.is_active())
            .map(|(i, _)| *i)
            .collect();
        if chosen.is_empty() && !start_at.is_active() {
            return refuse("Choose a sync for it to run, or have it start at login");
        }
        let (start, enable) = (start_now.is_active(), start_at.is_active());
        let (view2, banner, name) = (view.clone(), banner.clone(), profile.clone());
        view.edit(
            view.generation(),
            Box::new(move |doc| {
                for i in chosen {
                    let mut list = profiles_of(doc, i);
                    if !list.contains(&name) {
                        list.push(name.clone());
                    }
                    set_profiles(doc, i, &list)?;
                }
                Ok(())
            }),
            move |result| {
                if let Err(e) = result {
                    banner.set_title(&e);
                    banner.set_revealed(true);
                    return;
                }
                dialog.close();
                glib::spawn_future_local(async move {
                    let mut failed = None;
                    if enable {
                        failed = act(&view2, &profile, UnitAction::Enable).await.err();
                    }
                    if start && failed.is_none() {
                        failed = act(&view2, &profile, UnitAction::Start).await.err();
                    }
                    match failed {
                        Some(e) => view2.ctx().toast(&format!("Added the profile. {e}")),
                        None => view2.ctx().toast(&format!("Added the {profile} daemon")),
                    }
                    refresh_soon(&view2);
                });
            },
        );
    });
}

/// Follow `profile`'s daemon's log in the journal.
pub fn show_log(view: &Rc<ScopeView>, profile: &str) {
    let unit = unit_name(profile);
    if crate::model::sandboxed() {
        // The Flatpak runtime has no journalctl, and the journal is the host's.
        view.ctx().toast(&format!(
            "The log is in the journal: journalctl --user -u {unit}"
        ));
        return;
    }
    let mut argv: Vec<&OsStr> = vec![OsStr::new("journalctl")];
    if view.scope == Scope::User {
        argv.push(OsStr::new("--user"));
    }
    for arg in [
        "--unit",
        &unit,
        "--lines",
        "300",
        "--follow",
        "--output",
        "short-iso",
        "--no-pager",
        "--quiet",
    ] {
        argv.push(OsStr::new(arg));
    }
    let process = match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
    ) {
        Ok(p) => p,
        Err(e) => {
            view.ctx()
                .toast(&format!("Could not read the journal: {}", e.message()));
            return;
        }
    };

    let buffer = gtk::TextBuffer::new(None);
    let text_view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(12)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&text_view)
        .vexpand(true)
        .build();
    // Until the journal has something to say, say so.
    let empty = adw::StatusPage::builder()
        .icon_name("text-x-generic-symbolic")
        .title("No Entries Yet")
        .description("New lines show here as the daemon writes them")
        .build();
    let content = gtk::Overlay::new();
    content.set_child(Some(&scrolled));
    content.add_overlay(&empty);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title(format!("Log of {unit}"))
        .content_width(900)
        .content_height(600)
        .child(&toolbar)
        .build();
    let end = buffer.create_mark(None, &buffer.end_iter(), false);

    if let Some(stdout) = process.stdout_pipe() {
        let input = gio::DataInputStream::new(&stdout);
        let (buffer, text_view, scrolled) = (buffer.clone(), text_view.clone(), scrolled.clone());
        glib::spawn_future_local(async move {
            while let Ok(Some(line)) = input.read_line_utf8_future(glib::Priority::DEFAULT).await {
                empty.set_visible(false);
                let adj = scrolled.vadjustment();
                let at_bottom = adj.value() + adj.page_size() >= adj.upper() - 40.0;
                buffer.insert(&mut buffer.end_iter(), &format!("{line}\n"));
                if at_bottom {
                    text_view.scroll_mark_onscreen(&end);
                }
            }
        });
    }
    dialog.connect_closed(move |_| process.force_exit());
    dialog.present(Some(&view.ctx().window));
}

/// Whether `snapshot`'s `profile` daemon runs outside systemd: it answers, but
/// its unit is not active.
pub fn runs_outside(snapshot: &Snapshot, profile: &str, active: bool) -> bool {
    !active
        && snapshot
            .daemons
            .iter()
            .any(|d| d.profile == profile && matches!(d.state, DaemonState::Running { .. }))
}
