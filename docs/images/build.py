#!/usr/bin/env python3
"""Builds the diagrams in docs/images/, in light and dark, with outlined IBM Plex lettering.

Needs fonttools (branding/source/requirements.txt). Run from anywhere:
    python3 docs/images/build.py
"""
from html import escape
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont

HERE = Path(__file__).resolve().parent
FONTS = HERE.parents[1] / 'branding' / 'fonts'
fonts = {name: TTFont(FONTS / file) for name, file in {
    'regular': 'IBMPlexSans-Regular.ttf', 'semibold': 'IBMPlexSans-SemiBold.ttf',
    'mono': 'IBMPlexMono-Regular.ttf'}.items()}

# Brand palette (branding/guidelines.md) and the app's timeline colors (ui/app/src/chart.ts).
THEMES = {
    'light': {'bg': '#F3F7F7', 'card': '#FFFFFF', 'line': '#CCDADF', 'text': '#15282F',
              'muted': '#60747C', 'accent': '#087F8C', 'tint': '#E3F1F2', 'on_accent': '#FFFFFF',
              'idle': '#C0BFBC'},
    'dark': {'bg': '#15282F', 'card': '#1C343C', 'line': '#36525B', 'text': '#F3F7F7',
             'muted': '#A5B7BF', 'accent': '#53C6CE', 'tint': '#1F3F47', 'on_accent': '#15282F',
             'idle': '#77767B'},
}
APPS, TERMINALS, SERVICES, SYSTEM, DEVICES = '#2190A4', '#9141AC', '#2EC27E', '#F5C211', '#E5A50A'


def width(value, size, weight='regular'):
    font = fonts[weight]
    cmap = font.getBestCmap()
    scale = size / font['head'].unitsPerEm
    return sum(font['hmtx'][cmap[ord(ch)]][0] * scale for ch in value)


# Glyph outlines used by the diagram being built, as {id: path data}; each is drawn once in <defs>.
glyph_defs = {}


def text(value, x, y, size=20, fill='#000', weight='regular', anchor='start'):
    """Lettering as outlines, so it looks the same without the fonts; y is the baseline."""
    if anchor == 'middle':
        x -= width(value, size, weight) / 2
    elif anchor == 'end':
        x -= width(value, size, weight)
    font = fonts[weight]
    cmap, glyphs = font.getBestCmap(), font.getGlyphSet()
    scale = size / font['head'].unitsPerEm
    uses = []
    for ch in value:
        name = cmap[ord(ch)]
        ident = f'{weight[0]}{ord(ch):x}'
        if ident not in glyph_defs:
            pen = SVGPathPen(glyphs)
            glyphs[name].draw(pen)
            glyph_defs[ident] = pen.getCommands()
        if glyph_defs[ident]:
            uses.append(f'<use href="#{ident}" transform="translate({x:.2f} {y:.2f}) '
                        f'scale({scale:.5f} {-scale:.5f})"/>')
        x += font['hmtx'][name][0] * scale
    return f'<g fill="{fill}" aria-label="{escape(value, quote=True)}">{"".join(uses)}</g>'


def rect(x, y, w, h, fill, stroke=None, rx=14, dash=False, stroke_width=2):
    line = f' stroke="{stroke}" stroke-width="{stroke_width}"' if stroke else ''
    if dash:
        line += ' stroke-dasharray="8 7"'
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}"{line}/>'


def arrow(points, color, width_=3):
    """A polyline ending in an arrowhead at its last point."""
    (x1, y1), (x2, y2) = points[-2], points[-1]
    d = 'M' + ' L'.join(f'{x} {y}' for x, y in points)
    length = ((x2 - x1) ** 2 + (y2 - y1) ** 2) ** 0.5
    ux, uy = (x2 - x1) / length, (y2 - y1) / length
    head = 12
    left = (x2 - ux * head - uy * head * 0.6, y2 - uy * head + ux * head * 0.6)
    right = (x2 - ux * head + uy * head * 0.6, y2 - uy * head - ux * head * 0.6)
    return (f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width_}" '
            f'stroke-linecap="round" stroke-linejoin="round"/>'
            f'<path d="M{left[0]:.1f} {left[1]:.1f} L{x2} {y2} L{right[0]:.1f} {right[1]:.1f}Z" '
            f'fill="{color}" stroke="{color}" stroke-width="2" stroke-linejoin="round"/>')


