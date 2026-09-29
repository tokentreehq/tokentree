// SPDX-License-Identifier: Apache-2.0
import esbuild from 'esbuild';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { cpSync, mkdirSync } from 'node:fs';

const here = dirname(fileURLToPath(import.meta.url));
const cliRoot = resolve(here, '..');
const repoRoot = resolve(cliRoot, '../..');

mkdirSync(resolve(cliRoot, 'migrations'), { recursive: true });
cpSync(resolve(repoRoot, 'packages/database/migrations'), resolve(cliRoot, 'migrations'), { recursive: true });

mkdirSync(resolve(cliRoot, 'data'), { recursive: true });
cpSync(resolve(repoRoot, 'packages/pricing/data'), resolve(cliRoot, 'data'), { recursive: true });

await esbuild.build({
  entryPoints: [resolve(cliRoot, 'src/main.ts')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  outfile: resolve(cliRoot, 'dist/main.js'),
  external: ['better-sqlite3'],
});

await esbuild.build({
  entryPoints: [resolve(cliRoot, 'src/index.ts')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  outfile: resolve(cliRoot, 'dist/index.js'),
  external: ['better-sqlite3'],
});
