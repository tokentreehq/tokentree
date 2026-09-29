# Loop 0002 — Claude measurement ingest

## Goal
Deliver a runnable vertical slice: discover Claude JSONL, stream tolerant observations, normalize and deduplicate them by the truth ladder, and commit only canonical events to the SQLite ledger.

## PRD sections
§§11.1–11.10, 16.5–16.17, 17.1–17.3, 19–21, 25 Phase 1.

## Files to touch
`packages/core`, `packages/database`, `packages/adapters/claude`, fixtures, tests, status, changelog, and this note.

## Tests
Streaming parse, unknown/truncated rows, source precedence, request dedupe, negative-delta anomaly, replay idempotence, filesystem permissions.

## Out of scope
Official OTLP receiver configuration, live hooks, classification, cost UI, and host support claims. The Claude adapter remains `partial` until a real-version compatibility matrix passes.

## Audit
- PASS — truth-ladder precedence and request identity deduplicate observations before ledger insertion.
- PASS — negative deltas create anomalies; unavailable token values remain null.
- PASS — usage events are append-only; replay is idempotent; source logs are read-only.
- PASS — parser streams JSONL and tolerates unknown/truncated records.
- PASS — tests, strict typecheck, fixtures, and privacy constraints pass.
- N/A — real-version compatibility and official receiver remain gated; no support claim is made.
