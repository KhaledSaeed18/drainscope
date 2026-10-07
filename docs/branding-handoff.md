# Branding handoff: Energy shares identity v1

Updated 7 October 2026. The canonical kit is maintained in [`../branding/`](../branding/README.md). This file is the continuing point for the Linux agent.

## Continuing point

The user selected **Energy shares**, the teal battery with three unequal shares, as the drainscope identity. Brand kit v1 has been built and reviewed on macOS and is now in this repository. The next task is to integrate the supplied assets into the Linux application and verify their appearance in GNOME. Carry forward the selected design.

This import adds the kit and handoff. The installed app icon, development installer, RPM spec, repository social-preview settings, and public websites have not been changed by this import.

The PDF explains the identity. The rest of the kit supplies the actual editable sources, SVG/PNG exports, fonts, tokens, and templates. Use the original files rather than extracting or tracing images from the PDF.

The initial Mac audit used repository commit `ae75654927ce39c33b54a2c07a368b28499c8fcb`. The repository import was prepared after syncing main to `c5033e633429a7339a3024b717d035ec8a8737af`. Your Linux working copy may be newer and may contain ongoing development. Preserve that work and adapt the integration to its current state.

## Inputs

The kit is already in `branding/`; a separate ZIP transfer is no longer needed. Incorporate the new commit using the workflow appropriate to the current Linux branch and working tree. Inspect local changes before synchronizing with main; do not reset or overwrite ongoing work.

Read these files first:

1. The current Linux repository's applicable `AGENTS.md` and `CLAUDE.md`, and relevant project plans.
2. [`branding/README.md`](../branding/README.md) and [`branding/guidelines.md`](../branding/guidelines.md).
3. [`branding/integration/README.md`](../branding/integration/README.md).
4. [`branding/drainscope-brand-guide.pdf`](../branding/drainscope-brand-guide.pdf) or [`branding/index.html`](../branding/index.html) for the visual reference. Open the HTML locally in a browser or serve `branding/` on loopback.

`exports/` contains production assets. `source/` contains authoring scripts and the editable logo. `tokens/` contains web/docs colors and typography. `licenses/` contains the IBM Plex license. `templates/` contains a screenshot frame and release-copy template. `validation.json` and `browser-validation.json` record the Mac-side checks.

These kit paths are relative to `branding/`. The headline is **See where your battery goes.** The short description is **Battery and energy usage by app, for the Linux desktop.** Consult the guidelines for exact colors, typography, and logo variants. Authoring tools are optional and remain separate from application runtime dependencies.

## Task to perform now

Implement the selected Energy shares identity in the current drainscope repository, preserving the ongoing development work.

1. **Inspect the current branch and changes.** Record the current commit and inspect the icon, development installer, RPM packaging, README, and any existing branding structure. Do not reset files, discard changes, or assume that the audited Mac commit is still current.

2. **Use the maintained kit.** The kit is already stored in `branding/`. Keep sources, production exports, usage rules, tokens, provenance, and font licenses together. Do not add authoring dependencies to the application runtime or commit virtual environments/node_modules. Ensure website/docs preview paths remain usable after moving the kit. Establish how future icon exports are synchronized into `data/app/icons/`.

3. **Integrate the app icons.** The full-color source is `branding/exports/app-icon/io.github.khaledsaeed18.Drainscope.svg`. The symbolic is `branding/exports/app-icon/io.github.khaledsaeed18.Drainscope-symbolic.svg`. The shipping full-color source is `data/app/icons/io.github.khaledsaeed18.Drainscope.svg`. Preserve the stable application ID. The provided `branding/integration/app-icons.patch` has not been applied by this kit import. From the repository root, run `git apply --check branding/integration/app-icons.patch` before applying it with `git apply branding/integration/app-icons.patch`. Use it only if it fits the current checkout, and do not apply it twice. If it does not fit, compare and adapt the asset changes without overwriting unrelated work.

4. **Complete symbolic installation.** The supplied patch replaces the full-color icon and adds the symbolic source; it does not install the symbolic. Update the current development installer and RPM installation/file lists so the symbolic is shipped in hicolor `symbolic/apps`. At the audited commit, these paths were `ui/app/build.mjs` and `packaging/drainscope.spec`. Inspect current equivalents rather than assuming the files are unchanged. Ensure the stable full-color icon continues to be installed through its existing path. Verify any UI reference that should use the new symbolic. Do not replace ordinary GNOME control icons indiscriminately.

5. **Apply launch branding.** Add the supplied README header through a stable repo-relative asset path, with useful alt text, and retain maintained installation/support information below it. Prepare the chosen GitHub social-preview image and release template for use; updating external GitHub settings or making a release is outside this immediate integration task.

6. **Verify in Linux.** Follow the repository's checks for the files actually changed. Read the current development-machine notes before installation: the documented dev machine runs COPR packages, and development installers can override those packages, including user-installed extensions and units under `/etc`. Prefer the established toolbox/package workflow or an isolated test environment. If using development installation, make the override and restoration explicit. Never run two nested GNOME shells at once, and give tests scratch `XDG_*` directories so their daemon cannot touch the real history database. Check the full-color icon in GNOME at ordinary launcher sizes and at 128/64/32 px. Check the symbolic at 16/32 px, including actual GTK recoloring in light and dark contexts. Verify installed paths, relevant packaged file lists, and package contents through the established workflow. Viewing an SVG alone does not prove that the app resolves its installed icon. Preserve libadwaita system typography, surfaces, controls, and the user's accent choice; web color tokens do not impose a desktop theme.

