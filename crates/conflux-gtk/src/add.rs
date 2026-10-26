//! Dialogs adding a sync or a remote. Nothing is written until Add, and only an
//! entry the daemon accepts, so half-filled ones never reach the file.

use std::rc::Rc;

use adw::prelude::*;
use conflux_core::config_edit::{toml_edit, Entry};

use crate::model::ConfigState;
use crate::scope::ScopeView;
use crate::ui;

/// A dialog with Cancel and Add, a banner for what is wrong, and `page`.
pub fn dialog(title: &str, page: &adw::PreferencesPage) -> (adw::Dialog, gtk::Button, adw::Banner) {
    let add = gtk::Button::builder()
        .label("Add")
        .css_classes(["suggested-action"])
        .build();
    let cancel = gtk::Button::builder().label("Cancel").build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&add);
    let banner = adw::Banner::new("");
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(page));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(520)
        .child(&toolbar)
        .build();
    let weak = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    dialog.set_default_widget(Some(&add));
    (dialog, add, banner)
}

fn entry(title: &str) -> adw::EntryRow {
    adw::EntryRow::builder()
        .title(title)
        .use_markup(false)
        .build()
}

fn text(row: &adw::EntryRow) -> String {
    row.text().trim().to_string()
}

/// Write `table` as a new `entry`, closing the dialog when it is in, or saying
/// in its banner why it is not.
fn add(
    view: &Rc<ScopeView>,
    dialog: &adw::Dialog,
    banner: &adw::Banner,
    entry: Entry,
    table: toml_edit::Table,
    done: &'static str,
) {
    let (dialog, banner, ctx) = (dialog.clone(), banner.clone(), view.ctx().clone());
    view.edit(
        view.generation(),
        Box::new(move |doc| doc.add(entry, table).map(|_| ())),
        move |result| match result {
            Ok(()) => {
                dialog.close();
                ctx.toast(done);
            }
            Err(e) => {
                banner.set_title(&e);
                banner.set_revealed(true);
            }
        },
    );
}

fn refuse(banner: &adw::Banner, why: &str) {
    banner.set_title(why);
    banner.set_revealed(true);
}

pub fn add_sync(view: &Rc<ScopeView>) {
    let Some(snapshot) = view.snapshot() else {
        return;
    };
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        return;
    };
    let remotes: Vec<String> = loaded.raw.remotes.iter().map(|r| r.id.clone()).collect();
    let window = view.ctx().window.clone();

    let kind = adw::ComboRow::builder()
        .title("Kind")
        .model(&gtk::StringList::new(&[
            "Sync with a remote",
            "Mirror to a folder",
            "Keep a git clone in sync",
        ]))
        .build();
    let remote = adw::ComboRow::builder().title("Remote").build();
    if remotes.is_empty() {
        remote.set_model(Some(&gtk::StringList::new(&[
            "None yet: add a remote first",
        ])));
        remote.set_sensitive(false);
    } else {
        let ids: Vec<&str> = remotes.iter().map(String::as_str).collect();
        remote.set_model(Some(&gtk::StringList::new(&ids)));
    }
    let remote_path = entry("Path on the remote (empty: its root)");
    let folder = entry("Folder to mirror to");
    ui::add_chooser(&window, view.scope, &folder, true, |_| {});
    let local = entry("Local folder");
    ui::add_chooser(&window, view.scope, &local, true, |_| {});
    let repo = entry("Git clone");
    ui::add_chooser(&window, view.scope, &repo, true, |_| {});
    let everything = adw::SwitchRow::builder()
        .title("Sync everything")
        .subtitle("Otherwise pick the paths to sync after adding it")
        .active(true)
        .build();
    let id = entry("Id, to name it in the CLI (optional)");
    let label = entry("Label (optional)");

    let what = adw::PreferencesGroup::new();
    for row in [
        kind.upcast_ref::<gtk::Widget>(),
        remote.upcast_ref(),
        remote_path.upcast_ref(),
        folder.upcast_ref(),
        repo.upcast_ref(),
        local.upcast_ref(),
        everything.upcast_ref(),
    ] {
        what.add(row);
    }
    let names = adw::PreferencesGroup::new();
    names.add(&id);
    names.add(&label);
    let page = adw::PreferencesPage::new();
    page.add(&what);
    page.add(&names);

    let show_kind = {
        let (remote, remote_path, folder, repo, local, everything) = (
            remote.clone(),
            remote_path.clone(),
            folder.clone(),
            repo.clone(),
            local.clone(),
            everything.clone(),
        );
        move |k: u32| {
            remote.set_visible(k == 0);
            remote_path.set_visible(k == 0);
            folder.set_visible(k == 1);
            repo.set_visible(k == 2);
            local.set_visible(k != 2);
            everything.set_visible(k != 2);
        }
    };
    show_kind(0);
    kind.connect_selected_notify(move |row| show_kind(row.selected()));

    let (dialog, add_button, banner) = dialog("Add Sync", &page);
    let view = view.clone();
    let weak = dialog.downgrade();
    add_button.connect_clicked(move |_| {
        let Some(dialog) = weak.upgrade() else {
            return;
        };
        let mut table = toml_edit::Table::new();
        for (key, row) in [("id", &id), ("label", &label)] {
            if !text(row).is_empty() {
                table.insert(key, toml_edit::value(text(row)));
            }
        }
        match kind.selected() {
            0 => {
                let Some(r) = remotes.get(remote.selected() as usize) else {
                    return refuse(&banner, "Add a remote first, or pick another kind");
                };
                table.insert("remote", toml_edit::value(r.as_str()));
                if !text(&remote_path).is_empty() {
                    table.insert("remote_path", toml_edit::value(text(&remote_path)));
                }
            }
            1 if text(&folder).is_empty() => {
                return refuse(&banner, "Choose the folder to mirror to");
            }
            1 => {
                table.insert("remote_fs", toml_edit::value(text(&folder)));
            }
            _ if text(&repo).is_empty() => return refuse(&banner, "Choose the git clone"),
            _ => {
                table.insert("git_repo", toml_edit::value(text(&repo)));
            }
        }
        if kind.selected() != 2 {
            if text(&local).is_empty() {
                return refuse(&banner, "Choose the local folder");
            }
            table.insert("local", toml_edit::value(text(&local)));
            if everything.is_active() {
                let mut all = toml_edit::Array::new();
                all.push("**");
                table.insert("include", toml_edit::value(all));
            }
        }
        add(
            &view,
            &dialog,
            &banner,
            Entry::Sync,
            table,
            "Added the sync",
        );
    });
    dialog.present(Some(&window));
}

