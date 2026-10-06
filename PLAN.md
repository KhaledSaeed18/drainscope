# drainscope — Plan

> Per-app battery and energy usage for the Linux desktop.
> "Firefox used 14% of your battery since you unplugged." Windows, macOS and Android have had this for years; Linux has not.

Status: **v0.1.1 released** (2026-10-06; GitHub and COPR). M0–M2 are done; M3–M5 are done apart from the items in [ROADMAP.md](ROADMAP.md), which tracks status and what's next. This file keeps the architecture, the privilege model and the original milestone scopes.

The selected Energy shares brand identity is maintained in [branding/](branding/README.md). Its application/package integration and Linux verification are tracked in [docs/branding-handoff.md](docs/branding-handoff.md).

---

## 1. Overview

### Problem
Linux laptop users cannot answer "what is draining my battery?" over time:

| Existing tool | Why it doesn't solve it |
|---|---|
| powertop | Root-only live snapshot, per process/kernel event, no history, no app grouping |
| Scaphandre, PowerJoular | Server/CLI exporters, per PID, no desktop app identity, no battery %, no history UI |
| Stormbreaker | Learned (ML) per-machine models, early stage |
| GNOME Power Statistics, BatteryScope, batterylog | Whole-battery curves and health only, nothing per app |

### What drainscope does
1. Measures energy from hardware counters (Intel/AMD RAPL via powercap, including `psys` where present) and, when discharging, the battery itself.
2. Attributes that energy to **apps** (GNOME/Flatpak app scopes), **terminal workloads** (`vte-spawn-*` / `ptyxis-spawn-*` scopes), **system services** (systemd units), the kernel, idle, and "display & devices", using a transparent, documented, proportional model (no ML).
3. Keeps local history (SQLite) and shows it in a GNOME Shell quick-settings section, a CLI, and later a libadwaita app.
4. Tracks suspend sessions (battery lost per sleep, drain per hour) and battery health over time.

### Goals
- Accurate enough to rank consumers correctly, with **published error bars** from a reproducible validation harness.
- Negligible overhead: the daemon targets < 0.5% of one CPU on average and < 30 MB RSS, and **reports its own usage** like any other consumer.
- No component runs as full root. The privileged part is tiny, sandboxed, D-Bus-activated and polkit-gated.
- Fully local. No network access at all.

### Non-goals (for now)
- Power *management* (TLP, tuned, auto-cpufreq already do that). drainscope observes; it does not tune.
- Exact per-process watts as ground truth. That is physically unmeasurable on this hardware. We make an estimate and state how good it is.
- Non-systemd or cgroup v1 systems.
- Discrete GPUs (NVIDIA/AMD dGPU) in v1. Intel iGPU via `uncore` + DRM fdinfo only.

---

## 2. Architecture

### Components

```
┌──────────────────── system bus / root side ─────────────────────┐
│ drainscope-sampler  (Rust, system service, D-Bus activated)     │
│   User=drainscope-sampler, AmbientCapabilities=CAP_DAC_READ_SEARCH│
│   reads /sys/class/powercap/*/energy_uj (0400 root)             │
│   exports io.github.khaledsaeed18.Drainscope.Sampler1           │
│   polkit: read-energy (allow_active only), per-sender rate limit│
│   quantized values, exits after 60 s idle                       │
└────────────────────────────▲────────────────────────────────────┘
                             │ system D-Bus
┌────────────────────────────┴──── user session ──────────────────┐
│ drainscope-daemon  (Rust, systemd --user service)               │
│   collectors (sys)  ─► interval engine ─► attribution (model)   │
│        │                                        │               │
│        │  RAPL client · cgroup · DRM fdinfo ·   ▼               │
│        │  power_supply · logind             store (SQLite)      │
│        └───────────────────────────────────►    │               │
│   exports io.github.khaledsaeed18.Drainscope.Monitor1 (session) │
└───────▲──────────────────────▲──────────────────────▲───────────┘
        │ session D-Bus        │                      │
  drainscope CLI (Rust)   GNOME Shell extension   libadwaita app
                          (TypeScript → GJS)      (TypeScript → GJS, M3)
```

### Module boundaries (Cargo workspace + pnpm workspace)

Layering follows **system integration → core → interface**, with storage isolated as a repository layer.