def card(t, x, y, w, h, title, lines, subtitle=None, emphasis=False, size=19, gap=31):
    body = rect(x, y, w, h, t['card'], t['accent'] if emphasis else t['line'],
                stroke_width=3 if emphasis else 2)
    body += text(title, x + 26, y + 44, 24, t['accent'], 'semibold')
    top = y + 78
    if subtitle:
        body += text(subtitle, x + 26, y + 72, 17, t['muted'])
        top += 24
    for i, line in enumerate(lines):
        # A line starting with a space continues the previous bullet.
        if not line.startswith(' '):
            body += f'<circle cx="{x + 32}" cy="{top + i * gap - 6}" r="3.5" fill="{t["accent"]}"/>'
        body += text(line.strip(), x + 46, top + i * gap, size, t['text'])
    return body


def svg(body, w, h, title, t):
    defs = ''.join(f'<path id="{ident}" d="{d}"/>' for ident, d in sorted(glyph_defs.items()) if d)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" '
            f'role="img"><title>{escape(title)}</title><defs>{defs}</defs>'
            f'{rect(0, 0, w, h, t["bg"], rx=20)}{body}</svg>\n')


def how_it_works(t):
    """README: measure, attribute, keep, show."""
    w, h = 1600, 620
    b = ''
    b += card(t, 40, 40, 440, 230, 'Energy', [
        'RAPL counters: processor,', '  graphics and memory', 'Battery discharge power'],
        subtitle='Measured by the hardware')
    b += card(t, 40, 300, 440, 280, 'Activity', [
        'CPU time of every cgroup', 'GPU time per app (DRM)', 'Wakeups and network bytes',
        'Which app has focus'], subtitle='What each app, tab and service did')
    # Attribute
    x, y, cw, ch = 580, 40, 480, 540
    b += rect(x, y, cw, ch, t['card'], t['accent'], stroke_width=3)
    b += text('Attribute', x + 30, y + 50, 28, t['accent'], 'semibold')
    b += text('drainscope-daemon, every 5 to 15 seconds', x + 30, y + 82, 17, t['muted'])
    steps = [
        ('Idle floor', ['Energy up to the machine’s learned', 'idle power is charged to Idle.']),
        ('Fair shares', ['Energy above it goes to whoever was', 'busy, by CPU time and GPU time.']),
        ('Display & devices', ['On battery, energy the processor', 'didn’t use: screen, Wi-Fi, disks.']),
    ]
    for i, (title, lines) in enumerate(steps):
        sy = y + 124 + i * 116
        b += f'<circle cx="{x + 48}" cy="{sy + 14}" r="18" fill="{t["accent"]}"/>'
        b += text(str(i + 1), x + 48, sy + 21, 20, t['on_accent'], 'semibold', 'middle')
        b += text(title, x + 82, sy + 21, 21, t['text'], 'semibold')
        for j, line in enumerate(lines):
            b += text(line, x + 82, sy + 53 + j * 28, 18, t['muted'])
    b += rect(x + 30, y + ch - 78, cw - 60, 50, t['tint'], rx=25)
    b += text('The shares always add up to what was measured', x + cw / 2, y + ch - 46, 17,
              t['text'], 'semibold', 'middle')
    b += arrow([(490, 155), (535, 155), (535, 280), (568, 280)], t['accent'])
    b += arrow([(490, 440), (535, 440), (535, 340), (568, 340)], t['accent'])
    # Keep and show
    b += card(t, 1160, 40, 400, 230, 'Local history', [
        'SQLite, readable only by you', '30 days by the minute,', '  a year by the hour'],
        subtitle='~/.local/state/drainscope')
    b += card(t, 1160, 300, 400, 280, 'See it', [
        'Quick Settings in GNOME', 'The drainscope app', 'The drainscope command',
        'CSV and JSON export'], subtitle='Through one D-Bus interface')
    b += arrow([(1070, 155), (1148, 155)], t['accent'])
    b += arrow([(1360, 274), (1360, 292)], t['accent'], 3)
    return w, h, b, 'How drainscope works: measure energy, attribute it, keep history, show it'


