# Loop 0009 — Rust-first production engine

## Goal
Replace the architecture's optional-Rust stance with a Rust-first production engine and ship a real vertical Rust path: Claude JSONL → canonical measurement/dedupe → SQLite WAL ledger → terminal report, plus privacy-safe hook enqueue.

## PRD sections
§§11, 12.4, 15–17, 19–22, 25, 27.

## Implemented
- Rust workspace pinned to 1.98.1.
- `tokentree-core`: measurement source ordering, canonical identity, request dedupe, completeness, SHA-256 identity, and exact integer-micro cost math.
- `tokentree-ledger`: bundled SQLite, WAL/busy timeout, shared schema migration, 0700/0600 permissions, idempotent transactional ingest, integrity check, and aggregation.
- `tokentree-claude`: bounded-memory recursive discovery and line-streaming parser tested against the public sanitized Claude 2.1.x fixture.
- `tokentree-otel`: loopback-only, bounded OTLP/HTTP JSON `/v1/logs` receiver for official `claude_code.api_request` events; raw request-body events are ignored.
- `tokentree` Rust CLI: `doctor`, `import claude`, `report --text`, and `hook-enqueue`.
- Claude plugin release launcher prefers the bundled Rust binary and uses the Node enqueue script only in development checkouts.
- CI now requires rustfmt, Rust tests, and Clippy with warnings denied in addition to TypeScript checks.

## Out of scope
The Rust migration is not complete: recursive tree/query/corrections, versioned price persistence, prototype migration, spool worker, and classification still use the TypeScript reference path. Signed cross-platform binary packaging is next. No dashboard work.

## Audit
- PASS — the Rust vertical slice is executable and tested, not an empty crate skeleton.
- PASS — the Rust and TypeScript paths share the exact SQLite migration.
- PASS — official telemetry outranks transcript observations in Rust dedupe.
- PASS — unknown pricing remains unavailable; exact cost uses category-specific rates and integer micros.
- PASS — hook enqueue drops prompt/tool payloads and writes only compact sanitized data.
- PASS — SQLite permissions, WAL, integrity, transactional ingest, and replay idempotence are tested.
- PASS — public Claude fixture parsing is tested with bounded line streaming.
- PASS — synthetic official Claude API-request telemetry was accepted over loopback OTLP/HTTP JSON and appeared in the Rust ledger report; non-loopback binding is rejected.
- PASS — rustfmt, all Rust tests, and Clippy `-D warnings` pass.