| Crate / package | Layer | Responsibility | May depend on | Must NOT |
|---|---|---|---|---|
| `drainscope-model` | core | Domain types (`ConsumerKey`, `Snapshot`, `IntervalDelta`, `ClosedWindow`), the **attribution model**, identity rules for parsing cgroup names, calibration math. Port traits are added only where a fake is needed; the daemon tests against fixture trees and an in-memory store instead. | std, thiserror | do any I/O, depend on tokio/zbus/rusqlite |
| `drainscope-sys` | system integration | Readers: powercap (sampler side), cgroup v2 walker, DRM fdinfo scanner, power_supply sysfs, `/proc/<pid>` facts, the logind client (sleep notifications and inhibitor). Every reader takes a `SysRoot` (base path) so it can be tested against fixture trees. | model, zbus (clients only), procfs/rustix | touch SQLite, define D-Bus servers |
| `drainscope-store` | repository | SQLite schema, migrations, the `Store` repository (windows, usage, rollups, retention, events, calibration). **The only crate that imports `rusqlite`.** | model, rusqlite | depend on sys or dbus |
| `drainscope-dbus` | interface contract | Introspection XML (source of truth in `data/dbus/`), zbus interface types and server/proxy definitions for Sampler1 and Monitor1, error enums. | model, zbus, serde | contain business logic |
| `drainscope-sampler` (bin) | interface + composition | Sampler1 server, polkit check, rate limiting, quantization, idle exit. | dbus, sys, model | read anything outside powercap |
| `drainscope-daemon` (bin) | composition root | Wires sys → model → store, runs the tick loop, serves Monitor1, handles logind sleep inhibitor, self-accounting. | all lib crates | contain attribution logic (that lives in model) |
| `drainscope` (bin, CLI) | interface | `top`, `report`, `sleep`, `status`, `doctor`. Talks **only** to Monitor1 (never opens the DB directly). | dbus, model, clap | import store or sys (exception: `doctor` uses sys read-only probes) |
| `ui/shared` (TS) | interface | Typed Monitor1 client, **validated GVariant decoders**, formatting (J → %, Wh, durations), view models. Pure logic runs under Node for tests. | @girs types | use `any`, unchecked casts |
| `ui/extension` (TS) | interface | GNOME Shell 50 and 51 extension: quick-settings section "Battery usage". | ui/shared | do heavy work or blocking calls on the Shell main loop |
| `ui/app` (TS, M3) | interface | libadwaita app: timelines, per-app detail, sleep sessions, health. | ui/shared | |
| `xtask` | dev tooling | `record-fixture`, `validate`, `dist`, `srpm`. | anything | ship in packages |

### Repository layout

```
drainscope/
├── Cargo.toml                 # workspace
├── crates/
│   ├── model/  sys/  store/  dbus/
├── bins/
│   ├── sampler/  daemon/  cli/
├── xtask/
├── ui/                        # pnpm workspace
│   ├── package.json  pnpm-workspace.yaml  tsconfig.base.json  eslint.config.js
│   ├── shared/  extension/  app/
├── data/
│   ├── dbus/                  # *.xml interfaces, system bus policy, activation .service files
│   ├── systemd/               # drainscope-sampler.service, drainscope.service (user)
│   ├── polkit/                # io.github.khaledsaeed18.Drainscope.policy
│   └── selinux/               # (M5) drainscope_sampler.te/.fc/.if
├── testdata/                  # recorded sysfs/procfs fixture trees + traces
├── packaging/
│   ├── drainscope.spec
│   └── .copr/Makefile
├── docs/
│   ├── adr/                   # architecture decision records
│   ├── attribution-model.md
│   ├── privilege-model.md
│   └── validation.md          # published error bars
├── PLAN.md
└── CLAUDE.md
```

---

## 3. Tech stack decisions

| Area | Decision | Rationale | Rejected |
|---|---|---|---|
| Daemons + CLI | **Rust** (edition 2024, stable) | Low overhead is a product requirement (an energy monitor must not burn energy); privileged code should be memory-safe; best D-Bus library available (zbus); single static-ish binaries | **Node:** D-Bus libs unmaintained (`dbus-next` last release 2022), ~40 MB idle RSS. **Go:** viable, but weaker D-Bus server ergonomics and codegen than zbus. |
| Async runtime | tokio | zbus `tokio` feature; mature ecosystem | async-std (deprecated), smol (fine, smaller ecosystem) |
| D-Bus | **zbus 5.x** (pure Rust) | Typed interfaces via macros, proxy codegen from XML (`zbus-xmlgen`), works on system + session bus, p2p connections for tests | dbus-rs (libdbus binding, C dependency) |
| Storage | **SQLite via rusqlite** (bundled), WAL mode, migrations tracked with `PRAGMA user_version` (no extra crate) | Local, single-file, zero-admin, great for time-series rollups at this scale | sqlx (compile-time DB needed, async not required), embedded TSDBs (overkill) |
| /proc parsing | `procfs` crate for `/proc/stat`, `/proc/<pid>/*`; plain `std::fs` for sysfs/cgroupfs | Native, structured | shelling out to `ps`/`cat` |
| CLI | clap 4 (derive) | standard | |
| Logging | `tracing` + `tracing-journald` | Structured logs straight into journald | |
| Errors | `thiserror` in libraries, `anyhow` only in bins | Typed errors at layer boundaries | |
| Property tests | `proptest` | Energy-conservation invariants of the model | |
| eBPF (M4) | **aya** (pure Rust, CO-RE; kernel has BTF) | No clang/BCC at runtime, same language | bpftrace (prototype only), libbpf-rs (needs clang toolchain at build) |
| Shell extension | **TypeScript → GJS ESM**, `@girs/gnome-shell` 50.x types, esbuild bundling (ESM, unminified), `tsc --noEmit` for type checking | Shell extensions must be GJS; TS is your strongest language; shares code with the app | plain JS (no type safety) |
| Desktop app (M3) | **TypeScript on GJS + GTK4 4.22 + libadwaita 1.9**, meson for install and GResources | Native GNOME look, shares `ui/shared` with the extension | gtk4-rs/Relm4 (one language fewer, slower UI iteration); Electron/Tauri (non-native, heavy) |
| Charts | Custom `Gtk.DrawingArea` + Cairo (later GtkSnapshot) | We need ~3 chart types: stacked bars, a timeline, sparklines | WebKitGTK + a JS chart library |
| JS tooling | pnpm workspace, TypeScript 7 (strict), typescript-eslint `strict-type-checked`, vitest for pure logic | Your standard toolchain | |
| Packaging | RPM (COPR), extensions.gnome.org, GitHub Releases; Flatpak for the app only | Daemons cannot be Flatpaks | |

