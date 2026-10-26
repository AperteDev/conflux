//! Edits to a config file that leave everything else in it as written.
//!
//! The file stays the single source of truth, also when a GUI edits it, so an
//! edit changes only the keys it touches: comments, key order, blank lines and
//! the spelling of untouched values (`"60m"` stays `"60m"`) survive. Removing a
//! key is how a setting goes back to inheriting its default.
//!
//! Edits are not validated here; check the result with [`ConfigDoc::config`]
//! (the daemon's own checks) before writing it with [`write_atomic`].

use std::fmt;
use std::io::Write;
use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::config::Config;
use crate::error::{Error, Result};

pub use toml_edit;

/// A part of the config that holds settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The `[daemon]` table.
    Daemon,
    /// The `[[remote]]` at this index.
    Remote(usize),
    /// The `[[sync]]` at this index.
    Sync(usize),
}

/// The kind of an array-of-tables entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A `[[remote]]`.
    Remote,
    /// A `[[sync]]`.
    Sync,
}

impl Entry {
    fn key(self) -> &'static str {
        match self {
            Entry::Remote => "remote",
            Entry::Sync => "sync",
        }
    }
}

/// A config file loaded for editing.
#[derive(Debug, Clone)]
pub struct ConfigDoc {
    doc: DocumentMut,
}

