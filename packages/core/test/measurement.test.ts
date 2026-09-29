// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { deduplicateObservations, snapshotDelta, tokenCompleteness, type UsageObservation } from '../src/index.js';
const base: UsageObservation = { adapter:'claude', source:'transcript_request', providerSessionId:'s', requestId:'r', observedAt:'2026-01-01', inputTokens:10, cachedInputTokens:2, cacheWriteTokens:1, outputTokens:3, reasoningTokens:null, sourcePath:'x', sourceOffset:0, adapterVersion:'0', parserVersion:'0' };
describe('measurement truth', () => {
  it('keeps one request and prefers official telemetry', () => {
    const official = { ...base, source:'official_telemetry' as const, inputTokens:11 };
    const result = deduplicateObservations([base, official]);
    expect(result.canonical).toEqual([official]);
    expect(result.conflicts).toHaveLength(1);
  });
  it('marks negative deltas unavailable rather than zero', () => {
    expect(snapshotDelta(base, { ...base, inputTokens:9 })).toEqual({ usage:null, negative:true });
  });
  it('implements the normative completeness formula', () => {
    expect(tokenCompleteness({ measuredRequests:8, unavailableRequests:1, anomalousRequests:1 })).toBe(80);
    expect(tokenCompleteness({ measuredRequests:0, unavailableRequests:0, anomalousRequests:0 })).toBeNull();
  });
});