def architecture(t):
    """docs/architecture.md: components, privileges and buses."""
    w, h = 1600, 1060
    b = ''
    # Kernel
    b += rect(40, 40, 1520, 150, t['card'], t['line'])
    b += text('Linux kernel and systemd', 70, 84, 24, t['accent'], 'semibold')
    b += text('Native interfaces only: no helper programs are run', 70, 112, 17, t['muted'])
    chips = ['powercap RAPL', 'eBPF: sched_switch, cgroup_skb', 'cgroup v2 cpu.stat',
             'DRM fdinfo', 'power_supply', 'logind']
    cx = 70
    for chip in chips:
        cw = width(chip, 17, 'mono') + 36
        b += rect(cx, 132, cw, 38, t['tint'], rx=19)
        b += text(chip, cx + 18, 157, 17, t['text'], 'mono')
        cx += cw + 14
    # System services
    b += rect(40, 250, 1060, 300, 'none', t['muted'], rx=18, dash=True)
    b += text('System services: not root, no network', 66, 284, 18, t['muted'], 'semibold')
    b += card(t, 66, 310, 500, 214, 'drainscope-sampler', [
        'Reads the root-only RAPL counters', 'Only CAP_DAC_READ_SEARCH',
        'Quantized to 10 mJ, 1 read/s per user'], subtitle='Sampler1 on the system bus')
    b += card(t, 590, 310, 486, 214, 'drainscope-probe  (optional)', [
        'Wakeups and bytes per cgroup', 'CAP_BPF, CAP_PERFMON, CAP_NET_ADMIN',
        'Sees no addresses or contents'], subtitle='Probe1 on the system bus')
    b += arrow([(480, 190), (480, 298)], t['accent'])
    b += arrow([(833, 190), (833, 298)], t['accent'])
    # Unprivileged reads
    b += rect(1140, 250, 420, 300, t['tint'], rx=18)
    b += text('Read directly, no privileges', 1170, 296, 20, t['text'], 'semibold')
    for i, line in enumerate(['CPU time per cgroup', 'GPU time of your own processes',
                              'Batteries', 'Suspend and resume']):
        b += f'<circle cx="{1176}" cy="{334 + i * 34}" r="3.5" fill="{t["accent"]}"/>'
        b += text(line, 1190, 340 + i * 34, 18, t['text'])
    b += text('Other users’ activity is summed,', 1170, 496, 17, t['muted'])
    b += text('never itemized', 1170, 520, 17, t['muted'])
    b += arrow([(1350, 190), (1350, 238)], t['accent'])
    # polkit gate
    b += rect(250, 578, 900, 50, t['card'], t['accent'], rx=25, stroke_width=2)
    b += text('polkit: the active local session only  ·  rate-limited per user', 700, 610, 18,
              t['text'], 'semibold', 'middle')
    b += arrow([(480, 524), (480, 566)], t['accent'])
    b += arrow([(833, 524), (833, 566)], t['accent'])
    b += arrow([(700, 628), (700, 708)], t['accent'])
    b += arrow([(1350, 550), (1350, 708)], t['accent'])
    # Session
    b += rect(40, 652, 1520, 376, 'none', t['muted'], rx=18, dash=True)
    b += text('Your session: no privileges', 66, 686, 18, t['muted'], 'semibold')
    x, y, cw, ch = 66, 720, 1468, 150
    b += rect(x, y, cw, ch, t['card'], t['accent'], stroke_width=3)
    b += text('drainscope-daemon', x + 30, y + 46, 26, t['accent'], 'semibold')
    b += text('systemd --user service  ·  Monitor1 on the session bus', x + 30, y + 76, 17, t['muted'])
    stages = ['Collect', 'Attribute', 'Store', 'Serve']
    sx = x + 520
    for i, stage in enumerate(stages):
        b += rect(sx, y + 40, 170, 64, t['tint'], rx=32)
        b += text(stage, sx + 85, y + 80, 20, t['text'], 'semibold', 'middle')
        if i < len(stages) - 1:
            b += arrow([(sx + 178, y + 72), (sx + 222, y + 72)], t['accent'])
        sx += 232
    b += text('history: ~/.local/state/drainscope (SQLite, 0600)', x + 30, y + 124, 17, t['muted'])
    # Clients
    clients = [('drainscope', 'Command line, Rust'), ('GNOME Shell extension', 'Quick Settings, TypeScript'),
               ('drainscope app', 'GTK 4 and libadwaita, TypeScript')]
    cw = (1468 - 2 * 24) / 3
    for i, (name, what) in enumerate(clients):
        cx = 66 + i * (cw + 24)
        b += rect(cx, 914, cw, 90, t['card'], t['line'])
        b += text(name, cx + 26, 952, 21, t['text'], 'semibold')
        b += text(what, cx + 26, 982, 17, t['muted'])
        b += arrow([(cx + cw / 2, 878), (cx + cw / 2, 904)], t['accent'])
    return w, h, b, 'drainscope components, their privileges and the buses between them'


