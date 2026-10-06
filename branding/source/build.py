#!/usr/bin/env python3
"""Rebuild the vector asset pack. Run from any directory with fonttools installed."""
from pathlib import Path
from html import escape
import json
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen

ROOT = Path(__file__).resolve().parents[1]
EXPORTS = ROOT / 'exports'
FONTS = ROOT / 'fonts'
INK, TEAL, CYAN, MIST, WHITE, SLATE = '#15282F', '#087F8C', '#53C6CE', '#F3F7F7', '#FFFFFF', '#60747C'
fonts = {name: TTFont(FONTS / file) for name, file in {
    'regular': 'IBMPlexSans-Regular.ttf', 'semibold': 'IBMPlexSans-SemiBold.ttf',
    'mono': 'IBMPlexMono-Regular.ttf'}.items()}

def width(value, size, weight='regular', tracking=0):
    font = fonts[weight]
    cmap = font.getBestCmap()
    scale = size / font['head'].unitsPerEm
    return sum(font['hmtx'][cmap[ord(ch)]][0] * scale for ch in value) + max(0, len(value)-1)*tracking

def lettering(value, x, y, size=24, fill=INK, weight='regular', tracking=0):
    font = fonts[weight]
    cmap, glyphs = font.getBestCmap(), font.getGlyphSet()
    scale = size / font['head'].unitsPerEm
    result = []
    for ch in value:
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs)
        glyphs[name].draw(pen)
        if pen.getCommands():
            result.append(f'<path d="{pen.getCommands()}" transform="translate({x:.4f} {y:.4f}) scale({scale:.6f} {-scale:.6f})"/>')
        x += font['hmtx'][name][0] * scale + tracking
    return f'<g fill="{fill}" aria-label="{escape(value, quote=True)}">' + ''.join(result) + '</g>'

def svg(body, w, h, title):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img"><title>{escape(title)}</title>{body}</svg>\n'

def save(relative, body, w, h, title):
    destination = EXPORTS / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(svg(body, w, h, title))

def mark(color=TEAL, shares=None):
    shares = shares or (TEAL, '#269EAA', CYAN)
    # A transparent ring; the common baseline and unequal bars encode shares.
    return (f'<rect x="8" y="12" width="104" height="72" rx="20" fill="none" stroke="{color}" stroke-width="8"/>'
            f'<rect x="112" y="34" width="12" height="28" rx="4" fill="{color}"/>'
            f'<rect x="20" y="24" width="36" height="48" rx="4" fill="{shares[0]}"/>'
            f'<rect x="60" y="32" width="24" height="40" rx="4" fill="{shares[1]}"/>'
            f'<rect x="88" y="40" width="12" height="32" rx="4" fill="{shares[2]}"/>')

def symbol(color='#2e3436'):
    return (f'<g fill="{color}"><path fill-rule="evenodd" d="M3 3H11A3 3 0 0 1 14 6V10A3 3 0 0 1 11 13H3A3 3 0 0 1 0 10V6A3 3 0 0 1 3 3Z M3 5A1 1 0 0 0 2 6V10A1 1 0 0 0 3 11H11A1 1 0 0 0 12 10V6A1 1 0 0 0 11 5Z"/>'
            '<rect x="14" y="6" width="2" height="4" rx="1"/>'
            '<path d="M3 6H5V10H3Z M6 7H8V10H6Z M9 8H11V10H9Z"/></g>')

def horizontal(color=INK, ring=TEAL, shares=None):
    return mark(ring, shares) + lettering('drainscope', 152, 70, 68, color, 'semibold', -.25)

LOGO_W = round(152 + width('drainscope', 68, 'semibold', -.25) + 4)

for variant, text_color, ring, shares in [
    ('primary', INK, TEAL, None), ('dark', WHITE, CYAN, (CYAN, '#93DCE1', WHITE)),
    ('mono', INK, INK, (INK,)*3), ('white', WHITE, WHITE, (WHITE,)*3)]:
    save(f'logo/drainscope-mark-{variant}.svg', mark(ring, shares), 128, 96, f'drainscope {variant} symbol')
    save(f'logo/drainscope-horizontal-{variant}.svg', horizontal(text_color,ring,shares), LOGO_W,96, f'drainscope {variant} logo')
    word_w = round(width('drainscope',68,'semibold',-.25)+4)
    save(f'logo/drainscope-wordmark-{variant}.svg', lettering('drainscope',2,66,68,text_color,'semibold',-.25),word_w,84,f'drainscope {variant} wordmark')
    stacked_w = 360
    body = f'<g transform="translate(116 4)">{mark(ring,shares)}</g>'
    body += lettering('drainscope',(stacked_w-width('drainscope',64,'semibold',-.25))/2,172,64,text_color,'semibold',-.25)
    save(f'logo/drainscope-stacked-{variant}.svg',body,360,196,f'drainscope {variant} stacked logo')

