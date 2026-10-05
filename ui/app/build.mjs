// Bundles the app into dist/drainscope-app, an executable GJS ES module.
import { build } from 'esbuild';
import { chmodSync, rmSync } from 'node:fs';

rmSync('dist', { recursive: true, force: true });
await build({
  entryPoints: ['src/main.ts'],
  outfile: 'dist/drainscope-app',
  bundle: true,
  format: 'esm',
  target: 'firefox128', // GJS 1.88 runs SpiderMonkey 140
  external: ['gi://*', 'resource://*', 'gettext', 'system', 'cairo'],
  banner: { js: '#!/usr/bin/env -S gjs -m' },
  legalComments: 'inline',
  logLevel: 'info',
});
chmodSync('dist/drainscope-app', 0o755);
