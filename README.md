# conflux

A background sync daemon for Linux. It keeps local directories in step with a
**WebDAV** server (Nextcloud & friends), a **Git** repository, or another
**directory** on disk — or keeps a **git clone you already have** up to date by
committing, fetching, rebasing and pushing it for you. You choose when each group
syncs and how much of the tree it owns, from a read-only pull of tracked dotfiles
to a full 1:1 mirror. One `conflux` binary is both the daemon and the CLI that
controls it.

## Features

- **Three backends plus git clones** — `webdav`, `git`, `filesystem`; or point
  `git_repo` at a working clone and conflux drives it through git itself, with no
  `[[remote]]`, no second copy of the tree, and history preserved.
- **Triggers per group** — `watch` (react to local edits, debounced), `timer`,
  `watch-both` (also watch a local directory remote), or `manual`. Any group can
  additionally poll the remote on a `pull_interval`, which is how changes from
  your other machines arrive.
- **Scoped syncing** — `include` globs decide what syncs two-way; `scope`
  (`include` / `remote` / `local` / `mirror`) decides what happens to everything
  else, so pointing a remote at `$HOME` cannot clobber untracked files.
- **Safe by default** — deletions are denied unless opted in, a max file size is
  enforced, and diverging edits are kept as `*.conflux-conflict-*` copies rather
  than overwritten. `conflux sync -n` previews any run.
- **Profiles** — one shared config can drive many hosts; each `--profile` gets its
  own state dir and control socket and runs only its own groups.
- **Made for systemd** — plain and profile-templated user and system units, with
  content-addressed (BLAKE3) change detection under the hood.

## Install

### Arch Linux

Three AUR packages — pick one:

| Package | Source |
| --- | --- |
| `conflux` | stable release, built from source |
| `conflux-git` | latest `master`, built from source |
| `conflux-bin` | stable release, prebuilt binary from GitHub Releases |

```sh
yay -S conflux        # or conflux-git / conflux-bin
```

Without an AUR helper, clone `https://aur.archlinux.org/conflux.git` and run
`makepkg -si`. (The `conflux-git` `PKGBUILD` also lives in this repo under
[`package/arch/`](package/arch/).)

### From source

`./install.sh install` builds and installs into your home directory: the binary in
`~/.local/bin`, `config.example.toml` as your starter config in
`~/.config/conflux/`, and systemd **user** units. Add `--no-build` to reuse an
existing `target/release/conflux`.

Add `--system` (as root) to install system-wide instead — binary in
`/usr/local/bin`, config in `/etc/conflux/`, systemd **system** units, running as a
dedicated `conflux` user:

```sh
./install.sh install                 # user install
sudo ./install.sh install --system   # system-wide
```

Uninstall the same way you installed, adding `--purge` to also drop the config,
state, and system user:

```sh
./install.sh uninstall [--system] [--purge]
```

