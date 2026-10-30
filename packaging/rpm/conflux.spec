# conflux and its GTK app for Fedora, built on COPR from .copr/Makefile (the
# crates vendored next to the sources). scripts/release.py sets the version and
# adds the changelog entry.
%bcond check 1

Name:           conflux
Version:        1.1.0
Release:        1%{?dist}
Summary:        Background file-sync daemon for WebDAV, git and local folders

SourceLicense:  GPL-3.0-or-later
# conflux, and the crates linked into it and the app (%%cargo_license_summary;
# LICENSE.dependencies lists them by crate). Check it again when the
# dependencies change.
License:        GPL-3.0-or-later AND Apache-2.0 AND BSD-2-Clause AND BSD-3-Clause AND CC0-1.0 AND CDLA-Permissive-2.0 AND ISC AND MIT AND MPL-2.0 AND Unicode-3.0 AND (Apache-2.0 OR BSL-1.0) AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR MIT) AND (BSD-2-Clause OR Apache-2.0 OR MIT) AND (CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception) AND (CC0-1.0 OR MIT-0 OR Apache-2.0) AND (MIT OR Apache-2.0 OR Zlib) AND (Unlicense OR MIT) AND (Zlib OR Apache-2.0 OR MIT)
URL:            https://github.com/AperteDev/conflux
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
# cargo vendor --locked, as .copr/Makefile makes it.
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 25
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  systemd-rpm-macros
BuildRequires:  pkgconfig(openssl)
BuildRequires:  pkgconfig(zlib)
BuildRequires:  blueprint-compiler
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1) >= 1.7
BuildRequires:  glib2-devel
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

%description
conflux keeps local directories in step with a WebDAV server (Nextcloud and
friends), a git repository or another directory, or keeps a git clone up to
date. Each sync group chooses when it runs (on changes, on a timer, by hand)
and how much of the tree it owns. One binary is both the daemon and the CLI,
with systemd units for user and system daemons, plain and per profile.

%package        gtk
Summary:        GNOME app to set up and watch conflux syncs
Requires:       %{name} = %{version}-%{release}
# The UI uses Adw.ToggleGroup; the library's soname does not say which 1.x it is.
Requires:       libadwaita%{?_isa} >= 1.7
Requires:       polkit

%description    gtk
Conflux shows every conflux sync with its live status, runs a sync or a dry
run, and edits syncs, remotes and defaults in the config file, keeping its
comments and layout. It runs a daemon per profile and shows its log. The
system-wide setup is unlocked with an administrator's password through pkexec
and a small helper.

%prep
%autosetup -p1 -a1
%cargo_prep -v vendor

%build
# The app finds its helper where the polkit policy names it.
export CONFLUX_HELPER_PATH=%{_prefix}/lib/conflux/conflux-helper
%cargo_build -- -p conflux -p conflux-gtk -p conflux-helper
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%cargo_vendor_manifest

%install
build=target/release
[ -d target/rpm ] && build=target/rpm
make install-cli install-gtk DESTDIR=%{buildroot} PREFIX=%{_prefix} LIBDIR=%{_prefix}/lib \
  BUILD=$build
# %%license and %%doc take these.
rm -r %{buildroot}%{_datadir}/licenses %{buildroot}%{_docdir}/conflux

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/dev.aperte.Conflux.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/dev.aperte.Conflux.metainfo.xml
%if %{with check}
%cargo_test -- -p conflux-core -p conflux -p conflux-helper -p conflux-gtk
%endif

%post
%systemd_post conflux.service conflux@.service
%systemd_user_post conflux.service conflux@.service

%preun
%systemd_preun conflux.service
%systemd_user_preun conflux.service

%postun
%systemd_postun_with_restart conflux.service
%systemd_user_postun_with_restart conflux.service

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md docs/FEATURES.md docs/CHANGELOG.md config.example.toml
%{_bindir}/conflux
%{_unitdir}/conflux.service
%{_unitdir}/conflux@.service
%{_userunitdir}/conflux.service
%{_userunitdir}/conflux@.service
%{_sysusersdir}/conflux.conf
%dir %{_sysconfdir}/conflux
%config(noreplace) %{_sysconfdir}/conflux/config.toml

%files gtk
%license LICENSE
%{_bindir}/conflux-gtk
%dir %{_prefix}/lib/conflux
%{_prefix}/lib/conflux/conflux-helper
%{_datadir}/applications/dev.aperte.Conflux.desktop
%{_metainfodir}/dev.aperte.Conflux.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/dev.aperte.Conflux.svg
%{_datadir}/icons/hicolor/symbolic/apps/dev.aperte.Conflux-symbolic.svg
%{_datadir}/polkit-1/actions/dev.aperte.conflux.policy

%changelog
* Mon Oct 26 2026 SelfRef <git@selfref.dev> - 1.1.0-1
- See CHANGELOG.md
