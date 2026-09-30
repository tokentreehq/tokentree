// SPDX-License-Identifier: Apache-2.0
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, join } from 'node:path';
import {
  type DiscoverOptions,
  type SessionRef,
  type UsageObservation,
} from '@tokentreehq/core';

export const HERMES_ADAPTER_VERSION = '0.2.0';
export const HERMES_PARSER_VERSION = '0.2.0';

export interface ParseAnomaly {
  readonly type: 'malformed_record' | 'schema_mismatch' | 'missing_provider_measurements';
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

export function decimalDollarsToMicros(s: string): number | null {
  const trimmed = s.trim();
  if (!trimmed) return null;
  const parts = trimmed.split('.');
  if (parts.length > 2) return null;
  const wholeStr = parts[0];
  const fracStr = parts[1] ?? '';
  if (!wholeStr || !/^\d+$/.test(wholeStr) || (fracStr && !/^\d+$/.test(fracStr))) return null;
  const whole = parseInt(wholeStr, 10);
  if (isNaN(whole)) return null;
  let frac = 0;
  if (fracStr.length <= 6) {
    frac = parseInt(fracStr.padEnd(6, '0'), 10);
  } else {
    const first6 = parseInt(fracStr.slice(0, 6), 10);
    const seventhChar = fracStr[6];
    const roundBit = seventhChar && parseInt(seventhChar, 10) >= 5 ? 1 : 0;
    frac = first6 + roundBit;
  }
  return whole * 1_000_000 + frac;
}

export function valueToMicros(val: unknown): number | undefined {
  if (typeof val === 'number') {
    if (val < 0) return undefined;
    return decimalDollarsToMicros(String(val)) ?? undefined;
  }
  if (typeof val === 'string') {
    return decimalDollarsToMicros(val) ?? undefined;
  }
  return undefined;
}

export function usdToMicros(costUsd: number): number {
  return valueToMicros(costUsd) ?? 0;
}

export async function parseHermesSession(ref: SessionRef): Promise<ParseResult> {
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

  const sessionId = typeof root.session_id === 'string'
    ? root.session_id
    : (ref.providerSessionId ?? basename(ref.sourcePath).replace(/\.json$/, ''));

  const observedAt = new Date().toISOString();
  const isFailed = root.failed === true;
  const modelStr = typeof root.model === 'string' ? root.model : undefined;
  const isFreeModel = modelStr?.includes(':free') || modelStr?.includes('free') || false;

  stats.parsed++;

  const rawInput = count(root.input_tokens);
  const rawOutput = count(root.output_tokens);
  const rawCacheRead = count(root.cache_read_tokens);
  const rawCacheWrite = count(root.cache_write_tokens);
  const rawReasoning = count(root.reasoning_tokens);

  const hasTokens = !isFailed && (
    rawInput !== null ||
    rawOutput !== null ||
    rawCacheRead !== null ||
    rawCacheWrite !== null ||
    rawReasoning !== null
  );

  const isUnmeasured = isFailed || !hasTokens;

  if (isUnmeasured) {
    stats.anomalies++;
    anomalies.push({
      type: 'missing_provider_measurements',
      sourcePath: ref.sourcePath,
      sourceOffset: 0,
      sourceValues: {
        reason: isFailed ? 'failed_request' : 'missing_measurements',
        apiCalls: count(root.api_calls),
      },
      sessionId,
      turnId: 'turn_1',
    });
  }

  const parsedCostMicros = valueToMicros(root.estimated_cost_usd);

  let providerCostMicros: number | undefined;
  if (!isUnmeasured) {
    if (parsedCostMicros !== undefined && parsedCostMicros > 0) {
      providerCostMicros = parsedCostMicros;
    } else if (isFreeModel) {
      providerCostMicros = 0;
    }
  }

  const source = isUnmeasured ? 'unavailable' : 'provider_fields';
  const subtype = isUnmeasured
    ? (isFailed ? 'hermes_failed_run' : 'hermes_unmeasured')
    : 'hermes_oneshot_usage';

  const mainObs: UsageObservation = {
    adapter: 'hermes',
    source,
    sourceSubtype: subtype,
    sourceEventId: `hermes:${sessionId}:main`,
    providerSessionId: sessionId,
    requestId: `hermes:${sessionId}:main`,
    turnId: 'turn_1',
    agentId: `hermes:${sessionId}`,
    observedAt,
    sourceTimestamp: observedAt,
    model: modelStr,
    inputTokens: isUnmeasured ? null : rawInput,
    outputTokens: isUnmeasured ? null : rawOutput,
    cachedInputTokens: isUnmeasured ? null : rawCacheRead,
    cacheWriteTokens: isUnmeasured ? null : rawCacheWrite,
    reasoningTokens: isUnmeasured ? null : rawReasoning,
    providerReportedCostMicros: providerCostMicros,
    sourcePath: ref.sourcePath,
    sourceOffset: 0,
    adapterVersion: HERMES_ADAPTER_VERSION,
    parserVersion: HERMES_PARSER_VERSION,
  };
  observations.push(mainObs);

  // Ingest auxiliary tasks (e.g. title_generation) if present
  const aux = object(root.auxiliary);
  const byTask = aux ? object(aux.by_task) : null;
  if (byTask) {
    for (const [taskName, taskRaw] of Object.entries(byTask)) {
      const task = object(taskRaw);
      if (!task) continue;

      stats.parsed++;
      const taskInput = count(task.input_tokens);
      const taskOutput = count(task.output_tokens);
      const taskRead = count(task.cache_read_tokens);
      const taskWrite = count(task.cache_write_tokens);
      const taskReasoning = count(task.reasoning_tokens);

      const taskHasTokens = (
        taskInput !== null ||
        taskOutput !== null ||
        taskRead !== null ||
        taskWrite !== null ||
        taskReasoning !== null
      );
      const taskIsUnmeasured = !taskHasTokens;

      const taskParsedCost = valueToMicros(task.estimated_cost_usd);
      let taskCostMicros: number | undefined;
      if (!taskIsUnmeasured) {
        if (taskParsedCost !== undefined && taskParsedCost > 0) {
          taskCostMicros = taskParsedCost;
        } else if (isFreeModel) {
          taskCostMicros = 0;
        }
      }

      const auxObs: UsageObservation = {
        adapter: 'hermes',
        source: taskIsUnmeasured ? 'unavailable' : 'provider_fields',
        sourceSubtype: `hermes_auxiliary_${taskName}`,
        sourceEventId: `hermes:${sessionId}:aux:${taskName}`,
        providerSessionId: sessionId,
        requestId: `hermes:${sessionId}:aux:${taskName}`,
        turnId: `task_${taskName}`,
        agentId: `hermes:${sessionId}:${taskName}`,
        parentAgentId: `hermes:${sessionId}`,
        observedAt,
        sourceTimestamp: observedAt,
        model: modelStr,
        inputTokens: taskIsUnmeasured ? null : taskInput,
        outputTokens: taskIsUnmeasured ? null : taskOutput,
        cachedInputTokens: taskIsUnmeasured ? null : taskRead,
        cacheWriteTokens: taskIsUnmeasured ? null : taskWrite,
        reasoningTokens: taskIsUnmeasured ? null : taskReasoning,
        providerReportedCostMicros: taskCostMicros,
        sourcePath: ref.sourcePath,
        sourceOffset: 0,
        adapterVersion: HERMES_ADAPTER_VERSION,
        parserVersion: HERMES_PARSER_VERSION,
      };
      observations.push(auxObs);
    }
  }

  return { observations, anomalies, stats };
}

export function discoverHermesSessions(root: string, options: DiscoverOptions = {}): SessionRef[] {
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
          } else if (st.isFile() && (entry.endsWith('.json') || entry.endsWith('.db'))) {
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
