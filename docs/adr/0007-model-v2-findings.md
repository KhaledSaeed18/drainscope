# 0007 — Model v2: charge apps for their network processing, not for wakeups

- Status: accepted (approved by the maintainer on 2026-10-05)
- Date: 2026-10-05

## Context

ADR 0006 staged eBPF counters for wakeups and network bytes as metrics first, with a model v2 to follow if they turned out to matter for attribution. `cargo xtask validate --activity` measured them on the dev machine, on battery (results in [docs/validation-activity.md](../validation-activity.md)):

- Timer loads at 250, 1000 and 4000 Hz. The probe counted 250, 1000 and 3990 wakeups/s for them.
- Downloads capped at 250, 500 and 750 kB/s (achieved 0.26, 0.44 and 0.66 MB/s; the link tops out near 0.75 MB/s).
- Deltas are taken against the rests around each phase. The first run used a single idle baseline at the start, and slow battery drift made the costs look negative.

## Findings

1. **Wakeups are cheap here.** RAPL rises by about 1.2–1.5 mW per 100 wakeups/s in two runs, roughly 12–15 µJ per wakeup. The battery can't resolve it (±0.1 W noise). Model v1 already charged the timer scope 0.01–0.11 W of a 0.11–0.20 W RAPL increase, so a dedicated wakeup term would move under 0.1 W even at 4000 wakeups/s.
2. **Network processing is not cheap, and v1 misattributes it.** RAPL rises by 211 mW per MB/s (R² = 0.974). Almost none of it is the app's own CPU time: the downloading scope used 0.002–0.017 CPUs. The rest is the kernel's network stack in softirq context, which v1 charges to Kernel. The app was charged 0.00–0.01 W. At video-streaming rates (≈ 0.6 MB/s) that is ≈ 0.13 W attributed to the wrong consumer.
3. **The radio's own cost is below the noise** at these rates (power outside RAPL vs. traffic: R² = 0.17). It stays in "display & devices".

## Proposed decision

- **No wakeup term.** Wakeups remain a diagnostic metric (`drainscope wakeups`). They're still worth showing, because high wakeup rates also keep other devices awake.
- **Model v2 moves network processing from Kernel to the apps that caused it, measured rather than estimated:**
  - The probe measures the kernel's CPU time in the network softirqs (`irq:softirq_entry`/`softirq_exit` for `NET_RX` and `NET_TX`).
  - Each tick, the matching share of the CPU energy charged to Kernel is redistributed to consumers in proportion to their bytes.
  - This needs no hard-coded joules-per-byte coefficient, so it holds on other hardware. The 211 mW per MB/s measured here becomes a validation check, not a model constant.
- `MODEL_VERSION` becomes 2. `docs/attribution-model.md` describes the step. `validate --activity` gains a check: the download scope's charge should reach most of its RAPL increase, as CPU loads already do (79–90%).

## Result (validated 2026-10-05)

With v2 installed, `validate --activity` on battery gives:

- Downloads are charged **20%** (median; 15–20%) of their RAPL increase, up from about 5% under v1.
- The network stack costs 170 mW per MB/s (211 in the earlier run). The radio, outside RAPL, now resolves at about 70 mW per MB/s (R² = 0.82).

A 20 s download at 0.8 MB/s shows where the rest of the gap comes from:

- **Measured time is moved correctly, but proportional sharing undercharges small activity.** The network softirqs took 0.66 s, which at 2.68 W per busy CPU accounts for most of the 0.14 W increase. But energy above the idle floor is shared in proportion to CPU time across everything running, and small, bursty activity costs more per CPU-second than that average: each burst wakes the package out of deep idle. Timer loads are undercharged the same way (0.01–0.10 W charged against 0.14–0.22 W). Large CPU loads, which dominate the average, get 79–90%. This applies to any low-utilization consumer, not only networking. A marginal-cost model would be a model v3, with its own ADR.
- **The Wi-Fi driver's threaded interrupt handler isn't counted.** `irq/<n>-iwlwifi` took 0.19 s, about 30% on top of the softirq time. It runs as a kernel thread, so it is Kernel's own time, but not in the network softirqs. Charging network devices' IRQ threads by bytes as well is a natural v2.1.

## Consequences

- Apps that stream or download are charged for the work they cause. Kernel shrinks by the same amount, so totals don't change.
- The probe gains a third small program (softirq timing, no new capability), and Probe1 an additive method.
- Without the probe, v2 behaves exactly like v1.