7. **Capture real product views.** Capture the app and Shell extension in useful states, including light and dark versions where supported. Use real application data or clearly labeled illustrative data. Remove personal identifiers. Record application version, GNOME version, display scale, theme, and capture date. Store originals separately from framed exports. Do not publish the screenshot template's insertion instructions. Prefer the app's energy/history view and the extension's battery breakdown for the first pair of screenshots.

8. **Return a reviewable result.** Keep the integration focused and describe exactly what changed. Use the current project's commit/review workflow. Report checks run, failures or limitations, the changed files, icon-context screenshots, product screenshots, and any remaining packaging work. If a draft PR is created as part of that workflow, include its URL. Do not automatically merge or publish a release as part of this task.

## Keep out of this integration

Do not change energy attribution, daemons, privileges, D-Bus contracts, database schema, package identifiers, or unrelated application functionality for branding purposes. Do not build or deploy the full website/docs site in this first integration task. The included website/docs layouts are studies for that later work.

## Existing verification and limits

The kit's Mac-side review checked outlined logo rendering, SVG parsing, raster sizes, transparency, ICO output, specified token contrast pairs, local font loading, and responsive light/dark browser layouts. All eight PDF guide pages were visually reviewed, with the crowded section corrected.

Those checks do not establish installed GNOME appearance, symbolic theme recoloring, RPM contents, or actual product screenshots. These are the Linux agent's next verification responsibilities. If the native preview reveals an optical or rendering problem, document it with context and propose a targeted adjustment to the selected identity.

No separate public license for original brand artwork has been chosen. Keep the included font license and record any maintainer decision about brand-artwork licensing before adopting a new public asset license.

## Completion report for continuing here

### Repository import checks (7 October 2026)

The relocated kit passed its asset validation: 29 SVGs, 41 PNGs, outlined logo lettering, ICO output, export dimensions, logo transparency, and 40 contrast pairs. The icon patch was checked against the import base. `cargo fmt --all --check` passed. No application/UI code was changed by this import, so no touched application-package or UI tests were required.

The repository-required workspace Clippy check was attempted on macOS and could not complete because the Linux eBPF probe requires kernel headers:

```text
bpf/wakeups.bpf.c:11:10: fatal error: 'linux/types.h' file not found
clang failed to compile bpf/wakeups.bpf.c
```

Run the relevant repository checks on Linux when completing icon installation and packaging integration. This failure is a host limitation, not a passed check.

Return:

- Current repository commit, integration branch/commit or draft PR URL, and changed-file summary.
- Checks executed and results, including any checks that could not run.
- Confirmation of development and package icon installation paths.
- Light/dark native icon and symbolic preview evidence.
- Real app/extension screenshots, with versions, theme, and scale.
- Any concrete adjustment needed to the assets.

Once this integration is reviewed, the next phase can use the same kit and real screenshots to build the public landing page, documentation site, and destination-specific distribution graphics. The brand strategy and assets should carry forward rather than be re-created.

### Linux integration (7 October 2026)

Done on main (Fedora 44, GNOME 50.3):

- `app-icons.patch` applied: new full-color icon and the symbolic icon in `data/app/icons/`.
- The symbolic icon is installed to hicolor `symbolic/apps` by `ui/app/build.mjs --install` and `packaging/drainscope.spec` (install and `%files`).
- The Shell extension's quick-settings tile and menu header use the symbolic icon instead of `battery-level-50-symbolic`. `ui/extension/build.mjs` copies it from `data/app/icons/` into `dist/icons/`; `xtask dist` packs it with `--extra-source=icons`.
- CI job `icons` fails if `data/app/icons/` and `branding/exports/app-icon/` differ.
- README opens with `readme-header.svg`; AppStream metadata has brand colors (`#53c6ce` light, `#15282f` dark).
- `/branding export-ignore` keeps the kit out of the release tarball and SRPM.

Checks: `pnpm typecheck`, `pnpm lint`, `pnpm test`; `cargo fmt --check`, clippy and tests for `xtask`; `appstreamcli validate --no-net`; `cargo xtask dist --rpm` (both icons in `drainscope-app`, `icons/` in the extension RPM and EGO zip, no `branding/` in the tarball). GTK 4 resolved both icons from a scratch `install-dev` home and recolored the symbolic for light and dark. Nested GNOME 50.5 (f44 toolbox) and 51.0 (f45 toolbox) shells on scratch XDG directories: extension `ACTIVE`, tile and menu header show the symbolic icon recolored in light and dark styles, no JS errors.

The GitHub social preview was set in the repository settings the same day.

Remaining: view the icons in the real session after the next release (launcher sizes and 128/64/32 px) and capture product screenshots (ROADMAP Next 2). No asset adjustments were needed.

## Prompt for the Linux agent

> Continue drainscope’s Energy shares branding integration. Read docs/branding-handoff.md, the current repository instructions, and branding/guidelines.md. Preserve my ongoing work and follow the documented COPR/toolbox testing workflow. Integrate the supplied app icons, complete symbolic installation and RPM packaging, apply README branding, run applicable checks, and capture real GNOME app/extension screenshots. Return a reviewable result with changed files, checks, screenshots, and remaining issues. The kit is already in the repository; carry forward the selected identity.
