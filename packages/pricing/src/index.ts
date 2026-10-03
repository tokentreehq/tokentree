// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

export interface PriceRate {
  readonly provider: string;
  readonly modelPattern: string;
  readonly effectiveFrom: string;
  readonly inputPerMillion: string;
  readonly cachedInputPerMillion?: string;
  readonly cacheWritePerMillion?: string;
  readonly outputPerMillion: string;
  readonly source: string;
}

export interface PriceSnapshot {
  readonly version: 1;
  readonly updated: string;
  readonly currency: 'USD';
  readonly models: readonly PriceRate[];
  readonly sha256: string;
}

function canonicalPayload(snapshot: Omit<PriceSnapshot, 'sha256'>): string {
  return JSON.stringify(snapshot);
}

export function snapshotHash(snapshot: Omit<PriceSnapshot, 'sha256'>): string {
  return createHash('sha256').update(canonicalPayload(snapshot)).digest('hex');
}

export function loadPriceSnapshot(path: string): PriceSnapshot {
  const parsed: unknown = JSON.parse(readFileSync(path, 'utf8'));
  if (!isPriceSnapshot(parsed)) throw new Error('Invalid TokenTree price snapshot shape');
  const { sha256, ...payload } = parsed;
  const actual = snapshotHash(payload);
  if (actual !== sha256) throw new Error(`Price snapshot hash mismatch: expected ${sha256}, got ${actual}`);
  return parsed;
}

function isPriceSnapshot(value: unknown): value is PriceSnapshot {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return candidate.version === 1 && typeof candidate.updated === 'string' && candidate.currency === 'USD' && Array.isArray(candidate.models) && typeof candidate.sha256 === 'string';
}
export function resolveSnapshotRate(snapshot: PriceSnapshot, model: string, at = new Date()): PriceRate | null {
  const candidates=snapshot.models.filter((rate)=>{
    const matches=rate.modelPattern.endsWith('*')?model.startsWith(rate.modelPattern.slice(0,-1)):model===rate.modelPattern;
    return matches&&new Date(rate.effectiveFrom).getTime()<=at.getTime();
  });
  return candidates.sort((a,b)=>b.effectiveFrom.localeCompare(a.effectiveFrom))[0]??null;
}
