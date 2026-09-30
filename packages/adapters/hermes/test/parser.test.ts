// SPDX-License-Identifier: Apache-2.0
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseHermesSession, usdToMicros } from '../src/index.js';

const root = resolve(import.meta.dirname, '../../../../');

describe('Hermes adapter parser', () => {
  it('converts usd to micros accurately', () => {
    expect(usdToMicros(0.00011)).toBe(110);
    expect(usdToMicros(0)).toBe(0);
    expect(usdToMicros(1.5)).toBe(1500000);
  });

  it('parses oneshot Hermes usage fixture with auxiliary task breakdown', async () => {
    const fixture = resolve(root, 'fixtures/parsers/hermes/oneshot-usage.json');
    const result = await parseHermesSession({ sourcePath: fixture, adapter: 'hermes' });

    expect(result.stats.parsed).toBe(2);
    expect(result.stats.malformed).toBe(0);
    expect(result.observations).toHaveLength(2);

    const mainObs = result.observations[0];
    expect(mainObs.adapter).toBe('hermes');
    expect(mainObs.sourceSubtype).toBe('hermes_oneshot_usage');
    expect(mainObs.providerSessionId).toBe('20260930_194647_5766b6');
    expect(mainObs.model).toBe('liquid/lfm-2.5-2.6b:free');
    expect(mainObs.inputTokens).toBe(13835);
    expect(mainObs.outputTokens).toBe(40);
    expect(mainObs.cachedInputTokens).toBe(576);
    expect(mainObs.reasoningTokens).toBe(29);
    expect(mainObs.providerReportedCostMicros).toBe(0);

    const auxObs = result.observations[1];
    expect(auxObs.sourceSubtype).toBe('hermes_auxiliary_title_generation');
    expect(auxObs.turnId).toBe('task_title_generation');
    expect(auxObs.inputTokens).toBe(249);
    expect(auxObs.outputTokens).toBe(464);
    expect(auxObs.reasoningTokens).toBe(453);
    expect(auxObs.parentAgentId).toBe('hermes:20260930_194647_5766b6');
  });

  it('handles failed oneshot runs cleanly with zero tokens', async () => {
    const fixture = resolve(root, 'fixtures/parsers/hermes/oneshot-failed.json');
    const result = await parseHermesSession({ sourcePath: fixture, adapter: 'hermes' });

    expect(result.stats.parsed).toBe(1);
    expect(result.observations).toHaveLength(1);
    expect(result.observations[0].sourceSubtype).toBe('hermes_failed_run');
    expect(result.observations[0].inputTokens).toBe(0);
    expect(result.observations[0].outputTokens).toBe(0);
  });

  it('detects malformed json gracefully', async () => {
    const fixture = resolve(root, 'fixtures/parsers/hermes/adversarial/corrupted.json');
    const result = await parseHermesSession({ sourcePath: fixture, adapter: 'hermes' });

    expect(result.stats.malformed).toBe(1);
    expect(result.observations).toHaveLength(0);
    expect(result.anomalies[0].type).toBe('malformed_record');
  });

  it('handles missing provider costs on paid models without fabricating zero', async () => {
    const fixture = resolve(root, 'fixtures/parsers/hermes/adversarial/missing-cost.json');
    const result = await parseHermesSession({ sourcePath: fixture, adapter: 'hermes' });

    expect(result.observations).toHaveLength(1);
    expect(result.observations[0].providerReportedCostMicros).toBeUndefined();
  });
});
