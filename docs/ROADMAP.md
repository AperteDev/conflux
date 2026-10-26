# Roadmap

The plans for conflux: what is decided, what is being worked on, and what comes later. Keep it
current: when a change makes a decision, finishes a step or drops a plan, update this file in the
same commit (tick the step, move it, or note why it was dropped).

Status markers: `[ ]` planned, `[~]` in progress, `[x]` done.

## Current state

- 1.1.0 released: daemon + CLI in one binary, WebDAV / Git / filesystem backends, `git_repo`
  syncs of an existing clone, triggers, scopes, profiles, `sync --dry-run`, SSH key selection,
  systemd units, Arch packages. Details in [`CHANGELOG.md`](CHANGELOG.md).

## GTK frontend (`conflux-gtk`)

A libadwaita app to view and edit the configuration and control the daemons, for the user and
the system scope.

### Goals and constraints

- **Optional.** Its own crate, binary and package. The daemon and CLI never depend on GTK;
  `default-members` keeps a plain `cargo build` / `cargo test` free of GTK dev libraries.
- **Compatible with a hand-written config.** The TOML file stays the only source of truth: the
  GUI keeps no state of its own and never rewrites the whole file.
- **Two scopes.** User (`~/.config/conflux/config.toml`, user socket, `systemd --user`) and
  System (`/etc/conflux/config.toml`, `/run/conflux/*.sock`, system units). System is readable
  by anyone and editable only after unlocking, like Printers in GNOME Settings.

### Decisions

- App id `dev.aperte.Conflux`; other ids use the `dev.aperte` namespace too (polkit action
  `dev.aperte.conflux.manage`, files `dev.aperte.Conflux.desktop`,
  `dev.aperte.Conflux.metainfo.xml`, `dev.aperte.conflux.policy`), so the repository can move to
  the AperteDev organization without renames.
- Rust with `gtk4` + `libadwaita` (at least 1.7, for `AdwToggleGroup`), `zbus` for systemd and
  logind. Static views are Blueprint files compiled into a GResource by `build.rs`; dynamic rows
  stay in Rust code.
- Layout: `AdwApplicationWindow` > `AdwToolbarView` > `AdwNavigationView`. The header bar holds
  an `AdwToggleGroup` (User | System) at the start, an `AdwViewSwitcher` (Syncs | Remotes) in the
  middle (an `AdwViewSwitcherBar` when narrow), and `[+]` plus the main menu (Preferences, Open
  config in editor, About) at the end.
- System socket: mode `0666`, and the daemon checks the peer's credentials (`SO_PEERCRED`).
  `Status` and `Info` are open to everyone (the config they describe is already world-readable);
  `Sync`, `Plan` and `Reload` need root, the service user or a member of the `conflux` group.
  The GUI sends those through the unlocked helper.
- System unlock: one `pkexec` launch of a long-lived, minimal `conflux-helper` (newline-delimited
  JSON over stdin/stdout). One polkit prompt per unlock; lock (button, idle timeout of 5 min,
  window closed, GUI gone so stdin hits EOF) ends the helper.

### UI

```
+---------------------------------------------------------------+
| [User | System]     [ Syncs | Remotes ]          [+]  [menu]  |
+---------------------------------------------------------------+
|  Some settings are locked                        [Unlock...]  |  System only
+---------------------------------------------------------------+
|  Instances                                [Add instance...]   |
|   (o) conflux.service        Running   [Start at login (x)] : |
|   ( ) conflux@desktop        Inactive  [               ( )] : |
|                                                               |
|  Sync groups                                     [Sync all]   |
|   [icon] dotfiles            ~  <->  dotfiles:/               |
|          timer 15m - 3 min ago - 2 up, 1 down       [sync] :  |
|   [icon] Documents           ~/Documents <-> nextcloud:docs   |
|          ERROR 10:42: 401 Unauthorized              [sync] :  |
+---------------------------------------------------------------+
```

- **Syncs page:** the Instances group (below), then the sync groups from the config with live
  status (last run, summary, error, a spinner while running). Row actions: Sync now, Dry run (a
  dialog with the plan, same markers as the CLI), Edit, Delete. Groups outside the shown
  instance's profile are dimmed, with their profile tags.
- **Sync detail page:** preference groups following `config.example.toml`: Identity (id,
  label), Mapping (remote, `remote_fs` or `git_repo`; local, remote_path), What (include,
  exclude, scope, deletions), When (trigger, interval, debounce, pull_interval), Other
  (empty_dirs, max_file_size, commit_msg_command, profiles). A `git_repo` group hides the
  file-level settings it rejects (remote_path, include, scope, deletions, empty_dirs,
  max_file_size) and shows commit_msg_command; the add dialog asks for the kind first.
- **Remotes page:** rows with a backend icon, id/label and URL; the detail page shows only the
  fields of the backend (auth for webdav and git over HTTPS, SSH key for git over SSH, branch and
  commit_msg_command for git, pull_interval / max_file_size overrides). `remote_fs` shortcuts are
  listed read-only as inline remotes, with "Convert to remote".
- **Inherited values:** every optional field offers "Inherit (current value)", e.g.
  `Deletions: Inherit (deny) / Deny / Allow`. Choosing it removes the key from the file.
- **Preferences** (`AdwPreferencesDialog`): the `[daemon]` table of the current scope
  (log_level, profile, debounce, interval, pull_interval, empty_dirs, deletions, max_file_size,
  exclude). App-only settings can join it later.
- **Empty and error states:** no config file (Create config), config does not parse (the error,
  plus Open in text editor), daemon not running (Start), system config unreadable (Unlock to
  view).
