# 0004 — No battery/RAPL conversion factor in model v1

- Status: accepted
- Date: 2026-10-05

## Context

ADR 0001 left one question open: does each watt RAPL measures cost more than a watt at the battery (voltage-regulator and conversion losses)? If so, part of "display & devices" should be charged to active consumers. Task 1.22 built `cargo xtask validate` to measure `k = Δbattery / Δ(package + dram)` under known CPU loads. It was run twice on the dev machine, on battery, with the backlight logged; results are in [docs/validation.md](../validation.md).

## Findings

- **RAPL is linear and repeatable:** ΔP ≈ 2.64 W per busy CPU in both runs (R² ≈ 0.96).
- **k < 1 at every clear load:** 0.69 / 0.78 / 0.88 for 1 / 2 / 4 busy CPUs in the second run, and 0.79 / 0.88 / 0.90 in the first. The battery reported a *smaller* increase than RAPL. That is physically impossible, so at least one sensor is miscalibrated: client-Intel RAPL is partly model-based, and `power_now` comes from the embedded controller.
- **The display didn't cause it.** GNOME dimming is off on this machine, and the backlight stayed at 742/6818 for the whole second run.
- **Attribution holds up anyway.** The running daemon gave the load's own scope 86–97% of each full-load phase's active energy (78% at 50% load, where background activity is relatively larger).

## Decision

- **No conversion factor.** Model v1 keeps `devices = battery − (package + dram)` over windows of ≥ 10 s. When RAPL exceeds the battery (more likely under heavy load on this machine), the window is measured against RAPL and the excess is recorded as `shortfall` (already implemented in `drainscope_model::window`), never as negative device energy.
- `MODEL_VERSION` stays 1: stored meanings don't change.
- Telling which sensor is wrong needs an external power meter. If one becomes available, rerun `cargo xtask validate` alongside it and revisit.

## Consequences

- Battery percentages are as accurate as the embedded controller's `power_now`. Per-app shares are as accurate as RAPL plus CPU-time attribution, which validated at 86–97% for isolated loads.
- Heavy-load windows can show non-zero shortfall on this machine. That is expected, not a bug.
