# 0006 — Where eBPF runs (M4)

- Status: accepted (approved by the maintainer on 2026-10-05)
- Date: 2026-10-05

## Context

M4 adds measurements that cgroup files and fdinfo can't give:

- **Wakeups per cgroup** (`sched_wakeup` and timer tracepoints): which app keeps the CPU out of deep idle while doing little work. These apps are invisible in CPU time.
- **Short-lived processes** (`sched_process_exit`): CPU time of processes that start and exit between two 5 s ticks in a scope that disappears with them.
- **Network bytes per cgroup** (`cgroup_skb`), to give apps a share of Wi-Fi energy that is now all in "display & devices".
- **CPU frequency per tick** (`cpu_frequency` tracepoint) for a model v2 that weights CPU time by frequency.

The dev machine has what this needs: kernel 7.1 with BTF (`/sys/kernel/btf/vmlinux`), `CONFIG_BPF_SYSCALL`, `CONFIG_CGROUP_BPF`, clang 22 and bpftool 7.6. Unprivileged BPF is disabled (`unprivileged_bpf_disabled = 2`), so loading programs needs `CAP_BPF`. Tracing programs also need `CAP_PERFMON`, and attaching `cgroup_skb` needs `CAP_NET_ADMIN`.

PLAN says eBPF runs "in the privileged sampler (or a sibling)".

## Options

1. **Extend the sampler** with `CAP_BPF` and `CAP_PERFMON`. This keeps one privileged process, but it ruins the sampler's minimal profile: `systemd-analyze security` 0.6, one capability, and `SystemCallFilter=@system-service` without `bpf()`. A bug in eBPF handling would then sit next to the RAPL counters, and the sampler would also become long-running.
2. **A sibling service, `drainscope-probe`** (recommended). It runs as its own sysusers user with `CAP_BPF CAP_PERFMON` (plus `CAP_NET_ADMIN` only if the network part is approved), is D-Bus activated on the system bus, and exits 60 s after its last subscriber disconnects. It loads programs compiled ahead of time and exports only aggregated counters. It gets its own SELinux module, starting permissive like the sampler's.
3. **No eBPF.** Use `/proc/<pid>/schedstat` and `/proc/interrupts` for rough wakeup estimates, with no network or exit capture.

## Proposed decision

Option 2, with these limits:

- **What crosses the boundary:** per-cgroup totals per tick: wakeups, CPU time of exited processes, and RX/TX bytes. No PIDs, command lines, addresses or packet contents.
- **Who may read what:** polkit `allow_active` like the sampler. A caller gets counters for its own `user-<uid>.slice` and for system services only. System services are already visible through world-readable `cpu.stat`, but network bytes of other users' cgroups are new information, so they are excluded.
- **Build toolchain:** eBPF programs are written in C and compiled with clang at build time (`build.rs`, using vmlinux.h from bpftool). The loader uses `aya` on stable Rust. aya's Rust-side eBPF needs nightly and `bpf-linker`, which would break the stable-only rule.
- **Interface:** a new `io.github.khaledsaeed18.Drainscope.Probe1` on the system bus, contract-tested like Sampler1. Like Sampler1 it is pulled, not pushed: the daemon reads cumulative counters every tick, rate-limited per caller, and the probe exits when nobody has read for 60 s. A generation number changes when the probe restarts and its counters reset.
- **Model:** wakeups and exit capture are shown as their own metrics first. Using them, or frequency, in attribution is model v2 (`MODEL_VERSION` 2) and needs a new `xtask validate` run.

## Staging

1. **Idle wakeups** (`CAP_BPF`, `CAP_PERFMON`): count switches from the idle task to a task, by the cgroup of the incoming task. Each one is the CPU leaving idle for that cgroup, which is what drains a "doing nothing" laptop. The daemon maps cgroup IDs (the cgroup directory's inode number) to consumers and serves recent rates live from memory, with no schema change.
2. **Exit capture**, then **network bytes** (`CAP_NET_ADMIN`), then **model v2**, each in its own change.

## Consequences

- There are two privileged services, each with one narrow job. The sampler keeps its profile.
- Packaging gains a `drainscope-probe` subpackage, and the user daemon degrades gracefully without it, as it already does without the sampler.
- Approval needed: the new capabilities (`CAP_BPF`, `CAP_PERFMON`, and `CAP_NET_ADMIN` for network bytes), the new system user, and the new D-Bus interface.

## Addendum (2026-10-09): `ReadAll`

Approved by the maintainer: Probe1 gains `ReadAll`, additive within version 1. It returns what `ReadWakeups`, `ReadNetwork` and `ReadNetworkTime` return, from one reading of the counters, with a `network_available` flag instead of failing when network counting is missing. The daemon used three calls per tick, each waking the probe and the bus broker; now it uses one. It falls back to the three methods when an older probe answers `UnknownMethod` (e.g. still running across an upgrade) and tries again after the probe restarts. Same polkit action, same visibility rules, its own rate limit.

Measured on 0.1.3 (2026-10-10), medians of 20 readings on one connection: the three calls cost 12 context switches in the probe, 18 in the system bus broker and 9 in the caller; `ReadAll` costs 4, 6 and 3. That is 20 fewer wakeups in the system services per tick: about 1.3 a second at the idle 15 s tick, 4 at the 5 s tick.
