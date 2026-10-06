// Bundles the app into dist/drainscope-app, an executable GJS ES module. With --install, also
// installs it for the current user with its desktop entry and icons (under ~/.local).
import { build } from 'esbuild';
import { chmodSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const ID = 'io.github.khaledsaeed18.Drainscope';
const DATA = '../../data/app';

rmSync('dist', { recursive: true, force: true });
await build({
  entryPoints: ['src/main.ts'],
  outfile: 'dist/drainscope-app',
  bundle: true,
  format: 'esm',
  target: 'firefox128', // GJS 1.88 runs SpiderMonkey 140
  external: ['gi://*', 'resource://*', 'gettext', 'system', 'cairo'],
  // A direct path: rpm rewrites /usr/bin/env shebangs, and -m is the only argument.
  banner: { js: '#!/usr/bin/gjs -m' },
  legalComments: 'inline',
  logLevel: 'info',
});
chmodSync('dist/drainscope-app', 0o755);

if (process.argv.includes('--install')) {
  const local = join(homedir(), '.local');
  const bin = join(local, 'bin/drainscope-app');
  mkdirSync(join(local, 'bin'), { recursive: true });
  cpSync('dist/drainscope-app', bin);
  const applications = join(local, 'share/applications');
  mkdirSync(applications, { recursive: true });
  // An absolute Exec, since ~/.local/bin may not be on the session's PATH.
  const desktop = readFileSync(join(DATA, `${ID}.desktop`), 'utf8').replace(/^Exec=.*$/m, `Exec=${bin}`);
  writeFileSync(join(applications, `${ID}.desktop`), desktop);
  for (const [dir, name] of [['scalable', `${ID}.svg`], ['symbolic', `${ID}-symbolic.svg`]]) {
    const icons = join(local, `share/icons/hicolor/${dir}/apps`);
    mkdirSync(icons, { recursive: true });
    cpSync(join(DATA, `icons/${name}`), join(icons, name));
  }
  console.log(`installed ${bin}`);
}
