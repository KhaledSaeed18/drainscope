# drainscope brand guidelines

Version 1.0 · Energy shares · 6 October 2026

## Positioning and audience

drainscope helps Linux laptop users understand where their battery energy goes. It attributes energy to apps, terminal workloads, services, and other consumers, and keeps local history. The primary audience is desktop users seeking understandable answers. Developers and contributors need technical depth in the documentation.

The brand personality is precise, approachable, calm, and transparent. The identity represents observation and explanation, with energy shares as its central visual idea.

## Naming and messages

Use **drainscope** in the logo, public project name, and prose. Preserve command, package, and application identifiers exactly. Do not change `io.github.khaledsaeed18.Drainscope` to match the lowercase wordmark.

- Main headline: **See where your battery goes.**
- Short description: **Battery and energy usage by app, for the Linux desktop.**
- Supporting message: **Local history. Transparent attribution. Native GNOME integration.**
- Installation action: **Install drainscope**.
- Technical exploration: **Read the attribution model**.

Write in sentence case with short verbs and specific units. Explain measured totals and estimated shares separately. State hardware support, validation scope, and limitations. Do not promise exact per-process energy, automatic optimization, or a battery-life improvement without evidence. Keep installation and compatibility claims synchronized with the source repository.

Example introductory paragraph:

> drainscope shows how battery and energy usage is shared among your apps, terminal workloads, and other consumers. Explore local history in the GNOME app or on the command line, and check which measurements your hardware supports.

## Symbol construction

The logo is a battery with three unequal shares on a common baseline. Its nominal canvas is 128 × 96 units. The shell has an 8-unit stroke, a 20-unit corner radius, and a compact terminal. The energy shares have equal corner radii and deliberately varied widths/heights. They are an identity motif, not specific measurement values.

The wordmark uses IBM Plex Sans SemiBold with modest tracking adjustment. Production lettering is converted to paths. The included editable master retains live type for future revisions.

### Logo variants

| Variant | Intended use |
|---|---|
| Primary | Teal shares and Ink wordmark on white or Mist |
| Dark | Cyan/light shares and white wordmark on Ink |
| Mono | One-color Ink on a light background |
| White | One-color white on a dark background |

Choose horizontal for headers, stacked for square or centered compositions, and wordmark-only where the symbol is already visible nearby. The mark alone is suitable for recognizable product identity, avatars, and repeated brand cues. Use the dedicated favicon or symbolic asset for small UI contexts.

### Spacing and minimum sizes

Clear space is at least one quarter of the symbol's nominal height around the entire lockup. With the mark at 96 px high, leave 24 px of clear space on all sides. Scale that rule proportionally.

Minimum digital sizes: horizontal lockup 180 px wide; standalone full mark 48 px wide; stacked lockup 160 px wide; standalone wordmark 140 px wide. At 16–32 px use the simplified symbolic artwork. Desktop icon sizes use the dedicated full-color icon.

Preserve proportions and share order. Do not rotate, stretch, apply decorative shadows, redraw the terminal, replace shares with a bolt, or add new colors. On photos or complex artwork, first place the lockup on a quiet contrasting surface.

## Colors

| Name | Hex | Role |
|---|---|---|
| Ink | `#15282F` | Primary text; dark brand background |
| Signal | `#087F8C` | Primary mark and brand color |
| Signal mid | `#269EAA` | Middle energy share; decorative identity element |
| Signal light | `#53C6CE` | Light share; dark-theme accent |
| Mist | `#F3F7F7` | Light background |
| White | `#FFFFFF` | Light surface; reversed logo |
| Slate | `#60747C` | Muted text on white and Mist |

Use deep teal `#076F7C` for small links and action backgrounds on light surfaces. The original Signal color does not reach 4.5:1 against Mist; it remains suitable for the mark and larger graphical elements. Use white button text on deep teal. Dark-theme actions use Signal light with Ink text.

Use the supplied JSON/CSS theme tokens for text, backgrounds, surfaces, focus, and status. Decorative borders are intentionally subtle and do not define an interactive control boundary on their own. Inputs and focus indicators need stronger visible boundaries where their design requires them.

Verified text combinations and their computed contrast ratios are in `validation.json`. Ratio checks cover specified token pairs, not every possible design application.

## Data and status

The website/docs data palette uses Apps, Terminals, Services, System, and Idle as stable categories with separate light/dark colors. A category color is not a status indicator. Use status tokens for success, warning, and danger only where the state warrants them.