Then [enable the service](#as-a-service-systemd).

## Configure

conflux reads one TOML file, resolved automatically — `$XDG_CONFIG_HOME/conflux/config.toml`
(usually `~/.config/conflux/config.toml`) in user mode, `/etc/conflux/config.toml`
in system mode. `$CONFLUX_CONFIG` or `--config <path>` overrides it, and
`conflux config paths` prints the resolved config, state, and socket locations.

[`config.example.toml`](config.example.toml) documents every option at its default
value; the tables in [Option reference](#option-reference) below are the summary.
Check your config before enabling the daemon:

```sh
conflux config validate   # errors, plus warnings for configs that parse but look wrong
conflux config show       # the resolved configuration, defaults filled in
```

### Examples

A `[[sync]]` group is one local directory and the rules for keeping it in step.
Most groups also need a `[[remote]]` naming what to sync against; `git_repo` and
`remote_fs` are the two that don't. A config can mix any number of either.

| To… | Write | Needs a `[[remote]]` |
| --- | --- | --- |
| [pull dotfiles from a git repo, pushing a few back](#dotfiles-from-a-git-repo-read-only-into-home) | `backend = "git"` + `scope`/`include` | yes |
| [mirror a folder to Nextcloud or another WebDAV server](#a-folder-mirrored-to-nextcloud-webdav) | `backend = "webdav"` + `scope = "mirror"` | yes |
| [mirror a folder to a backup disk](#a-folder-mirrored-to-a-backup-disk) | `backend = "filesystem"` | yes |
| […the same, spelled shorter](#a-folder-mirrored-to-a-backup-disk) | `remote_fs = "/mnt/backup"` | no |
| [keep a git clone you already have up to date](#keeping-a-git-clone-in-sync) | `git_repo = "~/notes"` | no |

#### Dotfiles from a git repo, read-only into `$HOME`

Here conflux compares the local tree against the remote itself and copies
individual files, with `scope` and `include` deciding what moves and which way:

```toml
[[remote]]
id = "dotfiles"
backend = "git"
url = "https://github.com/me/dotfiles.git"
branch = "main"
username = "me"
password_command = "secret-tool lookup service conflux"   # beats a plaintext password

[[sync]]
remote = "dotfiles"
local = "~"
# Pull every tracked file down read-only; only the `include` paths are pushed
# back, so untracked files in $HOME are never touched.
scope = "remote"
include = [".bashrc", ".config/nvim", ".config/fish"]
trigger = "timer"
interval = "15m"
```

`local` is a plain directory here, **not** a clone: conflux keeps its own private
clone under the state dir and writes the files into `$HOME`. To have an existing
`~/dotfiles` clone itself kept up to date, use
[`git_repo`](#keeping-a-git-clone-in-sync) instead.

#### A folder mirrored to Nextcloud (WebDAV)

```toml
[[remote]]
id = "nextcloud"
backend = "webdav"
url = "https://cloud.example.com/remote.php/dav/files/me/"
username = "me"
password_command = "secret-tool lookup service conflux"

[[sync]]
remote = "nextcloud"
local = "~/Documents"
remote_path = "documents"
# Sync the whole tree both ways, ignoring `include`, and propagate removals so the
# two copies stay identical. This opts out of the safe default, so point a mirror
# only at a directory you want kept in lock-step — never at $HOME.
scope = "mirror"
deletions = "allow"
trigger = "watch"
```

#### A folder mirrored to a backup disk

The same mirror against a local directory. `watch-both` reacts to edits on either
side; only a `filesystem` remote exposes a directory that can be watched like
that:

```toml
[[remote]]
id = "backup"
backend = "filesystem"
url = "/mnt/backup/conflux"

[[sync]]
remote = "backup"
local = "~/Documents"
remote_path = "documents"
scope = "mirror"
deletions = "allow"
trigger = "watch-both"
```

For a one-off target the `[[remote]]` adds nothing the path doesn't already say,
so `remote_fs` replaces it — conflux synthesizes the `filesystem` remote itself.
This behaves identically to the group above; prefer the explicit form when several
groups share one target or to set remote-level `pull_interval`/`max_file_size`:

```toml
[[sync]]
remote_fs = "/mnt/backup/conflux"
local = "~/Documents"
remote_path = "documents"
scope = "mirror"
deletions = "allow"
trigger = "watch-both"
```

#### Keeping a git clone in sync

Point `git_repo` at a clone you already have — `~/notes`, `~/dotfiles`, anything
with a `.git` of its own — and conflux drives that repository through git itself:
on every run it commits the working tree if it changed, fetches, fast-forwards or
**rebases** your commits onto upstream, then pushes if the branch is ahead.

```toml
[[sync]]
git_repo = "~/notes"
# Everything else is optional; these are the defaults:
# trigger = "watch"      # react to local edits; or "timer" (with `interval`), or "manual"
# debounce = "5s"        # settle time before a watch-triggered sync
# pull_interval = "5m"   # how often to fetch, since a git remote cannot be watched
# commit_msg_command = "printf 'notes: %s' \"$(date -I)\""   # default: "conflux sync from <hostname>"
```

There is no `[[remote]]`: the current branch, its upstream, and its credentials
all come from the clone (see [Git authentication](#git-authentication)). Nothing
is reconciled file by file either — `.gitignore` decides what is tracked, commits
are authored with the repository's own `user.name`/`user.email`, and the group is
named after its directory, so `conflux status` lists it as `notes` and
`conflux sync notes` runs it.

Worth knowing before you enable it:

- **Your work in progress gets committed.** That is the point for a notes or
  dotfiles repo, and rarely what you want for a project you are developing. The
  daemon syncs every group once at startup, so it happens as soon as it starts.
- **Conflicts come back to you.** If your commits and upstream touch the same
  lines, conflux aborts the rebase and reports it: your work stays committed
  locally, the repository is left clean, and nothing is pushed. Resolve it with
  `git pull --rebase` and the next sync carries on. Beyond that rebase of your own
  unpushed commits, history is never rewritten and nothing is ever hard-reset.
- **It refuses rather than guesses** when `git_repo` is not a clone (or is
  missing), the branch has no upstream, HEAD is detached, or a merge, rebase, or
  cherry-pick is in progress. Each says what to do about it — in
  `conflux config validate` up front, otherwise once per attempt in the log and in
  `conflux status`. These are not retried with backoff, since nothing will change
  until you act.
- **File-level settings do not apply** and are rejected by `config validate`:
  `remote_path`, `include`, `scope`, `deletions`, `empty_dirs`, `max_file_size`.
  Submodules are not touched.

#### Git authentication

For a **`git_repo`** clone there is nothing to configure: conflux authenticates the
way your own `git` would — the `IdentityFile`(s) for that host in `~/.ssh/config`,
then `ssh-agent`, then the default keys; and for HTTPS, the `credential.helper`
configured for the repository.

For a **`git` `[[remote]]`** over SSH (`ssh://git@host/…` or `git@host:…`) conflux
tries, in order: `identity_file`/`identity_file_command` from the config, the
`IdentityFile`(s) for the host in `~/.ssh/config`, `ssh-agent`, then the default
keys (`~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa`, …). So on an interactive machine a
`~/.ssh/config` entry is enough:

```
Host git.example.com
    IdentityFile ~/.ssh/my_key
```

For a headless daemon or a deploy key, point the remote straight at one:

```toml
[[remote]]
id = "test"
backend = "git"
url = "ssh://git@git.example.com:222/me/test.git"
identity_file = "~/.ssh/deploy_key"
# …or fetch the raw key material from a secret store, like password_command:
# identity_file_command = "cat /run/secrets/deploy_key"
```

Over HTTPS a `git` remote uses `username` with `password`/`password_command`, like
a WebDAV remote. conflux reads `IdentityFile` itself, so the more exotic
`~/.ssh/config` directives — `ProxyJump`, `Hostname`/`Port` rewriting, `Include` —
are not applied.

### Option reference

`[daemon]` holds `log_level`, the active `profile`, and the defaults every group
inherits: `debounce`, `interval`, `pull_interval`, `empty_dirs`, `deletions`,
`max_file_size`, and `exclude` (which defaults to `.git`, `.svn`, `.hg`,
`.DS_Store`, `Thumbs.db`, `*.swp` and is *added to* each group's own `exclude`).

`[[remote]]` — one block per remote, referenced by a group's `remote`:

| Option | Notes |
| --- | --- |
| `id`, `backend`, `url` | required; `backend` is `webdav` / `git` / `filesystem`, `url` is a base URL, repo URL, or directory path |
| `username`, `password`, `password_command` | auth for WebDAV and git-over-HTTPS; prefer the command form |
| `identity_file`, `identity_file_command` | git-over-SSH key, as a path or raw key material |
| `branch` | git only; defaults to the remote's default branch |
| `commit_msg_command` | git only; default `conflux sync from <hostname>` |
| `pull_interval`, `max_file_size` | override the `[daemon]` defaults for this remote's groups |
| `label` | descriptive only |

`[[sync]]` — one block per group:

| Option | Default | Notes |
| --- | --- | --- |
| `remote` / `remote_fs` / `git_repo` | — | what to sync against; set exactly one |
| `local` | — | local root; `~` and `$VARS` expanded. Supplied by `git_repo` |
| `remote_path` | remote root | subdirectory of the remote this group maps to |
| `include` | `[]` | globs synced two-way. Empty matches nothing; `["**"]` everything |
| `scope` | `include` | what happens outside `include`: `include` ignores it, `remote` pulls it read-only, `local` pushes it, `mirror` syncs the whole tree ignoring `include` |
| `exclude` | `[]` | globs never synced, added to the `[daemon]` list |
| `deletions` | `deny` | `allow` propagates a deletion to the other side |
| `trigger` | `watch` | `watch` / `timer` / `watch-both` (needs a `filesystem` remote) / `manual` |
| `interval` | `1h` | tick for `trigger = "timer"` |
| `debounce` | `5s` | quiet period before a `watch` sync |
| `pull_interval` | off; `5m` for git | extra pull-only run, whatever the trigger; `0s` disables |
| `empty_dirs` | `ignore` | `prune` removes empty dirs, `mirror` syncs them; ignored for git |
| `max_file_size` | `100MB` | larger files are skipped; `0` is unlimited |
| `profiles` | `["default"]` | which `--profile` runs this group |
| `id`, `label` | — | `id` targets the group from the CLI |
| `commit_msg_command` | — | `git_repo` only |

### Environment variables

The daemon inherits the environment of whatever starts it — the systemd unit or
your shell — and so does any `password_command` / `commit_msg_command` you
configure (e.g. `password_command = 'printf %s "$CONFLUX_PASSWORD"'`).

| Variable | Effect |
| --- | --- |
| `CONFLUX_CONFIG` | Config file path, overriding the default location (same as `--config`). |
| `CONFLUX_LOG` | Tracing filter: `debug`, `trace`, or a directive like `info,conflux_core=debug`. Overrides `[daemon] log_level`. |
| `XDG_CONFIG_HOME` | User-mode config dir (default `~/.config`). |
| `XDG_STATE_HOME` | User-mode state dir, holding the index and git clones (default `~/.local/state`). |
| `XDG_RUNTIME_DIR` | Where the control socket lives; falls back to the state dir. |

## Run

### As a service (systemd)

Each install mode ships a plain unit and a profile-templated one, so you can
enable the implicit `default` profile or a named one:

```sh
systemctl --user enable --now conflux            # user install, default profile
systemctl --user enable --now conflux@desktop    # user install, named profile
sudo systemctl enable --now conflux              # system install
```

For debugging, run `conflux daemon` in the foreground instead, with
`CONFLUX_LOG=debug` for detail.

> **The daemon syncs every active group once at startup**, whatever its `trigger`
> — `manual` only disables *automatic* triggers after that first run. Starting or
> reloading the daemon therefore syncs for real immediately; there is no "start
> paused" mode. Vet a new group before it points at real data (see below).

### CLI

The CLI talks to the running daemon over its control socket; add `--system` for a
system daemon and `--profile <name>` to target one profile.

```sh
conflux status              # each group, its trigger, and its last run or error
conflux sync                # sync every group now
conflux sync dotfiles       # just one group, by `id` or `remote:remote_path` label
conflux sync -n             # dry run: what a sync would change, changing nothing
conflux reload              # re-read the config without restarting
conflux config validate     # check the config (also: show, paths)
```

A dry run (`-n`) prints one line per path — `↑` upload, `↓` download, `✗` delete,
`!` conflict with its newer-wins winner, `·` skip. It fetches the current remote
state to compare against, but never writes, deletes, or pushes. Conflicts are
predicted from modification times, so a file the real sync finds identical on both
sides shows as a conflict yet resolves to no change.

Because it goes through the daemon, `-n` previews the *next* sync of a running
daemon — the changes you have made since the last one. It is **not** a way to
preview a brand-new group, which the daemon already synced at startup. To vet a
new group's direction and filters, bring it up in a throwaway setup first: point
`--config` at a temp file whose group uses a scratch directory or a `filesystem`
remote, run `conflux daemon` in the foreground, edit files, and watch `-n`.
