//! Dialogs adding a sync or a remote. Nothing is written until Add, and only an
//! entry the daemon accepts, so half-filled ones never reach the file.

use std::cell::RefCell;
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

/// Write `table` as a new `entry`, closing the dialog and calling `added` when
/// it is in, or saying in its banner why it is not.
fn add(
    view: &Rc<ScopeView>,
    dialog: &adw::Dialog,
    banner: &adw::Banner,
    entry: Entry,
    table: toml_edit::Table,
    done: &'static str,
    added: impl FnOnce() + 'static,
) {
    let (dialog, banner, ctx) = (dialog.clone(), banner.clone(), view.ctx().clone());
    view.edit(
        view.generation(),
        Box::new(move |doc| doc.add(entry, table).map(|_| ())),
        move |result| match result {
            Ok(()) => {
                dialog.close();
                ctx.toast(done);
                added();
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

/// What a new sync syncs, as Add Sync offers it: its label, the `scope` it
/// writes (`None`: the default), and whether it includes everything.
const WHAT: [(&str, Option<&str>, bool); 4] = [
    ("Everything, both ways", None, true),
    ("Picked paths, both ways", None, false),
    ("Upload everything", Some("local"), false),
    ("Download everything", Some("remote"), false),
];

/// When a new sync of `kind` (a remote, a folder, a git clone) can run: the
/// `trigger` written (`None`: the default, on local changes) and its label.
/// Only a folder can be watched for changes on its side.
fn triggers(kind: u32, interval: &str) -> Vec<(Option<&'static str>, String)> {
    let mut list = vec![(None, "On local changes".to_string())];
    if kind == 1 {
        list.push((Some("watch-both"), "On changes on either side".to_string()));
    }
    list.push((Some("timer"), format!("Every {interval}")));
    list.push((Some("manual"), "Only when asked".to_string()));
    list
}

/// A remote as Add Sync lists it: its label and id, or its id.
fn remote_name(id: &str, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("{label} ({id})"),
        None => id.to_string(),
    }
}

/// Show `names` in `row`, or that there are none yet.
fn fill_remotes(row: &adw::ComboRow, names: &[String]) {
    if names.is_empty() {
        row.set_model(Some(&gtk::StringList::new(&["None yet"])));
        row.set_sensitive(false);
    } else {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        row.set_model(Some(&gtk::StringList::new(&names)));
        row.set_sensitive(true);
    }
}

pub fn add_sync(view: &Rc<ScopeView>) {
    let Some(snapshot) = view.snapshot() else {
        return;
    };
    let ConfigState::Loaded(loaded) = &snapshot.config else {
        return;
    };
    // The remotes' ids and names; one added from here joins them.
    let remotes: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(
        loaded
            .raw
            .remotes
            .iter()
            .map(|r| (r.id.clone(), remote_name(&r.id, r.label.as_deref())))
            .collect(),
    ));
    let interval = crate::text::duration(loaded.raw.daemon.interval);
    let window = view.ctx().window.clone();

    let kind = adw::ComboRow::builder()
        .title("Kind")
        .model(&gtk::StringList::new(&[
            "Sync with a remote",
            "Sync with a folder",
            "Keep a git clone in sync",
        ]))
        .build();
    let remote = adw::ComboRow::builder().title("Remote").build();
    let names: Vec<String> = remotes.borrow().iter().map(|(_, n)| n.clone()).collect();
    fill_remotes(&remote, &names);
    let new_remote = adw::ButtonRow::builder()
        .title("Add a Remote…")
        .start_icon_name("list-add-symbolic")
        .build();
    let remote_path = entry("Path on the remote (empty: its root)");
    let folder = entry("Folder to sync with");
    ui::add_chooser(&window, view.scope, &folder, true, |_| {});
    let local = entry("Local folder");
    ui::add_chooser(&window, view.scope, &local, true, |_| {});
    let repo = entry("Git clone");
    ui::add_chooser(&window, view.scope, &repo, true, |_| {});
    let labels: Vec<&str> = WHAT.iter().map(|(label, _, _)| *label).collect();
    let what = adw::ComboRow::builder()
        .title("What it syncs")
        .use_subtitle(true)
        .build();
    // Set after `use-subtitle`, which only follows later changes.
    what.set_model(Some(&gtk::StringList::new(&labels)));
    let when = adw::ComboRow::builder()
        .title("When it syncs")
        .use_subtitle(true)
        .build();
    // The triggers `when` offers now.
    let trigger_values: Rc<RefCell<Vec<Option<&'static str>>>> = Rc::default();
    let id = entry("Id, to name it in the CLI (optional)");
    let label = entry("Label (optional)");

    let target = adw::PreferencesGroup::new();
    for row in [
        kind.upcast_ref::<gtk::Widget>(),
        remote.upcast_ref(),
        new_remote.upcast_ref(),
        remote_path.upcast_ref(),
        folder.upcast_ref(),
        repo.upcast_ref(),
        local.upcast_ref(),
    ] {
        target.add(row);
    }
    let how = adw::PreferencesGroup::new();
    how.add(&what);
    how.add(&when);
    let names = adw::PreferencesGroup::new();
    names.add(&id);
    names.add(&label);
    let page = adw::PreferencesPage::new();
    page.add(&target);
    page.add(&how);
    page.add(&names);

    let show_kind = {
        let (remote, new_remote, remote_path, folder, repo, local, what, when) = (
            remote.clone(),
            new_remote.clone(),
            remote_path.clone(),
            folder.clone(),
            repo.clone(),
            local.clone(),
            what.clone(),
            when.clone(),
        );
        let trigger_values = trigger_values.clone();
        move |k: u32| {
            remote.set_visible(k == 0);
            new_remote.set_visible(k == 0);
            remote_path.set_visible(k == 0);
            folder.set_visible(k == 1);
            repo.set_visible(k == 2);
            local.set_visible(k != 2);
            what.set_visible(k != 2);
            let list = triggers(k, &interval);
            let labels: Vec<&str> = list.iter().map(|(_, l)| l.as_str()).collect();
            when.set_model(Some(&gtk::StringList::new(&labels)));
            *trigger_values.borrow_mut() = list.into_iter().map(|(v, _)| v).collect();
        }
    };
    show_kind(0);
    kind.connect_selected_notify(move |row| show_kind(row.selected()));

    {
        let (view, remotes, remote) = (view.clone(), remotes.clone(), remote.clone());
        new_remote.connect_activated(move |_| {
            let (remotes, remote) = (remotes.clone(), remote.clone());
            add_remote_then(&view, move |id, name| {
                remotes.borrow_mut().push((id, name));
                let names: Vec<String> = remotes.borrow().iter().map(|(_, n)| n.clone()).collect();
                fill_remotes(&remote, &names);
                remote.set_selected(names.len() as u32 - 1);
            });
        });
    }

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
                let remotes = remotes.borrow();
                let Some((r, _)) = remotes.get(remote.selected() as usize) else {
                    return refuse(&banner, "Add a remote first, or pick another kind");
                };
                table.insert("remote", toml_edit::value(r.as_str()));
                if !text(&remote_path).is_empty() {
                    table.insert("remote_path", toml_edit::value(text(&remote_path)));
                }
            }
            1 if text(&folder).is_empty() => {
                return refuse(&banner, "Choose the folder to sync with");
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
            let (_, scope, everything) = WHAT[(what.selected() as usize).min(WHAT.len() - 1)];
            if let Some(scope) = scope {
                table.insert("scope", toml_edit::value(scope));
            }
            if everything {
                let mut all = toml_edit::Array::new();
                all.push("**");
                table.insert("include", toml_edit::value(all));
            }
        }
        let trigger = trigger_values
            .borrow()
            .get(when.selected() as usize)
            .copied()
            .flatten();
        if let Some(trigger) = trigger {
            table.insert("trigger", toml_edit::value(trigger));
        }
        add(
            &view,
            &dialog,
            &banner,
            Entry::Sync,
            table,
            "Added the sync",
            || {},
        );
    });
    dialog.present(Some(&window));
}

pub fn add_remote(view: &Rc<ScopeView>) {
    add_remote_then(view, |_, _| {});
}

/// Add Remote, calling `added` with the new remote's id and name once it is in.
fn add_remote_then(view: &Rc<ScopeView>, added: impl Fn(String, String) + 'static) {
    let added = Rc::new(added);
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
        let (added, new_id) = (added.clone(), text(&id));
        let name = remote_name(
            &new_id,
            Some(text(&label)).filter(|l| !l.is_empty()).as_deref(),
        );
        add(
            &view,
            &dialog,
            &banner,
            Entry::Remote,
            table,
            "Added the remote",
            move || added(new_id, name),
        );
    });
    dialog.present(Some(&window));
}
