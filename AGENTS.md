# Agent notes

## Keep the docs current

Update these in the same commit as the change that affects them:

- `docs/ROADMAP.md` holds the plans: decisions, steps in progress, later ideas. Read it before
  working on a feature. Tick a step when it is done, record a decision when one is made, and note
  why a plan was dropped.
- `README.md` gives an overview: what conflux offers in a few lines, install, getting started.
  `docs/FEATURES.md` describes in detail what users can do today: features, examples, every
  option, commands, the app. Update them with any user-visible change (a new option, command,
  backend, install path or behaviour), along with `config.example.toml` for config options.
- `docs/CHANGELOG.md`: add user-visible changes under `## [Unreleased]`. It follows
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/): `Added`, `Changed`, `Deprecated`,
  `Removed`, `Fixed`, `Security`; version links at the end.

## Releases

- Releases are cut by the manual Release workflow (`.github/workflows/release.yml`, run with the
  version, e.g. `gh workflow run release.yml -f version=1.2.0`). `scripts/release.py` sets the
  version in `Cargo.toml`, `Cargo.lock` and the RPM spec (with a changelog entry), moves the
  `[Unreleased]` changes and link under it, and adds the release with those notes to the
  app's metainfo. CI then checks and builds the packages with that version; if that passes,
  the workflow commits `chore(release): X`, tags `vX`, and publishes the GitHub release with
  the changelog section as notes, the packages and `SHA256SUMS`. Don't bump versions or edit
  the release sections by hand.
- Two packages, in every format: `conflux` (daemon and CLI) and `conflux-gtk` (the app and its
  helper). Each is installed into a tree by the root `Makefile` (`install-cli`, `install-gtk`;
  `install-flatpak` for the app alone), and every package is made from those trees: the
  tarballs, the `.deb` packages (`scripts/deb.sh`), the RPMs (`packaging/rpm/conflux.spec`,
  built on COPR from `.copr/Makefile`) and the Flatpak (`packaging/flatpak/`). A new installed
  file goes into the `Makefile`, the spec's `%files` and, if it needs care, `scripts/deb.sh`.
- The AUR packages (`conflux`, `conflux-bin`, `conflux-git`, and the app's) live on the AUR,
  not here. The CLI tarball's name and layout (`conflux-vX-<arch>-linux.tar.gz`, one top-level
  directory with `usr/` and `etc/`) are what `conflux-bin` uses: keep them stable.
- The metainfo (`data/dev.aperte.Conflux.metainfo.xml`) must pass `appstreamcli validate`, and
  the desktop file `desktop-file-validate`; CI checks both.
- The spec's `License:` lists the licenses of the crates linked into the binaries; check it
  again when the dependencies change.

## CI

- `.github/workflows/ci.yml` runs on every push and pull request (and is called by the Release
  workflow): `cargo fmt --check`, clippy with warnings as errors and the tests of the whole
  workspace (in a Fedora container, for the GTK app's libraries), and the packages: the CLI
  tarballs for x86_64 and aarch64 (static builds with `cross` and vendored OpenSSL), the app's
  tarball (built on Fedora 43, the oldest supported GNOME), the `.deb` packages on Ubuntu
  26.04 (installed there to check their dependencies), the Flatpak bundle, and the RPMs as
  COPR builds them.
- Run the checks before committing:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  ```

## GTK app (`crates/conflux-gtk`)

- Optional: the workspace's `default-members` leave it out, so `cargo build` / `cargo test`
  build the daemon and CLI only; `-p conflux-gtk` or `--workspace` include it. It needs GTK 4,
  libadwaita 1.7+, `blueprint-compiler` and Rust 1.92 (the other crates stay at 1.85).
- Static views are Blueprint files in `ui/`, compiled by `build.rs` and loaded with
  `ui::builder!`; rows are built in Rust. Text that only depends on the config lives in the
  GTK-free `model.rs` / `text.rs`, with unit tests.
- The config file stays the source of truth: never write it whole. GUI edits go through
  `ScopeView::edit` (one at a time, refused if the file changed since it was read, checked like
  the daemon checks it, then reloaded by the running daemons), which edits with
  `conflux_core::config_edit`. Editable rows come from `form.rs`.

## Testing the GTK app

- Use a scratch setup, never the user's own config or daemons: a temp dir with a
  `config.toml`, then `CONFLUX_CONFIG=<dir>/config.toml XDG_RUNTIME_DIR=<dir>/run
  XDG_STATE_HOME=<dir>/state` for both `target/debug/conflux daemon` and the app (put the
  temp dir under `target/`). The System scope then reads the real `/etc/conflux` read-only.
- To see and drive it, run `scripts/gui-shell.sh <scratch dir>` in the background: a headless
  nested GNOME Shell on its own bus, with the app maximized in it. Then
  `scripts/gui-drive.py click X Y | scroll X Y N | type TEXT | key NAME | shot FILE` (a
  1400x1100 screen). Stop it by the pid of its `gnome-shell --headless` (match it with
  `[w]ayland-display=conflux-gui-shell`); never `pkill gnome-shell`, which is the user's own.
- The app there stays on the host's session bus, so it sees and controls the user's real
  systemd units: never start, stop or enable `conflux*` units, or toggle lingering, while
  testing (they would run the user's real config, or change what starts at login). Test
  those through profiles that exist only in the scratch config, with their switches off.
- The system scope's unlock without root or a password prompt (which would appear on the
  user's real screen): set `CONFLUX_HELPER_COMMAND=$PWD/target/debug/conflux-helper` (no
  pkexec) and `CONFLUX_TEST_SYSTEM_ROOT=<dir>` (a scratch "system": `config.toml`, `run/`
  with a scratch daemon's socket, `state/`) when starting `scripts/gui-shell.sh`. The helper
  honours the test root only when not root, and refuses units there. Never run the real
  `pkexec` path in tests.
- `scripts/sandbox.sh [--isolated]` sets all of this up in `target/sandbox` (user and system
  configs and daemons, the helper's test mode) and runs the app on the display in
  `WAYLAND_DISPLAY`; `WAYLAND_DISPLAY=conflux-gui-shell` puts it in the nested shell.
  `--isolated` runs it in bubblewrap, with only `target/sandbox` writable, a private session
  bus and no system bus: systemd units, lingering and `pkexec` are out of its reach.
- `scripts/gui-shell.sh` starts the app as `dev.aperte.Conflux.Test` (`--gapplication-app-id`),
  so a copy the user has open stays theirs; never kill a `conflux-gtk` without that id.
- GTK's Broadway backend (`gtk4-broadwayd`, Playwright on `http://127.0.0.1:8087`) aborts as
  soon as the Syncs page shows two daemon rows (`append_node_ops` assertion), and when a
  dialog closes; use it only for quick looks at other pages.
- Stop the scratch daemon by its pid; `pkill -f "...conflux daemon"` also matches the shell
  running the command and kills it.

## Commits

- Conventional Commits, e.g. `feat(core): add config_edit module`,
  `fix(daemon): keep the socket after a failed reload`.
