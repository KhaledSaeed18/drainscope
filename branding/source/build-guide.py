"""Build the portable visual guide after build.py/render.cjs and browser previews."""
from pathlib import Path
import json
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.lib.colors import HexColor
from reportlab.lib.utils import ImageReader
from reportlab.graphics import renderPDF
from svglib.svglib import svg2rlg

ROOT=Path(__file__).resolve().parents[1]
W,H=900,640
INK,TEAL,CYAN,MIST,SLATE='#15282F','#087F8C','#53C6CE','#F3F7F7','#60747C'
for name,file in [('Plex','IBMPlexSans-Regular.ttf'),('PlexSemi','IBMPlexSans-SemiBold.ttf'),('PlexMono','IBMPlexMono-Regular.ttf')]:
    pdfmetrics.registerFont(TTFont(name,str(ROOT/'fonts'/file)))
c=canvas.Canvas(str(ROOT/'drainscope-brand-guide.pdf'),pagesize=(W,H))
c.setTitle('drainscope - Energy shares brand guide v1')
c.setAuthor('drainscope')
c.setSubject('Identity, typography, color, icons, digital patterns and Linux handoff')
page=0

def text(value,x,y,size=15,color=INK,font='Plex'):
    c.setFillColor(HexColor(color));c.setFont(font,size);c.drawString(x,y,value)

def paragraph(value,x,y,width=365,size=15,color=INK,font='Plex',leading=None):
    leading=leading or size*1.48
    words=value.split();lines=[];line=''
    for word in words:
        candidate=(line+' '+word).strip()
        if pdfmetrics.stringWidth(candidate,font,size)>width and line:lines.append(line);line=word
        else:line=candidate
    if line:lines.append(line)
    for line in lines:text(line,x,y,size,color,font);y-=leading
    return y

def image(relative,x,y,width,height):
    vector=(ROOT/relative).with_suffix('.svg')
    if vector.exists():
        drawing=svg2rlg(str(vector));iw,ih=drawing.width,drawing.height;scale=min(width/iw,height/ih)
        c.saveState();c.translate(x+(width-iw*scale)/2,y+(height-ih*scale)/2);c.scale(scale,scale)
        renderPDF.draw(drawing,c,0,0);c.restoreState();return
    picture=ImageReader(str(ROOT/relative));iw,ih=picture.getSize();scale=min(width/iw,height/ih)
    c.drawImage(picture,x+(width-iw*scale)/2,y+(height-ih*scale)/2,width=iw*scale,height=ih*scale,mask='auto')

def start(title,subtitle):
    global page
    page+=1
    c.setFillColor(HexColor('#FFFFFF'));c.rect(0,0,W,H,fill=1,stroke=0)
    text('drainscope',48,598,20,INK,'PlexSemi')
    text('Energy shares / Identity v1',610,598,13,SLATE)
    c.setStrokeColor(HexColor('#CCDADF'));c.line(48,576,852,576)
    text(title,48,527,32,INK,'PlexSemi')
    paragraph(subtitle,48,494,790,14,SLATE)

def end():
    c.setStrokeColor(HexColor('#CCDADF'));c.line(48,44,852,44)
    text('6 October 2026 / Complete specifications in guidelines.md',48,25,11,SLATE)
    text(str(page),835,25,11,SLATE)
    c.showPage()

page=1
image('exports/preview/brand-overview.png',24,20,852,596)
c.showPage()

start('A clear mark, with room to breathe.','Unequal shares in a battery enclosure. A consistent baseline. Flat artwork for identity use.')
image('exports/logo/drainscope-horizontal-primary.png',55,335,370,105)
c.setFillColor(HexColor(INK));c.roundRect(455,330,390,125,12,fill=1,stroke=0)
image('exports/logo/drainscope-horizontal-dark.png',473,341,350,95)
image('exports/logo/drainscope-stacked-mono.png',92,137,160,156)
image('exports/logo/drainscope-mark-primary.png',313,180,122,95)
y=paragraph('Clear space',480,294,340,20,INK,'PlexSemi')-7
y=paragraph('Leave 24 px around a lockup with a 96 px-high mark. Scale that space proportionally.',480,y,350,14)-12
y=paragraph('Minimum digital widths',480,y,350,18,INK,'PlexSemi')-7
y=paragraph('Horizontal 180 px; stacked 160 px; wordmark 140 px. Full mark 48 px. Symbolic 16-32 px.',480,y,350,14)-12
paragraph('Preserve proportions, spacing, share order, and supplied colors. Avoid shadows and busy backgrounds.',480,y,350,13,SLATE)
end()

start('Signal, with restraint.','Separate brand colors from action, status, and data roles. Check the specific combinations you use.')
for i,(label,color) in enumerate([('Ink',INK),('Signal',TEAL),('Signal light',CYAN),('Mist',MIST),('White','#FFFFFF'),('Slate',SLATE)]):
    x=48+i*135
    c.setFillColor(HexColor(color));c.setStrokeColor(HexColor('#CCDADF'));c.roundRect(x,350,115,100,8,fill=1,stroke=1)
    text(label,x,328,15,INK,'PlexSemi');text(color,x,306,13,SLATE,'PlexMono')
