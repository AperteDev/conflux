#!/bin/sh
# Package a staged tree as a .deb: `conflux` (the daemon and CLI, from `make
# install-cli`) or `conflux-gtk` (the app and its helper, from `make
# install-gtk`). Run it on the Ubuntu release the package is for: dpkg-shlibdeps
# takes the dependencies from the libraries the binaries link to, as that
# release names them.
#
#   scripts/deb.sh <conflux|conflux-gtk> <staged tree> <version> <out dir>
set -eu

package=$1 stage=$2 version=$3 out=$4
arch=$(dpkg --print-architecture)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cp -a "$stage" "$work/root"
mkdir -p "$work/root/DEBIAN" "$work/root/usr/share/doc/$package"
cp "$(dirname "$0")/../LICENSE" "$work/root/usr/share/doc/$package/copyright"
# Debian ships man pages compressed.
if [ -d "$work/root/usr/share/man" ]; then
  find "$work/root/usr/share/man" -type f -exec gzip -9n {} +
fi

# dpkg-shlibdeps reads a source package's control file from the current directory.
mkdir -p "$work/src/debian"
printf 'Source: conflux\n\nPackage: %s\nArchitecture: any\n' "$package" > "$work/src/debian/control"
shlibs() {
  (cd "$work/src" && dpkg-shlibdeps -O "$@") | sed -n 's/^shlibs:Depends=//p'
}

case $package in
conflux)
  depends=$(shlibs "$work/root/usr/bin/conflux")
  extra="Recommends: systemd
Section: utils"
  description="Background file-sync daemon for WebDAV, git and local folders
 conflux keeps local directories in step with a WebDAV server (Nextcloud and
 friends), a git repository or another directory, or keeps a git clone up to
 date. Each sync group chooses when it runs (on changes, on a timer, by hand)
 and how much of the tree it owns. One binary is both the daemon and the CLI.
 .
 Includes systemd units for user and system daemons, plain and per profile."
  # The starter config is the admin's to edit: kept on upgrades.
  echo /etc/conflux/config.toml > "$work/root/DEBIAN/conffiles"
  cat > "$work/root/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
  # The system daemon's user (/usr/lib/sysusers.d/conflux.conf).
  if command -v systemd-sysusers >/dev/null 2>&1; then
    systemd-sysusers conflux.conf || true
  fi
  if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
  fi
fi
EOF
  chmod 755 "$work/root/DEBIAN/postinst"
  ;;
conflux-gtk)
  # libadwaita's version comes from the widgets the UI uses (Adw.ToggleGroup),
  # which linking does not show: below, in place of the one found.
  depends=$(shlibs "$work/root/usr/bin/conflux-gtk" "$work/root/usr/lib/conflux/conflux-helper" |
    sed 's/libadwaita-1-0 ([^)]*), *//; s/, *libadwaita-1-0 ([^)]*)//')
  depends="$depends, libadwaita-1-0 (>= 1.7), conflux (= $version), pkexec"
  extra="Section: gnome"
  description="GNOME app to set up and watch conflux syncs
 Conflux shows every conflux sync with its live status, runs a sync or a dry
 run, and edits syncs, remotes and defaults in the config file, keeping its
 comments and layout. It runs a daemon per profile and shows its log. The
 system-wide setup is unlocked with an administrator's password through
 pkexec and a small helper."
  ;;
*)
  echo "unknown package: $package" >&2
  exit 1
  ;;
esac

cat > "$work/root/DEBIAN/control" <<EOF
Package: $package
Version: $version
Architecture: $arch
Maintainer: SelfRef <git@selfref.dev>
Installed-Size: $(du -sk "$work/root" | cut -f1)
Depends: $depends
$extra
Priority: optional
Homepage: https://github.com/AperteDev/conflux
Description: $description
EOF

mkdir -p "$out"
dpkg-deb --root-owner-group --build "$work/root" "$out/${package}_${version}_$arch.deb"
