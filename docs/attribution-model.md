# Attribution model

How drainscope turns measured energy into "who used it". The code is `crates/model` (`attribution`, `activity`, `calibration`, `window`); the version is `MODEL_VERSION`, stored with every window.

## Inputs, every 5 s tick

- **RAPL energy** (through the sampler), split into **parts**: `core`, `uncore` (the integrated GPU), the rest of the package (`package − core − uncore`), and `dram`. `psys` is used only when it passes a plausibility check (ADR 0001).
- **CPU time** per cgroup (cgroup v2 `cpu.stat`), as "own time": a cgroup's usage minus its children's. Leaves are processes. The root's own time is the kernel, including IRQ and softirq time (`CONFIG_IRQ_TIME_ACCOUNTING`). Inner nodes' own time is processes that exited.
- **GPU time** per DRM client (fdinfo `drm-engine-*`).
- **Battery** discharge power, when on battery.
- **Network counters** from the optional eBPF probe (model v2): bytes per cgroup, and the kernel's time in the network softirqs.

Cgroups map to **consumers** (apps, terminal tabs, services, sessions, Kernel, …) by the identity rules in `model::cgroup`.

## Model v1

1. **Idle floor.** For each part there is a learned floor: the median power of quiet intervals, kept as a histogram per power source. Each part's energy up to its floor is charged to *Idle*.
2. **Active energy.** Each part's energy above its floor is shared by activity weights. `uncore` is split by GPU time. `core`, the package rest and `dram` are split by CPU time. Without any activity the energy goes to Idle.
3. **Platform.** With a trusted `psys`, `psys − package − dram` is charged to *Platform*.
4. **Battery reconciliation.** Over windows of at least 10 s on battery, `battery − (package + dram)` is *Display & devices*. If the battery reports less than RAPL, the shortfall is recorded, not attributed. No conversion factor is applied (ADR 0004).
5. **Battery-only mode.** Without the sampler, battery power above a learned battery idle floor is split by CPU time.

The sum over consumers always equals the measured energy (property-tested).

## Model v2 (ADR 0007)

v1 charges the network stack's work to Kernel, because it runs in softirq context, outside any app's cgroup. Measured on the dev machine, it costs 211 mW per MB/s ([validation-activity.md](validation-activity.md)).

v2 adds one step, before step 2. The probe's network softirq time for the tick (`NET_TX` + `NET_RX`) is taken out of Kernel's CPU-time weight, and given to consumers in proportion to the bytes they sent and received in the tick (`Activity::charge_network`). The step:

- never moves more than Kernel's own time;
- keeps the total CPU time exact, so the energy split stays conserving;
- uses only measured time, so it needs no per-byte coefficient and holds on other hardware.

Without the probe, or across a probe restart (new counter generation), v2 attributes exactly like v1. The trace-replay golden test, which has no probe data, gives identical numbers.

## Validation

- `cargo xtask validate`: CPU loads; results in [validation.md](validation.md).
- `cargo xtask validate --activity`: timer and download loads, measured with the probe; results in [validation-activity.md](validation-activity.md). Under v2, a download's "Charged" watts should reach most of its RAPL increase, as CPU loads already do (79–90%).
