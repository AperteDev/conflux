//! Rows that edit one setting of the config each, the GNOME way: a combo or a
//! list applies at once, a text entry on its apply button (or Enter). A setting
//! left empty, or set to "Inherit", is removed from the file, so it inherits its
//! default again. A refused edit puts the row back and says why.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use conflux_core::config::parse_byte_size;
use conflux_core::config_edit::{toml_edit, ConfigDoc, Section};
use gtk::glib;

use crate::editor::Edit;
use crate::scope::ScopeView;
use crate::ui;

/// Where a form shows what went wrong.
pub type Toaster = Rc<dyn Fn(&str)>;

/// What a text entry holds, deciding how its text is written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A humantime duration, e.g. `5s`; checked by the daemon's parser.
    Duration,
    /// A byte count or a size like `100MB`; digits alone are written as a number.
    Size,
    /// A folder, with a button to pick it; written with `~` for the home folder.
    Folder,
    /// A file, with a button to pick it.
    File,
}

/// The rows editing one section of the config (`[daemon]`, a remote, a sync).
pub struct Form {
    view: Rc<ScopeView>,
    section: Section,
    /// The page's generation (see `ScopeView::generation`).
    generation: u64,
    editable: bool,
    toast: Toaster,
}

impl Form {
    pub fn new(view: &Rc<ScopeView>, section: Section, toast: Toaster) -> Rc<Self> {
        Rc::new(Form {
            view: view.clone(),
            section,
            generation: view.generation(),
            editable: view.editable(),
            toast,
        })
    }

    pub fn editable(&self) -> bool {
        self.editable
    }

    pub fn view(&self) -> &Rc<ScopeView> {
        &self.view
    }

    /// Write `edit`, then tell `done` whether it was written.
    pub fn commit(&self, edit: Edit, done: impl FnOnce(bool) + 'static) {
        let toast = self.toast.clone();
        self.view
            .edit(self.generation, edit, move |result| match result {
                Ok(()) => done(true),
                Err(e) => {
                    toast(&e);
                    done(false);
                }
            });
    }

    /// A text setting. `hint` says what an empty entry means (the inherited or
    /// default value); `required` ones cannot be emptied.
    pub fn entry(
        self: &Rc<Self>,
        title: &str,
        key: &'static str,
        current: Option<String>,
        kind: Kind,
        hint: Option<&str>,
        required: bool,
    ) -> adw::EntryRow {
        let row = adw::EntryRow::new();
        self.setup_entry(
            row.upcast_ref(),
            title,
            current,
            kind,
            hint,
            move |section, text| setting(section, key, text, kind, required),
        );
        row
    }

    /// A secret setting, e.g. a password, hidden as typed.
    pub fn password(
        self: &Rc<Self>,
        title: &str,
        key: &'static str,
        current: Option<String>,
    ) -> adw::PasswordEntryRow {
        let row = adw::PasswordEntryRow::new();
        self.setup_entry(
            row.upcast_ref(),
            title,
            current,
            Kind::Text,
            None,
            move |section, text| setting(section, key, text, Kind::Text, false),
        );
        row
    }

    /// A text entry whose text becomes the edit `to_edit` makes of it, or the
    /// message why it cannot.
    pub fn custom_entry(
        self: &Rc<Self>,
        title: &str,
        current: Option<String>,
        to_edit: impl Fn(Section, String) -> Result<Edit, String> + 'static,
    ) -> adw::EntryRow {
        let row = adw::EntryRow::new();
        self.setup_entry(row.upcast_ref(), title, current, Kind::Text, None, to_edit);
        row
    }

    fn setup_entry(
        self: &Rc<Self>,
        row: &adw::EntryRow,
        title: &str,
        current: Option<String>,
        kind: Kind,
        hint: Option<&str>,
        to_edit: impl Fn(Section, String) -> Result<Edit, String> + 'static,
    ) {
        row.set_title(&match hint {
            Some(hint) => format!("{title} (empty: {hint})"),
            None => title.to_string(),
        });
        row.set_use_markup(false);
        let written = Rc::new(RefCell::new(current.unwrap_or_default()));
        row.set_text(&written.borrow());
        row.set_show_apply_button(true);
        row.set_sensitive(self.editable);

        // The rows own the form; it holds no widget, so this makes no cycle.
        let form = self.clone();
        let apply = Rc::new(move |row: &adw::EntryRow| {
            let text = row.text().trim().to_string();
            if text == *written.borrow() {
                return;
            }
            let edit = match to_edit(form.section, text.clone()) {
                Ok(edit) => edit,
                Err(e) => {
                    (form.toast)(&e);
                    ui::reset_entry(row, &written.borrow());
                    return;
                }
            };
            let row = row.clone();
            let written = written.clone();
            form.commit(edit, move |ok| {
                if ok {
                    *written.borrow_mut() = text;
                } else {
                    ui::reset_entry(&row, &written.borrow());
                }
            });
        });
        let on_apply = apply.clone();
        row.connect_apply(move |row| on_apply(row));

        if matches!(kind, Kind::Folder | Kind::File) && self.editable {
            let window = self.view.ctx().window.clone();
            ui::add_chooser(&window, row, kind == Kind::Folder, move |row| apply(row));
        }
    }

