// SPDX-License-Identifier: Apache-2.0
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import {
  type DiscoverOptions,
  type SessionRef,
  type UsageObservation,
} from '@tokentreehq/core';

export const GROK_ADAPTER_VERSION = '0.2.0';
export const GROK_PARSER_VERSION = '0.2.0';

export interface ParseAnomaly {
  readonly type: 'malformed_record' | 'schema_mismatch' | 'token_sum_mismatch';
  readonly sourcePath: string;
  readonly sourceOffset: number;
  readonly sourceValues: unknown;
  readonly sessionId?: string | undefined;
  readonly turnId?: string | undefined;
}

export interface ParseStats {
  parsed: number;
  unknown: number;
  malformed: number;
  anomalies: number;
}

export interface ParseResult {
  readonly observations: UsageObservation[];
  readonly anomalies: ParseAnomaly[];
  readonly stats: ParseStats;
}

type JsonObject = Record<string, unknown>;

function object(value: unknown): JsonObject | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function count(value: unknown): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

export function ticksToMicros(ticks: number): number {
  return Math.floor(ticks / 1000);
}

export async function parseGrokSession(ref: SessionRef): Promise<ParseResult> {
  const observations: UsageObservation[] = [];
  const anomalies: ParseAnomaly[] = [];
  const stats: ParseStats = { parsed: 0, unknown: 0, malformed: 0, anomalies: 0 };

  let rawContent: string;
  try {
    rawContent = readFileSync(ref.sourcePath, 'utf8');
  } catch (err) {
    stats.malformed++;
    stats.anomalies++;
    anomalies.push({
      type: 'malformed_record',
      sourcePath: ref.sourcePath,
      sourceOffset: 0,
      sourceValues: { error: String(err) },
    });
    return { observations, anomalies, stats };
  }

  if (rawContent.trim() === '') {
    return { observations, anomalies, stats };
  }

  let root: JsonObject;
  try {
    const parsed = JSON.parse(rawContent);
    const obj = object(parsed);
    if (!obj) throw new Error('Root must be an object');
    root = obj;
  } catch (err) {
    stats.malformed++;
    stats.anomalies++;
    anomalies.push({
      type: 'malformed_record',
      sourcePath: ref.sourcePath,
      sourceOffset: 0,
      sourceValues: { error: String(err), length: rawContent.length },
    });
    return { observations, anomalies, stats };
  }

  const sessionId = typeof root.sessionId === 'string' ? root.sessionId : (ref.providerSessionId ?? 'grok_session');
  const updatedAt = typeof root.updatedAt === 'string' ? root.updatedAt : new Date().toISOString();

  const sessionObj = object(root.session);
  const turnsArray = Array.isArray(root.turns) ? root.turns : null;

  const primaryModel = typeof sessionObj?.primaryModelId === 'string' ? sessionObj.primaryModelId : undefined;

  let turnCount = 0;

  if (turnsArray && turnsArray.length > 0) {
    for (const item of turnsArray) {
      const turn = object(item);
      if (!turn) continue;

      stats.parsed++;
      turnCount++;

      const turnNumber = count(turn.turnNumber) ?? turnCount;
      const turnId = `turn_${turnNumber}`;
      const endedAt = typeof turn.endedAt === 'string' ? turn.endedAt : updatedAt;
      const turnModel = typeof turn.primaryModelId === 'string' ? turn.primaryModelId : primaryModel;

      const inputTokens = count(turn.inputTokens);
      const outputTokens = count(turn.outputTokens);
      const cachedReadTokens = count(turn.cachedReadTokens);
      const cacheCreationTokens = count(turn.cacheCreationTokens);
      const reasoningTokens = count(turn.reasoningTokens);
      const totalTokens = count(turn.totalTokens);
      const costTicks = count(turn.costUsdTicks);

      if (totalTokens !== null && totalTokens > 0 && inputTokens !== null && outputTokens !== null) {
        if (totalTokens !== inputTokens + outputTokens) {
          stats.anomalies++;
          anomalies.push({
            type: 'token_sum_mismatch',
            sourcePath: ref.sourcePath,
            sourceOffset: 0,
            sourceValues: { reported: totalTokens, sum: inputTokens + outputTokens },
            sessionId,
            turnId,
          });
        }
      }

      const costMicros = costTicks !== null ? ticksToMicros(costTicks) : undefined;

      const obs: UsageObservation = {
        adapter: 'grok',
        source: 'provider_fields',
        sourceSubtype: 'grok_turn_usage',
        sourceEventId: `grok:${sessionId}:${turnId}`,
        providerSessionId: sessionId,
        requestId: `grok:${sessionId}:${turnId}`,
        turnId,
        observedAt: endedAt,
        sourceTimestamp: endedAt,
        model: turnModel,
        inputTokens: inputTokens ?? null,
        outputTokens: outputTokens ?? null,
        cachedInputTokens: cachedReadTokens ?? null,
        cacheWriteTokens: cacheCreationTokens ?? null,
        reasoningTokens: reasoningTokens ?? null,
        providerReportedCostMicros: costMicros,
        sourcePath: ref.sourcePath,
        sourceOffset: 0,
        adapterVersion: GROK_ADAPTER_VERSION,
        parserVersion: GROK_PARSER_VERSION,
      };

      observations.push(obs);
    }
  }

  // Precedence / truth ladder: fallback to session summary only if NO turns were present
  if (turnCount === 0 && sessionObj) {
    stats.parsed++;

    const inputTokens = count(sessionObj.inputTokens);
    const outputTokens = count(sessionObj.outputTokens);
    const cachedReadTokens = count(sessionObj.cachedReadTokens);
    const cacheCreationTokens = count(sessionObj.cacheCreationTokens);
    const reasoningTokens = count(sessionObj.reasoningTokens);
    const costTicks = count(sessionObj.costUsdTicks);
    const costMicros = costTicks !== null ? ticksToMicros(costTicks) : undefined;

    const obs: UsageObservation = {
      adapter: 'grok',
      source: 'provider_fields',
      sourceSubtype: 'grok_session_usage',
      sourceEventId: `grok:${sessionId}:session_summary`,
      providerSessionId: sessionId,
      requestId: `grok:${sessionId}:session_summary`,
      observedAt: updatedAt,
      sourceTimestamp: updatedAt,
      model: primaryModel,
      inputTokens: inputTokens ?? null,
      outputTokens: outputTokens ?? null,
      cachedInputTokens: cachedReadTokens ?? null,
      cacheWriteTokens: cacheCreationTokens ?? null,
      reasoningTokens: reasoningTokens ?? null,
      providerReportedCostMicros: costMicros,
      sourcePath: ref.sourcePath,
      sourceOffset: 0,
      adapterVersion: GROK_ADAPTER_VERSION,
      parserVersion: GROK_PARSER_VERSION,
    };

    observations.push(obs);
  }

  return { observations, anomalies, stats };
}

export function discoverGrokSessions(root: string, options: DiscoverOptions = {}): SessionRef[] {
  if (options.since) {
    // mtime filtering is deferred to checkpoint-aware ingest
  }
  const sessions: SessionRef[] = [];

  function scan(dir: string) {
    try {
      const entries = readdirSync(dir);
      for (const entry of entries) {
        const full = join(dir, entry);
        try {
          const st = statSync(full);
          if (st.isDirectory()) {
            scan(full);
          } else if (st.isFile() && entry === 'usage.json') {
            sessions.push({
              sourcePath: full,
              providerSessionId: full,
            });
          }
        } catch {
          // ignore unreadable
        }
      }
    } catch {
      // ignore unreadable
    }
  }

  scan(root);
  return sessions;
}
