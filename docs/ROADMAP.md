# Roadmap

The plans for conflux: what is decided, what is being worked on, and what comes later. Keep it
current: when a change makes a decision, finishes a step or drops a plan, update this file in the
same commit (tick the step, move it, or note why it was dropped).

Status markers: `[ ]` planned, `[~]` in progress, `[x]` done.

## Current state

- 1.1.0 released: daemon + CLI in one binary, WebDAV / Git / filesystem backends, `git_repo`
  syncs of an existing clone, triggers, scopes, profiles, `sync --dry-run`, SSH key selection,
  systemd units, Arch packages. Details in [`CHANGELOG.md`](CHANGELOG.md).
- Unreleased: the GTK app (steps 1 to 5 below) and packages for both (steps 6 to 9).

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
- Rust with `gtk4` 0.11 + `libadwaita` 0.9 (libadwaita 1.7 or newer, for `AdwToggleGroup`);
  the crate needs Rust 1.92, the daemon and CLI stay at 1.85. systemd and logind are reached
  through gio's D-Bus (`gio::DBusConnection`) on GTK's own main loop rather than `zbus`, so no
  second async runtime. Static views are Blueprint files compiled by `build.rs` and loaded with
  `gtk::Builder` (as in Todav); dynamic rows stay in Rust code.
- Layout: `AdwApplicationWindow` > `AdwToolbarView` > `AdwNavigationView`. The header bar holds
  an `AdwToggleGroup` (User | System) at the start, an `AdwViewSwitcher` (Syncs | Remotes) in the
  middle (an `AdwViewSwitcherBar` when narrow), and `[+]` plus the main menu (Preferences, Open
  config in editor, About) at the end.
- System socket: mode `0666`, and the daemon checks the peer's credentials (`SO_PEERCRED`).
  `Status` and `Info` are open to everyone (the config they describe is already world-readable);
  `Sync`, `Plan` and `Reload` need root, the service user or a member of the `conflux` group
  (supplementary groups read with `SO_PEERGROUPS`). The GUI sends those through the unlocked
  helper. A user daemon's socket keeps its default mode in the private `$XDG_RUNTIME_DIR`.
- Profile names: 1-64 ASCII letters, digits, `_`, `-` and `.`, not starting with `.` or `-`
  (`paths::validate_profile`, checked by `Paths::resolve`). `.` is allowed so host names work
  as profiles.
- System unlock: one `pkexec` launch of a long-lived, minimal `conflux-helper` (newline-delimited
  JSON over stdin/stdout). One polkit prompt per unlock, which then lasts until Lock or until
  the window closes (the GUI gone, so stdin hits EOF); no idle timeout.

### UI

