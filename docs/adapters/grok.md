# Grok CLI Adapter (`@tokentreehq/adapter-grok` / `tokentree-grok`)

The Grok adapter provides first-class ingestion and accounting for the xAI Grok CLI.

## Storage Architecture & Session Discovery

Grok stores sessions under:
```
~/.grok/sessions/<url_encoded_workspace_path>/<session_id>/
```

Key session files:
- **`usage.json` (authoritative)**: Telemetry record containing exact per-turn and cumulative session token counts and costs.
- **`events.jsonl` (enrichment)**: Event stream with `turn_started`, `turn_ended`, tool execution, and session relationships.

## Truth Ladder & Accounting Rules

1. **Precedence**:
   - `grok_turn_usage` (from the `turns` array in `usage.json`) is the primary authoritative source.
   - If the `turns` array is populated, each turn is ingested as an individual observation with a unique `turn_id` (`turn_1`, `turn_2`, etc.).
   - The session summary counter (`grok_session_usage`) is suppressed to prevent double counting.
   - If no turns are present, the adapter falls back to the `session` summary counter.

2. **Cost Calculation**:
   - Grok records cost natively in integer ticks representing nanodollars ($10^{-9}$ USD): `costUsdTicks`.
   - TokenTree converts ticks to microdollars ($10^{-6}$ USD) exactly:
     $$\text{micros} = \left\lfloor \frac{\text{costUsdTicks}}{1000} \right\rfloor$$
   - Provider-reported costs are stored in `provider_reported_cost_micros`.

3. **Token Categories**:
   - `input_tokens`: `inputTokens`
   - `output_tokens`: `outputTokens`
   - `cached_input_tokens`: `cachedReadTokens`
   - `cache_write_tokens`: `cacheCreationTokens`
   - `reasoning_tokens`: `reasoningTokens`

4. **Zero-Token Runs & Interrupted Sessions**:
   - Sessions interrupted or terminated due to billing exhaustion (e.g. HTTP 402) produce `usage.json` records with 0 tokens.
   - The parser ingests these records cleanly without crashing or dropping accounting boundaries.

5. **Durable Ingestion Checkpoints**:
   - The adapter calculates a SHA-256 hash of `usage.json` and records byte offsets in `ingestion_checkpoints`.
   - Re-running `tokentree import grok` on unchanged files is idempotent and consumes 0 unnecessary database writes.

## Privacy & Security

- `usage.json` contains zero prompt text, user code, or completion content.
- The adapter opens session files exclusively with read-only permissions (`O_RDONLY`).
