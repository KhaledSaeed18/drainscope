# Troubleshooting

Start with `drainscope doctor`. It checks each part and prints what to do about anything that fails (✗) or is missing (!).

## What `drainscope doctor` reports

| `drainscope doctor` says | What it means, and what to do |
|---|---|
| ✗ the drainscope daemon isn't running | It isn't enabled: `systemctl --user enable --now drainscope.service`. The Quick Settings tile says "Not running" and the app says "drainscope isn't running" in this case too. |
| ✗ the drainscope daemon doesn't start | Its log says why: `journalctl --user -u drainscope.service -n 20`. A history written by a newer drainscope stops it on purpose: install that version again, or move `~/.local/state/drainscope/drainscope.db` away to start a new history. After 5 failed starts in 2 minutes systemd stops retrying; once the cause is fixed, run `systemctl --user reset-failed drainscope.service` and start it again. |
| ✗ the RAPL sampler isn't installed | Install `drainscope-sampler`. Until then, battery energy is split by CPU time and nothing is measured on AC. |
| ✗ polkit refused access to the energy counters | Only the user at the machine's own seat may read them: SSH sessions and a session you switched away from are refused by design. `drainscope status` then says the counters are unavailable. |
| ✗ sampler failed: … | `journalctl -u drainscope-sampler.service` |
| ! the eBPF probe isn't installed | Optional: install `drainscope-probe` for `drainscope wakeups`, `drainscope network` and the share of network work charged to apps. |
| ! probe failed: … | `journalctl -u drainscope-probe.service`. The probe needs a kernel with BPF and cgroup v2. |
| ! no RAPL energy counters on this machine | Virtual machines and some ARM laptops: drainscope uses battery readings split by CPU time. |
| ! no system battery found | A desktop: energy is still measured with RAPL, but there is no battery usage to report. |
| ! …: reports GPU cycles, not busy time / no per-app GPU statistics | GPU energy counts as Idle instead of going to the app that used the GPU ([ADR 0005](adr/0005-amd-and-other-gpus.md)). |
| ✗ no cgroup v2 CPU accounting | drainscope needs the unified cgroup v2 hierarchy, the default on current distributions. |

## Other things that can look wrong

- **The tile says "Can’t read history" or "Not answering".** The daemon is running but failing. Its log says why: `journalctl --user -u drainscope.service -n 20`.
- **The disk is full.** drainscope keeps measuring but can't record. The log says so once, and recording resumes when there is room again (checked every minute).
- **There's no in-use/background split for apps.** The split needs the GNOME Shell extension turned on, because the extension reports which app has focus. Energy used while focus wasn't reported is left out of the split ("while focus wasn't reported").

How each failure was tested, and how drainscope recovers from it without a restart: [Failure modes](failure-modes.md).
