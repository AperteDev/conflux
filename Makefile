# Install conflux and its GTK app into a tree: the packages (tarballs, .deb, RPM,
# Flatpak) are made of these. For an install from source into your home folder or
# /usr/local, install.sh does more (starter config, user, units enabled).
#
#   make build-cli install-cli DESTDIR=$PWD/target/stage-cli
#   make build-gtk install-gtk DESTDIR=$PWD/target/stage-gtk
#
# BUILD is where the binaries are (another for a cross build). The app is built
# knowing where its helper is installed, so build and install with one PREFIX.
PREFIX ?= /usr
BINDIR ?= $(PREFIX)/bin
LIBDIR ?= $(PREFIX)/lib
DATADIR ?= $(PREFIX)/share
SYSCONFDIR ?= /etc
BUILD ?= target/release
HELPER = $(LIBDIR)/conflux/conflux-helper
ID = dev.aperte.Conflux
ICONS = $(DESTDIR)$(DATADIR)/icons/hicolor

build-cli:
	cargo build --release --locked -p conflux

build-gtk:
	CONFLUX_HELPER_PATH=$(HELPER) cargo build --release --locked -p conflux-gtk -p conflux-helper

# The daemon and CLI, its units (pointed at BINDIR), the system user and the
# starter config.
install-cli:
	install -Dm755 $(BUILD)/conflux $(DESTDIR)$(BINDIR)/conflux
	for unit in conflux conflux@; do \
	  sed 's|/usr/bin/conflux|$(BINDIR)/conflux|' systemd/$$unit.user.service | \
	    install -Dm644 /dev/stdin $(DESTDIR)$(LIBDIR)/systemd/user/$$unit.service; \
	  sed 's|/usr/bin/conflux|$(BINDIR)/conflux|' systemd/$$unit.system.service | \
	    install -Dm644 /dev/stdin $(DESTDIR)$(LIBDIR)/systemd/system/$$unit.service; \
	done
	install -Dm644 systemd/conflux.sysusers $(DESTDIR)$(LIBDIR)/sysusers.d/conflux.conf
	install -Dm644 config.example.toml $(DESTDIR)$(SYSCONFDIR)/conflux/config.toml
	install -Dm644 config.example.toml $(DESTDIR)$(DATADIR)/doc/conflux/config.example.toml
	install -Dm644 LICENSE $(DESTDIR)$(DATADIR)/licenses/conflux/LICENSE

# The app, its helper and what the desktop needs of it: desktop file, metainfo,
# icons and the polkit policy naming the helper.
install-gtk:
	install -Dm755 $(BUILD)/conflux-gtk $(DESTDIR)$(BINDIR)/conflux-gtk
	install -Dm755 $(BUILD)/conflux-helper $(DESTDIR)$(HELPER)
	install -Dm644 data/$(ID).desktop $(DESTDIR)$(DATADIR)/applications/$(ID).desktop
	install -Dm644 data/$(ID).metainfo.xml $(DESTDIR)$(DATADIR)/metainfo/$(ID).metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/$(ID).svg $(ICONS)/scalable/apps/$(ID).svg
	install -Dm644 data/icons/hicolor/symbolic/apps/$(ID)-symbolic.svg \
	  $(ICONS)/symbolic/apps/$(ID)-symbolic.svg
	sed 's|/usr/lib/conflux/conflux-helper|$(HELPER)|' data/dev.aperte.conflux.policy | \
	  install -Dm644 /dev/stdin $(DESTDIR)$(DATADIR)/polkit-1/actions/dev.aperte.conflux.policy
	install -Dm644 LICENSE $(DESTDIR)$(DATADIR)/licenses/conflux-gtk/LICENSE

# The app alone, for the Flatpak: no helper, as the sandbox cannot run it.
install-flatpak:
	install -Dm755 $(BUILD)/conflux-gtk $(DESTDIR)$(BINDIR)/conflux-gtk
	install -Dm644 data/$(ID).desktop $(DESTDIR)$(DATADIR)/applications/$(ID).desktop
	install -Dm644 data/$(ID).metainfo.xml $(DESTDIR)$(DATADIR)/metainfo/$(ID).metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/$(ID).svg $(ICONS)/scalable/apps/$(ID).svg
	install -Dm644 data/icons/hicolor/symbolic/apps/$(ID)-symbolic.svg \
	  $(ICONS)/symbolic/apps/$(ID)-symbolic.svg

.PHONY: build-cli build-gtk install-cli install-gtk install-flatpak