Label chart series and include text values or an accessible table when charts convey measurements. Use spacing or separating lines between stacked series; where color distinction matters, add patterns or another non-color cue. Never rely on color alone to communicate a category or severity. Meeting contrast against the background does not prove that every neighboring pair is distinguishable.

The supplied identity graphics contain illustrative bars, not actual measurements. Mark mock data explicitly. Never present the brand's three shares as a product screenshot.

## Typography

Website/documentation: IBM Plex Sans Regular (400) for prose and SemiBold (600) for headings and controls. IBM Plex Mono Regular (400) is for code and commands. Self-host the supplied fonts or derivatives that preserve their license. Fall back to system-ui/sans-serif and monospace respectively.

Default web sizes: 16 px body, 14 px secondary text, 20/24 px subheadings, 32/48/64 px headings. Set prose line height around 1.55 and limit line length to 68 characters. Use `font-variant-numeric: tabular-nums` for aligned numerical columns when supported. Preserve explicit measurement units. Do not make all small labels monospace.

The native app uses GNOME's system typography. The logo does not force a custom font onto application controls.

## Layout and motion

Use a 4 px spacing foundation with 8, 12, 16, 24, 32, 48, 64, and 96 px steps. Left-align prose and numerical explanations. Use clear groups and meaningful dividers; keep the energy-share mark as the primary memorable element.

Web controls use a 6 px radius and panels about 12 px. These values apply to web/docs; native GNOME widgets retain their own styles. Avoid decorative gradients and repeated floating-card treatments.

Use brief transitions, around 160 ms, only where they explain an interaction. Respect reduced motion. Do not use an endlessly animated draining battery or imply a live reading in marketing graphics.

## GNOME icon system

The application icon is separate from the flat brand mark. It uses a 128 × 128 canvas, a 2 px geometry grid, a four-pixel darker front profile, and a bottom at y=116 matching the referenced template's horizontal icon baseline. No external drop shadow is added.

The symbolic icon uses 16 × 16 geometry and one fill color; GTK can recolor it. The bundled white symbolic preview is only a web demonstration of that recoloring. The development icon uses muted shares and a distinct small badge; its `.Devel` filename is an asset variant, not a request to change the stable app ID.

Review the icons at 128, 64, and 32 px in GNOME and the symbolic version at 16 and 32 px. Native desktop context, theme recoloring, and final optical weight still require Linux review. Use GNOME App Icon Preview where available.

## Launch assets

- GitHub social previews: 1280 × 640, light and dark.
- Website social cards: 1200 × 630, light and dark.
- README header: 1280 × 360, SVG and PNG.
- Favicon: SVG, PNG at 16/32/48 px, and a multi-size ICO.
- Touch icon: 180 × 180 PNG. Web app icons: 192 and 512 px.
- Screenshot template: 1600 × 1100 editable SVG. Insert a real screenshot, replace the title, and remove insertion instructions before publishing.

Capture screenshots on Linux with identifiable personal information removed and realistic app data. Record app version, desktop version, theme, and scaling. Keep light/dark sets consistent. App/store destination dimensions should be checked when those listings are prepared; no store-specific graphics are claimed as final here.

## Website and docs plan

The included HTML studies define the future visual system, not a deployed website.

The homepage should lead with the message and a real app view, then explain use cases, installation, hardware support, local history, and measurement limits. Link to source and documentation. Keep package instructions maintained rather than baking version numbers into brand artwork.

Documentation should provide installation, supported hardware, how attribution works, validation, troubleshooting, command reference, and contribution guidance. Use the supplied diagram style for measured inputs, estimated shares, and local-history outputs. Callouts should explain a concrete limitation or action.

## Reproduction and source management

The source generator creates vector shapes and outlines lettering from the supplied Plex fonts. The renderer produces PNG and ICO exports and an asset inventory. Keep editable originals, production exports, provenance, and font licenses together.

When integrating with the application, use its existing icon ID and installation paths. The kit's optional authoring tools do not become application runtime dependencies. Follow `integration/README.md` for the prepared icon patch and the symbolic installation work required on Linux.

No separate public license for original brand artwork has been selected. The bundled fonts remain under OFL. Do not infer asset licensing from the application's GPL without a maintainer decision.

## Sources

- Product: https://github.com/KhaledSaeed18/drainscope
- GNOME app icons: https://developer.gnome.org/hig/guidelines/app-icons.html
- GNOME symbolic icons: https://developer.gnome.org/hig/guidelines/ui-icons.html
- GNOME UI styling: https://developer.gnome.org/hig/guidelines/ui-styling.html
- IBM Plex: https://github.com/IBM/plex
