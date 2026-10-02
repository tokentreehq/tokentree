// SPDX-License-Identifier: Apache-2.0
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseGrokSession, ticksToMicros } from '../src/index.js';

const root = resolve(import.meta.dirname, '../../../../');

describe('Grok adapter parser', () => {
  it('converts ticks to micros accurately', () => {
    expect(ticksToMicros(1000)).toBe(1);
    expect(ticksToMicros(19099100000)).toBe(19099100);
  });

  it('parses single turn Grok fixture with exact tokens and costs', async () => {
    const fixture = resolve(root, 'fixtures/parsers/grok/single-turn.json');
    const result = await parseGrokSession({ sourcePath: fixture, adapter: 'grok' });

    expect(result.stats.parsed).toBe(1);
    expect(result.stats.malformed).toBe(0);
    expect(result.observations).toHaveLength(1);

    const obs = result.observations[0];
    expect(obs.adapter).toBe('grok');
    expect(obs.sourceSubtype).toBe('grok_turn_usage');
    expect(obs.turnId).toBe('turn_1');
    expect(obs.inputTokens).toBe(2471317);
    expect(obs.outputTokens).toBe(36242);
    expect(obs.cachedInputTokens).toBe(2166784);
    expect(obs.reasoningTokens).toBe(27450);
    expect(obs.providerReportedCostMicros).toBe(19099100);
  });

  it('parses multi-turn Grok fixture and suppresses session-level counter', async () => {
    const fixture = resolve(root, 'fixtures/parsers/grok/multi-turn.json');
    const result = await parseGrokSession({ sourcePath: fixture, adapter: 'grok' });

    expect(result.stats.parsed).toBe(2);
    expect(result.observations).toHaveLength(2);
    expect(result.observations[0].turnId).toBe('turn_1');
    expect(result.observations[1].turnId).toBe('turn_2');
  });

  it('handles zero tokens failed runs cleanly', async () => {
    const fixture = resolve(root, 'fixtures/parsers/grok/zero-tokens-failed.json');
    const result = await parseGrokSession({ sourcePath: fixture, adapter: 'grok' });

    expect(result.stats.parsed).toBe(1);
    expect(result.stats.anomalies).toBe(1);
    expect(result.anomalies[0].type).toBe('missing_provider_measurements');
    expect(result.observations[0].source).toBe('unavailable');
    expect(result.observations[0].sourceSubtype).toBe('grok_turn_failed');
    expect(result.observations[0].inputTokens).toBeNull();
    expect(result.observations[0].outputTokens).toBeNull();
  });

  it('detects malformed json safely', async () => {
    const fixture = resolve(root, 'fixtures/parsers/grok/adversarial/corrupted.json');
    const result = await parseGrokSession({ sourcePath: fixture, adapter: 'grok' });

    expect(result.stats.malformed).toBe(1);
    expect(result.observations).toHaveLength(0);
    expect(result.anomalies[0].type).toBe('malformed_record');
  });

  it('detects category sum mismatch', async () => {
    const fixture = resolve(root, 'fixtures/parsers/grok/adversarial/sum-mismatch.json');
    const result = await parseGrokSession({ sourcePath: fixture, adapter: 'grok' });

    expect(result.stats.anomalies).toBe(1);
    expect(result.anomalies[0].type).toBe('token_sum_mismatch');
  });
});