    /// A setting with a fixed set of values. `options` pairs each value (`None`:
    /// removed, so inherited) with its label. When the setting is not in the
    /// file, the option `unset_means` is shown, and choosing it writes nothing.
    pub fn choice(
        self: &Rc<Self>,
        title: &str,
        key: &'static str,
        options: Vec<(Option<String>, String)>,
        current: Option<String>,
        unset_means: Option<&'static str>,
    ) -> adw::ComboRow {
        let labels: Vec<&str> = options.iter().map(|(_, label)| label.as_str()).collect();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&labels))
            .use_markup(false)
            .sensitive(self.editable)
            .build();
        let values: Vec<Option<String>> = options.into_iter().map(|(v, _)| v).collect();
        let find = |v: Option<&str>| values.iter().position(|o| o.as_deref() == v);
        let shown = match current.as_deref() {
            Some(v) => find(Some(v)),
            None => find(None).or_else(|| find(unset_means)),
        };
        row.set_selected(shown.unwrap_or(0) as u32);

        let written = Rc::new(RefCell::new(current));
        let selected = Rc::new(Cell::new(row.selected()));
        let quiet = Rc::new(Cell::new(false));
        let form = self.clone();
        row.connect_selected_notify(move |row| {
            if quiet.get() || row.selected() == selected.get() {
                return;
            }
            let Some(value) = values.get(row.selected() as usize).cloned() else {
                return;
            };
            if written.borrow().is_none() && (value.is_none() || value.as_deref() == unset_means) {
                selected.set(row.selected());
                return;
            }
            let section = form.section;
            let edit: Edit = match value.clone() {
                Some(v) => Box::new(move |doc: &mut ConfigDoc| doc.set(section, key, v.as_str())),
                None => Box::new(move |doc: &mut ConfigDoc| doc.unset(section, key).map(|_| ())),
            };
            let (row, written, selected, quiet) = (
                row.clone(),
                written.clone(),
                selected.clone(),
                quiet.clone(),
            );
            form.commit(edit, move |ok| {
                if ok {
                    *written.borrow_mut() = value;
                    selected.set(row.selected());
                } else {
                    quiet.set(true);
                    row.set_selected(selected.get());
                    quiet.set(false);
                }
            });
        });
        row
    }

    /// A list setting as a group of its own: one row per item, with a button to
    /// remove it, and an entry to add one. When the list ends up equal to
    /// `default`, the setting is removed from the file instead.
    pub fn list(
        self: &Rc<Self>,
        title: &str,
        description: &str,
        key: &'static str,
        items: Vec<String>,
        default: Vec<String>,
        add_title: &str,
    ) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(title)
            .description(description)
            .build();
        let list = Rc::new(ListEditor {
            form: self.clone(),
            group: group.downgrade(),
            key,
            default,
            items: RefCell::new(items),
            rows: RefCell::default(),
            add: adw::EntryRow::builder()
                .title(add_title)
                .show_apply_button(true)
                .use_markup(false)
                .build(),
            editable: self.editable,
        });
        let weak = Rc::downgrade(&list);
        list.add.connect_apply(move |entry| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            let item = entry.text().trim().to_string();
            if item.is_empty() {
                return;
            }
            let mut items = list.items.borrow().clone();
            if !items.contains(&item) {
                items.push(item);
            }
            let entry = entry.clone();
            list.save(items, move || ui::reset_entry(&entry, ""));
        });
        list.render();
        // The group keeps its editor alive; the editor holds the group weakly.
        // SAFETY: the key is only ever used with this type.
        unsafe { group.set_data("list-editor", list) };
        group
    }
}

/// The edit writing `text` as `key` of `section`, or removing it when empty.
fn setting(
    section: Section,
    key: &'static str,
    text: String,
    kind: Kind,
    required: bool,
) -> Result<Edit, String> {
    if text.is_empty() {
        if required {
            return Err(format!("`{key}` cannot be empty"));
        }
        return Ok(Box::new(move |doc| doc.unset(section, key).map(|_| ())));
    }
    let value: toml_edit::Value = match kind {
        Kind::Duration => {
            humantime::parse_duration(&text).map_err(|_| {
                format!("{text} is not a duration; write it like 30s, 5m or 1h (0s: off)")
            })?;
            text.into()
        }
        Kind::Size => {
            let not_a_size = || {
                format!("{text} is not a size; write it like 500KB, 100MB or 2GiB (0: no limit)")
            };
            parse_byte_size(&text).map_err(|_| not_a_size())?;
            // A bare byte count is written as a number.
            match text.parse::<i64>() {
                Ok(n) => n.into(),
                Err(_) if text.chars().all(|c| c.is_ascii_digit()) => return Err(not_a_size()),
                Err(_) => text.into(),
            }
        }
        _ => text.into(),
    };
    Ok(Box::new(move |doc| doc.set(section, key, value)))
}

