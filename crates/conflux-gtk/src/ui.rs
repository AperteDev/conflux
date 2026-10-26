//! Small helpers for building the UI.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::text;

/// Load a Blueprint view compiled by `build.rs`.
macro_rules! builder {
    ($name:literal) => {
        gtk::Builder::from_string(include_str!(concat!(env!("OUT_DIR"), "/", $name, ".ui")))
    };
}
pub(crate) use builder;

/// The object `id` of a view; a missing one is a bug in the UI file.
pub fn get<T: IsA<glib::Object>>(b: &gtk::Builder, id: &str) -> T {
    b.object(id)
        .unwrap_or_else(|| panic!("no `{id}` in the UI file"))
}

/// A row showing user text as is (paths, messages): markup off.
pub fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .use_markup(false)
        .build()
}

/// A read-only setting: its name small, its value in full and selectable.
pub fn property(title: &str, value: &str) -> adw::ActionRow {
    let row = row(title, value);
    row.add_css_class("property");
    row.set_subtitle_selectable(true);
    row
}

/// An icon showing a state, colored by a style class (`success`, `warning`,
/// `error`, `dim-label`).
pub fn state_icon(icon: &str, class: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class(class);
    image
}

/// A status page with a spinner, for while something loads.
pub fn loading(title: &str) -> adw::StatusPage {
    let page = adw::StatusPage::builder().title(title).build();
    page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&page))));
    page
}

/// Put `text` in an entry row without revealing its apply button, as setting
/// it otherwise counts as an edit waiting to be applied.
pub fn reset_entry(row: &adw::EntryRow, text: &str) {
    let apply = row.shows_apply_button();
    row.set_show_apply_button(false);
    row.set_text(text);
    row.set_show_apply_button(apply);
}

/// Make `child` the visible page `name` of `stack`, replacing any old one.
pub fn replace_child(stack: &gtk::Stack, name: &str, child: &impl IsA<gtk::Widget>) {
    if let Some(old) = stack.child_by_name(name) {
        stack.remove(&old);
    }
    stack.add_named(child, Some(name));
    stack.set_visible_child_name(name);
}

/// Add a button to `row` that picks a folder (or a file) and puts its path in
/// the row, with `~` for the home folder, then calls `picked`. Returns the
/// button, for rows that need it only sometimes.
pub fn add_chooser(
    window: &adw::ApplicationWindow,
    row: &adw::EntryRow,
    folder: bool,
    picked: impl Fn(&adw::EntryRow) + 'static,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(if folder {
            "folder-open-symbolic"
        } else {
            "document-open-symbolic"
        })
        .tooltip_text(if folder {
            "Choose a Folder"
        } else {
            "Choose a File"
        })
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let target = row.clone();
    let window = window.clone();
    let picked = std::rc::Rc::new(picked);
    button.connect_clicked(move |_| {
        let target = target.clone();
        let picked = picked.clone();
        let done = move |result: Result<gio::File, glib::Error>| {
            if let Some(path) = result.ok().and_then(|f| f.path()) {
                target.set_text(&text::path(&path));
                picked(&target);
            }
        };
        let dialog = gtk::FileDialog::new();
        if folder {
            dialog.select_folder(Some(&window), gio::Cancellable::NONE, done);
        } else {
            dialog.open(Some(&window), gio::Cancellable::NONE, done);
        }
    });
    row.add_suffix(&button);
    button
}