**Learning curve (explicit):**
- **Rust (moderate → high):** ownership and borrowing, async with tokio, zbus interface macros. Mitigation: the model crate is synchronous and pure, so start there. Async only appears in bins and D-Bus.
- **GJS / GObject Introspection (moderate):** GObject lifecycle, signals, `GLib.Variant`, the Shell extension lifecycle (`enable`/`disable`). Docs are thin; we'll read gnome-shell's own source.
- **Linux power interfaces (low → moderate):** RAPL domains and wraparound, cgroup v2 accounting, DRM fdinfo, logind inhibitors. Documented in `docs/` as we go.
- **SELinux policy authoring (M5, moderate → high):** optional hardening milestone.

---

## 4. Data flow

### Clocks and units
- Intervals are measured on `CLOCK_MONOTONIC` (excludes suspend, so suspend is handled as a separate session). Wall-clock (`CLOCK_REALTIME`) is used only for stored timestamps.
- Counters: `u64` microjoules. Model math: `f64` joules. Time deltas: `u64` nanoseconds.

### Tick (every 5 s; at 2 s the daemon would exceed its 0.5% CPU budget, measured in task 1.19)

```
1. read energy      Sampler1.ReadCounters() → {domain → cumulative µJ} (wrap handled sampler-side)
2. read activity    cgroup walk: cpu.stat usage_usec per leaf group (+ /proc/stat for totals)
                    DRM fdinfo: drm-engine-* ns per DRM client → pid → cgroup
3. read supply      power_supply: status, power_now, energy_now, energy_full (all batteries)
4. diff             snapshot(t) − snapshot(t−1) → IntervalDeltas  (pure, in model)
5. attribute        model::attribute(IntervalDeltas, Calibration) → Vec<(ConsumerKey, EnergySplit)>
6. persist          store: insert interval + usage_raw rows (one transaction)
7. publish          Monitor1 emits Tick signal (live view); rollups run every minute/hour
```

### Discovering consumers (identity rules, in `model`)

| cgroup path pattern | ConsumerKey | Display |
|---|---|---|
| `…/app.slice/app-gnome-<appid>-<pid>.scope` | `app:<appid>` | from `.desktop` (resolved in the UI: `Shell.AppSystem` in the extension, `Gio.DesktopAppInfo` in the app) |
| `…/app.slice/app-flatpak-<appid>-<n>.scope` | `app:<appid>` | same |
| `…/app.slice/app-gnome-<appid>@<id>.service` (autostart) | `app:<appid>` | same |
| `…/app.slice/dbus-:<addr>-<bus name>@<n>.service` (D-Bus-activated apps, e.g. Ptyxis) | `app:<bus name>` | same as apps |
| `…/app.slice/{vte,ptyxis}-spawn-<uuid>.scope` | `term:<leader comm>` | "Terminal: pnpm" |
| `…/session.slice/org.gnome.Shell@{wayland,user}.service` | `shell` | "GNOME Shell" |
| `/user.slice/user-<uid>.slice/session-<id>.scope` (logind sessions: sudo, ssh, tty) | `session:<id>` | labelled with the session leader's `comm` |
| `…/user@<uid>.service/{app,session,background}.slice/<unit>.service` | `user-unit:<unit>` | "User service: pipewire" |
| `/system.slice/<unit>.service` | `unit:<unit>` | "System: dnf-makecache" |
| `…/libpod-<id>.scope`, `/machine.slice/*` | `container:<name>` | name from the cgroup / podman labels (later) |
| `/user.slice/user-<other uid>.slice/**` | `other-users` | aggregated for privacy |
| CPU time not covered by any cgroup | `kernel` | "Kernel" |
| calibrated idle floor | `idle` | "Idle" |
| `psys − (package + dram)` | `platform` | "Chipset & platform" |
| `battery − psys` (on battery) | `devices` | "Display & devices" |
| drainscope's own scope | `self` | "drainscope" (shown honestly) |

