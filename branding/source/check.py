"""Validate exported geometry, font-free logos, raster sizes and token pairs."""
from pathlib import Path
from xml.etree import ElementTree as ET
import hashlib
import json
from PIL import Image

root=Path(__file__).resolve().parents[1]
tokens=json.loads((root/'tokens/brand.json').read_text())
def luminance(h):
    c=[int(h[i:i+2],16)/255 for i in (1,3,5)]
    c=[v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4 for v in c]
    return sum(v*w for v,w in zip(c,[.2126,.7152,.0722]))
def contrast(a,b):
    low,high=sorted([luminance(a),luminance(b)])
    return (high+.05)/(low+.05)
checks=[]
for theme,t in tokens['themes'].items():
    for surface in ['background','surface']:
        for role in ['text','textMuted','action','success','warning','danger']:
            ratio=contrast(t[role],t[surface]);assert ratio>=4.5,(theme,role,surface,ratio)
            checks.append({'theme':theme,'foreground':role,'background':surface,'ratio':round(ratio,2),'threshold':4.5})
        ratio=contrast(t['focus'],t[surface]);assert ratio>=3
        checks.append({'theme':theme,'foreground':'focus','background':surface,'ratio':round(ratio,2),'threshold':3})
    ratio=contrast(t['onAction'],t['action']);assert ratio>=4.5
    checks.append({'theme':theme,'foreground':'onAction','background':'action','ratio':round(ratio,2),'threshold':4.5})
    for role,value in tokens['data'][theme].items():
        ratio=contrast(value,t['surface']);assert ratio>=3
        checks.append({'theme':theme,'foreground':'data.'+role,'background':'surface','ratio':round(ratio,2),'threshold':3})
svg_count=0
for file in (root/'exports').rglob('*.svg'):
    element=ET.parse(file).getroot();svg_count+=1
    assert element.get('viewBox') and element.get('width') and element.get('height'),file
    assert not any(e.tag.endswith('script') for e in element.iter()),file
    if file.parent.name=='logo':assert not any(e.tag.endswith('text') for e in element.iter()),file
png_count=0
for file in (root/'exports').rglob('*.png'):
    image=Image.open(file);image.verify();png_count+=1
    if file.parent.name=='logo':
        image=Image.open(file).convert('RGBA');assert image.getpixel((0,0))[3]==0,file
expected={'github-social-light.png':(1280,640),'github-social-dark.png':(1280,640),'website-social-light.png':(1200,630),'website-social-dark.png':(1200,630),'readme-header.png':(1280,360)}
for file,size in expected.items():assert Image.open(root/'exports/social'/file).size==size,file
assert Image.open(root/'exports/web/apple-touch-icon.png').size==(180,180)
assert Image.open(root/'exports/web/favicon.ico').size==(48,48)
sources={'IBMPlexSans-Regular.ttf':'https://raw.githubusercontent.com/IBM/plex/master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-Regular.ttf','IBMPlexSans-SemiBold.ttf':'https://raw.githubusercontent.com/IBM/plex/master/packages/plex-sans/fonts/complete/ttf/IBMPlexSans-SemiBold.ttf','IBMPlexMono-Regular.ttf':'https://raw.githubusercontent.com/IBM/plex/master/packages/plex-mono/fonts/complete/ttf/IBMPlexMono-Regular.ttf'}
provenance={'date':'2026-10-06','projectCommit':'ae75654927ce39c33b54a2c07a368b28499c8fcb','artwork':'Original editable SVG construction from the selected Energy shares direction.','fonts':[{'file':'fonts/'+name,'source':url,'sha256':hashlib.sha256((root/'fonts'/name).read_bytes()).hexdigest()} for name,url in sources.items()]}
(root/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
report={'version':'1.0.0','date':'2026-10-06','svgCount':svg_count,'pngCount':png_count,'checks':checks,'notes':['Token-pair contrast is not a full accessibility audit of future implementations.','PNG logos have transparent corners; social cards have intentional backgrounds.','Actual GNOME desktop context and real product screenshots await Linux review.']}
(root/'validation.json').write_text(json.dumps(report,indent=2)+'\n')
print(f'Validated {svg_count} SVGs, {png_count} PNGs, outlined logos, ICO, dimensions, transparency, and {len(checks)} contrast pairs.')