y=paragraph('Readable actions',48,260,365,22,INK,'PlexSemi')-10
y=paragraph('Use deep teal #076F7C for small links and button backgrounds on light surfaces. Keep #087F8C as the brand mark color.',48,y,365)-14
paragraph('On dark surfaces, use cyan actions with Ink button text. Slate is muted text on white and Mist. Subtle borders are decorative, not sufficient control boundaries.',48,y,365,14,SLATE)
report=json.loads((ROOT/'validation.json').read_text())
rows=[('Ink on Mist','light','text','background'),('Slate on Mist','light','textMuted','background'),('White on deep teal','light','onAction','action'),('Cyan on Ink','dark','action','background'),('Muted text on Ink','dark','textMuted','background')]
text('Verified text combinations',470,260,22,INK,'PlexSemi')
for i,(label,theme,fg,bg) in enumerate(rows):
    ratio=next(x['ratio'] for x in report['checks'] if x['theme']==theme and x['foreground']==fg and x['background']==bg)
    y=218-i*32;text(label,470,y,14);text(f'{ratio:.2f}:1',767,y,14,TEAL,'PlexMono')
paragraph('Ratios apply to these token pairs. Chart categories also need labels and non-color cues where necessary.',470,70,360,10,SLATE)
end()

start('Technical clarity. Human language.','IBM Plex Sans for web and documentation; IBM Plex Mono for commands. GNOME retains system typography.')
text('Aa Bb Cc',48,390,60,INK,'PlexSemi');text('0123456789',48,335,40)
y=paragraph('Regular 400 / SemiBold 600',48,285,360,18,TEAL,'PlexSemi')-14
y=paragraph('Body 16 px, secondary 14 px. Headings 32-64 px. Prose line height around 1.55, with a maximum width of 68 characters.',48,y,360)-14
text('drainscope doctor',48,136,18,INK,'PlexMono')
text('drainscope report --since 24h',48,102,16,INK,'PlexMono')
y=paragraph('See where your battery goes.',470,415,365,30,INK,'PlexSemi')-24
y=paragraph('Battery and energy usage by app, for the Linux desktop.',470,y,365,19)-22
y=paragraph('Use the lowercase project name and sentence case. Explain measured totals and estimated shares separately. State hardware support and limitations.',470,y,365)-18
paragraph('Describe observation and diagnosis. Keep claims grounded in the product; do not promise automatic savings or exact per-process energy.',470,y,365,14,SLATE)
end()

start('At home in GNOME.','A dedicated full-color icon, a small symbolic, and a distinct development-build variant.')
for x,size,label in [(74,160,'Full-color'),(330,96,'Compact'),(504,48,'Small')]:
    image('exports/app-icon/io.github.khaledsaeed18.Drainscope.svg',x,272,size,size)
    text(label,x,243,13,SLATE)
image('exports/app-icon/io.github.khaledsaeed18.Drainscope-symbolic.svg',665,326,64,64)
text('16 px symbolic source',636,243,13,SLATE)
y=paragraph('App icon',48,192,365,20,INK,'PlexSemi')-10
paragraph('128 px canvas, 2 px grid, 4 px darker front profile, and y=116 bottom baseline. No external shadow. Review at 128, 64, and 32 px on Linux.',48,y,365,14)
y=paragraph('Respect the desktop',470,192,365,20,INK,'PlexSemi')-10
paragraph('Retain libadwaita surfaces, system fonts, user-selected accent, and native controls. GTK recolors the 16 px symbolic. Real desktop context still needs Linux review.',470,y,365,14)
end()

start('Website direction.','Lead with the promise, a real app view when available, and a maintained installation path.')
image('exports/preview/website-study-light.png',48,214,804,240)
y=paragraph('A direct product story',48,176,365,21,INK,'PlexSemi')-12
paragraph('Explain apps and terminal workloads, local history, and GNOME/CLI access. Add genuine screenshots from Linux before the public website launch.',48,y,365,14)
y=paragraph('Built from shared tokens',470,176,365,21,INK,'PlexSemi')-12
paragraph('Use self-hosted fonts, responsive layouts, accessible actions, visible focus, and reduced motion. The included HTML is a layout study, not a published site.',470,y,365,14)
end()

start('Documentation direction.','Readable prose, explicit commands, and concrete explanations of measurement limits.')
image('exports/preview/docs-study-light.png',48,174,804,280)
y=paragraph('Measured totals, estimated shares',48,139,365,20,INK,'PlexSemi')-10
paragraph('Use callouts to explain a specific limitation or next action. Keep support and validation notes visible.',48,y,365,13)
y=paragraph('A useful information structure',470,139,365,20,INK,'PlexSemi')-10
paragraph('Installation, hardware support, attribution, validation, troubleshooting, CLI reference, and contribution guidance.',470,y,365,13)
end()

start('Maintain the identity with the project.','Sources, exports, tokens, templates, licensing, and a focused Linux handoff.')
y=paragraph('Ready assets',48,416,365,23,INK,'PlexSemi')-14
for value in ['Outlined SVG logo variants and transparent PNGs.','GNOME app, symbolic, and development icons.','Favicons, touch icons, and web app icons.','README header and light/dark social cards.','Screenshot frame and release copy template.','JSON/CSS tokens and bundled Plex fonts.']:
    y=paragraph(value,48,y,365,15)-12
y=paragraph('Reproduce and integrate',470,416,365,23,INK,'PlexSemi')-14
y=paragraph('source/build.py generates vectors. source/render.cjs generates rasters and the manifest. source/check.py verifies the specified geometry and color pairs.',470,y,365)-16
y=paragraph('The app and symbolic icons ship from data/app/icons under the stable app ID; CI keeps them identical to exports/app-icon. Regenerate here, then copy both icons there.',470,y,365,14)-16
y=paragraph('Next: real product screenshots, then the website/docs build and destination-specific listing assets.',470,y,365,14,SLATE)-16
paragraph('Font licenses are included. A separate public license for original brand artwork has not been selected. Full references and exact rules are in guidelines.md.',470,y,365,12,SLATE)
end()
c.save()
print(f'Created {page}-page brand guide: {ROOT / "drainscope-brand-guide.pdf"}')
