// SPDX-License-Identifier: Apache-2.0
export type CapabilityState = 'available' | 'degraded' | 'blocked' | 'unavailable' | 'unknown';
export interface AdapterCapabilities {
  readonly subagent_tokens_already_in_parent: boolean | 'unknown';
  readonly official_request_telemetry: CapabilityState;
  readonly transcript_fallback: CapabilityState;
}
export interface DetectionResult { readonly detected: boolean; readonly captureMode: 'hooks' | 'logs' | 'manual' | 'none'; readonly detail?: string; }
export interface SessionRef { readonly providerSessionId: string; readonly sourcePath: string; }
export interface DiscoverOptions { readonly since?: Date; }
export interface UsageEvent { readonly eventHash: string; readonly requestId?: string; readonly inputTokens: number | null; readonly outputTokens: number | null; }
export interface Watcher { close(): Promise<void>; }
export interface AgentAdapter {
  readonly id: string;
  detect(): Promise<DetectionResult>;
  discoverSessions(options?: DiscoverOptions): AsyncIterable<SessionRef>;
  parseSession(ref: SessionRef): AsyncIterable<UsageEvent>;
  watch?(callback: (event: UsageEvent) => void): Promise<Watcher>;
  capabilities(): AdapterCapabilities;
}
