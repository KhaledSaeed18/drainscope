# 0008 — Model v3: charge network interrupt threads with the network softirqs

- Status: accepted (requested by the maintainer on 2026-10-09); validated 2026-10-10
- Date: 2026-10-09

## Context

Model v2 (ADR 0007) moves the kernel's network-softirq time from Kernel to the consumers that caused the traffic, by bytes. Its validation found the rest of the network stack's CPU work in one more place: the Wi-Fi driver's threaded interrupt handler, `irq/135-iwlwifi` on the dev machine. During a 20 s download at 0.8 MB/s it ran 0.19 s, about 30% on top of the 0.66 s of softirqs. It is a kernel thread, so its time is Kernel's own time, and v2 leaves it there.

ADR 0007 called charging it by bytes "a natural v2.1". `MODEL_VERSION` is an integer stored with every window and served over Monitor1, so the change is model **v3**. The marginal-cost model ADR 0007 calls "v3" becomes v4.

## Decision

- Each tick, add the CPU time of the network devices' threaded interrupt handlers to the network softirq time that `Activity::charge_network` moves by bytes.
- Find them without privileges, in `crates/sys` (`irq` module):
  - the network devices' interrupts are their MSI vectors (`/sys/class/net/<if>/device/msi_irqs/`), or the legacy line (`device/irq`) without MSI; virtual interfaces have no `device`;
  - their handlers are the kernel threads named `irq/<n>-…` for those interrupt numbers, found by scanning `/proc/<pid>/comm`. The daemon caches the list and rescans once a minute, or when a thread exits;
  - a thread's time is the first field of `/proc/<pid>/schedstat` (nanoseconds; valid even with `kernel.sched_schedstats=0`), counted only for threads present at both ends of a tick.
- Matching interrupt numbers, not driver names: handler names are whatever the driver passed to `request_irq` (`iwlwifi`, `enp3s0-rx-0`, …).
- `MODEL_VERSION` becomes 3. `drainscope doctor` lists the threads it found.

Not chosen: timing the threads in the eBPF probe (`sched_switch`). It would be more precise for very short runs, but needs a new probe program and Probe1 method for time procfs already reports.

## Consequences

- Apps that stream or download are charged for the driver's interrupt work too. Kernel shrinks by the same amount; totals don't change, and nothing moves more than Kernel's own time.
- No new privileges, capabilities, D-Bus methods or schema changes. The user unit already sees kernel threads' `/proc` entries (no `ProtectProc=`).
- Without the probe (no bytes), v3 attributes exactly like v1, like v2. The trace-replay golden test is unchanged apart from its version header.
- Drivers that handle interrupts directly have no thread; their hard-IRQ time stays with Kernel, since the kernel doesn't report it per interrupt.
- To validate: with the v3 daemon running, `cargo xtask validate --activity` on battery. Downloads should be charged more than v2's 20% of their RAPL increase; record the result here and in `docs/validation-activity.md`.
- First attempt, 2026-10-09: inconclusive. The connection only reached 0.10–0.13 MB/s (every server tried), so the download phases didn't move RAPL measurably. `validate --activity` now checks the download speed first and stops below 0.5 MB/s.

## Result (validated 2026-10-10)

On battery over a phone hotspot (the home connection was capped at 0.13 MB/s), with the v3 daemon running:

- **The mechanism works.** During a 20 s download at 0.82 MB/s, `irq/135-iwlwifi` ran 66 ms next to 299 ms of network softirqs: v3 moves 22% more network time to the app than v2 (ADR 0007 measured +29% on the home network).
- **Downloads were charged 16%** (median; 12–16%) of their RAPL increase ([validation-activity.md](../validation-activity.md)). That isn't comparable with v2's 20%: on this network the download phases added only 0.04–0.08 W, and it didn't follow the traffic (R² = 0.18, against 170–211 mW per MB/s and R² > 0.9 on the home network), so these ratios are mostly noise.
- The extra time is small (about 0.36 CPU-seconds per 20 s here), and CPU-time sharing undercharges light work whatever time it is given (ADR 0009). So v3 is correct and safe (it only moves measured Kernel time, never more than Kernel has), but on its own it doesn't visibly raise a download's charge. It ships in 0.1.3.