def attribution(t):
    """docs/architecture.md: how one window of battery energy is split."""
    w, h = 1600, 560
    b = ''
    b += text('One window on battery: where 6.6 W went', 60, 78, 26, t['text'], 'semibold')
    b += text('Illustrative numbers. Each window is at least 10 s, so the battery’s slow readings line up '
              'with the processor’s.', 60, 110, 18, t['muted'])
    segments = [  # watts, label, color, label color
        (1.25, 'Idle', t['idle'], '#15282F'),
        (1.40, 'Firefox', APPS, '#FFFFFF'),
        (0.96, 'Terminal: cargo', TERMINALS, '#FFFFFF'),
        (0.38, 'Shell', SERVICES, '#15282F'),
        (0.22, '', SYSTEM, '#15282F'),
        (2.40, 'Display & devices', DEVICES, '#15282F'),
    ]
    total = sum(s[0] for s in segments)
    left, right, top, bar = 60, 1540, 270, 96
    scale = (right - left) / total
    x = left
    edges = [x]
    clip = f'<clipPath id="bar"><rect x="{left}" y="{top}" width="{right - left}" height="{bar}" rx="14"/></clipPath>'
    parts = ''
    for watts, label, color, ink in segments:
        sw = watts * scale
        parts += f'<rect x="{x:.1f}" y="{top}" width="{sw + 0.5:.1f}" height="{bar}" fill="{color}"/>'
        if label:
            parts += text(label, x + sw / 2, top + 46, 19, ink, 'semibold', 'middle')
            parts += text(f'{watts:.2f} W', x + sw / 2, top + 74, 17, ink, 'regular', 'middle')
        x += sw
        edges.append(x)
    b += clip + f'<g clip-path="url(#bar)">{parts}</g>'
    kernel_mid = (edges[4] + edges[5]) / 2
    b += f'<path d="M{kernel_mid:.1f} {top + bar + 4} V{top + bar + 20}" stroke="{t["muted"]}" stroke-width="2"/>'
    b += text('Kernel 0.22 W', kernel_mid, top + bar + 40, 16, t['muted'], 'regular', 'middle')

    def bracket(x1, x2, y, label, sub, up=True):
        d = 14 if up else -14
        path = f'M{x1 + 2:.1f} {y + d} V{y} H{x2 - 2:.1f} V{y + d}'
        out = f'<path d="{path}" fill="none" stroke="{t["accent"]}" stroke-width="3" stroke-linejoin="round"/>'
        ty = y - 16 if up else y + 34
        out += text(label, (x1 + x2) / 2, ty - (24 if up and sub else 0), 19, t['text'], 'semibold', 'middle')
        if sub:
            out += text(sub, (x1 + x2) / 2, ty if up else ty + 26, 17, t['muted'], 'regular', 'middle')
        return out

    b += bracket(left, right, 172, 'Battery discharge, measured', '')
    b += bracket(left, edges[5], 240, 'RAPL: processor, graphics and memory, measured', '')
    b += bracket(left, edges[1], top + bar + 70, 'Idle floor', 'learned from quiet moments', up=False)
    b += bracket(edges[1] + 6, edges[5], top + bar + 70, 'Above the floor: shared by CPU and GPU time',
                 'apps, terminal tabs, services and the kernel', up=False)
    b += bracket(edges[5] + 6, right, top + bar + 70, 'Battery minus RAPL', 'the screen and the rest',
                 up=False)
    return w, h, b, 'How drainscope splits one window of battery energy between consumers'


def main():
    for build in (how_it_works, architecture, attribution):
        for scheme, theme in THEMES.items():
            glyph_defs.clear()
            w, h, body, title = build(theme)
            name = build.__name__.replace('_', '-')
            (HERE / f'{name}-{scheme}.svg').write_text(svg(body, w, h, title, theme))
    print(f'Wrote {len(THEMES) * 3} diagrams to {HERE}')


if __name__ == '__main__':
    main()
