// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';

export const SOURCE_RANK = {
  unavailable: 0,
  explicit_cli: 1,
  snapshot_delta: 2,
  transcript_request: 3,
  provider_fields: 4,
  official_telemetry: 5
} as const;
export type MeasurementSource = keyof typeof SOURCE_RANK;

export interface TokenUsage {
  readonly inputTokens: number | null;
  readonly cachedInputTokens: number | null;
  readonly cacheWriteTokens: number | null;
  readonly outputTokens: number | null;
  readonly reasoningTokens: number | null;
}

export interface UsageObservation extends TokenUsage {
  readonly adapter: string;
  readonly source: MeasurementSource;
  readonly sourceSubtype?: string | undefined;
  readonly sourceEventId?: string | undefined;
  readonly sourceProcessId?: string | undefined;
  readonly sourceSequence?: number | undefined;
  readonly providerSessionId: string;
  readonly requestId?: string | undefined;
  readonly turnId?: string | undefined;
  readonly agentId?: string | undefined;
  readonly parentAgentId?: string | undefined;
  readonly sourceTimestamp?: string | undefined;
  readonly observedAt: string;
  readonly model?: string | undefined;
  readonly serviceTier?: string | undefined;
  readonly region?: string | undefined;
  readonly providerReportedCostMicros?: number | undefined;
  readonly sourcePath: string;
  readonly sourceOffset: number;
  readonly adapterVersion: string;
  readonly parserVersion: string;
}

export function hasMeasuredTokens(value: TokenUsage): boolean {
  return [value.inputTokens, value.cachedInputTokens, value.cacheWriteTokens, value.outputTokens, value.reasoningTokens].some((item) => item !== null);
}

export function canonicalIdentity(observation: UsageObservation): string {
  if (observation.requestId) return `${observation.adapter}:request:${observation.requestId}`;
  if (observation.sourceEventId) return `${observation.adapter}:event:${observation.sourceEventId}`;
  if ((observation.sourceSubtype === 'codex_turn_counter' || observation.source === 'snapshot_delta') && observation.turnId) {
    return `${observation.adapter}:counter:${observation.providerSessionId}:${observation.turnId}`;
  }
  return [observation.adapter, observation.providerSessionId, observation.turnId ?? '', observation.model ?? '', observation.sourceTimestamp ?? '', observation.inputTokens ?? '', observation.outputTokens ?? '', observation.sourceOffset].join(':');
}

export function eventHash(observation: UsageObservation): string {
  return createHash('sha256').update(canonicalIdentity(observation)).digest('hex');
}

export interface DedupeResult { readonly canonical: UsageObservation[]; readonly conflicts: Array<{ readonly identity: string; readonly kept: UsageObservation; readonly rejected: UsageObservation }>; }

export function deduplicateObservations(observations: readonly UsageObservation[]): DedupeResult {
  const byIdentity = new Map<string, UsageObservation>();
  const conflicts: DedupeResult['conflicts'][number][] = [];
  for (const observation of observations) {
    const identity = canonicalIdentity(observation);
    const current = byIdentity.get(identity);
    if (!current) { byIdentity.set(identity, observation); continue; }
    const currentRank = SOURCE_RANK[current.source];
    const nextRank = SOURCE_RANK[observation.source];
    const keepNext = nextRank > currentRank || (nextRank === currentRank && hasMeasuredTokens(observation) && !hasMeasuredTokens(current));
    const kept = keepNext ? observation : current;
    const rejected = keepNext ? current : observation;
    if (JSON.stringify(tokenTuple(kept)) !== JSON.stringify(tokenTuple(rejected))) conflicts.push({ identity, kept, rejected });
    byIdentity.set(identity, kept);
  }
  return { canonical: [...byIdentity.values()], conflicts };
}

function tokenTuple(value: TokenUsage): readonly (number | null)[] {
  return [value.inputTokens, value.cachedInputTokens, value.cacheWriteTokens, value.outputTokens, value.reasoningTokens];
}

export function snapshotDelta(before: TokenUsage, after: TokenUsage): { usage: TokenUsage | null; negative: boolean } {
  const values = tokenTuple(after).map((value, index) => {
    const previous = tokenTuple(before)[index] ?? null;
    return value === null || previous === null ? null : value - previous;
  });
  if (values.some((value) => value !== null && value < 0)) return { usage: null, negative: true };
  return { usage: { inputTokens: values[0] ?? null, cachedInputTokens: values[1] ?? null, cacheWriteTokens: values[2] ?? null, outputTokens: values[3] ?? null, reasoningTokens: values[4] ?? null }, negative: false };
}

export interface CompletenessInput { readonly measuredRequests: number; readonly unavailableRequests: number; readonly anomalousRequests: number; }
export function tokenCompleteness(input: CompletenessInput): number | null {
  const denominator = input.measuredRequests + input.unavailableRequests + input.anomalousRequests;
  return denominator === 0 ? null : 100 * input.measuredRequests / denominator;
}
