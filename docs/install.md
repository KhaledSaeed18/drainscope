# Installing

drainscope needs systemd, cgroup v2 and polkit. The packages target Fedora 44, 45 and rawhide; the GNOME Shell extension supports GNOME 50 and 51.

| Package | What it is | Needed |
|---|---|---|
| `drainscope` | The daemon and the `drainscope` command | Yes |
| `drainscope-sampler` | Reads the processor's energy counters | Recommended: without it, battery readings are split by CPU time and nothing is measured on AC |
| `drainscope-probe` | eBPF counters for wakeups and network traffic per app | Optional: `drainscope wakeups`, `drainscope network` and the network share of apps |
| `drainscope-selinux` | SELinux policy for the sampler and the probe | Pulled in automatically on SELinux systems |
| `gnome-shell-extension-drainscope` | The Quick Settings menu; also reports which app has focus | Recommended |
| `drainscope-app` | The desktop app | Optional |

## From COPR (recommended)

```bash
sudo dnf copr enable khaledsaeed18/drainscope
sudo dnf install drainscope drainscope-sampler drainscope-probe drainscope-app gnome-shell-extension-drainscope
systemctl --user enable --now drainscope.service
drainscope doctor
```

The packages are signed with the COPR project's key, which `dnf` imports on first install. Updates arrive through `dnf upgrade`, which also restarts the running daemon.

Log out and back in to load the GNOME Shell extension (Wayland loads extensions at login), then turn it on in the Extensions app or with `gnome-extensions enable drainscope@khaledsaeed18.github.io`. The desktop app is **drainscope** in the app grid.

## From a release

Prefer COPR, which gives you signed packages and updates. To install without it, download the six binary RPMs (not the `.src.rpm`) from the [latest release](https://github.com/KhaledSaeed18/drainscope/releases/latest) into an empty directory. They're built for Fedora 44. Then:

```bash
sudo dnf install ./*.x86_64.rpm ./*.noarch.rpm
systemctl --user enable --now drainscope.service
drainscope doctor
```

Turn on the extension as above, after logging out and back in.

## From source

See [Contributing](../CONTRIBUTING.md#build-and-install-from-source). Don't combine a source install with the packages: it installs units in `/etc`, which override the packaged ones.

## Removing

```bash
systemctl --user disable --now drainscope.service
sudo dnf remove drainscope drainscope-sampler drainscope-probe drainscope-selinux drainscope-app gnome-shell-extension-drainscope
sudo dnf copr disable khaledsaeed18/drainscope   # if installed from COPR
```

Your history stays in `~/.local/state/drainscope/`. Delete that directory to remove it too.
