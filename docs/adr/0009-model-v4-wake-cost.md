# 0009 — Model v4: charging the cost of waking the machine

- Status: proposed (research; decision pending the data described below)
- Date: 2026-10-09

## Context

Models v1–v3 split the energy above the idle floor by CPU time (and GPU time for the integrated GPU). That undercharges light, bursty work. On the dev machine, downloads get about 20% of the RAPL increase they cause and timer loads 7–50%, while large CPU loads get 79–90% (ADR 0007, [validation-activity.md](../validation-activity.md)).

The hypothesis was that energy above the floor has two parts: **work**, roughly proportional to busy CPU time, and **wake overhead**, the package leaving its deep idle states, which costs nearly the same whether the CPU then works for 10 µs or 10 ms. A CPU-time split hands the overhead to whoever computes most, not to whoever wakes the machine.

## Findings so far (2026-10-09)

From the raw samples of earlier `validate` runs on battery (one CPU run, three activity runs; phases compared with the rests around them), and an idle-state count on AC:

1. **A busy CPU's price is not constant.** One fully busy CPU added 4.5 W, two added 3.2 W each, four 2.8 W each. The first busy core pays for waking the package and runs at the highest turbo clock.
2. **Timer loads cost about the same at any rate.** 250, 1000 and 4000 Hz timers added 0.11–0.22 W. After charging their CPU time at the price above, the remainder per idle exit falls from 290–380 µJ at 250 Hz to below zero at 4000 Hz. **There is no constant price per wakeup**, so a "joules per idle exit" term would be wrong.
3. **Downloads barely wake the CPU themselves** (2–80 idle exits/s for the downloading scope). Their cost is the kernel's network work, which models v2 and v3 already move to them by bytes.
4. **Power follows where idle time is spent, not how often idle states are entered.** Counting entries into each cpuidle state: a 250 Hz timer added about 750 entries/s into C8 and deeper; a 1000 Hz timer added about 2700/s, almost all into C1E–C6 and few deep ones, yet cost as much or more; 4000 Hz added about 6700/s, mostly C1E and C3. Frequent wakers keep CPUs in shallow states, which draw more while idle.

## Options

- **A. Split the overhead by share of idle exits.** Each tick: work = busy CPU time at a learned price (findings 1); overhead = active energy − work, floored at zero; overhead is split by each consumer's share of the probe's idle exits; work by CPU time as now. For any cost that depends only on the machine's total wake activity, the Aumann–Shapley fair division gives exactly the share of exits, whatever the cost's shape, so finding 2 doesn't rule this out. Needs the probe; without it, attribution stays v3. No new privileges or interfaces.
- **B. Weight exits by the idle state they end.** Like A, with exits from deep states weighted more, from a probe program on `power:cpu_idle`. A new probe program and a Probe1 change; only worth it if A falls short.
- **C. Charge displaced residency.** Estimate how much deep-idle time each consumer takes from the package. The closest to finding 4, but nothing reports it per consumer without heavy tracing.

## Next

`validate --activity` now also records the machine's total idle exits and each cpuidle state's entries and time per sample. The next battery run (also needed for model v3, ADR 0008) gives the data to replay option A offline: what each timer and download phase would be charged, against what it adds to RAPL.

Proposed success criteria for adopting a v4: timer and download loads charged at least 60% (median) of their RAPL increase; CPU loads stay at 79–100%; the energy split stays exact (property-tested); without the probe, identical to v3.
