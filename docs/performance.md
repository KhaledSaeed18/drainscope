# Performance

What drainscope costs, measured on 2026-10-12 with v0.1.7 (the COPR packages) on the dev machine
(i7-8550U, Fedora 44, GNOME 50), on AC. ROADMAP "Next", item 5. The 48-hour soak test
(`cargo xtask cost`) adds averages over normal use.

## The daemon per tick

CPU time of `drainscope-daemon` from `/proc/<pid>/stat` over 3 minutes per phase, ticks counted
from the `Tick` signal.

| Phase | Ticks in 180 s | CPU per tick | Of one CPU |
|---|---|---|---|
| Idle: no app, Quick Settings menu closed (15 s ticks) | 12 | 25 ms | 0.17% |
| App open, "Since unplugged" (5 s ticks) | 36 | 37 ms | 0.73% |
| App open, "Last 7 days" (5 s ticks; usage reloaded once a minute) | 36 | 31 ms | 0.61% |

Earlier: 0.28% idle (v0.1.3), and 1.45% with the app open on the last 24 hours (v0.1.5).

Memory (resident): daemon 11 MB (14 MB peak), sampler 5 MB, probe 32 MB including its eBPF
maps. The history was 15 MB after a week of use.

## History queries

### In the store

`cargo test -p drainscope-store --release --test query_times -- --ignored --nocapture`: 30 days
of seeded windows, median of several runs.

| Range | usage | focus | covered time |
|---|---|---|---|
| 1 h | 0.03 ms | 0.01 ms | 0.03 ms |
| 24 h | 0.25 ms | 0.10 ms | 0.58 ms |
| 7 d | 1.8 ms | 0.6 ms | 14–18 ms |
| 30 d | 7.3 ms | 2.5 ms | 61–78 ms |

### Over D-Bus, on the real history

Round trip of each `Monitor1` call from a client to the running daemon, median of 9–21 calls.

| Call | 24 h | 7 d | 30 d |
|---|---|---|---|
| `GetUsage` (by consumer / by kind) | 18 / 17 ms | 22 / 25 ms | 19 / 17 ms |
| `GetCoverage` | 4 ms | 35 ms | 3 ms |
| `GetFocus` | 4 ms | 5 ms | 4 ms |

`GetSummary` 9 ms; `GetSleepHistory` (30 d) 2 ms; `GetWakeups` and `GetNetwork` 2 ms.

Covered time over 7 days is the slowest query: ranges inside the minute rollups' 7-day
retention count distinct minutes across every consumer's rows (`Store::covered_ms`), while
older ranges count hours. The app's 7-day view asks for it once a minute (since 0.1.6), about
0.06% of one CPU while that view is open, and no view goes past 7 days; a faster query would
need a per-minute coverage table (a schema change), not worth it before 1.0.

## The app's start-up

From launching `drainscope-app` to its window showing the usage total, read through AT-SPI:
0.82, 0.90 and 0.90 s. The data is there in the first frame AT-SPI can read.
