#!/usr/bin/env python3
"""Compose framed screenshots from the raw captures (session.sh + plan.json).

    python compose.py <light capture dir> <dark capture dir>

Each capture dir holds the full-stage PNGs and geometry.json written by the capture extension.
Copies them to ../originals/, then writes ../exports/: window cutouts, the Quick Settings inset,
feature cards and the README hero, in light and dark. Needs Pillow (../../source/requirements.txt).
"""
from pathlib import Path
import json
import shutil
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
KIT = HERE.parents[1]
OUT = HERE.parent / 'exports'
ORIGINALS = HERE.parent / 'originals'
FONTS = KIT / 'fonts'

INK, SIGNAL, MIST, WHITE, SLATE = '#15282F', '#087F8C', '#F3F7F7', '#FFFFFF', '#60747C'
THEMES = {
    'light': {'bg': MIST, 'text': INK, 'muted': SLATE, 'logo': 'drainscope-horizontal-primary.png',
              'shadow': (21, 40, 47), 'shadow_alpha': 0.16, 'outline': None},
    'dark': {'bg': INK, 'text': MIST, 'muted': '#A5B7BF', 'logo': 'drainscope-horizontal-dark.png',
             'shadow': (0, 0, 0), 'shadow_alpha': 0.45, 'outline': (255, 255, 255, 26)},
}
WINDOW_RADIUS = 15  # libadwaita window corners, logical pixels
CAPTION = 'Illustrative data · drainscope 0.1.2 · GNOME 50 · 200% scale'

# (capture, file stem, title, body)
FEATURES = [
    ('app-unplugged', 'usage', 'See where your battery goes.',
     'Every app, terminal command and service gets its share of the energy used since you '
     'unplugged, on a timeline you can read at a glance.'),
    ('app-detail', 'detail', 'Know what each app costs.',
     'Open any entry to see its energy, its average power, and how that energy splits '
     'between the processor, graphics and the display.'),
    ('app-activity', 'activity', 'Find what keeps your laptop busy.',
     'Wakeups and network traffic per app, over the last minute, reveal the background work '
     'that drains a battery while you are not looking.'),
    ('app-sleep-health', 'sleep-health', 'Sleep and battery health, recorded.',
     'Each suspend shows how long it lasted, what it cost and what woke the machine. '
     'Daily readings follow each battery’s capacity.'),
    ('app-week', 'week', 'Look back over the week.',
     'History stays on your machine, so you can compare today with the last seven days. '
     'Nothing leaves your laptop.'),
    ('quick-settings', 'quick-settings', 'Right in Quick Settings.',
     'The GNOME Shell extension lists the biggest consumers since you unplugged, one click '
     'from the top bar.'),
]


def font(weight, size):
    name = {'regular': 'IBMPlexSans-Regular.ttf', 'semibold': 'IBMPlexSans-SemiBold.ttf'}[weight]
    return ImageFont.truetype(str(FONTS / name), size)


def rounded_mask(size, radius, supersample=4):
    """An antialiased rounded-rectangle alpha mask."""
    w, h = size
    big = Image.new('L', (w * supersample, h * supersample), 0)
    ImageDraw.Draw(big).rounded_rectangle(
        (0, 0, w * supersample - 1, h * supersample - 1), radius * supersample, fill=255)
    return big.resize(size, Image.LANCZOS)


def cutout(image, box, radius):
    """Crop `box` (pixels) and round its corners."""
    piece = image.crop(box).convert('RGBA')
    piece.putalpha(ImageChops.multiply(piece.getchannel('A'), rounded_mask(piece.size, radius)))
    return piece


def outlined(piece, radius, colour):
    """A hairline outline inside the rounded edge, for dark surfaces on dark backgrounds."""
    if colour is None:
        return piece
    w, h = piece.size
    s = 4
    ring = Image.new('L', (w * s, h * s), 0)
    draw = ImageDraw.Draw(ring)
    draw.rounded_rectangle((0, 0, w * s - 1, h * s - 1), radius * s, fill=255)
    draw.rounded_rectangle((2 * s, 2 * s, w * s - 1 - 2 * s, h * s - 1 - 2 * s), max(radius - 2, 0) * s, fill=0)
    ring = ring.resize((w, h), Image.LANCZOS)
    line = Image.new('RGBA', (w, h), colour[:3] + (0,))
    line.putalpha(ImageChops.multiply(ring, Image.new('L', (w, h), colour[3])))
    return Image.alpha_composite(piece, line)


def drop_shadow(canvas, piece, at, theme, spread=1.0):
    """Two shadow layers (ambient and contact) under `piece` placed at `at`."""
    rgb, strength = theme['shadow'], theme['shadow_alpha']
    for blur, offset, alpha in ((60 * spread, 36 * spread, strength), (10 * spread, 6 * spread, strength * 0.9)):
        pad = int(blur * 3)
        layer = Image.new('RGBA', (piece.width + 2 * pad, piece.height + 2 * pad), rgb + (0,))
        shape = Image.new('L', layer.size, 0)
        shape.paste(piece.getchannel('A'), (pad, pad))
        shape = shape.filter(ImageFilter.GaussianBlur(blur))
        layer.putalpha(shape.point(lambda v, a=alpha: int(v * a)))
        canvas.alpha_composite(layer, (int(at[0] - pad), int(at[1] - pad + offset)))
    canvas.alpha_composite(piece, (int(at[0]), int(at[1])))


