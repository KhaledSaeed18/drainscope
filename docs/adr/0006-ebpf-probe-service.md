# 0006 — Where eBPF runs (M4)

- Status: **proposed**, awaiting the maintainer's approval (it changes the privilege model)
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
- **Interface:** a new `io.github.khaledsaeed18.Drainscope.Probe1` on the system bus (Subscribe, then a per-tick Counters signal to subscribers only), contract-tested like Sampler1.
- **Model:** wakeups and exit capture are shown as their own metrics first. Using them, or frequency, in attribution is model v2 (`MODEL_VERSION` 2) and needs a new `xtask validate` run.

## Consequences

- There are two privileged services, each with one narrow job. The sampler keeps its profile.
- Packaging gains a `drainscope-probe` subpackage, and the user daemon degrades gracefully without it, as it already does without the sampler.
- Approval needed: the new capabilities (`CAP_BPF`, `CAP_PERFMON`, and `CAP_NET_ADMIN` for network bytes), the new system user, and the new D-Bus interface.
