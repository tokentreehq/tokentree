# Delivery status


## Rust-first engine migration
- [x] PRD and decisions updated: Rust owns production measurement paths
- [x] Rust canonical measurement/dedupe/completeness/exact-cost core
- [x] Rust SQLite WAL ledger, shared migration, permissions, ingest, integrity, and aggregation
- [x] Rust streaming Claude parser against public sanitized 2.1.x fixture
- [x] Rust CLI `doctor`, `import claude`, `report --text`, and `hook-enqueue`
- [x] CI rustfmt, tests, and Clippy warnings-as-errors
- [x] Rust price snapshot verification, rate resolution, and exact cost calculation
- [x] Rust project resolution (override, safe config, git, manifest, cwd)
- [x] Rust conservative boundary classifier and secret-redacting label generator
- [x] Rust hook spool worker with crash-safe SQLite checkpointing
- [ ] Port recursive tree/query/corrections to Rust
- [ ] Signed cross-platform binaries and scoped npm launcher

## Phase 0 — Fixtures, schema, and naming
- [x] Locked names and distribution decisions
- [x] Monorepo and CI baseline
- [x] Complete v5.1 SQLite schema and migration metadata
- [x] Sanitized parser fixture format and compatibility matrix template
- [x] Boundary corpus schema, scorer, and required CHILD fixture
- [x] Checksummed price snapshot loader and threat model

## Phase 1 — Measurement core
- [x] Streaming Claude JSONL discovery/parser baseline
- [x] Truth-ladder source precedence and request deduplication
- [x] Append-only SQLite ingest, single-writer queue, WAL, permissions
- [x] Negative-delta anomalies and normative completeness formula
- [x] Integer-micro cost engine with cache-category rates
- [x] `doctor`, `reconcile`, `report --text`, and `import claude`
- [x] Prototype migration preview/apply, backup, and idempotence
- [ ] Real-version Claude compatibility matrix and official OTLP receiver
- [x] Verified Sonnet 4.6 rate snapshot and versioned cost persistence
- [ ] Broader verified model-rate coverage
- [ ] Crash-spool recovery integration and large-history performance proof

## Phase 2 — Plugin and classification
- [x] Current Claude plugin/marketplace structure
- [x] Enqueue-only lifecycle hooks with prompt/payload privacy
- [x] Project override/config/Git/manifest/cwd resolution baseline
- [x] Conservative CONTINUE/CHILD/SWITCH classifier and redacted labels
- [x] Explicit measured/unavailable `start` and `stop`
- [x] Ledger-only TokenTree skill
- [x] Checkpointed background hook spool consumer
- [ ] Transient-prompt classifier worker
- [x] Recursive `/tokentree` project/work-item tree and structured query
- [x] Attach/detach/note corrections
- [ ] One-time clarification flow
- [ ] Bundled release CLI and real Claude compatibility proof

## Later phases
Phase 3 dashboard/corrections and Phase 4 hardening remain gated. Phase 5+ hosts and organization features must not start before Phase 4 passes. Public-beta acceptance is not claimed.
