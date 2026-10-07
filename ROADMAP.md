# Roadmap

What's done, what's in progress, and what to work on next. [PLAN.md](PLAN.md) holds the architecture and the original milestone scopes; this file tracks status. Last updated 2026-10-07.

## Released

**v0.1.2** (2026-10-07): [GitHub release](https://github.com/KhaledSaeed18/drainscope/releases/tag/v0.1.2) and [COPR](https://copr.fedorainfracloud.org/coprs/khaledsaeed18/drainscope/) (Fedora 44, 45, rawhide). New app icon and a symbolic icon; the extension tile uses the drainscope symbolic icon; the daemon stops when its session bus closes.

**v0.1.1** (2026-10-06): [GitHub release](https://github.com/KhaledSaeed18/drainscope/releases/tag/v0.1.1) and [COPR](https://copr.fedorainfracloud.org/coprs/khaledsaeed18/drainscope/) (Fedora 44, 45, rawhide). The daemon locks its database so a second instance can't double-count; the extension supports GNOME 51.

**v0.1.0** (2026-10-05): [GitHub release](https://github.com/KhaledSaeed18/drainscope/releases/tag/v0.1.0) with the source tarball (vendored crates, offline build), the SRPM, RPMs for Fedora 44 and the GNOME Shell extension zip.

## Done

### M0 — Feasibility (ADR 0001)
- [x] Toolchain, workspace, CI, lint settings.
- [x] Trace recorder, throwaway attribution spike, go/no-go: go, with model changes.

### M1 — Daemon, sampler, CLI
- [x] `model`: identity rules, snapshot diffing, idle floor, attribution v1, windows, battery reconciliation (proptest invariants).
- [x] `sys`: powercap, cgroups, DRM fdinfo, batteries, logind; fixture and trace-replay tests.
- [x] `store`: SQLite with migrations, rollups, retention.
- [x] `drainscope-sampler`: static system user, only `CAP_DAC_READ_SEARCH`, polkit, rate limit, quantization, exposure 0.6.
- [x] `drainscope-daemon`: 5 s ticks (0.38% CPU, 8.5 MB), plug/unplug, suspend sessions, Monitor1.
- [x] CLI: `summary`, `report`, `top`, `sleep`, `status`, `doctor`.
- [x] `xtask validate` on battery (docs/validation.md, ADR 0004).

### M2 — GNOME Shell extension
- [x] Quick-settings "Battery" tile with the top 5 since unplugging; GNOME 50 and 51 (51 tested in a nested shell).

### M3 — Desktop app, sleep and health
- [x] libadwaita app: since unplugged / hour / 24 h / 7 days, stacked timeline, per-consumer breakdown, folded tail.
- [x] Sleep sessions with wake reasons (wakeup sources and IRQ), `Monitor1.GetSleepHistory`.
- [x] Battery health recorded daily (migration 2), `Monitor1.GetBatteryHealth`, `drainscope health`.

### M4 — eBPF precision (ADR 0006, ADR 0007)
- [x] `drainscope-probe`: own system user, `CAP_BPF` + `CAP_PERFMON` + `CAP_NET_ADMIN` (empty network namespace), exposure 0.7.
- [x] Idle exits per cgroup (`sched_switch`): `drainscope wakeups`, app section. Validated live.
- [x] Network bytes per cgroup (`cgroup_skb`, loopback excluded): `drainscope network`, app section. Validated: 20 MB counted as 20.64 MB.
- [x] Costs measured on battery (`validate --activity`): wakeups 1.2–1.5 mW per 100/s; network stack 170–211 mW per MB/s; radio ≈ 70 mW per MB/s.
- [x] Model v2: network-softirq time moves from Kernel to apps by bytes. Downloads charged 20% of their cost (v1: 5%).

### M5 — Hardening and distribution
- [x] SELinux modules for the sampler and the probe, enforcing, no denials; `drainscope-selinux` package.
- [x] RPM spec with seven packages; `cargo xtask dist` builds the tarball, SRPM, RPMs and extension zip.
- [x] AMD support from fixtures and docs (ADR 0005); `doctor` explains GPU and RAPL support.
- [x] GitHub releases v0.1.0, v0.1.1 and v0.1.2.
- [x] COPR `khaledsaeed18/drainscope`: Fedora 44, 45 and rawhide, signed. v0.1.2 validated from COPR on Fedora 44 (GNOME 50.5) and 45 (GNOME 51.0) with `packaging/validate-copr.sh`; the dev machine runs the COPR packages.
- [x] Daemon single instance per database: exclusive `flock` on `drainscope.db.lock`, exit status 3 when held (v0.1.1).
- [x] Daemon stops when its session bus closes (`Connection::closed()`), saving state and exiting with status 0; previously a daemon D-Bus-activated outside systemd outlived its bus (v0.1.2).

### Brand identity
- [x] Energy shares identity v1: editable SVG masters, logo/icon exports, colors, fonts, social graphics, templates, tokens, and visual guide in [branding/](branding/README.md).
- [x] New app icon and a symbolic icon (`data/app/icons/`), installed by `install-dev` and the RPM to hicolor `scalable/apps` and `symbolic/apps`. The extension's quick-settings tile uses the symbolic icon, shipped in the extension and the EGO zip. CI checks the shipped icons match `branding/exports/`. README header, AppStream brand colors; the brand kit is left out of release tarballs. Verified: GTK resolves and recolors the installed icons, RPM contents, nested GNOME 50.5 and 51.0 shells with the extension `ACTIVE` and no JS errors (v0.1.2).
- [x] GitHub social preview set in the repository settings (`branding/exports/social/github-social-*.png`).
- [x] Icons checked in the real session after upgrading to v0.1.2 (app grid, dash, Alt+Tab, Quick Settings tile; light and dark).
- [x] Product screenshots, light and dark, with illustrative data: README hero, feature cards, window cutouts, AppStream screenshots ([branding/screenshots](branding/screenshots/README.md)). Reproducible with `branding/screenshots/tools/capture.sh`: a demo Monitor1 service and a headless GNOME Shell on scratch directories.

## Next (in priority order)

1. **Publish the extension on extensions.gnome.org** (deferred by the maintainer). Upload the zip from the latest release (GNOME 50 and 51; validated); needs an EGO account. The screenshot is ready ([docs/distribution.md](docs/distribution.md)).
2. **Model v2.1: count the network driver's IRQ thread** (e.g. `irq/134-iwlwifi`, about 30% more network work), charged by bytes like the softirqs. Small; needs one battery validation run.
3. **Model v3: marginal-cost attribution** (needs an ADR first). Proportional sharing undercharges light, bursty consumers: downloads get about 20% of their cost and timer loads 7–50%, while large CPU loads get 79–90%. Charge each consumer the power it adds, measured against the idle floor and current utilization.
4. **Daemon self-cost:** about 4 wakeups/s (≈ 20 per 5 s tick). Batch the D-Bus calls and the collector thread.
5. **Suspend test on real hardware** (suspend is masked on the dev machine): verify sleep sessions and wake reasons end to end.

## Later

- Exit capture for short-lived processes (eBPF `sched_process_exit`, ADR 0006 stage 2).
- Foreground vs background time per app (M3 scope).
- Intel xe driver (`drm-cycles-*`) and an AMD integrated-GPU part (ADR 0005); needs that hardware for validation.
- Flatpak for the app on Flathub (the daemon stays an RPM).
- Fedora official packaging (needs every Rust dependency packaged; see docs/distribution.md).
- Ideas: backlight-weighted display share, per-app notifications, CSV/JSON export, Prometheus output, a KDE Plasma widget.

## Known limitations

- Light, bursty activity is undercharged by proportional sharing (see Next 5 and docs/attribution-model.md).
- AMD integrated-GPU energy is split by CPU time; xe GPUs aren't split per app (ADR 0005).
- Validated on one machine (i7-8550U, i915, Wi-Fi). Results on other hardware are welcome as `validate` reports.
