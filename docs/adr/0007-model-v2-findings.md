# 0007 — Model v2: charge apps for their network processing, not for wakeups

- Status: **proposed** (needs the maintainer's approval: it changes attribution output)
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

## Consequences

- Apps that stream or download are charged for the work they cause. Kernel shrinks by the same amount, so totals don't change.
- The probe gains a third small program (softirq timing, no new capability), and Probe1 an additive method.
- Without the probe, v2 behaves exactly like v1.
