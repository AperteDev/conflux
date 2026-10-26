# Agent notes

## Keep the docs current

Update these in the same commit as the change that affects them:

- `docs/ROADMAP.md` holds the plans: decisions, steps in progress, later ideas. Read it before
  working on a feature. Tick a step when it is done, record a decision when one is made, and note
  why a plan was dropped.
- `README.md` describes what users can do today: features, install, configuration, commands.
  Update it with any user-visible change (a new option, command, backend, install path or
  behaviour), along with `config.example.toml` for config options.
- `docs/CHANGELOG.md`: add user-visible changes under `## [Unreleased]`. It follows
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/): `Added`, `Changed`, `Deprecated`,
  `Removed`, `Fixed`, `Security`; version links at the end.

## Releases

- Releases are cut by the manual Release workflow (`.github/workflows/release.yml`, run with the
  version, e.g. `gh workflow run release.yml -f version=1.1.0`). `scripts/release.py` sets the
  version in `Cargo.toml` and `Cargo.lock` and moves the `[Unreleased]` changes and link under
  it. CI then checks and builds the packages with that version; if that passes, the workflow
  commits `chore(release): X`, tags `vX`, and publishes the GitHub release with the changelog
  section as notes, the tarballs and `SHA256SUMS`. Don't bump versions or edit the release
  sections by hand.
- `package/arch/PKGBUILD` is the `conflux-git` package; its `pkgver` comes from `git describe`,
  so it needs no version bump. The tarball name and layout
  (`conflux-vX-<arch>-linux.tar.gz`, one top-level directory with `usr/` and `etc/`) are used by
  the `conflux-bin` AUR package: keep them stable.

## CI

- `.github/workflows/ci.yml` runs on every push and pull request (and is called by the Release
  workflow): `cargo fmt --check`, clippy with warnings as errors and the tests of the whole
  workspace (in a Fedora container, for the GTK app's libraries), and the release tarballs for
  x86_64 and aarch64 (static builds of the CLI with `cross` and vendored OpenSSL).
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
- The config file stays the source of truth: never write it whole; edits go through
  `conflux_core::config_edit`.

## Testing the GTK app

- Use a scratch setup, never the user's own config or daemons: a temp dir with a
  `config.toml`, then `CONFLUX_CONFIG=<dir>/config.toml XDG_RUNTIME_DIR=<dir>/run
  XDG_STATE_HOME=<dir>/state` for both `target/debug/conflux daemon` and the app (put the
  temp dir under `target/`). The System scope then reads the real `/etc/conflux` read-only.
- To see and drive it, run it on GTK's Broadway backend: `gtk4-broadwayd :7`, then the app with
  `GDK_BACKEND=broadway BROADWAY_DISPLAY=:7` and `XDG_CONFIG_HOME` pointing at a dir whose
  `gtk-4.0/settings.ini` sets `gtk-enable-animations=false`; open `http://127.0.0.1:8087` in
  Playwright. Click by coordinates (`page.mouse` down/up), and reload the page after each
  click: broadway.js loses track of the page when a view changes and stops drawing, while the
  app goes on fine. Some icons may not be drawn; that is Broadway.
- Stop the scratch daemon by its pid; `pkill -f "...conflux daemon"` also matches the shell
  running the command and kills it.

## Commits

- Conventional Commits, e.g. `feat(core): add config_edit module`,
  `fix(daemon): keep the socket after a failed reload`.
