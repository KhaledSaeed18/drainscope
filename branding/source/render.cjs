// Raster exports from SVG originals. npm install in source/ first.
const fs = require('node:fs/promises');
const path = require('node:path');
const sharp = require('sharp');
const root = path.resolve(__dirname, '..');
const exportsDir = path.join(root, 'exports');
async function raster(relative, width, target) {
  const input = path.join(exportsDir, relative);
  const output = path.join(exportsDir, target);
  await fs.mkdir(path.dirname(output), { recursive: true });
  await sharp(input, { density: 300 }).resize({ width }).png().toFile(output);
}
async function main() {
  for (const variant of ['primary', 'dark', 'mono', 'white']) {
    for (const type of ['mark','horizontal','stacked','wordmark']) {
      const file=`logo/drainscope-${type}-${variant}.svg`;
      await raster(file, type==='mark'?512:1200, file.replace('.svg','.png'));
    }
  }
  const id='io.github.khaledsaeed18.Drainscope';
  for(const size of [32,48,64,128,256,512])
    await raster(`app-icon/${id}.svg`,size,`app-icon/png/drainscope-${size}.png`);
  for(const size of [16,32,64])
    await raster(`app-icon/${id}-symbolic.svg`,size,`app-icon/png/drainscope-symbolic-${size}.png`);
  for(const size of [16,32,48])
    await raster('web/favicon.svg',size,`web/favicon-${size}.png`);
  await raster('web/apple-touch-icon.svg',180,'web/apple-touch-icon.png');
  await raster('web/apple-touch-icon.svg',192,'web/icon-192.png');
  await raster('web/apple-touch-icon.svg',512,'web/icon-512.png');
  for(const name of ['github-social-light','github-social-dark','website-social-light','website-social-dark','readme-header'])
    await raster(`social/${name}.svg`,name.startsWith('website')?1200:1280,`social/${name}.png`);
  await raster('preview/brand-overview.svg',1600,'preview/brand-overview.png');
  // ICO with embedded PNG payloads: supported by modern desktop browsers.
  const payloads=await Promise.all([16,32,48].map(s=>fs.readFile(path.join(exportsDir,`web/favicon-${s}.png`))));
  const head=Buffer.alloc(6+16*payloads.length);head.writeUInt16LE(1,2);head.writeUInt16LE(payloads.length,4);
  let offset=head.length;
  payloads.forEach((png,i)=>{const s=[16,32,48][i],at=6+i*16;head[at]=s;head[at+1]=s;head.writeUInt16LE(1,at+4);head.writeUInt16LE(32,at+6);head.writeUInt32LE(png.length,at+8);head.writeUInt32LE(offset,at+12);offset+=png.length;});
  await fs.writeFile(path.join(exportsDir,'web/favicon.ico'),Buffer.concat([head,...payloads]));
  const entries=[];
  async function scan(dir){for(const e of await fs.readdir(dir,{withFileTypes:true})){const p=path.join(dir,e.name);if(e.isDirectory())await scan(p);else {const stat=await fs.stat(p);entries.push({path:path.relative(root,p),bytes:stat.size});}}}
  await scan(exportsDir);
  await fs.writeFile(path.join(root,'asset-manifest.json'),JSON.stringify({version:'1.0.0',assets:entries},null,2)+'\n');
  console.log(`Rendered exports and recorded ${entries.length} assets.`);
}
main().catch(e=>{console.error(e);process.exitCode=1;});
