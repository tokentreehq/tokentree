# Hermes / OpenRouter Adapter (`@tokentreehq/adapter-hermes` / `tokentree-hermes`)

The Hermes adapter provides first-class telemetry ingestion and accounting for the Hermes AI agent across OpenRouter, Anthropic, and other supported providers.

## Storage Architecture & Data Sources

Hermes persists usage telemetry across two primary locations:

1. **State Database (`state.db`)**:
   - Location: `~/.hermes/state.db` (Linux/macOS) or `%LOCALAPPDATA%\hermes\state.db` (Windows).
   - Table `session_model_usage`: Authoritative task-level usage partitioned by `(session_id, model, billing_provider, billing_base_url, task)`.
   - Table `sessions`: Session lineage, timing, working directory, and subagent hierarchy (`parent_session_id`).

2. **One-Shot Usage Reports (`--usage-file PATH`)**:
   - Written automatically when invoking `hermes -z <PROMPT> --usage-file <PATH>`.
   - Contains main execution metrics and an `auxiliary` object detailing auxiliary tasks (e.g., `title_generation`).

## Truth Ladder & Accounting Rules

1. **Precedence**:
   - Task-level breakdown rows from `session_model_usage` are authoritative.
   - The session summary row from the `sessions` table is suppressed when detailed `session_model_usage` entries exist.
   - For one-shot reports, both the main generation and auxiliary tasks are ingested with distinct task identities.

2. **Subagent & Multi-Agent Hierarchy**:
   - `sessions.parent_session_id` maps directly to `parent_agent_id: hermes:<parent_session_id>`.
   - Auxiliary tasks map their `parent_agent_id` to the parent session.

3. **Provider Costs & Pricing Fallback**:
   - Provider-reported costs (`actual_cost_usd` or `estimated_cost_usd`) are parsed using exact string decimal-to-integer-micros conversion (`decimal_dollars_to_micros(&str)` / `valueToMicros(val)`) with finite range bounds and explicit rejection of scientific notation/exponents.
   - Precedence order: `actual_cost` > `cost_usd` > `total_cost` > `estimated_cost`.
   - Free models (e.g. models ending in `:free`) report 0 micros native cost.
   - Non-free models with missing provider cost fall back to TokenTree's versioned price snapshot (`PriceSnapshot`). If the rate is unlisted, cost remains unavailable (never fabricated as $0.00).

4. **Zero-Token & Failed Runs**:
   - Aborted runs or provider errors (e.g., HTTP 429 rate limit or HTTP 404 retired model) with `failed: true` are recorded as `hermes_failed_run` with 0 tokens.

5. **Durable Ingestion Checkpoints & Active WAL Support**:
   - For `state.db`: The adapter tracks transactional logical row checkpoints keyed by stable row identity (`(session_id, model, task)`) in `ingestion_checkpoints`, calculating independent category deltas and supporting active WAL commits without mutating the source database.
   - For JSON usage files: Tracked via SHA-256 file hashes.

## Privacy & Security

- Telemetry tables and usage files are read with read-only database connections (`SQLITE_OPEN_READ_ONLY`) and read-only file streams.
- Prompt text, completions, and environment credentials (`OPENROUTER_API_KEY`) are never stored or exported.
