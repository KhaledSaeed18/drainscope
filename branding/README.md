# drainscope — Energy shares brand kit v1

Selected direction: Energy shares. Prepared 6 October 2026.

Open `index.html` for the visual guide, light/dark switch, and website/documentation studies. Read `guidelines.md` for exact usage rules. The PDF guide is a portable visual reference.

The canonical Linux continuation guide is [`../docs/branding-handoff.md`](../docs/branding-handoff.md). Native icon installation and packaging integration are the next Linux task.

## What is included

- Flat symbol, wordmark, horizontal and stacked lockups in primary, dark, monochrome, and white variants.
- Outlined production SVGs and transparent PNG exports; editable SVG master with live lettering.
- GNOME full-color icon, dedicated 16 px symbolic icon, and development-build variant.
- Favicon SVG, PNG, and ICO; touch icon and 192/512 px web app icons.
- README header; light and dark GitHub and website social cards.
- Screenshot frame, release copy template, and a measurement diagram.
- Bundled IBM Plex Sans Regular/SemiBold and IBM Plex Mono Regular fonts with OFL license.
- JSON/CSS tokens, reproduction scripts, export manifest, validation report, and Linux integration notes.

## Maintenance

Production SVGs are editable shapes; logo lettering is outlined to avoid font substitution. Use `source/drainscope-logo-editable.svg` for lettering edits after installing the included Plex fonts. The geometry and canonical palette are defined in `source/build.py`.

To regenerate vectors from the kit root:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -r source/requirements.txt
python source/build.py
```

To regenerate raster exports:

```sh
cd source
pnpm install
pnpm render
```

The authoring tools are optional and are not drainscope runtime dependencies. Copy production SVGs directly when no regeneration is needed. The archive excludes virtual environments and node_modules.

The existing website/docs preview PNGs are included in the kit. After rebuilding vectors/rasters, run `python source/check.py` to validate assets and color tokens, and `python source/build-guide.py` to rebuild the PDF from those assets and preview images. Re-capture the layout previews in a browser if you change `index.html`.

## Review status

Vector/raster rendering, font-free logos, nominal export sizes, transparency, token contrast, and browser layouts were checked locally. GNOME desktop context review and real product screenshots require Linux. This import supplies the kit and handoff; installed icons, installer/RPM behavior, repository social-preview settings, and site publication are still pending.

## Provenance and licensing

The original vector artwork was constructed for this project from the chosen conceptual direction. The earlier AI-generated concept board is exploratory and is not the production logo master. No separate public brand-artwork license has been chosen. Font licensing is documented in `licenses/IBM-Plex-OFL.txt`; do not remove it when redistributing the fonts or outlined lettering.

Source repository audited at `ae75654927ce39c33b54a2c07a368b28499c8fcb`. Font source URLs and file checksums are recorded in `provenance.json`.
