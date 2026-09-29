# Codex Fixture Documentation

This directory contains versioned sanitized fixtures derived from actual supported Codex logs and adversarial accounting fixtures.

## 1. Real Formats

### `app-server-v1.jsonl`
- **Source**: OpenAI Codex App Server (Protocol Version `2024-11-05` / `codex-app-server-v1`)
- **Format**: JSON-RPC / streaming events over standard input/output
- **Supported Fields**:
  - `method` / `event`: `turn/start`, `thread/tokenUsage/updated`, `turn/complete`
  - `params.threadId` / `session_id`: Unique conversation thread identifier
  - `params.turnId` / `turn_id`: Monotonic turn boundary identifier
  - `params.tokenUsage`:
    - `inputTokens`: Prompt tokens sent to the model
    - `cachedInputTokens`: Input tokens served from prompt cache
    - `outputTokens`: Completion tokens produced by the model
    - `reasoningTokens`: Internal thinking tokens generated during reasoning
  - `params.model`: Model identifier (e.g. `o3-mini`, `gpt-4o`)
  - `params.agentId`: Unique agent identifier for subagent/parallel execution
  - `params.parentAgentId`: Parent agent identifier for hierarchical lineage

### `rollout-v1.jsonl`
- **Source**: OpenAI Codex CLI Rollout Logs (`~/.codex/sessions/**/*.jsonl`, Version `codex-rollout-v1`)
- **Format**: Line-delimited JSON log records
- **Supported Fields**:
  - `type`: `session_meta`, `turn_start`, `model_request`, `turn_summary`, `turn_end`
  - `session_id`: Session UUID
  - `turn_id`: Turn UUID or sequential counter
  - `model`: Model name
  - `usage`:
    - `prompt_tokens`: Raw input tokens
    - `prompt_tokens_details.cached_tokens`: Cached input tokens
    - `completion_tokens`: Output tokens
    - `completion_tokens_details.reasoning_tokens`: Reasoning tokens
  - `timestamp`: ISO-8601 observation timestamp

---

## 2. Adversarial Test Fixtures (`adversarial/`)

- `case-a-detailed-only.jsonl`: Authoritative request-level events without counter fallback.
- `case-b-counter-only.jsonl`: Standalone turn counter fallback when detailed events are unavailable.
- `case-c-detailed-equal-counter.jsonl`: Detailed events accompanied by an exact matching covering counter. Counter is suppressed.
- `case-d-detailed-conflicting-counter.jsonl`: Detailed events accompanied by a conflicting counter. Counter is suppressed and an anomaly is recorded.
- `case-e-repeated-counters.jsonl`: Identical repeated turn counters. Deduplication ensures single evaluation.
- `case-f-cumulative-resets.jsonl`: Cumulative stream experiencing counter resets / negative deltas.
- `case-g-subagent-covering-counter.jsonl`: Subagent child and parent events covered by a turn summary counter.
- `case-unknown-version.jsonl`: Schema specifying unsupported protocol version (`v999.0`). Flagged as degraded anomaly.