app_id = 'io.github.khaledsaeed18.Drainscope'
# All principal icon coordinates are on the 2 px grid; 4 px front profile.
icon = (f'<rect x="110" y="62" width="10" height="24" rx="4" fill="#075E69"/>'
        '<rect x="12" y="34" width="100" height="82" rx="22" fill="#075E69"/>'
        f'<rect x="12" y="30" width="100" height="82" rx="22" fill="{TEAL}"/>'
        f'<rect x="110" y="58" width="10" height="24" rx="4" fill="{TEAL}"/>'
        f'<rect x="20" y="38" width="84" height="66" rx="14" fill="{MIST}"/>'
        f'<rect x="26" y="44" width="30" height="54" rx="4" fill="{TEAL}"/>'
        '<rect x="60" y="54" width="22" height="44" rx="4" fill="#269EAA"/>'
        f'<rect x="86" y="64" width="12" height="34" rx="4" fill="{CYAN}"/>')
save(f'app-icon/{app_id}.svg',icon,128,128,'drainscope application icon')
save(f'app-icon/{app_id}-symbolic.svg',symbol(),16,16,'drainscope symbolic application icon')
save('preview/drainscope-symbolic-white.svg',symbol(WHITE),16,16,'drainscope symbolic icon on dark surfaces; GTK recolors the installed icon')
nightly = icon.replace(TEAL,'#60747C').replace('#075E69','#435861').replace('#269EAA','#82979E').replace(CYAN,'#B3C8CE')
nightly += '<path d="M92 100L101 116H83Z" fill="#E9BE68"/><path d="M92 104V110M92 112V113" stroke="#15282F" stroke-width="2"/>'
save(f'app-icon/{app_id}.Devel.svg',nightly,128,128,'drainscope development-build icon')
save('web/favicon.svg',f'<rect width="64" height="64" rx="14" fill="{TEAL}"/><g transform="translate(8 8) scale(3)">{symbol(WHITE)}</g>',64,64,'drainscope favicon')
save('web/apple-touch-icon.svg',f'<rect width="180" height="180" rx="36" fill="{TEAL}"/><g transform="translate(18 18) scale(9)">{symbol(WHITE)}</g>',180,180,'drainscope touch icon')

def social(w,h,dark=False):
    bg, fg, secondary = (INK,WHITE,'#A5B7BF') if dark else (MIST,INK,SLATE)
    scale = w/1280
    body = f'<rect width="{w}" height="{h}" fill="{bg}"/><g transform="scale({scale})">'
    body += f'<g transform="translate(64 44) scale(.62)">{horizontal(fg,CYAN if dark else TEAL,(CYAN,"#93DCE1",WHITE) if dark else None)}</g>'
    body += lettering('See where your',64,256,72,fg,'semibold')+lettering('battery goes.',64,344,72,fg,'semibold')
    body += lettering('Battery and energy usage by app,',64,414,28,secondary)+lettering('for the Linux desktop.',64,453,28,secondary)
    body += f'<g transform="translate(870 214) scale(2.35)">{mark(CYAN if dark else TEAL,(CYAN,"#93DCE1",WHITE) if dark else None)}</g>'
    body += f'<path d="M64 540H1216" stroke="{("#47636D" if dark else "#CCDADF")}" stroke-width="1"/>'
    body += lettering('Open source. Local history. Native GNOME integration.',64,579,21,secondary)
    return body+'</g>'

