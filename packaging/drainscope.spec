%global uuid drainscope@khaledsaeed18.github.io
%global app_id io.github.khaledsaeed18.Drainscope
%global selinuxtype targeted
%global selinux_modules drainscope_sampler drainscope_probe

Name:           drainscope
Version:        0.1.4
Release:        1%{?dist}
Summary:        Per-app battery and energy usage for the Linux desktop

# drainscope is GPL-3.0-or-later; the rest covers the bundled Rust crates, listed with their
# licenses in LICENSE.dependencies. `cargo xtask dist` checks this expression is current.
License:        GPL-3.0-or-later AND (Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT) AND (Apache-2.0 OR MIT) AND (MIT OR Unlicense) AND MIT AND Unicode-3.0 AND Zlib
URL:            https://github.com/KhaledSaeed18/drainscope
# Built by `cargo xtask dist`: the git tree plus vendored crates (offline build) and the
# bundled GNOME Shell extension.
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88
BuildRequires:  gcc
# The probe's eBPF program (bins/probe/bpf/) is compiled with clang.
BuildRequires:  clang
BuildRequires:  libbpf-devel
BuildRequires:  kernel-headers
BuildRequires:  systemd-rpm-macros
BuildRequires:  desktop-file-utils
BuildRequires:  appstream
BuildRequires:  selinux-policy-devel
BuildRequires:  bzip2

# The daemon degrades gracefully without RAPL (battery-only attribution).
Recommends:     %{name}-sampler = %{version}-%{release}
Recommends:     %{name}-probe = %{version}-%{release}
Suggests:       gnome-shell-extension-%{name} = %{version}-%{release}
Suggests:       %{name}-app = %{version}-%{release}

%description
drainscope measures energy from hardware counters (RAPL) and the battery and attributes it
to apps, terminal workloads and system services through cgroup v2, DRM fdinfo and systemd
scopes. It keeps a local history and answers "which app used my battery since I
unplugged?" No component runs as root, and nothing leaves the machine.

This package contains the per-user daemon and the drainscope command-line tool.

%package sampler
Summary:        Sandboxed RAPL energy counter service for drainscope
# Confined by its own SELinux module where SELinux is in use.
Requires:       (%{name}-selinux = %{version}-%{release} if selinux-policy-%{selinuxtype})

%description sampler
Reads the root-only RAPL energy counters with only CAP_DAC_READ_SEARCH, as a dedicated
system user in a tight sandbox, and serves them over D-Bus to the active local session
(polkit), rate-limited and quantized. Started on demand; exits when idle.

%package selinux
Summary:        SELinux policy confining the drainscope sampler and probe
BuildArch:      noarch
Requires:       selinux-policy-%{selinuxtype}
Requires(post): selinux-policy-%{selinuxtype}
%{?selinux_requires}

%description selinux
SELinux modules that confine drainscope-sampler and drainscope-probe to their own domains:
reading sysfs, cgroupfs and kernel BTF, loading their eBPF program, talking to D-Bus and polkit,
and logging. Nothing else.

%package probe
Summary:        Sandboxed eBPF activity probe for drainscope
Requires:       (%{name}-selinux = %{version}-%{release} if selinux-policy-%{selinuxtype})

%description probe
Counts how often each app and service wakes the processor from idle with an eBPF program,
as a dedicated system user with only CAP_BPF and CAP_PERFMON in a tight sandbox, and serves
per-cgroup totals over D-Bus to the active local session (polkit). Other users' activity is
never exposed. Started on demand; exits when idle.

%package -n gnome-shell-extension-%{name}
Summary:        GNOME Shell quick-settings tile for drainscope
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       gnome-shell >= 50

%description -n gnome-shell-extension-%{name}
A "Battery" tile in GNOME's quick settings listing what used the battery since the charger
was unplugged.

%package app
Summary:        Battery usage history for drainscope
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       gjs
Requires:       gtk4
Requires:       libadwaita >= 1.6

%description app
A GNOME app showing what used the battery since you unplugged, over the last hour, day or
week, and how much each suspend cost.

%prep
%autosetup
# Build only from the crates vendored into the tarball.
cat >> .cargo/config.toml <<'EOF'

