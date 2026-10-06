# Linux integration

The kit is now maintained in the repository under `branding/`. The application asset patch in this directory remains unapplied. Read [`../../docs/branding-handoff.md`](../../docs/branding-handoff.md) for the canonical continuing instructions.

The patch was prepared against `ae75654927ce39c33b54a2c07a368b28499c8fcb` and checked against import base `c5033e633429a7339a3024b717d035ec8a8737af` on main. Check it again against your current Linux checkout before applying it.

## App icon

`app-icons.patch` replaces the full-color icon under the existing stable app ID and adds a symbolic source beside it. The patch does not change the application ID or the app's theme behavior.

From the root of your Linux drainscope checkout:

```sh
git apply --check branding/integration/app-icons.patch
git apply branding/integration/app-icons.patch
```

The current development installer (`ui/app/build.mjs`) and RPM spec already install the full-color icon. The new full-color file therefore follows those existing paths.

The symbolic source also needs to be added to the development installer and RPM installation/file lists before distributing it. Use `io.github.khaledsaeed18.Drainscope-symbolic.svg` in the hicolor `symbolic/apps` directory and retain the stable app ID for the normal icon. This wiring is not included in the asset-only patch.

Review using GNOME App Icon Preview and the installed app at 128, 64, and 32 px. Review the symbolic at 16 and 32 px under both theme variants. Preserve libadwaita's system colors and typography. Run the applicable repo checks if you edit installer or UI code.

## README and GitHub

Reference the already committed `branding/exports/social/readme-header.svg` from the README with useful alt text when applying launch branding. Keep actual install/support information as maintained text below it.

Upload `exports/social/github-social-light.png` or the dark variant in the repository's social-preview settings when ready. No GitHub settings were changed by this handoff.

## Website and documentation

Use `tokens/brand.css` and `tokens/brand.json` as the starting theme system. Preserve font files and the OFL license, updating their relative URLs when you move the CSS. Use the web favicon/touch icons and the 1200 × 630 social card.

`index.html` is a visual guide and includes website/docs layout studies. It is not a complete public site. Build the actual pages around maintained product content and real Linux screenshots.

## Screenshots and listings

Use the screenshot frame as an editable template, replace the title, insert a real screenshot, and delete the insertion instructions. Capture app/extension views with personal information removed, consistent scaling, and recorded app/desktop versions. Check current destination dimensions when preparing EGO/Flathub or other listings.

The identity assets can be reviewed here; installed desktop appearance and actual screenshots need the Linux machine.
