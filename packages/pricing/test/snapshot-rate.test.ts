// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import{describe,expect,it}from'vitest';import{loadPriceSnapshot,resolveSnapshotRate}from'../src/index.js';
const path = fileURLToPath(new URL('../data/prices.json', import.meta.url));
describe('verified shipped rates',()=>{it('resolves the documented Sonnet 4.6 categories',()=>{const rate=resolveSnapshotRate(loadPriceSnapshot(path),'claude-sonnet-4-6',new Date('2026-09-29'));expect(rate).toMatchObject({inputPerMillion:'3.00',cachedInputPerMillion:'0.30',cacheWritePerMillion:'3.75',outputPerMillion:'15.00'});});it('keeps unknown models unavailable',()=>expect(resolveSnapshotRate(loadPriceSnapshot(path),'unknown')).toBeNull());});