[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
export RUSTFLAGS="%{build_rustflags}"
cargo build --release --frozen --offline \
    -p drainscope-sampler -p drainscope-probe -p drainscope-daemon -p drainscope-cli
make -C data/selinux -f %{_datadir}/selinux/devel/Makefile \
    $(for module in %{selinux_modules}; do echo $module.pp; done)
bzip2 -9 data/selinux/*.pp

%install
install -Dpm0755 target/release/drainscope-sampler %{buildroot}%{_libexecdir}/drainscope-sampler
install -Dpm0755 target/release/drainscope-probe %{buildroot}%{_libexecdir}/drainscope-probe
install -Dpm0755 target/release/drainscope-daemon %{buildroot}%{_bindir}/drainscope-daemon
install -Dpm0755 target/release/drainscope %{buildroot}%{_bindir}/drainscope

install -Dpm0644 data/systemd/drainscope-sampler.service %{buildroot}%{_unitdir}/drainscope-sampler.service
install -Dpm0644 data/systemd/drainscope-probe.service %{buildroot}%{_unitdir}/drainscope-probe.service
install -Dpm0644 data/systemd/user/drainscope.service %{buildroot}%{_userunitdir}/drainscope.service
install -Dpm0644 data/sysusers/drainscope.conf %{buildroot}%{_sysusersdir}/drainscope.conf
install -Dpm0644 data/sysusers/drainscope-probe.conf %{buildroot}%{_sysusersdir}/drainscope-probe.conf

install -Dpm0644 -t %{buildroot}%{_datadir}/dbus-1/system.d data/dbus/system.d/*.conf
install -Dpm0644 -t %{buildroot}%{_datadir}/dbus-1/system-services data/dbus/system-services/*.service
install -Dpm0644 -t %{buildroot}%{_datadir}/dbus-1/services data/dbus/services/*.service
install -Dpm0644 -t %{buildroot}%{_datadir}/dbus-1/interfaces data/dbus/interfaces/*.xml
install -Dpm0644 -t %{buildroot}%{_datadir}/polkit-1/actions data/polkit/*.policy

install -Dpm0644 -t %{buildroot}%{_datadir}/selinux/packages/%{selinuxtype} data/selinux/*.pp.bz2

install -dm0755 %{buildroot}%{_datadir}/gnome-shell/extensions/%{uuid}
cp -a ui/extension/dist/. %{buildroot}%{_datadir}/gnome-shell/extensions/%{uuid}/

install -Dpm0755 ui/app/dist/drainscope-app %{buildroot}%{_bindir}/drainscope-app
install -Dpm0644 -t %{buildroot}%{_datadir}/applications data/app/%{app_id}.desktop
install -Dpm0644 -t %{buildroot}%{_metainfodir} data/app/%{app_id}.metainfo.xml
install -Dpm0644 -t %{buildroot}%{_datadir}/icons/hicolor/scalable/apps data/app/icons/%{app_id}.svg
install -Dpm0644 -t %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps data/app/icons/%{app_id}-symbolic.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
export RUSTFLAGS="%{build_rustflags}"
cargo test --release --frozen --offline --workspace --exclude xtask

%post
%systemd_user_post drainscope.service

%preun
%systemd_user_preun drainscope.service

# Restart running daemons after an upgrade, so the new one (and its migrations) runs without
# waiting for the next login.
%postun
%systemd_user_postun_with_restart drainscope.service

%post sampler
%systemd_post drainscope-sampler.service

%preun sampler
%systemd_preun drainscope-sampler.service

%postun sampler
%systemd_postun_with_restart drainscope-sampler.service

%pre selinux
%selinux_relabel_pre -s %{selinuxtype}

%post selinux
%selinux_modules_install -s %{selinuxtype} $(for module in %{selinux_modules}; do echo %{_datadir}/selinux/packages/%{selinuxtype}/$module.pp.bz2; done)

%postun selinux
if [ $1 -eq 0 ]; then
    %selinux_modules_uninstall -s %{selinuxtype} %{selinux_modules}
fi

%posttrans selinux
%selinux_relabel_post -s %{selinuxtype}

%post probe
%systemd_post drainscope-probe.service

%preun probe
%systemd_preun drainscope-probe.service

%postun probe
%systemd_postun_with_restart drainscope-probe.service

%files
%license LICENSE LICENSE.dependencies
%doc README.md PLAN.md docs/validation.md
%{_bindir}/drainscope
%{_bindir}/drainscope-daemon
%{_userunitdir}/drainscope.service
%{_datadir}/dbus-1/services/io.github.khaledsaeed18.Drainscope.Monitor.service
%{_datadir}/dbus-1/interfaces/io.github.khaledsaeed18.Drainscope.Monitor1.xml
%{_datadir}/dbus-1/interfaces/io.github.khaledsaeed18.Drainscope.Sampler1.xml
%{_datadir}/dbus-1/interfaces/io.github.khaledsaeed18.Drainscope.Probe1.xml

%files sampler
%license LICENSE LICENSE.dependencies
%{_libexecdir}/drainscope-sampler
%{_unitdir}/drainscope-sampler.service
%{_sysusersdir}/drainscope.conf
%{_datadir}/dbus-1/system.d/io.github.khaledsaeed18.Drainscope.Sampler.conf
%{_datadir}/dbus-1/system-services/io.github.khaledsaeed18.Drainscope.Sampler.service
%{_datadir}/polkit-1/actions/io.github.khaledsaeed18.Drainscope.policy

%files selinux
%license LICENSE
%{_datadir}/selinux/packages/%{selinuxtype}/drainscope_sampler.pp.bz2
%{_datadir}/selinux/packages/%{selinuxtype}/drainscope_probe.pp.bz2

%files probe
%license LICENSE LICENSE.dependencies
%{_libexecdir}/drainscope-probe
%{_unitdir}/drainscope-probe.service
%{_sysusersdir}/drainscope-probe.conf
%{_datadir}/dbus-1/system.d/io.github.khaledsaeed18.Drainscope.Probe.conf
%{_datadir}/dbus-1/system-services/io.github.khaledsaeed18.Drainscope.Probe.service
%{_datadir}/polkit-1/actions/io.github.khaledsaeed18.Drainscope.Probe.policy

%files -n gnome-shell-extension-%{name}
%license LICENSE
%{_datadir}/gnome-shell/extensions/%{uuid}/

%files app
%license LICENSE
%{_bindir}/drainscope-app
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog
* Sat Oct 10 2026 Khaled Saeed <147975926+KhaledSaeed18@users.noreply.github.com> - 0.1.4-1
- Each app's energy is split into while in use and in the background (ADR 0011)
- GNOME Shell extension: report the focused app to the daemon (app ID only)
- Daemon: Monitor1 SetFocus, EndFocus and GetFocus; schema migration 3
- App: the detail page shows energy while in use and in the background
- CLI: report shows In use and Background columns; exports gain focus fields

* Sat Oct 10 2026 Khaled Saeed <147975926+KhaledSaeed18@users.noreply.github.com> - 0.1.3-1
- Attribution model v3: network interrupt threads are charged to the apps causing the traffic
- Daemon: tick every 15 s while no view is open (5 s while one is); lower CPU and wakeups
- Probe: ReadAll returns wakeups, traffic and network time in one call
- GNOME Shell extension: follow the daemon's ticks only while the menu is open
- CLI: --format json|csv export; doctor lists the network interrupt threads
- Short-lived processes are labelled as such instead of "Exited processes"

* Wed Oct 07 2026 Khaled Saeed <147975926+KhaledSaeed18@users.noreply.github.com> - 0.1.2-1
- Daemon: stop when the session bus closes
- New app icon; install a symbolic app icon
- GNOME Shell extension: use the drainscope symbolic icon for the quick-settings tile

* Tue Oct 06 2026 Khaled Saeed <147975926+KhaledSaeed18@users.noreply.github.com> - 0.1.1-1
- Daemon: hold an exclusive lock on the database so a second daemon can't double-count
- GNOME Shell extension: support GNOME 51; extensions.gnome.org review fixes

* Mon Oct 05 2026 Khaled Saeed <147975926+KhaledSaeed18@users.noreply.github.com> - 0.1.0-1
- First release: per-user daemon, sandboxed RAPL sampler and eBPF probe with SELinux policy,
  CLI, GNOME Shell extension and app
