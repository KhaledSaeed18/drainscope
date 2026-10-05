# 0005 — AMD processors and GPU drivers other than i915

- Status: accepted
- Date: 2026-10-05

## Context

drainscope was built and validated on an Intel laptop with an i915 integrated GPU. RAPL exposes `package`, `core`, `uncore` (the integrated GPU), `dram` and `psys` there. Model v1 splits the `uncore` part by each client's DRM engine busy time and every other part by CPU time.

Other hardware differs in two ways that matter:

- **AMD Zen** exposes RAPL through the same `intel-rapl` powercap zones (the kernel's `intel_rapl_msr` driver), but only `package` and `core`. There is no `uncore` or `dram` domain. The integrated GPU of an APU draws from the package.
- **GPU drivers** report per-client use differently. i915, amdgpu, msm, panfrost, panthor and v3d report `drm-engine-<engine>: <ns> ns`. Intel's newer xe driver reports only `drm-cycles-<engine>` and `drm-total-cycles-<engine>` (GPU clock ticks). nouveau, radeon and the NVIDIA driver report no per-client busy time.

## Decision

1. **AMD works with model v1 as is.** Without an `uncore` domain, integrated-GPU energy stays in the package remainder and is split by CPU time. GPU-heavy apps are under-charged and CPU-heavy apps over-charged by the GPU's share. A dedicated GPU part for AMD would have to be estimated from engine time instead of measured. That is a model change, and it needs validation on AMD hardware (`cargo xtask validate` plus a GPU load), which we don't have. It waits for a contributor with an AMD laptop.
2. **amdgpu fdinfo is parsed** like i915's (busy nanoseconds summed over engines). Its engine time is still collected but only weights a part named `uncore`, so on AMD it has no effect yet (see 1).
3. **xe is not supported yet.** Cycles are valid relative weights among xe clients, since they share one GPU clock. They can't be mixed with nanosecond-reporting clients, though. Until xe hardware is available for validation, xe clients contribute no GPU time, so the `uncore` part's energy above its idle floor is credited to Idle.
4. **`drainscope doctor` says which case applies:** it names the GPU driver and whether its statistics are used, and notes when RAPL has no `uncore` domain.

## Consequences

- AMD and xe users get correct totals, with a coarser per-app CPU/GPU split, and `doctor` tells them so.
- Fixture tests pin the AMD powercap layout, amdgpu fdinfo and the no-`uncore` attribution, so a future model v2 for AMD has a baseline.
- Supporting xe cycles or an AMD GPU part bumps `MODEL_VERSION` and needs a validation run on that hardware.
