// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { resolve } from 'node:path';
import { parseCodexSession, discoverCodexSessions } from '../src/index.js';

describe('Codex parser and hook correlation', () => {
  const fixturePath = resolve(import.meta.dirname, '../../../../fixtures/parsers/codex/public-small-codex.jsonl');

  it('correlates hook boundaries with app-server token events', async () => {
    const res = await parseCodexSession({ providerSessionId: 'ses_cdx_fallback', sourcePath: fixturePath });

    expect(res.stats.parsed).toBeGreaterThanOrEqual(4);
    expect(res.stats.malformed).toBe(1);
    expect(res.anomalies.length).toBeGreaterThanOrEqual(2);

    const req1 = res.observations.find((o) => o.requestId === 'req_cdx_001');
    expect(req1).toBeDefined();
    expect(req1?.providerSessionId).toBe('ses_cdx_test');
    expect(req1?.turnId).toBe('turn_cdx_001');
    expect(req1?.inputTokens).toBe(150);
    expect(req1?.outputTokens).toBe(50);
    expect(req1?.cachedInputTokens).toBe(30);
    expect(req1?.reasoningTokens).toBe(20);

    const sub = res.observations.find((o) => o.requestId === 'req_cdx_sub_001');
    expect(sub).toBeDefined();
    expect(sub?.agentId).toBe('codex_child_agent');
    expect(sub?.parentAgentId).toBe('codex_main_agent');
    expect(sub?.turnId).toBe('turn_cdx_001');

    const cum1 = res.observations.find((o) => o.requestId === 'req_cdx_cum_001');
    expect(cum1).toBeDefined();
    expect(cum1?.inputTokens).toBe(100);
    expect(cum1?.outputTokens).toBe(30);
    expect(cum1?.reasoningTokens).toBe(10);

    const negAnom = res.anomalies.find((a) => a.type === 'negative_delta');
    expect(negAnom).toBeDefined();
    expect(negAnom?.sessionId).toBe('ses_cdx_test');
    expect(negAnom?.turnId).toBe('turn_cdx_001');

    // Privacy test: ensure raw prompts/completions never appear in observations
    const jsonStr = JSON.stringify(res.observations);
    expect(jsonStr).not.toContain('SUPER_SECRET');
    expect(jsonStr).not.toContain('SECRET_PASSWORD');
  });

  it('discovers session files in directory', () => {
    const dir = resolve(import.meta.dirname, '../../../../fixtures/parsers/codex');
    const sessions = discoverCodexSessions(dir);
    expect(sessions.length).toBeGreaterThan(0);
    expect(sessions.some((s) => s.sourcePath.includes('public-small-codex.jsonl'))).toBe(true);
  });
});
