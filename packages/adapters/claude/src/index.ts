// SPDX-License-Identifier: Apache-2.0
import { createReadStream, opendirSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { snapshotDelta, type AgentAdapter, type AdapterCapabilities, type DetectionResult, type DiscoverOptions, type SessionRef, type TokenUsage, type UsageEvent, type UsageObservation } from '@tokentreehq/core';

export const CLAUDE_ADAPTER_VERSION = '0.1.0';
export const CLAUDE_PARSER_VERSION = '0.1.0';

export interface ParseAnomaly { readonly type: 'negative_delta' | 'malformed_record'; readonly sourcePath: string; readonly sourceOffset: number; readonly sourceValues: unknown; }
export interface ParseStats { parsed: number; unknown: number; malformed: number; }
export interface ParseResult { readonly observations: UsageObservation[]; readonly anomalies: ParseAnomaly[]; readonly stats: ParseStats; }

type JsonObject = Record<string, unknown>;
function object(value: unknown): JsonObject | null { return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as JsonObject : null; }
function text(value: unknown): string | undefined { return typeof value === 'string' && value.length > 0 ? value : undefined; }
function count(value: unknown): number | null { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null; }

function extractUsage(record: JsonObject): TokenUsage | null {
  const message = object(record.message);
  const usage = object(record.usage) ?? object(message?.usage);
  if (!usage) return null;
  const result = {
    inputTokens: count(usage.input_tokens ?? usage.inputTokens),
    cachedInputTokens: count(usage.cache_read_input_tokens ?? usage.cached_input_tokens ?? usage.cachedInputTokens),
    cacheWriteTokens: count(usage.cache_creation_input_tokens ?? usage.cache_write_tokens ?? usage.cacheWriteTokens),
    outputTokens: count(usage.output_tokens ?? usage.outputTokens),
    reasoningTokens: count(usage.reasoning_tokens ?? usage.reasoningTokens)
  };
  return Object.values(result).every((value) => value === null) ? null : result;
}

function observation(record: JsonObject, ref: SessionRef, offset: number, usage: TokenUsage, source: UsageObservation['source']): UsageObservation {
  const message = object(record.message);
  return {
    adapter:'claude', source,
    sourceEventId:text(record.event_id ?? record.uuid),
    sourceProcessId:text(record.process_id),
    sourceSequence:count(record.sequence) ?? undefined,
    providerSessionId:text(record.session_id) ?? ref.providerSessionId,
    requestId:text(record.request_id ?? message?.id),
    turnId:text(record.turn_id ?? record.prompt_id),
    agentId:text(record.agent_id), parentAgentId:text(record.parent_agent_id),
    sourceTimestamp:text(record.timestamp), observedAt:new Date().toISOString(),
    model:text(record.model ?? message?.model), serviceTier:text(record.service_tier), region:text(record.region),
    providerReportedCostMicros:count(record.cost_micros) ?? undefined,
    ...usage, sourcePath:ref.sourcePath, sourceOffset:offset,
    adapterVersion:CLAUDE_ADAPTER_VERSION, parserVersion:CLAUDE_PARSER_VERSION
  };
}

export async function parseClaudeSession(ref: SessionRef): Promise<ParseResult> {
  const observations: UsageObservation[] = [];
  const anomalies: ParseAnomaly[] = [];
  const stats: ParseStats = { parsed:0, unknown:0, malformed:0 };
  let offset = 0;
  let priorSnapshot: TokenUsage | null = null;
  const lines = createInterface({ input:createReadStream(ref.sourcePath, { encoding:'utf8' }), crlfDelay:Infinity });
  for await (const line of lines) {
    const lineOffset = offset;
    offset += Buffer.byteLength(line, 'utf8') + 1;
    if (line.trim() === '') continue;
    let record: JsonObject;
    try { const parsed = object(JSON.parse(line)); if (!parsed) throw new Error('record must be an object'); record = parsed; }
    catch { stats.malformed++; anomalies.push({ type:'malformed_record', sourcePath:ref.sourcePath, sourceOffset:lineOffset, sourceValues:{ length:Buffer.byteLength(line) } }); continue; }
    const usage = extractUsage(record);
    if (!usage) { stats.unknown++; continue; }
    const kind = text(record.type);
    if (kind === 'usage_snapshot') {
      if (priorSnapshot) {
        const delta = snapshotDelta(priorSnapshot, usage);
        if (delta.negative || !delta.usage) anomalies.push({ type:'negative_delta', sourcePath:ref.sourcePath, sourceOffset:lineOffset, sourceValues:{ before:priorSnapshot, after:usage } });
        else { observations.push(observation(record, ref, lineOffset, delta.usage, 'snapshot_delta')); stats.parsed++; }
      }
      priorSnapshot = usage;
      continue;
    }
    const source = kind === 'otel_api_request' ? 'official_telemetry' : kind === 'provider_usage' ? 'provider_fields' : 'transcript_request';
    observations.push(observation(record, ref, lineOffset, usage, source)); stats.parsed++;
  }
  return { observations, anomalies, stats };
}

export function discoverClaudeSessions(root: string, options: DiscoverOptions = {}): SessionRef[] {
  const found: SessionRef[] = [];
  const walk = (dir: string): void => {
    let handle;
    try { handle = opendirSync(dir); } catch { return; }
    try {
      for (;;) {
        const entry = handle.readSync(); if (!entry) break;
        const path = join(dir, entry.name);
        if (entry.isDirectory()) walk(path);
        else if (entry.isFile() && entry.name.endsWith('.jsonl')) {
          if (options.since) { /* mtime filtering is deferred to checkpoint-aware ingest. */ }
          found.push({ providerSessionId:entry.name.slice(0,-6), sourcePath:path });
        }
      }
    } finally { handle.closeSync(); }
  };
  walk(root);
  return found.sort((a,b) => a.sourcePath.localeCompare(b.sourcePath));
}

export class ClaudeAdapter implements AgentAdapter {
  readonly id = 'claude';
  constructor(private readonly root: string) {}
  async detect(): Promise<DetectionResult> { return { detected:discoverClaudeSessions(this.root).length > 0, captureMode:'logs', detail:'Historical JSONL fallback; hooks/official telemetry not configured' }; }
  async *discoverSessions(options?: DiscoverOptions): AsyncIterable<SessionRef> { for (const ref of discoverClaudeSessions(this.root, options)) yield ref; }
  async *parseSession(ref: SessionRef): AsyncIterable<UsageEvent> {
    const result = await parseClaudeSession(ref);
    for (const item of result.observations) yield { eventHash:'pending-normalization', requestId:item.requestId, inputTokens:item.inputTokens, outputTokens:item.outputTokens };
  }
  capabilities(): AdapterCapabilities { return { subagent_tokens_already_in_parent:'unknown', official_request_telemetry:'not_configured', transcript_fallback:'degraded' }; }
}