- **Add flows:** `[+]` opens an `AdwDialog` for a new sync or remote; it writes only once the
  entry is valid, so half-filled entries never reach the file.

### Editing the config

New module `conflux_core::config_edit`, on `toml_edit` (already used by `toml`):

- Load into a `DocumentMut` for edits, and into the typed structs without `normalize()` for
  display, so `~/Documents` shows as written and `remote_fs` remotes are not materialized.
- Change only the touched keys: comments, key order, blank lines and value spelling (`"60m"`
  stays `"60m"`) survive. New `[[sync]]` / `[[remote]]` tables go at the end.
- Renaming a remote id updates every `remote = "..."` using it; deleting a remote still in use
  is blocked, listing its syncs.
- Every write: apply the edit to the text, validate with `Config::from_toml_str` (the daemon's
  own checks) plus `warnings()`, write atomically (temp file + rename, keeping owner and mode),
  then `Reload` every running daemon of that scope.
- Instant apply (GNOME HIG): switches and combo rows save at once, entry rows on their apply
  button. A failed validation reverts the row and shows a toast.
- A `gio::FileMonitor` reloads the view on external edits; with an edit dialog open, a banner
  asks to reload instead.
- Tests in core with fixtures: comments survive, inherit removes the key, renames cascade, edits
  to `config.example.toml` give a minimal diff.

### Daemon and IPC changes

- Move the synchronous IPC client from `crates/conflux/src/control.rs` to
  `conflux_core::ipc::client`, shared by the CLI and the GUI (`gio::spawn_blocking`).
- `GroupStatus` gains `id: Option<String>` and `running: bool` (`#[serde(default)]`, so old and
  new peers still talk), to match status entries to config entries.
- `Request::Info`: version, profile and the config path in use.
- Status polled about every 3 s while the window is visible.
- System socket permissions and peer-credential checks as decided above.

### Instances

An instance is one daemon of a scope and profile: `conflux.service` for `default`,
`conflux@<profile>.service` otherwise; its socket and state dir come from `Paths::resolve`.

- **Discovery:** profiles named in the config's syncs plus `default`; unit files and units
  matching `conflux@*.service` (systemd D-Bus); sockets in the runtime dir, which shows daemons
  started by hand as "running, not managed by systemd" (no start/stop for those).
- **Rows:** profile and unit name, state (running, inactive, failed), an enable switch ("Start
  at login" / "Start at boot"), and a menu with Restart, Reload, View log, Remove.
- **Detail page:** unit, socket, state dir, the config path the daemon reads (from `Info`), and
  every sync group with a switch for its membership in this profile.
- **Add instance:** profile name checked against `[A-Za-z0-9_-]+` (one check in `conflux-core`,
  used by the CLI too); a checklist of syncs to include, written to their `profiles` (a sync
  without `profiles` is implicitly in `default`, so adding `desktop` writes
  `["default", "desktop"]`); "Start now" and "Start at login/boot".
- **Remove instance:** stop and disable the unit; optionally drop the profile from the syncs and
  delete the instance's state dir.
- **User scope:** "Keep running after logout" sets lingering through logind (allowed for oneself
  by default).
- **Sync detail:** "Profiles" becomes a multi-select of known instances; only `default` with no
  key in the file keeps the key out.
- **View log:** `journalctl [--user] -u <unit> -f` in a dialog (the system journal needs the
  `systemd-journal` group or the helper).
- Without the template unit installed, Add and Enable are hidden with a note; discovery by
  socket still works.

### Privileged helper (`crates/conflux-helper`)

Depends only on `conflux-core`; GTK never runs as root. Fixed commands:

- `write_config(text)`: validate, write atomically to `/etc/conflux/config.toml` (environment
  overrides ignored).
- `unit(action, profile)`: start, stop, restart, reload, enable, disable.
- `ipc(profile, request)`: `Sync`, `Plan`, `Reload` to the system socket.
- `remove_state(profile)`: only `/var/lib/conflux/<valid-profile>`.
- `journal(profile)`.

### Workspace layout

```
crates/conflux-core     + ipc::client, config_edit, Info, GroupStatus fields, profile check
crates/conflux          uses the core client; peer-credential socket checks
crates/conflux-helper   new, privileged, minimal
crates/conflux-gtk      new
data/                   .desktop, metainfo, icons, polkit policy, gresource
```

### Steps

1. [ ] **Core prep** (no UI): move the IPC client; `GroupStatus` fields and `Info`;
   `config_edit` with tests; shared profile-name check; system socket `0666` with
   peer-credential checks.
2. [ ] **Read-only GUI:** window, header, lists, live status, instance discovery and states,
   Sync now and Dry run in the user scope.
3. [ ] **Editing (user scope):** detail pages, add/delete dialogs, Preferences, file monitor,
   validation, automatic reload.
4. [ ] **Instance management (user scope):** add and remove, enable switches, profile
   membership, lingering, log viewer.
5. [ ] **System unlock:** helper and polkit policy, banner and lock binding, privileged writes,
   unit control and IPC, journal, state removal, automatic re-lock.
6. [ ] **Packaging:** split PKGBUILD (`conflux`, `conflux-gtk`, helper), `install.sh --gtk`, a CI
   job building the GUI (a container with libadwaita >= 1.7; the static cross builds stay
   CLI-only), README section, screenshots.

## Later

- Field-level validation errors; an include/exclude preview of the local tree.
- Keyring helper for `password_command` (the system daemon cannot reach a user keyring).
- Event stream from the daemon instead of status polling; desktop notifications on sync errors.
- Translations (gettext).
- Flatpak of the GUI, user scope only (the system scope cannot work from a sandbox).