impl ConfigDoc {
    /// Parse config text. Only the TOML syntax is checked here.
    pub fn parse(text: &str) -> Result<Self> {
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|e| Error::ConfigEdit(e.to_string()))?;
        Ok(ConfigDoc { doc })
    }

    /// The config as the daemon would load it: resolved and validated.
    pub fn config(&self) -> Result<Config> {
        Config::from_toml_str(&self.to_string())
    }

    /// The config as written: parsed against the schema, nothing resolved.
    pub fn raw(&self) -> Result<Config> {
        Config::parse_raw(&self.to_string())
    }

    /// The value of `key` in `section`, if set.
    pub fn get(&self, section: Section, key: &str) -> Option<&Value> {
        self.table(section)?.get(key)?.as_value()
    }

    /// Set `key` in `section`. An existing value keeps its surrounding spacing
    /// and trailing comment, and is left untouched when it already reads the
    /// same. Setting a key of `[daemon]` creates the table when it is missing.
    pub fn set(&mut self, section: Section, key: &str, value: impl Into<Value>) -> Result<()> {
        let mut value = value.into();
        let table = self.table_mut(section)?;
        match table.get_mut(key) {
            Some(Item::Value(old)) => {
                if same_value(old, &value) {
                    return Ok(());
                }
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            Some(_) => {
                return Err(Error::ConfigEdit(format!(
                    "`{key}` in {section} is a table, not a value"
                )));
            }
            None => {
                table.insert(key, Item::Value(value));
            }
        }
        Ok(())
    }

    /// Remove `key` from `section`, so the setting inherits its default again.
    /// Returns whether it was set.
    pub fn unset(&mut self, section: Section, key: &str) -> Result<bool> {
        if section == Section::Daemon && self.doc.get("daemon").is_none() {
            return Ok(false);
        }
        Ok(self.table_mut(section)?.remove(key).is_some())
    }

    /// Append a `[[remote]]` or `[[sync]]` after the last one of its kind (or at
    /// the end of the file for the first) and return its index.
    pub fn add(&mut self, entry: Entry, table: Table) -> Result<usize> {
        let array = match self.doc.get_mut(entry.key()) {
            Some(item) => item.as_array_of_tables_mut().ok_or_else(|| {
                Error::ConfigEdit(format!(
                    "`{}` is not written as [[{}]] tables",
                    entry.key(),
                    entry.key()
                ))
            })?,
            None => {
                let mut table = table;
                self.take_trailing(table.decor_mut());
                let mut array = ArrayOfTables::new();
                array.push(table);
                self.doc.insert(entry.key(), Item::ArrayOfTables(array));
                return Ok(0);
            }
        };
        array.push(table);
        Ok(array.len() - 1)
    }

    /// How many `[[remote]]` or `[[sync]]` tables there are.
    pub fn len(&self, entry: Entry) -> usize {
        self.array(entry).map_or(0, ArrayOfTables::len)
    }

    /// Remove the `[[sync]]` at `index`, with the comments right above it.
    pub fn remove_sync(&mut self, index: usize) -> Result<()> {
        let array = self.array_mut(Entry::Sync)?;
        if index >= array.len() {
            return Err(missing(Section::Sync(index)));
        }
        array.remove(index);
        Ok(())
    }

    /// Remove the `[[remote]]` at `index`. Refused while a sync still uses it.
    pub fn remove_remote(&mut self, index: usize) -> Result<()> {
        let id = self.remote_id(index)?;
        let users = self.syncs_using(&id);
        if !users.is_empty() {
            return Err(Error::ConfigEdit(format!(
                "remote `{id}` is used by {}; change or remove {} first",
                users.join(", "),
                if users.len() == 1 { "it" } else { "them" }
            )));
        }
        self.array_mut(Entry::Remote)?.remove(index);
        Ok(())
    }

    /// Change the id of the `[[remote]]` at `index`, and every `remote = "..."`
    /// that refers to it.
    pub fn rename_remote(&mut self, index: usize, new_id: &str) -> Result<()> {
        let old_id = self.remote_id(index)?;
        if old_id == new_id {
            return Ok(());
        }
        self.set(Section::Remote(index), "id", new_id)?;
        for i in 0..self.len(Entry::Sync) {
            if self.get(Section::Sync(i), "remote").and_then(Value::as_str) == Some(&old_id) {
                self.set(Section::Sync(i), "remote", new_id)?;
            }
        }
        Ok(())
    }

    /// Labels of the syncs whose `remote` is `id`: their `id`, else their
    /// `local`, else their position.
    pub fn syncs_using(&self, id: &str) -> Vec<String> {
        let Some(syncs) = self.array(Entry::Sync) else {
            return Vec::new();
        };
        syncs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.get("remote").and_then(Item::as_str) == Some(id))
            .map(|(i, t)| {
                let name = |key| t.get(key).and_then(Item::as_str).map(str::to_string);
                name("id")
                    .or_else(|| name("local"))
                    .map(|n| format!("sync `{n}`"))
                    .unwrap_or_else(|| format!("sync #{}", i + 1))
            })
            .collect()
    }

    /// Move the text after the last table (in a file of only comments, all of
    /// it) above a new top-level table, which would otherwise be written before
    /// that text, so the file reads as before with the table appended.
    fn take_trailing(&mut self, decor: &mut toml_edit::Decor) {
        let trailing = self.doc.trailing().as_str().unwrap_or_default().to_string();
        if trailing.is_empty() {
            return;
        }
        let gap = if trailing.ends_with("\n\n") { "" } else { "\n" };
        decor.set_prefix(format!("{trailing}{gap}"));
        self.doc.set_trailing("");
    }

    fn remote_id(&self, index: usize) -> Result<String> {
        self.get(Section::Remote(index), "id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| missing(Section::Remote(index)))
    }

    fn array(&self, entry: Entry) -> Option<&ArrayOfTables> {
        self.doc.get(entry.key())?.as_array_of_tables()
    }

    fn array_mut(&mut self, entry: Entry) -> Result<&mut ArrayOfTables> {
        self.doc
            .get_mut(entry.key())
            .and_then(Item::as_array_of_tables_mut)
            .ok_or_else(|| Error::ConfigEdit(format!("the config has no [[{}]]", entry.key())))
    }

    fn table(&self, section: Section) -> Option<&Table> {
        match section {
            Section::Daemon => self.doc.get("daemon")?.as_table(),
            Section::Remote(i) => self.array(Entry::Remote)?.get(i),
            Section::Sync(i) => self.array(Entry::Sync)?.get(i),
        }
    }

    fn table_mut(&mut self, section: Section) -> Result<&mut Table> {
        let (entry, index) = match section {
            Section::Daemon => {
                if self.doc.get("daemon").is_none() {
                    let mut table = Table::new();
                    self.take_trailing(table.decor_mut());
                    self.doc.insert("daemon", Item::Table(table));
                }
                return self.doc["daemon"].as_table_mut().ok_or_else(|| {
                    Error::ConfigEdit("`daemon` is not written as a [daemon] table".into())
                });
            }
            Section::Remote(i) => (Entry::Remote, i),
            Section::Sync(i) => (Entry::Sync, i),
        };
        self.array_mut(entry)?
            .get_mut(index)
            .ok_or_else(|| missing(section))
    }
}

