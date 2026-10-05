// Bundles the extension into dist/ as unminified ESM (extensions.gnome.org requires readable
// code). With --install, also copies it into ~/.local/share/gnome-shell/extensions/<uuid>/.
import { build } from 'esbuild';
import { cpSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const metadata = JSON.parse(readFileSync('metadata.json', 'utf8'));

rmSync('dist', { recursive: true, force: true });
await build({
  entryPoints: ['src/extension.ts'],
  outfile: 'dist/extension.js',
  bundle: true,
  format: 'esm',
  target: 'firefox128', // GJS 1.88 runs SpiderMonkey 140
  external: ['gi://*', 'resource://*', 'gettext', 'system', 'cairo'],
  legalComments: 'inline',
  logLevel: 'info',
});
cpSync('metadata.json', 'dist/metadata.json');
cpSync('stylesheet.css', 'dist/stylesheet.css');

if (process.argv.includes('--install')) {
  const target = join(homedir(), '.local/share/gnome-shell/extensions', metadata.uuid);
  rmSync(target, { recursive: true, force: true });
  mkdirSync(target, { recursive: true });
  cpSync('dist', target, { recursive: true });
  console.log(`installed to ${target}`);
}
