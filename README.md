<p align="center">
  <img src="data/icons/hicolor/scalable/apps/dev.aperte.Conflux.svg" width="128" alt="Conflux icon">
</p>

<h1 align="center">Conflux</h1>

<p align="center">File sync for Linux: a background daemon, a CLI to control it and a GNOME app to set it up.</p>

<p align="center">
  <a href="https://github.com/AperteDev/conflux/actions/workflows/ci.yml"><img src="https://github.com/AperteDev/conflux/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

conflux keeps local directories in step with a **WebDAV** server (Nextcloud and
friends), a **Git** repository, or another **directory** on disk, or keeps a **git
clone you already have** up to date by committing, fetching, rebasing and pushing
it for you. You choose when each group syncs and how much of the tree it owns,
from a read-only pull of tracked dotfiles to a full 1:1 mirror. One `conflux`
binary is both the daemon and the CLI that controls it; **Conflux**, the optional
GNOME app, shows and edits the same setup.

conflux is developed in my free time. If it keeps your files in order, you can
support its development:

[![Support me on Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/N5E229GJPZ)
[![Sponsor on GitHub](https://img.shields.io/badge/Sponsor-EA4AAA?style=for-the-badge&logo=githubsponsors&logoColor=white)](https://github.com/sponsors/SelfRef)
[![Donate using Liberapay](https://liberapay.com/assets/widgets/donate.svg)](https://liberapay.com/SelfRef/donate)

## Features

- Sync folders with a WebDAV server (Nextcloud and others), a Git repository or
  another directory, such as a backup disk.
- Keep a git clone you already have up to date: conflux commits, fetches, rebases
  and pushes it for you.
- Sync on local edits, on a timer or by hand, and pull changes from your other
  machines on an interval.
- Choose how much of a tree each sync owns: from a read-only pull of a few
  dotfiles to a full two-way mirror.
- Safe by default: nothing is deleted unless you allow it, a file changed on both
  sides keeps both versions, and a dry run shows what a sync would do.
- One config for several machines: profiles decide which syncs each one runs.
- Runs as a systemd service, for your user or the whole system, controlled with
  the `conflux` CLI.
- The Conflux app shows and edits the syncs, remotes and daemons of your user and
  of the system.

All of what conflux does, with examples and every option, is described in
[FEATURES.md](docs/FEATURES.md). Once installed, `man conflux` describes the
commands and `man 5 conflux` the config file.

## Install

conflux comes as two packages: **`conflux`**, the daemon and CLI, and
**`conflux-gtk`**, the optional app (it needs `conflux` and libadwaita 1.7 or
newer; its prebuilt packages, GNOME 49 or newer).

**Arch Linux and derivatives:** install from the
[AUR](https://aur.archlinux.org/packages?K=conflux) with an AUR helper:

```sh
yay -S conflux conflux-gtk
```

These build the latest release from source; `conflux-bin` and `conflux-gtk-bin`
install the prebuilt release instead, and `conflux-git` and `conflux-gtk-git`
build the latest commit.

**Other distributions:** download the packages from the
[latest release](https://github.com/AperteDev/conflux/releases/latest):

```sh
# Ubuntu 26.04 and newer (conflux also for arm64)
sudo apt install ./conflux_<version>_amd64.deb ./conflux-gtk_<version>_amd64.deb

# Any distribution: the daemon and CLI (x86_64 or aarch64), and the app (x86_64),
# as files to unpack into /
sudo tar -xzf conflux-v<version>-x86_64-linux.tar.gz --strip-components=1 -C /
sudo tar -xzf conflux-gtk-<version>-x86_64.tar.gz -C /
sudo systemd-sysusers conflux.conf   # the system daemon's user

# Flatpak: the app alone, for your own syncs and daemons (the daemon still comes
# from the conflux package or the tarball)
flatpak install --user dev.aperte.Conflux.flatpak
```

Fedora RPMs are built from [`packaging/rpm/conflux.spec`](packaging/rpm/conflux.spec)
(see [Building packages](#building-packages)).

### From source

`./install.sh install` builds and installs the daemon and CLI into your home
directory: the binary in `~/.local/bin`, its man pages in `~/.local/share/man`,
`config.example.toml` as your starter config in `~/.config/conflux/`, and systemd
**user** units. Add `--no-build` to reuse an existing `target/release/conflux`.

Add `--system` (as root) to install system-wide instead — binary in
`/usr/local/bin`, man pages in `/usr/local/share/man`, config in `/etc/conflux/`,
systemd **system** units, running as a dedicated `conflux` user:

```sh
./install.sh install                 # user install
sudo ./install.sh install --system   # system-wide
```

Uninstall the same way you installed, adding `--purge` to also drop the config,
state, and system user:

```sh
./install.sh uninstall [--system] [--purge]
```

The app installs with `make`, after the daemon. It needs Rust 1.92 or newer, GTK
4, libadwaita 1.7 or newer, `blueprint-compiler` and GLib's
`glib-compile-resources`:

```sh
make build-gtk PREFIX=/usr/local && sudo make install-gtk PREFIX=/usr/local
```

Then [configure and run it](#getting-started).

### Building packages

The root [`Makefile`](Makefile) installs each package's files into a tree
(`install-cli`, `install-gtk`, `install-flatpak`), and every package is made from
it: [`scripts/deb.sh`](scripts/deb.sh) makes the `.deb` packages,
[`packaging/flatpak/`](packaging/flatpak/) holds the Flatpak manifest, and
[`packaging/rpm/conflux.spec`](packaging/rpm/conflux.spec) builds `conflux` and
`conflux-gtk` as RPMs; COPR builds them with `.copr/Makefile`, which also works
locally:

```sh
make -f .copr/Makefile srpm outdir=$PWD/target/rpm
```

Every push is checked and packaged by [CI](.github/workflows/ci.yml); releases are
made by the manual [Release](.github/workflows/release.yml) workflow and listed in
the [changelog](docs/CHANGELOG.md).

## Getting started

conflux reads one TOML file: `~/.config/conflux/config.toml` for your user,
`/etc/conflux/config.toml` for the system. Start from
[`config.example.toml`](config.example.toml), which lists every option at its
default, or from the [examples](docs/FEATURES.md#examples); or open the Conflux
app, which creates and edits it for you. Check it before starting the daemon:

```sh
conflux config validate
```

Then enable the daemon, for your user or the system, the default profile or a
named one:

```sh
systemctl --user enable --now conflux            # your user, default profile
systemctl --user enable --now conflux@desktop    # your user, profile "desktop"
sudo systemctl enable --now conflux              # the system
```

> **The daemon syncs every group once as soon as it starts**, so point a new
> group at real data only once you are sure of it.

`conflux status` shows each sync and its last run, `conflux sync` runs them now
and `conflux sync -n` shows what a sync would change; see `man conflux` or
[FEATURES.md](docs/FEATURES.md#daemon-and-cli) for the rest.

## Roadmap

Planned work, including the rest of the GTK app, is tracked in
[`docs/ROADMAP.md`](docs/ROADMAP.md). Changes in each release are listed in
[`docs/CHANGELOG.md`](docs/CHANGELOG.md).