save('social/github-social-light.svg',social(1280,640),1280,640,'drainscope GitHub social preview')
save('social/github-social-dark.svg',social(1280,640,True),1280,640,'drainscope dark GitHub social preview')
save('social/website-social-light.svg',social(1200,630),1200,630,'drainscope website social card')
save('social/website-social-dark.svg',social(1200,630,True),1200,630,'drainscope dark website social card')
banner = f'<rect width="1280" height="360" rx="20" fill="{MIST}"/>'
banner += f'<g transform="translate(56 34) scale(.68)">{horizontal()}</g>'
banner += lettering('See where your battery goes.',56,208,52,INK,'semibold')
banner += lettering('Battery and energy usage by app, for the Linux desktop.',56,262,25,SLATE)
banner += f'<g transform="translate(1000 88) scale(1.6)">{mark()}</g>'
save('social/readme-header.svg',banner,1280,360,'drainscope README header')

board = f'<rect width="1600" height="1120" fill="{WHITE}"/>'
board += lettering('drainscope',64,77,36,INK,'semibold')+lettering('Energy shares / Identity v1',1038,74,22,SLATE)
board += '<path d="M64 108H1536" stroke="#CCDADF"/>'
board += f'<g transform="translate(82 178) scale(3.25)">{mark()}</g>'
board += lettering('See where your',650,244,70,INK,'semibold')+lettering('battery goes.',650,326,70,INK,'semibold')
board += lettering('Clear shares. Local history. A native Linux experience.',650,389,26,SLATE)
board += f'<g transform="translate(82 532) scale(1.1)">{horizontal()}</g>'
board += f'<rect x="720" y="470" width="816" height="210" rx="16" fill="{INK}"/><g transform="translate(788 525) scale(1.13)">{horizontal(WHITE,CYAN,(CYAN,"#93DCE1",WHITE))}</g>'
board += lettering('Desktop icon',82,759,21,SLATE)+lettering('Symbolic icon',338,759,21,SLATE)
board += f'<g transform="translate(80 780) scale(1.4)">{icon}</g><g transform="translate(344 822) scale(5)">{symbol(INK)}</g>'
for i,(name,color) in enumerate([('Ink',INK),('Signal',TEAL),('Signal light',CYAN),('Mist',MIST),('Slate',SLATE)]):
    x=650+i*178
    board+=f'<rect x="{x}" y="782" width="150" height="88" rx="8" fill="{color}"/>'
    board+=lettering(name,x,908,20,INK,'semibold')+lettering(color,x,940,18,SLATE,'mono')
board += '<path d="M64 1008H1536" stroke="#CCDADF"/>'
board += lettering('IBM Plex Sans',82,1058,28,INK,'semibold')+lettering('Aa Bb Cc  0123456789',420,1058,28,INK)
board += lettering('Flat web mark. Subtle GNOME depth. Outlined logo lettering.',82,1092,19,SLATE)
save('preview/brand-overview.svg',board,1600,1120,'drainscope energy shares brand overview')

tokens = {
 'version':'1.0.0', 'name':'drainscope',
 'brand':{'ink':INK,'signal':TEAL,'signalLight':CYAN,'signalMid':'#269EAA','mist':MIST,'white':WHITE,'slate':SLATE},
 'themes':{
  'light':{'background':MIST,'surface':WHITE,'text':INK,'textMuted':SLATE,'action':'#076F7C','onAction':WHITE,'focus':TEAL,'border':'#CCDADF','success':'#24734E','warning':'#8C5A10','danger':'#B13E4D'},
  'dark':{'background':INK,'surface':'#1D343C','text':MIST,'textMuted':'#A5B7BF','action':CYAN,'onAction':INK,'focus':CYAN,'border':'#47636D','success':'#77C9A0','warning':'#E9BE68','danger':'#F29AA7'}},
 'data':{'light':{'apps':TEAL,'terminals':'#7662AC','services':'#24734E','system':'#956414','idle':SLATE},'dark':{'apps':CYAN,'terminals':'#BAA4E8','services':'#77C9A0','system':'#E9BE68','idle':'#A5B7BF'}},
 'typography':{'sans':'IBM Plex Sans','mono':'IBM Plex Mono','weights':{'regular':400,'semibold':600},'scale':[12,14,16,20,24,32,48,64],'bodyLineHeight':1.55,'maxProseWidth':'68ch'},
 'spacing':[4,8,12,16,24,32,48,64,96], 'radius':{'control':6,'panel':12},
 'motion':{'durationMs':160,'easing':'ease-out','reducedMotion':'Disable decorative motion'},
 'native':'Use libadwaita styles and user-selected accent; web tokens do not override the desktop theme.'}
