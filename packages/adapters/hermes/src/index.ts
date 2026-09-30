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
  readonly type: 'malformed_record' | 'schema_mismatch';
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

export function usdToMicros(costUsd: number): number {
  if (costUsd <= 0) return 0;
  return Math.round(costUsd * 1_000_000);
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

  const inputTokens = count(root.input_tokens) ?? (isFailed ? 0 : undefined);
  const outputTokens = count(root.output_tokens) ?? (isFailed ? 0 : undefined);
  const cacheReadTokens = count(root.cache_read_tokens) ?? (isFailed ? 0 : undefined);
  const cacheWriteTokens = count(root.cache_write_tokens) ?? (isFailed ? 0 : undefined);
  const reasoningTokens = count(root.reasoning_tokens) ?? (isFailed ? 0 : undefined);

  let providerCostMicros: number | undefined;
  if (typeof root.estimated_cost_usd === 'number' && root.estimated_cost_usd > 0) {
    providerCostMicros = usdToMicros(root.estimated_cost_usd);
  } else if (isFreeModel) {
    providerCostMicros = 0;
  }

  const subtype = isFailed ? 'hermes_failed_run' : 'hermes_oneshot_usage';

  const mainObs: UsageObservation = {
    adapter: 'hermes',
    source: 'provider_fields',
    sourceSubtype: subtype,
    sourceEventId: `hermes:${sessionId}:main`,
    providerSessionId: sessionId,
    requestId: `hermes:${sessionId}:main`,
    turnId: 'turn_1',
    agentId: `hermes:${sessionId}`,
    observedAt,
    sourceTimestamp: observedAt,
    model: modelStr,
    inputTokens: inputTokens ?? null,
    outputTokens: outputTokens ?? null,
    cachedInputTokens: cacheReadTokens ?? null,
    cacheWriteTokens: cacheWriteTokens ?? null,
    reasoningTokens: reasoningTokens ?? null,
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

      let taskCostMicros: number | undefined;
      if (typeof task.estimated_cost_usd === 'number' && task.estimated_cost_usd > 0) {
        taskCostMicros = usdToMicros(task.estimated_cost_usd);
      } else if (isFreeModel) {
        taskCostMicros = 0;
      }

      const auxObs: UsageObservation = {
        adapter: 'hermes',
        source: 'provider_fields',
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
        inputTokens: taskInput ?? null,
        outputTokens: taskOutput ?? null,
        cachedInputTokens: taskRead ?? null,
        cacheWriteTokens: taskWrite ?? null,
        reasoningTokens: taskReasoning ?? null,
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

export function discoverHermesSessions(root: string, _options: DiscoverOptions = {}): SessionRef[] {
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
