// SPDX-License-Identifier: Apache-2.0
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { loadPriceSnapshot } from '../src/index.js';

const fixture = new URL('../data/prices.json', import.meta.url).pathname;

describe('price snapshots', () => {
  it('accepts a correctly hashed snapshot', () => {
    expect(loadPriceSnapshot(fixture).models).toEqual([]);
  });
  it('rejects tampering', () => {
    const target = join(mkdtempSync(join(tmpdir(), 'tokentree-')), 'prices.json');
    copyFileSync(fixture, target);
    const changed = readFileSync(target, 'utf8').replace('2026-09-29', '2026-09-30');
    writeFileSync(target, changed);
    expect(() => loadPriceSnapshot(target)).toThrow(/hash mismatch/);
  });
});
