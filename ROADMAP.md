# Roadmap

What's done, what's in progress, and what to work on next. [PLAN.md](PLAN.md) holds the architecture and the original milestone scopes; this file tracks status. Last updated 2026-10-07.

## Released

**Unreleased (on `main`):** model v3 charges network interrupt threads to apps by traffic (ADR 0008); `drainscope doctor` lists them. While nobody watches, the daemon ticks every 15 s instead of 5: idle on battery it uses 0.28% of one CPU instead of 0.82–1.0%, and wakes 0.5 times a second instead of 3. Probe1 gains `ReadAll`, and the extension no longer wakes GNOME Shell every 5 s while its menu is closed.

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
- [x] Model v3 (ADR 0008): the network devices' threaded interrupt handlers (`irq/135-iwlwifi` here) are charged by bytes with the softirqs, read from procfs without privileges; `drainscope doctor` lists them (unreleased; battery validation pending).

### M5 — Hardening and distribution
- [x] SELinux modules for the sampler and the probe, enforcing, no denials; `drainscope-selinux` package.
- [x] RPM spec with seven packages; `cargo xtask dist` builds the tarball, SRPM, RPMs and extension zip.
- [x] AMD support from fixtures and docs (ADR 0005); `doctor` explains GPU and RAPL support.
- [x] GitHub releases v0.1.0, v0.1.1 and v0.1.2.
- [x] COPR `khaledsaeed18/drainscope`: Fedora 44, 45 and rawhide, signed. v0.1.2 validated from COPR on Fedora 44 (GNOME 50.5) and 45 (GNOME 51.0) with `packaging/validate-copr.sh`; the dev machine runs the COPR packages.
- [x] Daemon single instance per database: exclusive `flock` on `drainscope.db.lock`, exit status 3 when held (v0.1.1).
- [x] Daemon stops when its session bus closes (`Connection::closed()`), saving state and exiting with status 0; previously a daemon D-Bus-activated outside systemd outlived its bus (v0.1.2).
- [x] Daemon self-cost, first pass (unreleased): a timerfd tick (tokio's timer wheel woke a 5 s sleep three times), the tick's sampler, probe and sysfs reads in flight together, and an allocation-free cgroup walk with relative opens. Measured on battery: 3.0 → 2.0 context switches/s, 0.82–1.0% → 0.72% of one CPU. The extension subscribes to Tick only while its menu is open.
- [x] Probe1 `ReadAll` (unreleased; ADR 0006 addendum): wakeups, traffic and network time in one call instead of three per tick; the daemon falls back to the three methods for an older probe.
- [x] Slower tick while nobody watches (unreleased): every 15 s, and every 5 s while the app or the Quick Settings menu is open (two Monitor1 calls within 20 s). Idle on battery: 0.28% of one CPU and 0.5 context switches/s (0.1.2: 0.82–1.0% and 3.0/s), under PLAN's 0.5% target. Unplugging is noticed within 15 s while idle. `validate` keeps the daemon live during its runs.
- [x] CSV and JSON export (unreleased): `--format json|csv` on `drainscope`, `report`, `sleep`, `health`, `wakeups` and `network`; every row, in joules, watts and Unix seconds, with the period, coverage and model version in JSON.

### Brand identity
- [x] Energy shares identity v1: editable SVG masters, logo/icon exports, colors, fonts, social graphics, templates, tokens, and visual guide in [branding/](branding/README.md).
- [x] New app icon and a symbolic icon (`data/app/icons/`), installed by `install-dev` and the RPM to hicolor `scalable/apps` and `symbolic/apps`. The extension's quick-settings tile uses the symbolic icon, shipped in the extension and the EGO zip. CI checks the shipped icons match `branding/exports/`. README header, AppStream brand colors; the brand kit is left out of release tarballs. Verified: GTK resolves and recolors the installed icons, RPM contents, nested GNOME 50.5 and 51.0 shells with the extension `ACTIVE` and no JS errors (v0.1.2).
- [x] GitHub social preview set in the repository settings (`branding/exports/social/github-social-*.png`).
- [x] Icons checked in the real session after upgrading to v0.1.2 (app grid, dash, Alt+Tab, Quick Settings tile; light and dark).
- [x] Product screenshots, light and dark, with illustrative data: README hero, feature cards, window cutouts, AppStream screenshots ([branding/screenshots](branding/screenshots/README.md)). Reproducible with `branding/screenshots/tools/capture.sh`: a demo Monitor1 service and a headless GNOME Shell on scratch directories.

## Next (in priority order)

1. **extensions.gnome.org review:** v0.1.2 submitted on 2026-10-09 ([listing](https://extensions.gnome.org/extension/11189/drainscope/)). Answer the reviewers; once approved, link it from the README install section ([docs/distribution.md](docs/distribution.md)).
2. **Validate model v3 on battery:** run the dev daemon in place of the packaged one, then `cargo xtask validate --activity` unplugged (about 10 minutes) on a connection of at least 0.5 MB/s (the 2026-10-09 attempt got 0.13 MB/s and was inconclusive; the harness now checks first). Record the download charge in ADR 0008 and docs/validation-activity.md, then release it (0.1.3). The same run records total idle exits and sleep-state time for model v4.
3. **Model v4: charge the cost of waking the machine** ([draft ADR 0009](docs/adr/0009-model-v4-wake-cost.md)). CPU-time sharing undercharges light, bursty work (downloads about 20% of their cost, timers 7–50%, large CPU loads 79–90%). Findings so far: there is no constant cost per wakeup, and power follows where idle time is spent. Candidate: split the overhead above the work's price by each consumer's share of idle exits. The next validation run records what's needed to replay it offline.
4. **Measure `ReadAll`** once the new probe is installed (0.1.3 or `install-dev`): probe and bus-broker wakeups per tick, against the three calls of 0.1.2.
5. **Suspend test on real hardware** (suspend is masked on the dev machine): verify sleep sessions and wake reasons end to end.

## Later

- Exit capture for cgroups that come and go between ticks ([ADR 0010](docs/adr/0010-exit-capture.md), proposed): about 0.5% of attributable energy on the dev machine now lands in `exited:*`. Design: the probe records exiting threads' CPU time by cgroup (`sched_process_exit`), the model moves it from the parent's `Exited` to the cgroup's own consumer. Needs approval for the probe program and a Probe1 method.
- Foreground vs background energy per app (M3 scope; [ADR 0011](docs/adr/0011-foreground-background.md), proposed): the Shell extension reports the focused app over Monitor1, the daemon splits each app's energy into in use, background and unknown, and the app view and CLI show it. Needs approval for the Monitor1 additions and a schema migration.
- Intel xe driver (`drm-cycles-*`) and an AMD integrated-GPU part (ADR 0005); needs that hardware for validation.
- Flatpak for the app on Flathub (the daemon stays an RPM).
- Fedora official packaging (needs every Rust dependency packaged; see docs/distribution.md).
- Website and documentation site (low priority, after the app and model work): built from the brand kit's tokens and layout studies and the product screenshots ([docs/branding-handoff.md](docs/branding-handoff.md)); includes PLAN M5's write-up of the model and validation.
- Ideas: backlight-weighted display share, per-app notifications, Prometheus output, a KDE Plasma widget.

## Known limitations

- Light, bursty activity is undercharged by proportional sharing (see Next 5 and docs/attribution-model.md).
- AMD integrated-GPU energy is split by CPU time; xe GPUs aren't split per app (ADR 0005).
- Validated on one machine (i7-8550U, i915, Wi-Fi). Results on other hardware are welcome as `validate` reports.
