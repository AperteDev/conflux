# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Conflux (`conflux-gtk`), a GTK (libadwaita) app: the syncs, remotes and daemons of the user
  and the system, live sync status, where each setting comes from,
  Sync Now and dry runs. It edits the user's config: every setting of a sync, a remote and
  `[daemon]`, adding and deleting syncs and remotes, and starting a config from the example.
  Edits change only the lines they touch, keep comments, are checked the way the daemon
  checks them, and are reloaded by the running daemons. It manages the user's daemons too:
  one per profile, started, stopped and set to start at login through systemd, added and
  removed with the profiles of their syncs, kept running after logout (lingering), with
  their log from the journal. The system's config and daemons are locked until unlocked
  with an administrator's password, as in GNOME Settings; then the same pages edit them
  through `conflux-helper`, a small program run as root with `pkexec` that only writes the
  system config, acts on the system daemons' units, passes them requests and deletes their
  state. It quits, locking the app again, on Lock or when the window closes.
- Daemons answer an `Info` request (version, profile, config file, process id), and report
  each group's `id` and whether it is syncing right now in their status.
- Packages for both: `.deb` packages for Ubuntu 26.04 and newer (`conflux` for amd64 and
  arm64, `conflux-gtk` for amd64), a tarball of the app next to the CLI's, a Flatpak bundle of
  the app (your own config and daemons only), and an RPM spec for Fedora with `conflux-gtk`
  as a subpackage, built on COPR. A `Makefile` installs either package's files.
- The system daemon's `conflux` user comes with the packages, from
  `/usr/lib/sysusers.d/conflux.conf`.

### Changed

- A user daemon's control socket moved into a directory of its own,
  `$XDG_RUNTIME_DIR/conflux/`, which the Flatpak app can be given. The CLI and the app still
  reach a daemon started before the update on the old path.
- A system daemon's control socket is open to every local user, who can read its status.
  Syncing, dry runs and reloading still need root, the `conflux` service user, or membership
  in the `conflux` group.
- Profile names are limited to ASCII letters, digits, `_`, `-` and `.` (1-64 characters, not
  starting with `.` or `-`), as they become unit, socket and directory names.
- Release assets come with one `SHA256SUMS` file instead of a `.sha256` file per tarball.

### Removed

- The Arch `PKGBUILD` in this repository: the `conflux`, `conflux-bin` and `conflux-git`
  packages are kept on the AUR.

## [1.1.0] - 2026-07-31

### Added

- **`git_repo` syncs** — point a `[[sync]]` at a git working clone you already
  have and conflux keeps it up to date through git itself: stage and commit local
  changes, fetch, fast-forward or rebase, push. No `[[remote]]` is involved and no
  second copy of the tree exists; `.gitignore` decides what is tracked, history is
  never rewritten, and a genuine conflict aborts the rebase and is reported
  instead of resolved by timestamp. A pull-only refresh fetches and integrates
  without committing work in progress.
- **`commit_msg_command` on a sync** — the `[[remote]]` option of the same name,
  now available per `git_repo` group; defaults to `conflux sync from <hostname>`.
- **`conflux sync --dry-run` (`-n`)** — print the changes a sync would make
  without making them, one line per path with a direction marker (`↑` upload,
  `↓` download, `✗` delete, `!` conflict, `·` skipped as oversize).
- **Git-over-SSH key selection** — new `identity_file` and
  `identity_file_command` on a `[[remote]]` for a deploy key held on disk or
  fetched from a secret store. conflux now reads `~/.ssh/config` `IdentityFile`
  entries itself and tries, in order: the configured key, the host's
  `~/.ssh/config` identities, ssh-agent, then the default key locations. A
  `git_repo` clone needs no configuration at all — it authenticates the way your
  own `git` would, including the repository's `credential.helper` over HTTPS.
- **Validation warnings for git syncs** — `conflux config validate` now flags a
  `git_repo` that is missing or is not a clone, and a git sync whose
  `pull_interval` is disabled (it would never see another host's commits).

### Changed

- **`trigger` now defaults to `watch`** instead of being required, so
  "just keep it in sync" is the default for a new group.
- **`pull_interval` defaults to 5 minutes for anything git** — a `git` remote or a
  `git_repo` clone, neither of which can be watched from the remote side. Every
  explicit value still wins, at every level. Other backends are unchanged (off
  unless set).
- **Preconditions are no longer retried** — a `git_repo` that is not a clone, a
  detached HEAD, a rebase left in progress, or a conflict awaiting resolution is
  reported once rather than driven through the retry backoff, since only the user
  can clear it.
- File-level settings (`remote_path`, `include`, `scope`, `deletions`,
  `empty_dirs`, `max_file_size`) are rejected rather than silently ignored on a
  `git_repo` group, as is `commit_msg_command` on a file-level sync.
- Arch `PKGBUILD` builds with `options=('!lto')`, and the release workflow moved
  to current action versions.

### Fixed

- A watched `git_repo` group no longer re-triggers itself forever: writes inside
  `.git` (which conflux's own commit produces) no longer count as local activity.

## [1.0.0] - 2026-07-11

First public release. A single `conflux` binary runs both the background
sync daemon and the CLI that controls it.

### Added

- **Multiple backends** — sync against WebDAV (Nextcloud & friends), Git (over
  HTTPS or SSH), or a `filesystem` directory (backup disk, testing).
- **`remote_fs` shortcut** — point a `[[sync]]` at a filesystem path directly,
  without declaring a `[[remote]]`; conflux synthesizes a `filesystem` remote
  for it. Supports `trigger = "watch-both"`.
- **Flexible triggers** — `watch` (debounced local edits), `timer` (fixed
  interval), `watch-both` (also watches a filesystem remote), and `manual`.
- **Scoped syncing** — `include` globs pick what syncs two-way, and `scope`
  (`include` / `remote` / `local` / `mirror`) governs everything else, so
  pointing a remote at `$HOME` can't clobber untracked files.
- **Safe by default** — deletions denied unless opted in, a configurable
  `max_file_size`, and diverging edits kept as local conflict copies rather
  than overwritten.
- **Periodic remote pulls** — a `pull_interval` catches changes made on other
  hosts even for `watch`-triggered groups.
- **Profiles** — one shared config drives many hosts; each `--profile` gets its
  own state dir and control socket and runs only its own sync groups.
- **systemd integration** — plain and profile-templated user and system units,
  with content-addressed (BLAKE3) change detection under the hood.
- **CLI** — `status`, `sync`, `reload`, and `config {validate,show,paths}`,
  talking to the daemon over its control socket.
- **Packaging** — Arch Linux `PKGBUILD` and a source install script
  (`install.sh`) for user and system installs.

[Unreleased]: https://github.com/AperteDev/conflux/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/AperteDev/conflux/releases/tag/v1.1.0
[1.0.0]: https://github.com/AperteDev/conflux/releases/tag/v1.0.0
