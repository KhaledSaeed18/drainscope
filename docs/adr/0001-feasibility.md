# 0001 — M0 feasibility: go, with model changes

- Status: accepted
- Date: 2026-10-05

## Context

M0 asked whether per-app energy attribution is credible on real hardware before building the architecture in PLAN.md. Measurements were taken on the development machine: Lenovo V330-15IKB, i7-8550U, Fedora 44, kernel 7.1.6, GNOME 50.3, two batteries (BAT0, BAT1).

Traces (git-ignored, in `testdata/local/`):

| Trace | Privileges | Length | Content |
|---|---|---|---|
| `m0-battery` | root (RAPL) | 600 s | Normal use on battery: Firefox with video, terminal, editor |
| `m0-controlled` | root (RAPL) | 330 s | 2 min of true idle (screen on, apps closed). The load phases failed (shell paste error), so only the idle part is used. |
| `m0-load` | user (no RAPL) | 240 s | Idle, then `stress-ng` at 1, 2 and 4 cores for 40 s each in named transient scopes (`load-Ncpu.scope`) |

Analysis was done with `xtask spike-attribute` and ad-hoc scripts over the traces.

## Findings

### 1. RAPL works, but `psys` is not a platform measurement on this machine
- `package-0`, `core`, `uncore` and `dram` behave as expected.
- `psys` is **below `package` in 299/299 intervals** (median ratio 0.74), which is impossible for a whole-platform measurement. It fits `psys ≈ 0.32 × package + 0.71 W` (r = 0.95). Its correlation with battery power is only weak (0.58; `package`: 0.46). The firmware evidently reports something other than whole-platform power under that name. The rejection rests on its magnitude, not its correlation.
- With `psys` trusted, the spike over-attributed by **+15.3%**. With `psys` rejected, energy balances exactly.

### 2. Battery power is a good whole-system signal, but it lags
- `power_now` changed in 270 of 300 samples (≈ 2 s updates). `energy_now` moves in 10 mWh steps and agrees with integrated `power_now` within 5.6% over 10 minutes.
- After a load step, `power_now` ramps over **6–8 s**. Per-2 s-interval battery values are therefore not aligned with per-interval activity.
- The machine switched batteries during measurement (BAT1 drained to "Not charging", BAT0 took over). Summing over all batteries is required, not optional.

### 3. Where the energy goes at near-idle
- True idle (screen on, apps closed): **package 1.49 W, core 0.19 W, dram 0.37 W, battery 5.10 W**.
- Light use (video in Firefox): package 1.75 W, battery 5.76 W. Only ~0.3 W of the package increase is above true idle; about 63% of battery power is outside package + dram (display, Wi-Fi, storage, conversion losses).

### 4. CPU load costs more at the battery than proportional models assume, and not linearly
- At the battery, the first busy core costs **3.56 W**, and each core costs **2.26 W** when 4 are busy. Fitting `ΔP = b + a × cores` to the 1- and 2-core points gives `b ≈ 1.1 W` (package and platform leaving deep idle) and `a ≈ 1.8 W/core`; this predicts the 4-core point within 6%.
- Conclusion: the cost of waking the platform is caused by activity. It belongs to the active consumers, not to "idle".

### 5. cgroup-based attribution is precise
- During load phases the `load-Ncpu.scope` cgroups received **93–97% of all busy CPU time**. The rest was genuine background work, including the recorder itself.
- Splitting each cgroup's own time as its delta minus its children's deltas accounts for processes that exited mid-interval. Only 0.5 s of 238 s was CPU time from such exited processes, and it is still attributed to their parent slice.
- Reading cgroup files one at a time produced "negative self time" totalling 0.53 s of 238 s (0.2%). Reading children before parents eliminates this by construction.

### 6. Identity rules needed corrections (now in PLAN.md)
- Ptyxis (the Fedora 44 default terminal) uses `ptyxis-spawn-<uuid>.scope`, not `vte-spawn-*`.
- D-Bus-activated apps live in `dbus-:<addr>-<bus name>@<n>.service` and must resolve to `app:<bus name>`.
- The `sudo` session that ran the recorder appeared as `session-c8.scope`; logind session scopes need a rule (`session:<id>`, labelled with their leader process).

### 7. GPU attribution works, with one trap
- i915 exposes per-client `drm-engine-*` time in fdinfo, readable for the user's own processes.
- PID 1 (fd store) and systemd-logind hold duplicates of GNOME Shell's DRM fd, which share its `drm-client-id`. Naive deduplication charged the compositor's GPU time to `init.scope`. Shared clients must never be attributed to fd brokers (PID 1, logind).

### 8. Collection overhead must be designed in
- The recorder used **8.3% of one CPU** (49.6 s over 598 s), mainly from re-scanning every process's fds every 2 s as root. The daemon must cache DRM fd holders (rescan only new PIDs, at a low rate), read only the user's own processes, and never serialize full snapshots.

## Decision

**Go.** Build M1 as planned, with these changes to the attribution model and collectors:

1. **`psys` is optional and must pass a plausibility check** (median `psys/package ≥ 1` over a warm-up window). Otherwise it is ignored and the reason is shown in `Status`.
2. **The idle floor comes from true idle, learned over time** — not from a single trace's low percentile. Candidate: a low percentile of per-domain power over intervals with < 0.25 busy CPUs and no GPU activity, persisted per power source, with an optional `drainscope calibrate` (2 min idle) to bootstrap it.
3. **Energy above the floor goes to active consumers.** Package, core and dram energy above the true-idle floor is split by activity share (CPU time; GPU time for `uncore`). This attributes the platform wake-up cost to whatever caused it.
4. **Battery data is used over windows, not per tick.** Battery energy is reconciled over windows of ≥ 10 s to absorb the 6–8 s lag. "Display & devices" is `battery − (package + dram)` over the same windows.
5. **The UI reports two figures per app**: share of battery used, and share of *attributable* (above-idle) energy. At light load the first is honestly small, because the display dominates. The second still answers "which app is costing me".
6. Collector changes: cgroup reads children-first; DRM clients deduplicated with fd brokers excluded; cached fd-holder discovery; logind session scopes get an identity rule.

## Open questions for M1

- **Conversion overhead factor:** at the battery, one busy core costs 2.3–3.6 W, but RAPL wasn't recorded during the load test. Measure `Δbattery / Δ(package + dram)` under load with RAPL (the first `xtask validate` run). If the factor is consistently > 1, attribute the matching share of "devices" to active consumers as conversion losses.
- **Frequency weighting:** the higher cost of the first core (single-core turbo, package wake-up) means plain CPU-time shares under-weight bursty single-threaded apps relative to parallel ones. Revisit in model v2 (M4) with per-CPU frequency.
- **AC mode** can only use package + dram on this machine, because `psys` is unusable.

## Consequences

- PLAN.md's model section changes accordingly. The plausibility checks, true-idle floor and windowed battery reconciliation become explicit M1 tasks.
- `xtask spike-attribute` is frozen. Its surviving logic is ported into `drainscope-model` and `drainscope-sys` in M1 with tests, and the spike is deleted afterwards.
