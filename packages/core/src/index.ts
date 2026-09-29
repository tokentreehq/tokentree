// SPDX-License-Identifier: Apache-2.0
export type CapabilityState = 'available' | 'supported' | 'partial' | 'degraded' | 'blocked' | 'blocked_by_policy' | 'not_configured' | 'unavailable' | 'unknown' | 'unsupported_version';
export interface AdapterCapabilities {
  readonly subagent_tokens_already_in_parent: boolean | 'unknown';
  readonly official_request_telemetry: CapabilityState;
  readonly transcript_fallback: CapabilityState;
}
export interface DetectionResult { readonly detected: boolean; readonly captureMode: 'hooks' | 'logs' | 'manual' | 'none'; readonly detail?: string; }
export interface SessionRef { readonly providerSessionId: string; readonly sourcePath: string; }
export interface DiscoverOptions { readonly since?: Date; }
export interface UsageEvent { readonly eventHash: string; readonly requestId?: string | undefined; readonly inputTokens: number | null; readonly outputTokens: number | null; }
export interface Watcher { close(): Promise<void>; }
export interface AgentAdapter {
  readonly id: string;
  detect(): Promise<DetectionResult>;
  discoverSessions(options?: DiscoverOptions): AsyncIterable<SessionRef>;
  parseSession(ref: SessionRef): AsyncIterable<UsageEvent>;
  watch?(callback: (event: UsageEvent) => void): Promise<Watcher>;
  capabilities(): AdapterCapabilities;
}
export { SOURCE_RANK, canonicalIdentity, deduplicateObservations, eventHash, hasMeasuredTokens, snapshotDelta, tokenCompleteness } from './measurement.js';
export type { CompletenessInput, DedupeResult, MeasurementSource, TokenUsage, UsageObservation } from './measurement.js';
export { enqueueSpool } from './spool.js';
export type { SpoolEnvelope } from './spool.js';
export { resolveProject } from './project.js';
export type { ProjectCandidate, ProjectDetectionMethod, ResolveProjectInput } from './project.js';
