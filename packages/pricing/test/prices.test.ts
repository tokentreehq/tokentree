// SPDX-License-Identifier: Apache-2.0
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { loadPriceSnapshot } from '../src/index.js';

const fixture = fileURLToPath(new URL('../data/prices.json', import.meta.url));

describe('price snapshots', () => {
  it('accepts a correctly hashed snapshot', () => {
    expect(loadPriceSnapshot(fixture).models).toHaveLength(1);
  });
  it('rejects tampering', () => {
    const target = join(mkdtempSync(join(tmpdir(), 'tokentree-')), 'prices.json');
    copyFileSync(fixture, target);
    const changed = readFileSync(target, 'utf8').replace('2026-09-29', '2026-09-30');
    writeFileSync(target, changed);
    expect(() => loadPriceSnapshot(target)).toThrow(/hash mismatch/);
  });
});