impl fmt::Display for ConfigDoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.doc.fmt(f)
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Section::Daemon => f.write_str("[daemon]"),
            Section::Remote(i) => write!(f, "[[remote]] #{}", i + 1),
            Section::Sync(i) => write!(f, "[[sync]] #{}", i + 1),
        }
    }
}

fn missing(section: Section) -> Error {
    Error::ConfigEdit(format!("the config has no {section}"))
}

/// Whether two values read the same, ignoring the spacing and comments around
/// them.
fn same_value(a: &Value, b: &Value) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.decor_mut().clear();
    b.decor_mut().clear();
    a.to_string().trim() == b.to_string().trim()
}

/// Whether `new` may replace `old` (`None`: there is no file yet). A config the
/// daemon accepts must not be replaced by one it refuses; one it already refuses
/// may be, so that it can be fixed step by step. A new file must be accepted.
pub fn check_replacement(old: Option<&str>, new: &str) -> Result<()> {
    if let Err(e) = Config::from_toml_str(new) {
        let already_refused = old.is_some_and(|old| Config::from_toml_str(old).is_err());
        if !already_refused {
            return Err(e);
        }
    }
    Ok(())
}

/// Replace the file at `path` with `text` in one step: write a temporary file
/// next to it, give it the old file's mode and owner, then rename it over the
/// old one. A reader never sees a half-written config.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the config path has no file name"))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        match std::fs::metadata(path) {
            Ok(meta) => {
                file.set_permissions(std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
                // Only root can give a file away; anyone else keeps it as is.
                let _ = std::os::unix::fs::fchown(&file, Some(meta.uid()), Some(meta.gid()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::Array;

    const CONFIG: &str = r#"# My conflux config.

[daemon]
debounce = "60m"   # long on purpose
log_level = "info"

# The NAS.
[[remote]]
id = "nas"
backend = "filesystem"
url = "/mnt/nas"

# Documents, mirrored.
[[sync]]
id = "docs"
remote = "nas"
local = "/home/me/Documents"
scope = "mirror"

[[sync]]
remote_fs = "/mnt/backup"
local = "/home/me/Pictures"
include = ["**"]
"#;

    fn doc() -> ConfigDoc {
        ConfigDoc::parse(CONFIG).unwrap()
    }

    #[test]
    fn untouched_text_round_trips() {
        assert_eq!(doc().to_string(), CONFIG);
    }

    #[test]
    fn set_replaces_only_the_value_and_keeps_its_comment() {
        let mut d = doc();
        d.set(Section::Daemon, "debounce", "2s").unwrap();
        assert_eq!(
            d.to_string(),
            CONFIG.replace(r#"debounce = "60m"   #"#, r#"debounce = "2s"   #"#)
        );
    }

    #[test]
    fn setting_the_same_value_changes_nothing() {
        let mut d = doc();
        d.set(Section::Daemon, "debounce", "60m").unwrap();
        d.set(Section::Sync(0), "scope", "mirror").unwrap();
        assert_eq!(d.to_string(), CONFIG);
    }

    #[test]
    fn unset_removes_the_key_so_it_inherits() {
        let mut d = doc();
        assert!(d.unset(Section::Sync(0), "scope").unwrap());
        assert!(!d.unset(Section::Sync(0), "scope").unwrap());
        assert_eq!(d.to_string(), CONFIG.replace("scope = \"mirror\"\n", ""));
        assert_eq!(d.config().unwrap().syncs[0].scope, None);
    }

    #[test]
    fn new_keys_go_at_the_end_of_their_table() {
        let mut d = doc();
        d.set(Section::Sync(1), "trigger", "manual").unwrap();
        assert_eq!(
            d.to_string(),
            CONFIG.replace(
                "include = [\"**\"]\n",
                "include = [\"**\"]\ntrigger = \"manual\"\n"
            )
        );
    }

    #[test]
    fn added_sync_follows_the_last_sync_and_validates() {
        let mut d = doc();
        let mut table = Table::new();
        table.insert("id", toml_edit::value("music"));
        table.insert("remote", toml_edit::value("nas"));
        table.insert("local", toml_edit::value("/home/me/Music"));
        table.insert("remote_path", toml_edit::value("music"));
        let mut include = Array::new();
        include.push("**");
        table.insert("include", toml_edit::value(include));
        assert_eq!(d.add(Entry::Sync, table).unwrap(), 2);
        let text = d.to_string();
        assert!(text.starts_with(CONFIG));
        assert!(text.ends_with(
            "\n[[sync]]\nid = \"music\"\nremote = \"nas\"\nlocal = \"/home/me/Music\"\nremote_path = \"music\"\ninclude = [\"**\"]\n"
        ));
        assert_eq!(d.config().unwrap().syncs.len(), 3);
    }

    #[test]
    fn first_entries_and_daemon_table_are_created_when_missing() {
        let mut d = ConfigDoc::parse("# only comments\n").unwrap();
        d.set(Section::Daemon, "log_level", "debug").unwrap();
        let mut remote = Table::new();
        remote.insert("id", toml_edit::value("r"));
        remote.insert("backend", toml_edit::value("filesystem"));
        remote.insert("url", toml_edit::value("/tmp/r"));
        d.add(Entry::Remote, remote).unwrap();
        let config = d.config().unwrap();
        assert_eq!(config.daemon.log_level, "debug");
        assert_eq!(config.remotes[0].id, "r");
        assert!(d.to_string().starts_with("# only comments\n"));
    }

    #[test]
    fn renaming_a_remote_updates_the_syncs_using_it() {
        let mut d = doc();
        d.rename_remote(0, "storage").unwrap();
        assert_eq!(d.to_string(), CONFIG.replace("\"nas\"", "\"storage\""));
        d.config().unwrap();
    }

    #[test]
    fn a_remote_in_use_is_not_removed() {
        let mut d = doc();
        let err = d.remove_remote(0).unwrap_err().to_string();
        assert!(err.contains("sync `docs`"), "{err}");
        d.remove_sync(0).unwrap();
        d.remove_remote(0).unwrap();
        let text = d.to_string();
        assert!(!text.contains("[[remote]]") && !text.contains("# The NAS."));
        assert!(!text.contains("# Documents, mirrored."));
        d.config().unwrap();
    }

    #[test]
    fn raw_keeps_paths_and_shortcuts_as_written() {
        let d = ConfigDoc::parse("[[sync]]\nremote_fs = \"~/b\"\nlocal = \"~/a\"\n").unwrap();
        let raw = d.raw().unwrap();
        assert_eq!(raw.remotes.len(), 0);
        assert_eq!(raw.syncs[0].local, Path::new("~/a"));
        assert_eq!(raw.syncs[0].remote_fs.as_deref(), Some(Path::new("~/b")));
    }

    #[test]
    fn syntax_errors_are_reported() {
        assert!(matches!(
            ConfigDoc::parse("[daemon\n"),
            Err(Error::ConfigEdit(_))
        ));
    }

    #[test]
    fn edits_to_the_example_config_stay_minimal() {
        let example = include_str!("../../../config.example.toml");
        let mut d = ConfigDoc::parse(example).unwrap();
        let mut sync = Table::new();
        sync.insert("remote_fs", toml_edit::value("/tmp/backup"));
        sync.insert("local", toml_edit::value("/tmp/docs"));
        d.add(Entry::Sync, sync).unwrap();
        d.set(Section::Daemon, "log_level", "debug").unwrap();
        let text = d.to_string();
        assert!(text.starts_with(example), "the example's text changed");
        d.config().unwrap();
    }

    #[test]
    fn replacements_may_not_break_an_accepted_config() {
        let good = "[[sync]]\nremote_fs = \"/b\"\nlocal = \"/a\"\n";
        let bad = "[[sync]]\nlocal = \"/a\"\n";
        assert!(check_replacement(Some(good), good).is_ok());
        assert!(check_replacement(Some(good), bad).is_err());
        assert!(check_replacement(Some(bad), bad).is_ok());
        assert!(check_replacement(None, good).is_ok());
        assert!(check_replacement(None, bad).is_err());
    }

    #[test]
    fn write_atomic_replaces_the_file_and_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
