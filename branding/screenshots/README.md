# Product screenshots

Screenshots of the drainscope app and GNOME Shell extension, in light and dark, for the README, extensions.gnome.org, GNOME Software and the future website.

**The data is illustrative.** The views run the released packages, but the numbers come from [`tools/demo-monitor.js`](tools/demo-monitor.js), a stand-in for the daemon that serves the Monitor1 D-Bus interface. Its story is a laptop on battery for 2 h 47 min: browsing, a build in a terminal, a document, a film, then a video call. Magnitudes follow the dev machine (i7-8550U, two 39 Wh batteries now at 31.4 and 30.3 Wh, about 5 W idle from the battery). Label any use of these images as illustrative.

| | |
|---|---|
| drainscope | 0.1.2 (COPR packages) |
| GNOME | Shell 50.5, libadwaita 1.9, GTK 4.22 (Fedora 44 toolbox) |
| Display | 2880 × 1800 virtual monitor at 200% (1440 × 900 logical) |
| Theme | Adwaita, teal accent, light and dark |
| Wallpaper | GNOME "blobs" (light and dark) |
| Captured | 7 October 2026 |

The Quick Settings shots replace two values that come from the capturing machine: the Wi-Fi network name shows "Home" and the battery level shows 67% (matching 33% used).

## Files

`originals/{light,dark}/` holds the untouched full-screen captures and the window and menu geometry. `exports/` holds what to use:

| File | Use |
|---|---|
| `exports/hero-{light,dark}.png` (2400 × 1500) | README, website hero: the app with Quick Settings beside it |
| `exports/features/*-{light,dark}.png` (2400 × 1500) | Feature cards with a title and a sentence: `usage`, `detail`, `activity`, `sleep-health`, `week`, `quick-settings` |
| `exports/cutouts/*-{light,dark}.png` | The app window alone with transparent rounded corners, and the Quick Settings inset: AppStream screenshots, docs, slides |
| `exports/desktop-{light,dark}.png` (1920 × 1200) | The whole desktop: app and open Quick Settings on the wallpaper |

For extensions.gnome.org, use `exports/features/quick-settings-light.png`, or `exports/cutouts/quick-settings-light.png` when only the extension itself should show.

## Regenerating

From the repository root, with a toolbox that has `gnome-shell` and the drainscope packages to show, and Python with [`../source/requirements.txt`](../source/requirements.txt):

```sh
branding/screenshots/tools/capture.sh drainscope-f44 .venv/bin/python
```

For each color scheme, [`tools/session.sh`](tools/session.sh) starts the demo service and a headless GNOME Shell on a scratch session bus with scratch XDG directories. The real session, its daemon and its database are not touched. The [`tools/capture@drainscope`](tools/capture@drainscope/extension.js) extension follows [`tools/plan.json`](tools/plan.json) (launch and place the app, click a row, scroll, open Quick Settings) and saves full-screen captures with the window and menu geometry. [`tools/compose.py`](tools/compose.py) copies them to `originals/` and builds `exports/` with the brand fonts and colors.

The plan clicks fixed positions in the 1440 × 900 layout: if the app's layout or the demo data's order changes, check the `click` steps. Don't run a capture while a nested or another headless shell runs.
