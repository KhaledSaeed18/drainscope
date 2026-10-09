# 0010 — Exit capture: CPU time of cgroups that come and go between ticks

- Status: proposed (design; implementation not approved yet)
- Date: 2026-10-10

## Context

A process's CPU time stays in its cgroup's `cpu.stat` after it exits, so exits inside long-lived cgroups (a terminal tab, an app's scope, a service) are already attributed correctly. The gap is a **cgroup created and removed between two ticks**: a `systemd-run` unit, a helper app launched for a moment, a D-Bus-activated service that exits quickly. Its time folds into its parent's counter, and the model charges the parent's own time to `Exited(slice)`, shown as "Exited processes (app.slice)" (`model::activity`). ADR 0006 planned capturing it as its stage 2.

Measured on the dev machine (2026-10-10):

- The `exited:*` consumers hold **about 0.5% of attributable energy** (0.47% of the whole history, 0.55% of the last 24 h), mostly `exited:app.slice`. That was with 5 s ticks; with the 15 s idle tick more short-lived cgroups fall between ticks, so expect somewhat more.
- A quiet minute removed no cgroups. A one-off `systemd-run --user` command created `app.slice/run-p…-i….service`, which lived 0.6 s and went to `exited:app.slice`. `toolbox run` creates none (it runs inside the container's existing cgroup).

So this is a precision improvement of around 1% of attributable energy, mostly for people who run many short commands as units. Its cost has to stay well below that.

## Options

- **A. Account CPU time per cgroup in eBPF on every context switch.** Exact, and would catch everything, but puts work on the scheduler's hottest path (thousands of switches a second) to recover about 1%. Rejected.
- **B. Read a cgroup's final CPU time when it is removed** (`cgroup:cgroup_rmdir`). Rare events, but the final value lives in per-CPU rstat counters that may not be flushed yet at removal, and reading them means following kernel-internal structures that changed in 6.15/6.16. Too fragile.
- **C. Record each exiting thread's CPU time by cgroup** (`sched:sched_process_exit`, ADR 0006's plan). Recommended:
  - The probe adds the exiting thread's `se.sum_exec_runtime` to a per-cgroup counter keyed by its default-hierarchy cgroup ID, in a bounded LRU map. Exits are tens per second, so the cost is negligible.
  - The same program records, the first time it sees a cgroup, its parent's ID and its own name (kernfs, up to 64 bytes). The probe can then name a cgroup that was created and removed between two of its own cgroup walks; parents persist and are already known. `kernfs_node.parent` became `__parent` in 6.15 (the dev machine's 7.2 kernel has `__parent`), so the program checks both with CO-RE.
  - Probe1 gains an additive `ReadExits`: (monotonic ns, generation, (cgroup path, cumulative ns of exited threads)), with the same polkit action and visibility rules as `ReadWakeups`. If it lands before `ReadAll` ships in a release, `ReadAll` carries it instead, keeping one call per tick.
  - Nothing new crosses the boundary beyond what ADR 0006 allowed: per-cgroup totals and cgroup names, which `/sys/fs/cgroup` already shows. No PIDs or command lines. No new capabilities: tracepoints need only the probe's `CAP_BPF` and `CAP_PERFMON`.
- **D. Only relabel.** Show `exited:app.slice` as "Short-lived apps and commands". Free, but attributes nothing better.

## Model change (with C)

Each tick, for every cgroup C missing from the current snapshot that has exit time E_C:

- C wasn't in the previous snapshot either (created and removed within the tick): its time this tick is the growth of E_C.
- C was in the previous snapshot with usage U_prev: all its threads have exited since, so its lifetime total is E_C and its time this tick is E_C − U_prev, floored at zero. Floored values cover a probe that started after C did and so missed earlier exits.

That time moves from the parent's own time (`Exited(slice)`) to the consumer C's path resolves to under the existing identity rules (for example `app-gnome-<app>-<pid>.scope` → that app), never more than the parent's own time. Totals stay exact. Without the probe, or across a probe restart, nothing moves and attribution is unchanged. Any identity rules for transient units (`run-*.service`) need test cases with real paths, per CLAUDE.md. It changes attribution output, so it bumps `MODEL_VERSION`.

## Validation

A `validate` phase that starts short CPU-bound `systemd-run --user --wait` units for 45 s. Today nearly all their energy goes to `exited:app.slice`; with capture, at least 90% should reach the units' consumers, with totals unchanged and the probe's CPU cost not measurably above its current one.

## Recommendation

Implement C after model v3 is validated and released and model v4 is decided (ADR 0009). It needs approval for the new probe program and the Probe1 method, like ADR 0007's. Until then, D is a cheap improvement to the label: done on 2026-10-10 ("Short-lived apps and commands" for `app.slice`, "Short-lived system services" for `system.slice` and `system-*.slice`, "Short-lived processes (slice)" otherwise, in the app, the extension and the CLI).
