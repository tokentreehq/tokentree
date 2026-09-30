// SPDX-License-Identifier: Apache-2.0
import { createReadStream, opendirSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import {
  type DiscoverOptions,
  type SessionRef,
  type TokenUsage,
  type UsageObservation,
} from '@tokentreehq/core';

export const CODEX_ADAPTER_VERSION = '0.2.0';
export const CODEX_PARSER_VERSION = '0.2.0';

export interface ParseAnomaly {
  readonly type: 'negative_delta' | 'malformed_record';
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

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function count(value: unknown): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function isHookBoundaryStart(type?: string): boolean {
  if (!type) return false;
  return ['turn/started', 'turn_start', 'turn/start', 'hook_boundary', 'turn_boundary', 'session/started'].includes(type);
}

export async function parseCodexSession(ref: SessionRef): Promise<ParseResult> {
  const observations: UsageObservation[] = [];
  const anomalies: ParseAnomaly[] = [];
  const stats: ParseStats = { parsed: 0, unknown: 0, malformed: 0, anomalies: 0 };
  let offset = 0;

  let activeSessionId: string | undefined = ref.providerSessionId;
  let activeTurnId: string | undefined;
  let activeAgentId: string | undefined;
  let activeParentAgentId: string | undefined;

  const cumulativeCounters = new Map<string, {
    inputTokens: number;
    cachedInputTokens: number;
    cacheWriteTokens: number;
    outputTokens: number;
    reasoningTokens: number;
  }>();

  const lines = createInterface({
    input: createReadStream(ref.sourcePath, { encoding: 'utf8' }),
    crlfDelay: Infinity,
  });

  for await (const line of lines) {
    const lineOffset = offset;
    offset += Buffer.byteLength(line, 'utf8') + 1;
    if (line.trim() === '') continue;

    let record: JsonObject;
    try {
      const parsed = object(JSON.parse(line));
      if (!parsed) throw new Error('record must be an object');
      record = parsed;
    } catch {
      stats.malformed++;
      stats.anomalies++;
      anomalies.push({
        type: 'malformed_record',
        sourcePath: ref.sourcePath,
        sourceOffset: lineOffset,
        sourceValues: { length: Buffer.byteLength(line) },
        sessionId: activeSessionId,
        turnId: activeTurnId,
      });
      continue;
    }

    const params = object(record.params);
    const recordType = text(record.type ?? record.event ?? record.boundary ?? record.method);

    if (isHookBoundaryStart(recordType)) {
      const sId = text(record.session_id ?? record.sessionId ?? record.thread_id ?? record.threadId ?? params?.threadId);
      if (sId) activeSessionId = sId;

      const tId = text(record.turn_id ?? record.turnId ?? record.id ?? params?.turnId);
      if (tId) activeTurnId = tId;

      const aId = text(record.agent_id ?? record.agentId ?? params?.agentId);
      if (aId) activeAgentId = aId;

      const paId = text(record.parent_agent_id ?? record.parentAgentId ?? params?.parentAgentId);
      if (paId) activeParentAgentId = paId;
    }

    const explicitSessionId = text(record.session_id ?? record.sessionId ?? record.thread_id ?? record.threadId ?? params?.threadId);
    if (explicitSessionId) activeSessionId = explicitSessionId;

    const isCumulative = record.is_cumulative === true || Boolean(record.cumulative_token_usage) || recordType === 'cumulative_usage';

    const usageObj = object(record.token_usage) ?? object(record.usage) ?? object(record.cumulative_token_usage) ?? object(params?.tokenUsage) ?? object(params?.usage);

    if (!usageObj) {
      if (!isHookBoundaryStart(recordType) && recordType !== 'turn/completed' && recordType !== 'turn_complete') {
        stats.unknown++;
      }
      continue;
    }

    const promptDetails = object(usageObj.prompt_tokens_details);
    const compDetails = object(usageObj.completion_tokens_details) ?? object(usageObj.output_tokens_details);

    const rawUsage = {
      inputTokens: count(usageObj.input_tokens ?? usageObj.inputTokens ?? usageObj.prompt_tokens),
      cachedInputTokens: count(usageObj.cached_input_tokens ?? usageObj.cachedInputTokens ?? usageObj.cache_read_input_tokens ?? promptDetails?.cached_tokens),
      cacheWriteTokens: count(usageObj.cache_creation_input_tokens ?? usageObj.cache_write_tokens ?? usageObj.cacheWriteTokens),
      outputTokens: count(usageObj.output_tokens ?? usageObj.outputTokens ?? usageObj.completion_tokens),
      reasoningTokens: count(usageObj.reasoning_tokens ?? usageObj.reasoningTokens ?? compDetails?.reasoning_tokens ?? compDetails?.thinking_tokens),
    };

    if (Object.values(rawUsage).every((v) => v === null)) {
      stats.unknown++;
      continue;
    }

    const turnId = text(record.turn_id ?? record.turnId ?? params?.turnId) ?? activeTurnId;
    const agentId = text(record.agent_id ?? record.agentId ?? params?.agentId) ?? activeAgentId;
    const parentAgentId = text(record.parent_agent_id ?? record.parentAgentId ?? params?.parentAgentId) ?? activeParentAgentId;
    const requestId = text(record.request_id ?? record.requestId ?? record.id ?? params?.requestId);
    const model = text(record.model ?? params?.model);

    const streamKey = `${model ?? 'default'}:${agentId ?? 'root'}`;

    let effectiveUsage: TokenUsage;

    if (isCumulative) {
      const prev = cumulativeCounters.get(streamKey);
      const curInput = rawUsage.inputTokens ?? 0;
      const curOutput = rawUsage.outputTokens ?? 0;
      const curCached = rawUsage.cachedInputTokens ?? 0;
      const curReasoning = rawUsage.reasoningTokens ?? 0;

      if (prev) {
        if (curInput < prev.inputTokens || curOutput < prev.outputTokens || curCached < prev.cachedInputTokens || curReasoning < prev.reasoningTokens) {
          stats.anomalies++;
          anomalies.push({
            type: 'negative_delta',
            sourcePath: ref.sourcePath,
            sourceOffset: lineOffset,
            sourceValues: { stream: streamKey, previous: prev, current: rawUsage },
            sessionId: activeSessionId,
            turnId,
          });
          cumulativeCounters.set(streamKey, {
            inputTokens: curInput,
            cachedInputTokens: curCached,
            cacheWriteTokens: rawUsage.cacheWriteTokens ?? 0,
            outputTokens: curOutput,
            reasoningTokens: curReasoning,
          });
          effectiveUsage = rawUsage;
        } else {
          effectiveUsage = {
            inputTokens: curInput - prev.inputTokens,
            cachedInputTokens: curCached - prev.cachedInputTokens,
            cacheWriteTokens: (rawUsage.cacheWriteTokens ?? 0) - prev.cacheWriteTokens,
            outputTokens: curOutput - prev.outputTokens,
            reasoningTokens: curReasoning - prev.reasoningTokens,
          };
          cumulativeCounters.set(streamKey, {
            inputTokens: curInput,
            cachedInputTokens: curCached,
            cacheWriteTokens: rawUsage.cacheWriteTokens ?? 0,
            outputTokens: curOutput,
            reasoningTokens: curReasoning,
          });
        }
      } else {
        cumulativeCounters.set(streamKey, {
          inputTokens: curInput,
          cachedInputTokens: curCached,
          cacheWriteTokens: rawUsage.cacheWriteTokens ?? 0,
          outputTokens: curOutput,
          reasoningTokens: curReasoning,
        });
        effectiveUsage = rawUsage;
      }
    } else {
      const entry = cumulativeCounters.get(streamKey) ?? {
        inputTokens: 0,
        cachedInputTokens: 0,
        cacheWriteTokens: 0,
        outputTokens: 0,
        reasoningTokens: 0,
      };
      entry.inputTokens += rawUsage.inputTokens ?? 0;
      entry.cachedInputTokens += rawUsage.cachedInputTokens ?? 0;
      entry.cacheWriteTokens += rawUsage.cacheWriteTokens ?? 0;
      entry.outputTokens += rawUsage.outputTokens ?? 0;
      entry.reasoningTokens += rawUsage.reasoningTokens ?? 0;
      cumulativeCounters.set(streamKey, entry);
      effectiveUsage = rawUsage;
    }

    const sourceKind = text(record.source_kind) ?? (recordType === 'turn_counter' || recordType === 'turn_summary' ? 'codex_turn_counter' : recordType === 'thread/tokenUsage/updated' ? 'codex_app_server' : 'codex_rollout');

    const source: UsageObservation['source'] = sourceKind === 'codex_turn_counter'
      ? 'snapshot_delta'
      : sourceKind === 'codex_app_server'
      ? 'official_telemetry'
      : 'transcript_request';

    // Privacy strictly enforced: no prompt or completion copied
    observations.push({
      adapter: 'codex',
      source,
      sourceSubtype: sourceKind,
      sourceEventId: text(record.event_id ?? record.uuid),
      providerSessionId: activeSessionId ?? ref.providerSessionId,
      requestId,
      turnId,
      agentId,
      parentAgentId,
      sourceTimestamp: text(record.timestamp ?? record.created_at),
      observedAt: new Date().toISOString(),
      model,
      serviceTier: text(record.service_tier),
      region: text(record.region),
      providerReportedCostMicros: count(record.cost_micros) ?? undefined,
      ...effectiveUsage,
      sourcePath: ref.sourcePath,
      sourceOffset: lineOffset,
      adapterVersion: CODEX_ADAPTER_VERSION,
      parserVersion: CODEX_PARSER_VERSION,
    });
    stats.parsed++;
  }

  return { observations, anomalies, stats };
}

export function discoverCodexSessions(root: string, options: DiscoverOptions = {}): SessionRef[] {
  const found: SessionRef[] = [];
  const walk = (dir: string): void => {
    let handle;
    try {
      handle = opendirSync(dir);
    } catch {
      return;
    }
    try {
      for (;;) {
        const entry = handle.readSync();
        if (!entry) break;
        const path = join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(path);
        } else if (entry.isFile() && entry.name.endsWith('.jsonl')) {
          if (options.since) {
            /* mtime filtering is deferred to checkpoint-aware ingest. */
          }
          found.push({ providerSessionId: entry.name.slice(0, -6), sourcePath: path });
        }
      }
    } finally {
      handle.closeSync();
    }
  };
  walk(root);
  found.sort((a, b) => a.sourcePath.localeCompare(b.sourcePath));
  return found;
}
