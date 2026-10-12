# Failure modes

The 1.0 checklist (ROADMAP "Next", item 3): every missing or failing part gives a clear message
and recovers without a restart. Each case was run once, on 2026-10-12, against `main` at
`37b6488` on the dev machine (Fedora 44, GNOME 50, SELinux enforcing). Every case except 9
ran a scratch daemon on its own session bus (`dbus-run-session`) with a scratch database,
against the real sampler and probe; the system-side changes (masking a service, a polkit
rule, a tmpfs) were made by a root helper and undone afterwards. Case 9 ran in a nested
GNOME Shell.

| # | Case | How it was caused | What the user sees | Recovery |
|---|---|---|---|---|
| 1 | Baseline | — | `status`: "full" within 1 s of start (the first tick runs at once) | — |
| 2 | No sampler | `systemctl mask --now drainscope-sampler` | `status`: battery only. `doctor`: ✗ sampler, with the activation error. One warning in the log. | 33 s after unmasking (30 s retry), no restart; logs "RAPL counters available" |
| 3 | No probe | `systemctl mask --now drainscope-probe` | `wakeups`/`network`: need the probe. `doctor`: ! probe, with the reason and the journal command. | 63 s after unmasking (60 s retry) |
| 4 | polkit refuses | a rules file returning `NO` for drainscope's actions | `doctor`: "polkit refused access to the energy counters" and "… to wakeup counts". `status`, `wakeups` and `network` blamed a missing service instead; since 0.1.7 they say the counters are unavailable and that `drainscope doctor` says why. | 302 s after the rule was removed: refusals back off 5 minutes by design |
| 5 | Database from a newer drainscope | `user_version` set to 999 | The daemon exits with status 4 (the unit doesn't restart on it) and logs "install that version again, or move the database away". The CLI says the daemon didn't answer and points to the journal. | Install the newer version, or move the database |
| 6a | Disk fills while running | a 1 MB tmpfs as the state directory, then filled | Nothing changes for the user: `Tick` goes on, the status is right. One warning: "measuring goes on, and the missing time stays unrecorded". | When space is freed: "writing the history works again", windows are stored |
| 6b | Disk already full at start | the same tmpfs filled before the daemon starts, with and without an existing database | The daemon couldn't open its database ("database or disk is full", or "disk I/O error" for an existing one) and exited. Since 0.1.7 it measures without recording and retries opening the history every minute. | — |
| 7 | Sampler rate limit | `drainscope doctor` six times while the daemon ticks every 5 s | Nothing: the rate-limited read is retried once, no RAPL gap | — |
| 8 | Daemon stopped and restarted under the app | the daemon killed while the app is open, restarted 30 s later | The app shows "drainscope isn't running" with the command to start it; its refreshes don't restart the daemon. | The app shows its data again as soon as the daemon is back |
| 9 | Extension disabled; GNOME Shell restarted | `gnome-extensions disable`/`enable`; the nested Shell killed and started again | Disabling sends `EndFocus` (focus becomes unknown); enabling resumes `SetFocus`. 0 JS errors. | The daemon keeps running across the Shell restart; the new Shell's extension reports focus again |

The extension's "daemon not running" tile ("Battery" / "Not running", headline "Start it with:
systemctl --user enable --now drainscope.service") and that a stopped daemon stays stopped were
checked in a nested shell on 2026-10-12 as well.

Not covered: a sampler or probe crashing mid-read (systemd restarts them on the next D-Bus
call; the daemon treats it like case 2 or 3), and suspend (checked separately on 2026-10-12:
three RTC-woken suspends, see ROADMAP).
