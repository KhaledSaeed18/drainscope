# Attribution model

How drainscope turns measured energy into "who used it". The code is `crates/model` (`attribution`, `activity`, `calibration`, `window`); the version is `MODEL_VERSION`, stored with every window.

## Inputs, every tick (5 s while the app or the Quick Settings menu is open, 15 s otherwise)

- **RAPL energy** (through the sampler), split into **parts**: `core`, `uncore` (the integrated GPU), the rest of the package (`package − core − uncore`), and `dram`. `psys` is used only when it passes a plausibility check (ADR 0001).
- **CPU time** per cgroup (cgroup v2 `cpu.stat`), as "own time": a cgroup's usage minus its children's. Leaves are processes. The root's own time is the kernel, including IRQ and softirq time (`CONFIG_IRQ_TIME_ACCOUNTING`). Inner nodes' own time is processes that exited.
- **GPU time** per DRM client (fdinfo `drm-engine-*`).
- **Battery** discharge power, when on battery.
- **Network counters** from the optional eBPF probe (model v2): bytes per cgroup, and the kernel's time in the network softirqs.
- **Network interrupt threads** (model v3): the CPU time of the network devices' threaded interrupt handlers, from procfs.

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

## Model v3 (ADR 0008)

Some network drivers handle their interrupts in a kernel thread, `irq/<n>-<driver>` (the dev machine's Wi-Fi: `irq/135-iwlwifi`). That time is Kernel's own time but not softirq time, so v2 left it with Kernel. During a download it added about 30% to the softirq time.

v3 adds that thread time to v2's network time, which is then moved by bytes as before:

- The network devices' interrupts are their MSI vectors (`/sys/class/net/<if>/device/msi_irqs/`), or the legacy line (`device/irq`) without MSI. Virtual interfaces have none.
- Their handler threads are the kernel threads named `irq/<n>-…` for those interrupts. The daemon looks them up once a minute, or sooner when one exits.
- Each thread's time is the first field of `/proc/<pid>/schedstat` (nanoseconds). Only threads present at both ends of a tick count, so a driver reload can't produce a bogus delta.

No privileges are needed. Drivers that handle interrupts directly (the dev machine's Ethernet) have no thread; their work is already hard-IRQ time, which stays with Kernel. `drainscope doctor` lists the threads it found. Without the probe there are no bytes to share by, so v3 also attributes exactly like v1.

### Known limitations

- Shares are proportional to CPU time, so low-utilization activity (network processing, timers) is charged less than its marginal cost. Each burst wakes the package from deep idle, which costs more per CPU-second than the average. Measured: downloads get about 20% of their RAPL increase under v2 (5% under v1), while large CPU loads get 79–90% (ADR 0007). v3 moves about 30% more network time; its measured effect is pending a battery run of `validate --activity`.
- Hard-IRQ time of network devices (drivers without a handler thread) remains Kernel's: the kernel doesn't report it per interrupt.

## Validation

- `cargo xtask validate`: CPU loads; results in [validation.md](validation.md).
- `cargo xtask validate --activity`: timer and download loads, measured with the probe; results in [validation-activity.md](validation-activity.md), including how much of a download's RAPL increase the running daemon's model charges it (about 20% under v2; see the known limitations).
