# 0011 — Foreground and background energy per app

- Status: accepted (approved by the maintainer after 0.1.3); implemented and validated 2026-10-10
- Date: 2026-10-10

## Context

PLAN's M3 promises per-app detail with "foreground vs background", the last open M3 item, and its ideas list a notification like "Slack has used 8% in the background". The question users ask is: did this app use the battery while I was using it, or behind my back?

What the daemon can and can't see (checked on GNOME 50, 2026-10-10):

- GNOME doesn't tell ordinary session processes which window is focused: `org.gnome.Shell.Introspect.GetWindows` and `GetRunningApplications` answer `AccessDenied` (they are reserved for the portal). Polling is out anyway.
- The drainscope Shell extension runs inside GNOME Shell, where `Shell.WindowTracker.get_default()` has `focus-app` and a `notify::focus-app` signal: the focused app, event-driven, a few changes a minute.
- Mutter's `org.gnome.Mutter.IdleMonitor` is readable by the daemon, but "idle" doesn't mean "not in use" (a film plays in a focused player while nobody touches the keyboard), so this design doesn't use it.

So the extension reports focus, and the daemon classifies energy. Without the extension (another desktop, or it's disabled), the split is unknown, never guessed.

## Decision

**Meaning.** An app's energy is *foreground* for the time one of its windows has focus, *background* for the rest of the time the focused app is known, and *unknown* while it isn't. Only `app:*` consumers are split. Terminal tabs (`term:*`) are not: the extension can't tell which tab has focus, so a build in another tab would be miscounted. Services, the kernel and so on have no windows.

**Reporting focus (extension → daemon).** Monitor1 gains, additively:

- `SetFocus(s app_id)`: the focused app's ID (`org.mozilla.firefox`, the desktop ID without `.desktop`), or `""` when nothing has focus (desktop, Overview, lock screen). The extension calls it on `notify::focus-app`, at `enable()`, and again whenever the daemon's bus name gets a new owner (daemon restarted).
- `EndFocus()`: focus is no longer reported (the extension was turned off), so treat it as unknown from now on. `disable()` calls `SetFocus("")` when the screen is locking (`Main.sessionMode.isLocked`: locked means nothing is in use) and `EndFocus()` otherwise.
- The daemon accepts both only from the GNOME Shell process, where extension code runs: the caller's PID (`GetConnectionUnixProcessID`) must be the PID of `org.gnome.Shell`'s owner. Comparing processes, not connections, holds whichever connection the extension's calls use. Other callers get `AccessDenied`, so no other program can fake the split. When that owner changes or disappears (Shell restarted or crashed), focus becomes unknown.
- Only the app ID crosses the bus: no window titles, no PIDs.

**Classifying energy (daemon).** The daemon keeps the focus timeline in memory (state changes with their `CLOCK_MONOTONIC` times). For each tick and each `app:*` consumer: foreground = energy × (seconds focused in the tick ÷ tick length), unknown = energy × (seconds unknown ÷ tick length), background = the rest. Energy within a tick is assumed even, which is good enough at 5 s ticks (the extension's own calls keep the daemon at 5 s while anyone is working with the menu or app open; at the 15 s idle tick the user isn't looking at drainscope, but focus changes are still timed exactly). Focused seconds per app are recorded too: that's screen time, which the app view can show next to the energy. Attribution itself doesn't change, so `MODEL_VERSION` stays.

**Storage.** A migration adds `focus_raw (window_id, consumer_id, foreground_j, unknown_j, focused_ms)` next to `usage_raw`, rolled up into `focus_minute` and `focus_hour` exactly like usage, with the same retention. Only app consumers get rows, and every app usage row has one: the migration gives history from before it rows with all of its energy unknown, so background never includes energy recorded before focus was tracked. (Columns on the usage tables would have meant migrating every existing row; a separate table keeps the hot tables untouched.)

**Reading it (Monitor1, additive).** `GetFocus(x since, x until, s power_source) → a(sddt)`: per app key, foreground J, unknown J, focused seconds. Clients join it with `GetUsage` rows; background = total − foreground − unknown.

**Showing it.**

- App detail page: "While in use: 2.1 Wh over 35 min · In the background: 0.4 Wh", with a note when part is unknown.
- Usage list: a subtitle "mostly in the background" when background is at least half of a row's energy and at least 0.1 Wh.
- CLI: `report` gains foreground and background columns for apps; `--format json|csv` gains `foreground_joules`, `background_joules`, `unknown_joules` and `focused_seconds`.
- Later, per-app notifications can use it ("Slack has used 8% of your battery in the background").

## Privacy

This records which app had focus, per minute after rollup: screen time. It stays on the machine like the energy history, is served only on the user's own session bus, and is pruned on the same schedule. Only app IDs are recorded, never window titles. The README's privacy notes say so.

## Consequences and limits

- Needs the Shell extension, so it's GNOME-only for now (KDE's KWin and wlroots' foreign-toplevel protocol could report focus later through the same `SetFocus`).
- Apps whose windows map to no desktop ID (`window:<n>` in Shell) can't be matched to a consumer: the extension reports nothing focused, so their own energy isn't split and other apps' energy meanwhile counts as background. Where an app's cgroup scope name differs from its desktop ID, the app consumer and the focus report won't match; the identity rules' tests should cover the common cases (GNOME and Flatpak launches).
- A new Monitor1 addition and a schema migration: both need the maintainer's approval (CLAUDE.md). No privilege changes: the daemon and extension stay unprivileged.
- The extension changes, so a new extensions.gnome.org review.

## Validation

Unit tests for the timeline split (focus changes inside a tick, unknown spans, `EndFocus`, a Shell restart) and the migration and rollups (focus sums equal raw sums). An integration test over a peer-to-peer bus for the caller check. On the machine: two apps with similar steady CPU load, focus alternated on a known schedule for 10 minutes; each app's foreground share should match its focused share within 5 percentage points, and totals must be unchanged from usage.

Estimated work: about two to three days across daemon, store, extension, app and CLI.

### Status (2026-10-10)

- Unit tests: the timeline and split (`model::focus`), the engine carrying each window's split, migration 3's backfill, and the rollups at all three resolutions with pruning.
- Peer-to-peer integration test: `SetFocus` and `EndFocus` from a caller that isn't the Shell get `AccessDenied` and leave focus unknown; `GetFocus` round-trips.
- On a private bus with the new daemon: `SetFocus` was refused before the test process owned `org.gnome.Shell`, accepted after, and refused from another process of the same user. Focused time per window matched the schedule to the tick (15 s, then 136 ms for a report just after a tick).
- On the machine (2026-10-10, AC, dev daemon and extension in the real GNOME 50 session): two test apps with the same steady load (a thread busy 10 ms of every 20 ms, about half a core each), each in its own `app-gnome-…` scope with a desktop entry. Focus alternated every 60 s for 10 minutes, five times each way, by reopening the app's window (GNOME doesn't let an app without user input take focus with `present()`, but a new window gets it). All ten switches reached the daemon.

  | App | Energy | In use | Background | Focused | In-use share | Focused share |
  |---|---|---|---|---|---|---|
  | A | 168.1 J | 79.8 J | 88.3 J | 280 s | 47.5% | 47.9% |
  | B | 168.1 J | 88.3 J | 79.8 J | 304 s | 52.5% | 52.0% |

  Each app's in-use share matched its focused share within 0.6 points (criterion: 5); nothing was unknown, and in use plus background equals each app's usage total.