struct ListEditor {
    form: Rc<Form>,
    group: glib::WeakRef<adw::PreferencesGroup>,
    key: &'static str,
    default: Vec<String>,
    items: RefCell<Vec<String>>,
    rows: RefCell<Vec<adw::ActionRow>>,
    add: adw::EntryRow,
    editable: bool,
}

impl ListEditor {
    fn render(self: &Rc<Self>) {
        let Some(group) = self.group.upgrade() else {
            return;
        };
        for row in self.rows.borrow_mut().drain(..) {
            group.remove(&row);
        }
        if self.add.parent().is_some() {
            group.remove(&self.add);
        }
        let items = self.items.borrow().clone();
        for item in items {
            let row = adw::ActionRow::builder()
                .title(&item)
                .use_markup(false)
                .title_selectable(true)
                .build();
            if self.editable {
                let remove = gtk::Button::builder()
                    .icon_name("list-remove-symbolic")
                    .tooltip_text("Remove")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                let weak = Rc::downgrade(self);
                remove.connect_clicked(move |_| {
                    if let Some(list) = weak.upgrade() {
                        let items: Vec<String> = list
                            .items
                            .borrow()
                            .iter()
                            .filter(|i| **i != item)
                            .cloned()
                            .collect();
                        list.save(items, || {});
                    }
                });
                row.add_suffix(&remove);
            }
            group.add(&row);
            self.rows.borrow_mut().push(row);
        }
        if self.editable {
            group.add(&self.add);
        }
    }

    fn save(self: &Rc<Self>, items: Vec<String>, then: impl FnOnce() + 'static) {
        let form = &self.form;
        let (section, key) = (form.section, self.key);
        let edit: Edit = if items == self.default {
            Box::new(move |doc| doc.unset(section, key).map(|_| ()))
        } else {
            let array: toml_edit::Array = items.iter().map(String::as_str).collect();
            Box::new(move |doc| doc.set(section, key, array))
        };
        let list = self.clone();
        form.commit(edit, move |ok| {
            if ok {
                *list.items.borrow_mut() = items;
                list.render();
                then();
            }
        });
    }
}

/// The value of `key` in `section` as written, for an entry or a combo.
pub fn written(doc: &ConfigDoc, section: Section, key: &str) -> Option<String> {
    let value = doc.get(section, key)?;
    Some(match value {
        toml_edit::Value::String(s) => s.value().clone(),
        other => {
            let mut other = other.clone();
            other.decor_mut().clear();
            other.to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_written() {
        let doc = ConfigDoc::parse(
            "[daemon]\ndebounce = \"60m\"   # slow\nmax_file_size = 1000\nexclude = [\"a\", \"b\"]\n",
        )
        .unwrap();
        assert_eq!(
            written(&doc, Section::Daemon, "debounce").as_deref(),
            Some("60m")
        );
        assert_eq!(
            written(&doc, Section::Daemon, "max_file_size").as_deref(),
            Some("1000")
        );
        assert_eq!(written(&doc, Section::Daemon, "interval"), None);
    }

    #[test]
    fn entries_write_their_kind() {
        let mut doc = ConfigDoc::parse("[daemon]\ndebounce = \"5s\"\n").unwrap();
        setting(
            Section::Daemon,
            "max_file_size",
            "1000".into(),
            Kind::Size,
            false,
        )
        .unwrap()(&mut doc)
        .unwrap();
        setting(
            Section::Daemon,
            "interval",
            "2h".into(),
            Kind::Duration,
            false,
        )
        .unwrap()(&mut doc)
        .unwrap();
        setting(
            Section::Daemon,
            "debounce",
            String::new(),
            Kind::Duration,
            false,
        )
        .unwrap()(&mut doc)
        .unwrap();
        assert_eq!(
            doc.to_string(),
            "[daemon]\nmax_file_size = 1000\ninterval = \"2h\"\n"
        );
        assert!(setting(Section::Sync(0), "local", String::new(), Kind::Folder, true).is_err());
        let err = setting(
            Section::Daemon,
            "debounce",
            "abc".into(),
            Kind::Duration,
            false,
        );
        assert!(err.is_err_and(|e| e.contains("not a duration")));
        let err = setting(
            Section::Daemon,
            "max_file_size",
            "big".into(),
            Kind::Size,
            false,
        );
        assert!(err.is_err_and(|e| e.contains("not a size")));
        let size = setting(
            Section::Daemon,
            "max_file_size",
            "2GiB".into(),
            Kind::Size,
            false,
        );
        assert!(size.is_ok());
    }
}