Escaping: unit names are systemd-escaped (`\x2d`); the model owns the unescape function.

### Attribution model v1 (full spec goes in `docs/attribution-model.md`; grounded in ADR 0001)
Per interval, for each available domain D ∈ {core, uncore, soc-rest = package−core−uncore, dram}:
1. `baseline_D = true_idle_power_D × Δt` → attributed to `idle`.
2. `active_D = max(0, E_D − baseline_D)` split across consumers by weight. Energy above true idle (including the platform's wake-up cost) is caused by activity, so it goes to the active consumers:
   - core, soc-rest, dram: CPU-time share `Δusage_usec_i / ΣΔusage_usec` (kernel included as a consumer)
   - uncore (iGPU): GPU-time share `Δdrm_engine_ns_i / ΣΔdrm_engine_ns`. If GPU time is 0, attribute to `shell` (compositing) or `idle`.
3. `psys` is used only if it passes a **plausibility check**: median `psys / package ≥ 1` over a warm-up window. On the dev machine it fails (ratio 0.74), so it is ignored there. If trusted: `platform = max(0, E_psys − E_package − E_dram)`.
4. If discharging: `devices = max(0, E_battery − E_soc)`, where `E_soc` is `psys` if trusted, otherwise package + dram. It is computed over **windows of ≥ 10 s**, because `power_now` lags load changes by 6–8 s.
5. **Invariant:** Σ attributed = total measured energy (battery when discharging, otherwise `E_soc`). Enforced by property tests.
6. **True-idle floor:** a low percentile of per-domain power over intervals with < 0.25 busy CPUs and no GPU activity. It is learned over the machine's history (not per session), persisted per power source, and can be bootstrapped with `drainscope calibrate` (2 min idle).
7. Each stored row carries `model_version` so future models never silently reinterpret history.
8. Reported per consumer: **share of battery** and **share of attributable (above-idle) energy**.
9. No conversion factor between RAPL and battery: measured `Δbattery / Δ(package + dram)` was below 1 under load, so the sensors disagree rather than revealing losses (ADR 0004).

Degraded modes (never crash, always report the mode in `Status`):
- Sampler unavailable or denied → on battery: battery energy above the learned *battery* idle floor split by CPU share (the floor, mostly the display, stays `idle`); on AC: CPU/GPU time only, no joules.
- Missing domains (e.g. AMD has no `psys` or `uncore`) → the domain is skipped and the fallback chain is documented.

### Suspend
- The daemon holds a logind **delay inhibitor**. On `PrepareForSleep(true)` it records the battery energy and the time, then releases the inhibitor.
- On `PrepareForSleep(false)` it records a `sleep_session`: duration, Wh and % lost, drain per hour, and the `mem_sleep` mode.
- Wake reason (`/sys/power/pm_wakeup_irq`, wakeup_sources) comes in M3.

### Storage schema

Authoritative in `crates/store/src/schema.rs` (migration 1). Summary:

```
consumers      (id, key UNIQUE, first_seen_ms)
windows        (id, start_ms, end_ms, power_source battery|ac, measurement battery|rapl|battery-only,
                measured_j, shortfall_j, model_version)                        -- retained 48 h
usage_raw      (window_id → windows ON DELETE CASCADE, consumer_id, cpu_j, gpu_j, other_j)
usage_minute   (bucket_ms, power_source, consumer_id, cpu_j, gpu_j, other_j)  -- retained 30 d
usage_hour     (same as usage_minute)                                          -- retained 1 y
power_events   (ts_ms, kind boot|plug|unplug|suspend|resume, battery_percent, energy_wh)
sleep_sessions (start_ms, end_ms, wh_lost, percent_lost, mem_sleep, wake_reason)
calibration    (power_source, part core|uncore|soc-rest|dram|battery, histogram BLOB)
psys_check     (at_least_package, below_package)                               -- single row
```

- **Rollups are written in the same transaction as each window** (window start decides the bucket), so minute and hour totals always equal the raw data they summarize; a property test checks this. Retention only deletes.
- Queries read the finest resolution still retained for the start of their range.
- Battery health (`energy_full` vs design, cycle count) is added by a later migration in M3.
- The database lives at `$XDG_STATE_HOME/drainscope/drainscope.db`, created `0600` before SQLite opens it.
- One daemon per database: before opening it, the daemon takes a non-blocking `flock` on `drainscope.db.lock`. If another daemon holds it (one started on a second session bus, say), it exits with status 3 instead of writing overlapping windows that would double-count energy.

### D-Bus API sketch (XML in `data/dbus/` is authoritative)

**Sampler1** (system bus, `io.github.khaledsaeed18.Drainscope.Sampler1`, path `/io/github/khaledsaeed18/Drainscope/Sampler`)
- `ReadCounters() → (t monotonic_ns, generation, a(st) [(domain, cumulative_uj)])`
- properties: `Domains: as`, `MinIntervalMs: u`, `QuantumUj: t`
- errors: `NotAuthorized`, `RateLimited`, `Unsupported`

**Monitor1** (session bus, `io.github.khaledsaeed18.Drainscope.Monitor1`, path `/io/github/khaledsaeed18/Drainscope/Monitor`)
- `GetSummary() → (on_battery b, since_unplug_ts x, pct_used d, top a(sddd))`
- `GetUsage(since x, until x, group_by s, power_source s) → a(ssdddd)` — key, kind, total_j, cpu_j, gpu_j, pct_battery
- `GetSleepSessions(since x) → a(xxdds)`
- `GetLive() → …` plus signal `Tick(…)`
- properties: `Status: s` (`full` | `battery-only` | `time-only`), `ModelVersion: u`, `Domains: as`

---

## 5. Privilege model

| Component | Runs as | Privileges | Why |
|---|---|---|---|
| `drainscope-sampler` | dedicated system user `drainscope-sampler` from sysusers.d (ADR 0003) | `AmbientCapabilities=CAP_DAC_READ_SEARCH` + `CapabilityBoundingSet=CAP_DAC_READ_SEARCH` | `energy_uj` is `0400 root` because of CVE-2020-8694 (Platypus). Bypassing read DAC is the minimum privilege that can read it. **Not root.** |
| `drainscope-daemon` | the user, `systemd --user` | none | cgroup files, `/proc/<own pids>`, DRM fdinfo of own processes and power_supply sysfs are all user-readable (verified on this machine) |
| CLI, extension, app | the user | none | only talk to Monitor1 |

Sampler hardening (enforced in CI by `systemd-analyze security --offline=yes`, exposure score target ≤ 2.0):
`ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp`, `PrivateDevices`, `PrivateNetwork=yes`, `IPAddressDeny=any`, `NoNewPrivileges=yes`, `RestrictAddressFamilies=AF_UNIX`, `SystemCallFilter=@system-service`, `SystemCallArchitectures=native`, `MemoryDenyWriteExecute`, `LockPersonality`, `RestrictNamespaces`, `ProtectKernelTunables` (sysfs stays readable), `ProtectKernelModules`, `ProtectProc=invisible`, `UMask=0077`, `ReadOnlyPaths=/sys/class/powercap /sys/devices/virtual/powercap`.

The user daemon's unit (`data/systemd/user/drainscope.service`) uses only hardening that works in a user manager without user namespaces: `NoNewPrivileges`, seccomp (`SystemCallFilter=@system-service`), `RestrictAddressFamilies=AF_UNIX`, `MemoryDenyWriteExecute`, `LockPersonality`, `RestrictRealtime`, `UMask=0077`, plus low `Nice`/`CPUWeight`. Its `systemd-analyze security` score (6.5) reflects the missing mount-namespace options, which would imply `PrivateUsers=` and hide the user's other processes' `/proc/<pid>/fd`, breaking GPU attribution. The daemon holds no privileges either way. `RestartPreventExitStatus=3` stops systemd from restarting a daemon that found the database lock taken. The daemon also stops (status 0, state saved) when its session bus closes, so one D-Bus-activated outside systemd, e.g. under `dbus-run-session`, doesn't outlive its session.

Activation: D-Bus-activated (`Type=dbus`, `BusName=`), exits after 60 s without callers. It costs nothing when unused and never needs `systemctl enable`.

Access control:
- **D-Bus policy:** any local user may call `Sampler1`, but every call is authorized via **polkit** action `io.github.khaledsaeed18.Drainscope.read-energy` (`allow_active=yes`, `allow_inactive=no`, `allow_any=no`). Only the user at the physical seat can sample, which keeps SSH users and other sessions out of the side channel. Authorization is cached per sender unique name.
- **Side-channel mitigation:** per-user (UID) minimum interval of 1000 ms, so reconnecting doesn't reset it, and readings quantized to 10 mJ (≈ 0.1% error at 1 s / 10 W). This is far coarser than the sampling rates the Platypus attack needs. The threat model is documented in `docs/privilege-model.md`.

SELinux: the sampler initially runs as `unconfined_service_t` (default for `bin_t` services). M5 adds a confined `drainscope_sampler_t` policy module allowing only powercap sysfs reads and D-Bus.

Data privacy: all data is local, per user, `0600`. No network in any component (`IPAddressDeny=any` on the user unit too). Other users' activity is aggregated into `other-users` and never broken down.

---

## 6. MVP milestone (M0 + M1)

Each task is small and has a concrete **Verify** step. Order matters: the model first (pure, learnable Rust), privileged code later.

### M0 — Toolchain and feasibility spike (1–3 days)

**Status: done (2026-10-05). Outcome: go, with model changes. See [ADR 0001](docs/adr/0001-feasibility.md).**
| # | Task | Verify |
|---|---|---|
| 0.1 | Install toolchain: `rustup` (stable + clippy + rustfmt), `gcc` (linker + bundled SQLite), `stress-ng`, `rpm-build`, `rpmdevtools`, `meson`. Later (M4): `clang`, `bpftool`. No `dbus-devel` needed (zbus is pure Rust). | `cargo --version`, `stress-ng --version` |
| 0.2 | Scaffold the workspace (empty crates, CI config, lint settings), plus the pnpm workspace with strict tsconfig | `cargo build`, `pnpm -r typecheck` pass |
| 0.3 | Spike: `xtask record-fixture` copies the relevant sysfs/procfs/cgroupfs files into `testdata/<name>/` every N seconds (run once with `sudo` for RAPL) | A 10-minute trace exists on battery and on AC |
| 0.4 | Spike: a throwaway attribution over the trace prints per-app watts | Numbers look sane: Firefox under load > idle apps; Σ ≈ battery draw within ~15% |
| 0.5 | Go/no-go note in `docs/adr/0001-feasibility.md` (domains present, `psys` behaviour, battery update rate) | ADR committed |

### M1 — MVP: daemon + sampler + CLI (≈ 3–4 weeks)

**Status (2026-10-05):** 1.1–1.21 and 1.23 implemented and verified on the dev machine (1.5 merged into 1.4). Overhead (1.19, release build, 5 s ticks, 5 min): 0.38% of one CPU, 8.5 MB RSS. 1.22 validated on battery ([docs/validation.md](docs/validation.md), [ADR 0004](docs/adr/0004-no-conversion-factor.md)): the daemon gave isolated loads 86–97% of their active energy; no battery/RAPL conversion factor. 1.17 verified live: plug/unplug recorded, and the attributed energy over the discharge (5% of capacity) matched the battery's own level drop (99.9% → 94.9%). 1.18 couldn't be verified live because suspend is disabled on the dev machine (`suspend.target` masked); the delay inhibitor is confirmed registered with logind, and the sleep logic is unit-tested.

**M1 is complete.**
| # | Task | Verify |
|---|---|---|
| 1.1 | `model`: domain types, `ConsumerKey`, identity rules (cgroup path → key, systemd unescape) | Table-driven unit tests over real cgroup names from this machine |
| 1.2 | `model`: snapshot diffing incl. counter wraparound, cgroups appearing and disappearing, PID reuse (pid + starttime) | Unit tests + proptest |
| 1.3 | `model`: attribution v1 + true-idle floor learning + `psys` plausibility check + windowed battery reconciliation | Proptest: conservation (Σ = measured ± ε), non-negativity, determinism; `psys` rejected on the M0 trace |
| 1.4 | `sys`: cgroup v2 walker (leaf `cpu.stat`) over a `SysRoot` | Fixture tests; live test (`#[ignore]`) on this machine |
| 1.5 | ~~`sys`: `/proc/stat` totals + kernel residual~~ Merged into 1.4: the root cgroup's `cpu.stat` is system-wide busy time, and kernel time is the root's own time in `model::delta` | Covered by 1.2 and 1.4 tests |
| 1.6 | `sys`: DRM fdinfo scanner (find `/dev/dri/*` fds once per pid and rescan only new pids at a low rate; re-read only those `fdinfo`s; dedupe by `drm-client-id`, **never charging fd brokers** like PID 1 and logind) | Fixture test incl. a shared client held by PID 1, logind and gnome-shell; live check shows Firefox render ns increasing |
| 1.7 | `sys`: power_supply reader (multi-battery sum; status mapping) | Fixture tests incl. BAT0+BAT1 |
| 1.8 | `sys`: powercap reader (domain discovery, `max_energy_range_uj` wrap) — used by the sampler | Fixture tests |
| 1.9 | `dbus`: Sampler1 + Monitor1 XML, zbus types, error enums | Round-trip test over a zbus p2p connection |
| 1.10 | `sampler` bin: Sampler1 server, rate limit, quantization, idle exit | Unit tests for limiter/quantizer; p2p integration test |
| 1.11 | `sampler`: polkit check (`CheckAuthorization` via zbus proxy to `org.freedesktop.PolicyKit1`) + per-sender cache | Manual: allowed from the GNOME session; denied from `ssh localhost` / `machinectl shell` |
| 1.12 | `data/`: sampler unit, D-Bus activation file, bus policy, polkit policy | `systemd-analyze security --offline=yes` ≤ 2.0; `busctl call` works after install |
| 1.13 | `store`: schema + migrations + `Store` repository | In-memory SQLite tests; migration-from-empty test |
| 1.14 | `store`: rollups (raw → minute → hour) + retention | Tests: rollup sums equal raw sums |
| 1.15 | `daemon`: tick loop composition, degraded modes, `Status` | Replay test: recorded trace → golden attribution output |
| 1.16 | `daemon`: Monitor1 server (`GetSummary`, `GetUsage`, `Tick`) | p2p tests; `busctl --user call` on the live daemon |
| 1.17 | `daemon`: power events (plug/unplug from `power_supply` status transitions, read each tick; UPower isn't needed) and "since unplug" | Manual: unplug → `GetSummary` resets |
| 1.18 | `daemon`: logind delay inhibitor + sleep sessions | Manual: suspend 2 min → a session row appears with Wh lost |
| 1.19 | `daemon`: self-accounting + overhead budget | `drainscope report` shows `drainscope`; < 0.5% CPU average, < 30 MB RSS |
| 1.20 | `cli`: `status`, `top` (live via `Tick`), `report --since <unplug\|1h\|24h\|7d> --by app\|unit\|kind` | Snapshot tests of rendering; manual run |
| 1.21 | `cli`: `doctor` (domains, sampler reachable, polkit, cgroup layout, DRM fdinfo, batteries) | Prints actionable diagnostics on this machine |
| 1.22 | `xtask validate`: scenarios (idle 10 min; `stress-ng --cpu {1,2,4}` and `--cpu 1 --cpu-load {25,50,100}` in dedicated transient scopes; GPU load via a WebGL page), launched by the harness itself (not a pasted shell line), recording RAPL and battery together | `docs/validation.md` with closure error %, linearity, leakage to other consumers, and the conversion-overhead factor (ADR 0001) |
| 1.23 | Dev install path: `xtask install-dev` (units, policy, binaries to `/usr/local`) and uninstall | Fresh install → `drainscope report` works after one unplug cycle |

**MVP done when:** on this laptop, unplugging for an hour and running `drainscope report --since unplug` gives a ranked list whose total matches the battery % used within the documented error, with no component running as root, the sampler scoring ≤ 2.0 exposure, and the daemon costing < 0.5% CPU.

---

## 7. Later milestones

| Milestone | Scope |
|---|---|
| **M2 — GNOME Shell extension** (done) | Quick-settings section "Battery usage since unplug" (top 5 + "Open drainscope"); live refresh via `Tick`; app names and icons via `Shell.AppSystem`; GNOME 50 and 51 ESM; strict TS with validated GVariant decoders; EGO-compliant (no work outside `enable`, full cleanup in `disable`). |
| **M3 — Desktop app + sleep and health** (done, except foreground vs background) | libadwaita app: since-unplug / 24 h / 7 d views, stacked timeline, per-app detail (CPU vs GPU, foreground vs background), sleep sessions with wake reason, battery health chart (`energy_full` vs design, cycle count). |
| **M4 — eBPF precision** (done for wakeups, network bytes and model v2; ADRs 0006, 0007; exit capture remains) | Per-app wakeups (timer/sched tracepoints) to find idle-drain culprits; capture short-lived processes at exit; per-cgroup network bytes (cgroup_skb) for a Wi-Fi share of "devices"; model v2 weighting CPU time by per-CPU frequency. eBPF runs in a sibling of the sampler (`drainscope-probe`) and exports only aggregated per-cgroup counters. As built, model v2 charges network-softirq time to apps by bytes; frequency weighting was not needed on the measured hardware. |
| **M5 — Hardening and distribution** (SELinux, packaging, GitHub releases and COPR done; EGO and Flatpak pending, see [docs/distribution.md](docs/distribution.md)) | SELinux policy modules for the sampler and the probe; COPR stable channel; EGO publication; Flatpak for the app; AMD support (no `psys`, different domains) tested on a donor machine or in CI with fixtures; docs site and a write-up of the model and validation. |
| **Later ideas** | Backlight-weighted display share; per-app notifications ("Slack has used 8% in the background"); export to CSV/JSON; Prometheus textfile output for homelab users; KDE Plasma widget (the D-Bus API makes it a pure UI addition). |

---

## 8. Testing strategy

| Layer | How |
|---|---|
| `model` | Unit tests + **proptest invariants**: energy conservation, non-negativity, wraparound, determinism, model versioning. No I/O, so tests are fast. |
| `sys` | Every reader takes a `SysRoot`. Tests run against **recorded fixture trees** in `testdata/` (captured from this machine by `xtask record-fixture`) plus synthetic edge cases: missing domains, AMD layout, three batteries, unescaped names. Live-hardware tests are `#[ignore]` and run manually. |
| `store` | In-memory SQLite; migration tests; rollup equals raw sum; retention boundaries. |
| `dbus` / bins | zbus **peer-to-peer connections** for contract tests (no bus daemon needed). Polkit is mocked behind a trait in unit tests and tested manually end to end. |
| `daemon` | **Trace replay**: feed recorded snapshot sequences through the full pipeline with fake sources and compare against golden reports. This catches regressions in identity, diffing and attribution together. |
| Accuracy | `xtask validate` on real hardware (battery). Results are versioned in `docs/validation.md` and re-run whenever the model changes. |
| Security | CI runs `systemd-analyze security --offline=yes` on the sampler unit with a score threshold; `cargo deny` for licenses and advisories. |
| TypeScript | `tsc --noEmit` strict, typescript-eslint `strict-type-checked` (bans `any` and unsafe member access/assignments), vitest for `ui/shared` (decoders, formatting, view models). GJS-only glue stays thin and is tested manually in a nested Shell (`dbus-run-session gnome-shell --devkit` on GNOME 49+). |
| CI | GitHub Actions: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `pnpm -r typecheck lint test`, RPM build inside a `fedora:44` container. |

---

## 9. Packaging and distribution

| Artifact | Contents | Channel |
|---|---|---|
| `drainscope` RPM | daemon, CLI, user unit, Monitor1 activation file, D-Bus interface XML | GitHub Releases (v0.1.1); COPR (`khaledsaeed18/drainscope`, Fedora 44, 45, rawhide) |
| `drainscope-sampler` RPM | sampler binary in `/usr/libexec`, system unit, sysusers.d entry, D-Bus system policy + activation file, polkit policy | same; recommended by `drainscope` |
| `drainscope-probe` RPM | eBPF probe in `/usr/libexec`, system unit, sysusers.d entry, D-Bus policy + activation file, polkit policy | same; recommended by `drainscope` |
| `drainscope-selinux` RPM (noarch) | policy modules for the sampler and the probe | same; pulled in by the sampler and probe on SELinux systems |
| `gnome-shell-extension-drainscope` RPM (noarch) + zip | bundled ESM extension | same, plus extensions.gnome.org (pending) |
| `drainscope-app` RPM (noarch) | GJS app, desktop entry, metainfo, icon | same; Flatpak on Flathub later, with `--talk-name=io.github.khaledsaeed18.Drainscope.Monitor` |

Notes:
- The spec (`packaging/drainscope.spec`) builds from a source tarball with vendored crates (`cargo vendor`), so COPR builds offline. `cargo xtask dist` produces the tarball, SRPM and RPMs (packaging/README.md). Fedora's official repositories would need every crate packaged separately (docs/distribution.md).
- The user service is enabled on first run (`systemctl --user enable --now drainscope.service`), documented in the README; `%systemd_user_post` handles presets.
- The sampler is never enabled: D-Bus activation starts it on demand.
- Each release is validated from COPR on every supported Fedora with `packaging/validate-copr.sh` in a toolbox: versions, signatures, file integrity, the database lock and the extension in a nested GNOME Shell of that release.
- Portable to any systemd + cgroup v2 distro; Fedora is the first-class target.

---

## 10. Risks and open questions

| Risk / question | Mitigation |
|---|---|
| Attribution accuracy (frequency and C-state effects, shared caches) | Publish error bars; model v2 with frequency weighting; never present estimates as exact (UI says "estimated") |
| Battery `power_now` lag (measured 6–8 s here) or coarse EC updates on some laptops | Reconcile over ≥ 10 s windows; cross-check with `energy_now` deltas |
| On AC there is no system-level truth | Switch the UI to "energy (Wh)" mode with `psys`, clearly labelled |
| Short-lived processes lost between ticks | Scope-level accounting retains most of it; eBPF exit capture in M4 |
| `psys` / `uncore` missing (AMD, some Intel) or implausible (dev machine: `psys` < `package`) | Domain-optional model with plausibility checks and documented fallbacks; AMD fixtures |
| Collection overhead (M0 recorder: 8% CPU as root) | Cache DRM fd holders, read only own processes, no full snapshots; overhead budget checked in task 1.19 |
| GNOME Shell API churn | List only tested versions in `shell-version` (now `["50", "51"]`); test each new GNOME in a nested shell from a toolbox (docs/distribution.md); small extension surface |
| polkit behaviour for D-Bus-activated system services under SELinux enforcing | Verify in task 1.11/1.12; ADR if a policy tweak is needed |
| ~~Open: reverse-DNS / app ID prefix~~ | Resolved: `io.github.khaledsaeed18` (GitHub `KhaledSaeed18`) |
| ~~Open: license~~ | Resolved: **GPL-3.0-or-later** for everything (GNOME ecosystem norm; EGO requires GPL-compatible extensions) |