pub fn add_remote(view: &Rc<ScopeView>) {
    let window = view.ctx().window.clone();
    let id = entry("Id, which syncs refer to it by");
    let label = entry("Label (optional)");
    let backend = adw::ComboRow::builder()
        .title("Backend")
        .model(&gtk::StringList::new(&[
            "WebDAV, e.g. Nextcloud",
            "Git",
            "Folder",
        ]))
        .build();
    let url = entry("URL");
    let username = entry("Username (optional)");
    let password_command = entry("Password command, e.g. secret-tool lookup ... (optional)");
    let branch = entry("Branch (empty: the remote's default)");

    let general = adw::PreferencesGroup::new();
    for row in [
        id.upcast_ref::<gtk::Widget>(),
        label.upcast_ref(),
        backend.upcast_ref(),
        url.upcast_ref(),
    ] {
        general.add(row);
    }
    let auth = adw::PreferencesGroup::builder()
        .title("Connection")
        .description("A password can also be set on the remote's page after adding it")
        .build();
    for row in [
        username.upcast_ref::<gtk::Widget>(),
        password_command.upcast_ref(),
        branch.upcast_ref(),
    ] {
        auth.add(row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&general);
    page.add(&auth);

    // A folder remote's location is a path, so it gets a chooser too.
    let pick = ui::add_chooser(&window, view.scope, &url, true, |_| {});
    let show_backend = {
        let (url, auth, branch) = (url.clone(), auth.clone(), branch.clone());
        move |b: u32| {
            url.set_title(if b == 2 { "Folder" } else { "URL" });
            pick.set_visible(b == 2);
            auth.set_visible(b != 2);
            branch.set_visible(b == 1);
        }
    };
    show_backend(0);
    backend.connect_selected_notify(move |row| show_backend(row.selected()));

    let (dialog, add_button, banner) = dialog("Add Remote", &page);
    let view = view.clone();
    let weak = dialog.downgrade();
    add_button.connect_clicked(move |_| {
        let Some(dialog) = weak.upgrade() else {
            return;
        };
        if text(&id).is_empty() {
            return refuse(&banner, "Give the remote an id");
        }
        if text(&url).is_empty() {
            return refuse(&banner, "Fill in where the remote is");
        }
        let b = backend.selected();
        let mut table = toml_edit::Table::new();
        table.insert("id", toml_edit::value(text(&id)));
        if !text(&label).is_empty() {
            table.insert("label", toml_edit::value(text(&label)));
        }
        let kind = ["webdav", "git", "filesystem"][b.min(2) as usize];
        table.insert("backend", toml_edit::value(kind));
        table.insert("url", toml_edit::value(text(&url)));
        if b != 2 {
            for (key, row) in [
                ("username", &username),
                ("password_command", &password_command),
            ] {
                if !text(row).is_empty() {
                    table.insert(key, toml_edit::value(text(row)));
                }
            }
        }
        if b == 1 && !text(&branch).is_empty() {
            table.insert("branch", toml_edit::value(text(&branch)));
        }
        add(
            &view,
            &dialog,
            &banner,
            Entry::Remote,
            table,
            "Added the remote",
        );
    });
    dialog.present(Some(&window));
}