token_dir=ROOT/'tokens';token_dir.mkdir(exist_ok=True)
(token_dir/'brand.json').write_text(json.dumps(tokens,indent=2)+'\n')
css='/* drainscope v1: web and documentation tokens. Native GNOME uses libadwaita. */\n'
css+='@font-face{font-family:"IBM Plex Sans";src:url("../fonts/IBMPlexSans-Regular.ttf") format("truetype");font-weight:400;font-display:swap;}\n'
css+='@font-face{font-family:"IBM Plex Sans";src:url("../fonts/IBMPlexSans-SemiBold.ttf") format("truetype");font-weight:600;font-display:swap;}\n'
css+='@font-face{font-family:"IBM Plex Mono";src:url("../fonts/IBMPlexMono-Regular.ttf") format("truetype");font-weight:400;font-display:swap;}\n'
def kebab(value):
    return ''.join('-'+c.lower() if c.isupper() else c for c in value)
for theme in ['light','dark']:
    css+=(':root, [data-theme="light"]' if theme=='light' else '[data-theme="dark"]')+'{\n'
    for k,v in tokens['themes'][theme].items(): css+=f'  --ds-{kebab(k)}:{v};\n'
    for k,v in tokens['data'][theme].items(): css+=f'  --ds-data-{k}:{v};\n'
    css+='}\n'
css+=':root{--ds-font-sans:"IBM Plex Sans",system-ui,sans-serif;--ds-font-mono:"IBM Plex Mono",monospace;--ds-radius-control:6px;--ds-radius-panel:12px;}\n'
css+='@media(prefers-color-scheme:dark){:root:not([data-theme]){'+''.join(f'--ds-{kebab(k)}:{v};' for k,v in tokens['themes']['dark'].items())+''.join(f'--ds-data-{k}:{v};' for k,v in tokens['data']['dark'].items())+'}}\n'
css+='@media(prefers-reduced-motion:reduce){.ds-motion{animation:none!important;transition:none!important;}}\n'
(token_dir/'brand.css').write_text(css)

source = ROOT/'source'
word_source = '<text x="152" y="70" font-family="IBM Plex Sans" font-size="68" font-weight="600" letter-spacing="-.25" fill="#15282F">drainscope</text>'
(source/'drainscope-logo-editable.svg').write_text(svg(mark()+word_source,LOGO_W,96,'Editable drainscope logo; install included IBM Plex Sans SemiBold'))

template = f'<rect width="1600" height="1100" fill="{MIST}"/>'
template += f'<g transform="translate(64 42) scale(.65)">{horizontal()}</g>'
template += lettering('Screenshot title',64,181,48,INK,'semibold')
template += lettering('Replace this title and insert a real Linux screenshot below.',64,228,24,SLATE)
template += '<rect x="64" y="278" width="1472" height="740" rx="12" fill="#FFFFFF" stroke="#CCDADF" stroke-width="2"/>'
template += lettering('Screenshot insertion area',540,672,28,SLATE)
(ROOT/'templates').mkdir(exist_ok=True)
(ROOT/'templates/screenshot-frame.svg').write_text(svg(template,1600,1100,'Editable screenshot framing template'))

diagram = f'<rect width="1100" height="470" fill="{WHITE}"/>'
diagram += lettering('From measurement to understanding',40,65,32,INK,'semibold')
for x,title,lines in [(40,'Measure',['Hardware counters','and battery readings']),(400,'Attribute',['Estimated energy shares','by app and consumer']),(760,'Explore',['Local history in GNOME','and the command line'])]:
    diagram+=f'<rect x="{x}" y="136" width="300" height="180" rx="12" fill="{MIST}" stroke="#CCDADF"/>'
    diagram+=lettering(title,x+24,186,26,TEAL,'semibold')
    for i,line in enumerate(lines):diagram+=lettering(line,x+24,240+i*30,19,INK)
    if x<760:diagram+=f'<path d="M{x+310} 226H{x+346}M{x+336} 218L{x+346} 226L{x+336} 234" fill="none" stroke="{TEAL}" stroke-width="3"/>'
diagram+=lettering('Measured totals and estimated attribution are distinct. Hardware support varies.',40,402,21,SLATE)
save('web/measurement-diagram.svg',diagram,1100,470,'Simplified drainscope measurement and attribution flow')

print(f'Generated {len(list(EXPORTS.rglob("*.svg")))} production SVGs in {EXPORTS}')