def wrap(draw, text, face, width):
    lines, line = [], ''
    for word in text.split():
        candidate = f'{line} {word}'.strip()
        if draw.textlength(candidate, font=face) > width and line:
            lines.append(line)
            line = word
        else:
            line = candidate
    return lines + [line]


def logo(theme, height):
    image = Image.open(KIT / 'exports/logo' / theme['logo']).convert('RGBA')
    return image.resize((round(image.width * height / image.height), height), Image.LANCZOS)


def window(shots, geometry, name, theme):
    g = geometry[name]
    s = g['scale']
    x, y, w, h = (v * s for v in g['window'])
    piece = cutout(shots[name], (x, y, x + w, y + h), WINDOW_RADIUS * s)
    return outlined(piece, WINDOW_RADIUS * s, theme['outline'])


def quick_settings(shots, geometry):
    """The top-right of the desktop: the top bar's status area and the open menus, in context."""
    g = geometry['desktop']
    s = g['scale']
    qx, qy, qw, qh = g['quickSettings']
    margin = 28
    box = ((qx - margin) * s, 0, min((qx + qw + margin) * s, shots['desktop'].width), (qy + qh + margin) * s)
    return cutout(shots['desktop'], box, 24 * s)


def save(image, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, optimize=True)
    print(f'{path.relative_to(KIT)}  {image.width}x{image.height}')


def feature_card(piece, title, body, theme, size=(2400, 1500)):
    W, H = size
    card = Image.new('RGBA', size, theme['bg'])
    draw = ImageDraw.Draw(card)
    left, column = 150, 860
    mark = logo(theme, 64)
    card.alpha_composite(mark, (left, 132))
    title_face, body_face = font('semibold', 84), font('regular', 38)
    title_lines = wrap(draw, title, title_face, column)
    body_lines = wrap(draw, body, body_face, column)
    block = len(title_lines) * 100 + 44 + len(body_lines) * 59
    y = (H - block) // 2 + 20
    for line in title_lines:
        draw.text((left - 4, y), line, font=title_face, fill=theme['text'])
        y += 100
    y += 44
    for line in body_lines:
        draw.text((left, y), line, font=body_face, fill=theme['muted'])
        y += 59
    draw.text((left, H - 120), CAPTION, font=font('regular', 26), fill=theme['muted'])
    # The screenshot, as large as fits the right column.
    area_x, area_w, area_h = 1120, W - 1120 - 110, H - 2 * 110
    scale = min(area_w / piece.width, area_h / piece.height)
    shown = piece.resize((round(piece.width * scale), round(piece.height * scale)), Image.LANCZOS)
    at = (area_x + (area_w - shown.width) / 2, (H - shown.height) / 2 - 10)
    drop_shadow(card, shown, at, theme)
    return card


def hero(app, qs, theme, size=(2400, 1500)):
    """The app window with the Quick Settings inset overlapping its right edge."""
    W, H = size
    card = Image.new('RGBA', size, theme['bg'])
    app_scale = (H - 2 * 100) / app.height
    app_shown = app.resize((round(app.width * app_scale), round(app.height * app_scale)), Image.LANCZOS)
    qs_scale = app_scale * 1.0
    qs_shown = qs.resize((round(qs.width * qs_scale), round(qs.height * qs_scale)), Image.LANCZOS)
    overlap = 70
    total = app_shown.width + qs_shown.width - overlap
    app_at = ((W - total) / 2, 100)
    qs_at = (app_at[0] + app_shown.width - overlap, 100 + (app_shown.height - qs_shown.height) / 2 + 90)
    drop_shadow(card, app_shown, app_at, theme)
    drop_shadow(card, qs_shown, qs_at, theme)
    return card


def main(light_dir, dark_dir):
    for scheme, source in (('light', Path(light_dir)), ('dark', Path(dark_dir))):
        theme = THEMES[scheme]
        geometry = json.loads((source / 'geometry.json').read_text())
        shots = {name: Image.open(source / f'{name}.png').convert('RGBA') for name in geometry}
        target = ORIGINALS / scheme
        target.mkdir(parents=True, exist_ok=True)
        for name in geometry:
            shutil.copyfile(source / f'{name}.png', target / f'{name}.png')
        shutil.copyfile(source / 'geometry.json', target / 'geometry.json')

        pieces = {name: window(shots, geometry, name, theme) for name in geometry if name.startswith('app-')}
        pieces['quick-settings'] = quick_settings(shots, geometry)
        for name, piece in pieces.items():
            save(piece, OUT / 'cutouts' / f'{name.removeprefix("app-")}-{scheme}.png')
        for capture, stem, title, body in FEATURES:
            save(feature_card(pieces[capture], title, body, theme), OUT / 'features' / f'{stem}-{scheme}.png')
        save(hero(pieces['app-unplugged'], pieces['quick-settings'], theme), OUT / f'hero-{scheme}.png')
        desktop = shots['desktop'].convert('RGB').resize((1920, 1200), Image.LANCZOS)
        save(desktop, OUT / f'desktop-{scheme}.png')


if __name__ == '__main__':
    main(*sys.argv[1:3])