```
+---------------------------------------------------------------+
| [User | System]     [ Syncs | Remotes ]          [+]  [menu]  |
+---------------------------------------------------------------+
|  Some settings are locked                        [Unlock...]  |  System only
+---------------------------------------------------------------+
|  Daemons                                  [Add instance...]   |
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

- **Syncs page:** config warnings, the Daemons group (instances, below), then the sync groups
  from the config with live status (last run, summary, error, a spinner while running). Row
  actions: Sync now, a menu with Dry run (a dialog with the plan) and Open local folder; Edit
  and Delete come with editing. Groups no running daemon runs are dimmed, with their profiles.
  A config the daemon would refuse shows a banner with the error; its syncs are still named
  as the daemon names them (`Config::parse_unvalidated`), so their status keeps showing.
- Only the shown scope is polled, every 3 s while the window is visible; rows are rebuilt only
  when the config text or the set of daemons changes, and updated in place otherwise.
- **Sync detail page:** preference groups following `config.example.toml`: Identity (id,
  label), Mapping (remote, `remote_fs` or `git_repo`; local, remote_path), What (include,
  exclude, scope, deletions), When (trigger, interval, debounce, pull_interval), Other
  (empty_dirs, max_file_size, commit_msg_command, profiles). A `git_repo` group hides the
  file-level settings it rejects (remote_path, include, scope, deletions, empty_dirs,
  max_file_size) and shows commit_msg_command; the add dialog asks for the kind first.
- **Remotes page:** rows with a backend icon, id/label and URL; the detail page shows only the
  fields of the backend (auth for webdav and git over HTTPS, SSH key for git over SSH, branch and
  commit_msg_command for git, pull_interval / max_file_size overrides). `remote_fs` shortcuts are
  listed as other targets, opening their sync; "Convert to remote" is left for later. The kind
  of an existing sync (remote, `remote_fs`, `git_repo`) is shown, not changed: that means a new
  sync.
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
- Every write (`conflux-gtk/src/editor.rs`, one at a time per scope): read the file and refuse
  if it is no longer the text the app last read; apply the edit; validate with
  `Config::from_toml_str` (the daemon's own checks); write atomically (temp file + rename,
  keeping owner and mode); then `Reload` every running daemon of that scope. An edit is refused
  only when it would turn an accepted config into a refused one, so a config that is already
  refused can be fixed step by step from the app.
- Instant apply (GNOME HIG): combo rows and lists save at once, entry rows on their apply
  button or Enter. An empty entry or an "Inherit" choice removes the key. Durations and sizes
  are checked before writing, with a message saying how to write them; any refused edit
  reverts its row and shows a toast.
- A `gio::FileMonitor` makes the app look again at once on any change, besides polling. When
  the file changes outside the app, open pages show a banner ("this page is out of date") and
  refuse further edits, since the positions of syncs and remotes may have moved; removing an
  entry from the app does the same for pages built before.
- A missing user config can be started from the app: it writes `config.example.toml` (every
  option, commented out at its default).
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

- **Discovery:** profiles named in the config's syncs plus `default`, and sockets in the runtime
  dir; systemd (D-Bus `ListUnitsByNames`, `GetUnitFileState`) tells each one's unit state and
  whether `conflux@.service` is installed. A daemon started by hand shows as "running, started
  outside systemd" and gets no start/stop: starting its unit would take over its socket.
- **Rows:** profile and unit name, state (running, inactive, failed), a start/stop button, an
  enable switch ("Start at login" / "Start at boot"), and a menu with Restart, View log,
  Remove. Reloading after a config edit already happens on every write, so no Reload item.
- **Detail page:** unit, socket, state dir, the config path the daemon reads (from `Info`), and
  every sync group with a switch for its membership in this profile.
- **Add instance:** profile name checked by `paths::validate_profile` (as the CLI does); a
  checklist of syncs to include, written to their `profiles` (a sync without `profiles` is
  implicitly in `default`, so adding `desktop` writes `["default", "desktop"]`); "Start now"
  and "Start at login/boot". With no sync chosen it must at least start at login, or nothing
  would keep it.
- **Remove instance:** stop and disable the unit; optionally drop the profile from the syncs
  (a list left as `["default"]` is removed, an empty one stays: that sync then never runs) and
  delete the instance's state dir. The `default` daemon cannot be removed.
- **User scope:** "Keep running after logout" sets lingering through logind (allowed for oneself
  by default); its state is read from `/var/lib/systemd/linger/<user>`.
- **Sync detail:** "Profiles" is a switch per known profile (those with a daemon, or used by
  another sync) plus an entry for a new one; only `default` writes no key.
- **View log:** `journalctl [--user] -u <unit> -f` in a dialog (the system journal needs the
  `systemd-journal` group or the helper).
- Without the template unit installed, Add and Enable are hidden with a note; discovery by
  socket still works.

### Privileged helper (`crates/conflux-helper`)

Depends only on `conflux-core`; GTK never runs as root. The protocol is
`conflux_core::helper` (`HelperRequest` / `HelperResponse`, one JSON line each way, answered in
order). Fixed commands:

- `WriteConfig { expected, text, reload }`: refused when the file no longer reads `expected`
  (`None`: no file yet) or when it would break a config the daemon accepts
  (`config_edit::check_replacement`, shared with the GUI); written atomically to
  `/etc/conflux/config.toml` (environment overrides ignored), then the daemons of `reload` (by
  profile) reload it. The GUI makes the edit itself (`editor::prepare`) and sends the text.
- `Unit { action, profile }`: start, stop, restart, enable, disable, through `systemctl`.
- `Ipc { profile, request }`: any request to the system socket, as root.
- `RemoveState { profile }`: only `/var/lib/conflux/<valid-profile>`, never the base dir.
- No journal command after all: the system journal is readable by the admin groups (`wheel`,
  `adm`, `systemd-journal`) on the usual distributions, so View Log reads it directly, and a
  live stream would not fit the one-answer-per-request protocol.
- It exits on stdin EOF: on Lock, or when the window closes or the GUI goes away. There is no
  idle timeout: unlocked stays unlocked while the window is open. If it dies otherwise, the
  GUI notices and locks again ("Locked again").
- Started as `pkexec <helper>`: the helper next to the app when there is one (a build from
  source; pkexec's generic action), else `/usr/lib/conflux/conflux-helper`, which the polkit
  policy `data/dev.aperte.conflux.policy` (`dev.aperte.conflux.manage`, `auth_admin_keep`)
  names. Testing without root: `CONFLUX_HELPER_COMMAND` replaces the command, and
  `CONFLUX_TEST_SYSTEM_ROOT=<dir>` gives the GUI and the helper (only when not root) a scratch
  "system" under `<dir>`; the helper then refuses units.
- In the system scope paths are shown and written whole: `~` there would be the system daemon's
  home, not the user's.

### Workspace layout

```
crates/conflux-core     + ipc::client, config_edit, Info, GroupStatus fields, profile check
crates/conflux          uses the core client; peer-credential socket checks
crates/conflux-helper   new, privileged, minimal
crates/conflux-gtk      new
data/                   .desktop, metainfo, icons, polkit policy, gresource
```

### Steps

1. [x] **Core prep** (no UI): move the IPC client; `GroupStatus` fields and `Info`;
   `config_edit` with tests; shared profile-name check; system socket `0666` with
   peer-credential checks.
2. [x] **Read-only GUI:** window, header, lists, live status, instance discovery and states,
   Sync now and Dry run (in both scopes; a system daemon refuses them unless the user may act
   on it), read-only detail pages of syncs and remotes showing where each value comes from.
3. [x] **Editing (user scope):** editable detail pages, Add Sync / Add Remote dialogs, Delete,
   Preferences (`[daemon]`), file monitor, validation, automatic reload, Create config. The
   System scope shows the same pages locked until step 5. Profiles are edited as a plain list
   until step 4 makes them a choice of instances.
4. [x] **Instance management (user scope):** add and remove, start/stop and enable, profile
   membership (from the daemon's page and the sync's), lingering, log viewer. Controlling
   units goes through the user's own systemd manager, so it needs no privileges.
5. [x] **System unlock:** helper and polkit policy, the lock banner (Unlock... / Lock) with
   every editing control bound to it, privileged writes, unit control and daemon requests,
   state removal, locking again when the window closes. The journal is read directly (see the helper).
6. [x] **Packaging, Arch and tarballs:** the app's data (icon, desktop file, metainfo, polkit
   policy) and its tarball; the root `Makefile` (`install-cli`, `install-gtk`,
   `install-flatpak`) every package is made from, in place of an `install.sh --gtk`. The
   icon, a PNG of it and its symbolic are in `data/icons/` (generated by
   `scripts/app_icon.py`: three meshed metal gears in the well of a metal plate, a sync sign
   stamped into the big one, a cloud and a folder on the small ones); the app also embeds
   them, for About in a build from source. The in-repo `PKGBUILD` is gone: the AUR packages
   are kept on the AUR, and the Release workflow moves `conflux` and `conflux-bin` to each
   release (`scripts/aur.sh`).
7. [x] **Debian and Ubuntu:** `.deb` packages of both, see [Packages](#packages).
8. [x] **Fedora COPR:** an RPM spec with both packages; CI builds it as COPR does. Left to do
   outside the repo: the COPR project, building from `.copr/Makefile` on each release tag.
9. [x] **Flatpak:** the app alone, user scope, see [Packages](#packages).
10. [ ] **Store listing:** screenshots in `docs/screenshots/` and in the metainfo (taken from
    the release tag, as Todav's are), and the AUR's `conflux-gtk`, `conflux-gtk-bin` and
    `conflux-gtk-git` packages.

## Packages

Two packages from the one source tree, in every format, released together with one version:

- **`conflux`**: the daemon and CLI, `config.example.toml` (as `/etc/conflux/config.toml`), the
  systemd units and `sysusers.d/conflux.conf`. No GTK, so servers and headless hosts install
  only this.
- **`conflux-gtk`**: the app, `conflux-helper` (only the app uses it) in
  `/usr/lib/conflux/`, the polkit policy, the `.desktop` file, the metainfo and the icons.
  Depends on `conflux` of the same version and on GTK 4 and libadwaita 1.7 or newer.

Shared groundwork:

- The Release workflow cuts both at once. `scripts/release.py` also sets the version in the
  RPM spec (with a changelog entry) and writes the release notes into the metainfo's
  `<releases>` (as in Todav); the `.deb` packages take the version from the tarballs' names.
- The `conflux` service user comes from `sysusers.d/conflux.conf`, which the `.deb` creates in
  its postinst and Fedora's RPM by itself. `install.sh` keeps `useradd` for installs from
  source.
- CI builds each package on every push and installs the `.deb` packages in a clean
  container, so a missing dependency fails the build; the RPMs are built as COPR builds them;
  the Release workflow attaches all but the RPMs to the GitHub release.

Per format:

- **Tarballs** (step 6): the CLI's per architecture, static builds (`conflux-vX-<arch>-linux`,
  one top-level directory, as the `conflux-bin` AUR package expects); the app's built on the
  oldest supported GNOME (Fedora 43), so its glibc runs on newer distributions, as files to
  unpack into `/`.
- **Debian and Ubuntu** (step 7): `scripts/deb.sh` makes a `.deb` from a staged install tree
  (as in Todav), `dpkg-shlibdeps` fills in the library dependencies. `conflux` for amd64 and
  arm64 from the CLI tarballs, each on its own architecture's runner; `conflux-gtk` for amd64
  from the app's tarball. Built on Ubuntu 26.04, the first LTS with libadwaita 1.7. Attached
  to GitHub releases; an apt repository can come later.
- **Fedora COPR** (step 8): `packaging/rpm/conflux.spec` with `conflux-gtk` as a subpackage,
  systemd scriptlets for the system and user units, built from `.copr/Makefile` (the crates
  vendored next to the sources). Its `License:` lists the linked crates' licenses; check it
  again when the dependencies change.
- **Flatpak** (step 9), `dev.aperte.Conflux` (`packaging/flatpak/`): the app only; the daemon
  comes from the host's `conflux` package. User scope only: a sandbox can neither run
  `pkexec` nor reach `/run/conflux`, so the System toggle is hidden there, and with no
  `journalctl` in the runtime View Log names the command instead. Permissions:
  `--filesystem=home` (the folders to sync, the config, picked folders keeping their real
  paths), `--filesystem=xdg-run/conflux:create` (the user daemons' sockets, now in a
  directory of their own: `$XDG_RUNTIME_DIR/conflux/`; the CLI and the app still try the old
  path for a daemon started before), `--talk-name=org.freedesktop.systemd1` (the user's
  units) and `--system-talk-name=org.freedesktop.login1` (lingering, which the app now reads
  from logind rather than `/var/lib/systemd/linger`). The app reads the host's config and
  state dirs (`HOST_XDG_*`), not the sandbox's. The bundle is built in CI and attached to
  releases; whether to submit it to Flathub, which asks for apps that work on their own, is
  decided later.

## Later

- Field-level validation errors; an include/exclude preview of the local tree.
- Keyring helper for `password_command` (the system daemon cannot reach a user keyring).
- Event stream from the daemon instead of status polling; desktop notifications on sync errors.
- Translations (gettext).
