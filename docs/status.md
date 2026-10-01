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
- [x] Port recursive tree/query/corrections to Rust
- [x] Cross-platform release pipeline with SHA-256 checksum generation and scoped npm launcher

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
- [x] Real-version Claude compatibility matrix and official OTLP receiver
- [x] Verified Sonnet 4.6 rate snapshot and versioned cost persistence
- [x] Broader verified model-rate coverage
- [x] Crash-spool recovery integration and large-history performance proof

## Phase 2 — Plugin and classification
- [x] Current Claude plugin/marketplace structure
- [x] Enqueue-only lifecycle hooks with prompt/payload privacy
- [x] Project override/config/Git/manifest/cwd resolution baseline
- [x] Conservative CONTINUE/CHILD/SWITCH classifier and redacted labels
- [x] Explicit measured/unavailable `start` and `stop`
- [x] Ledger-only TokenTree skill
- [x] Checkpointed background hook spool consumer
- [x] Transient-prompt classifier worker
- [x] Recursive `/tokentree` project/work-item tree and structured query
- [x] Attach/detach/note/merge/split corrections with transactional 10,000 bp invariants
- [x] One-time clarification flow
- [x] Bundled release CLI and real Claude compatibility proof
- [x] Multi-root project tree aggregation, root deactivation, and directory move
- [x] Subagent capability-based reconciliation and duplicate counter detection

## Phase 3 — Dashboard, static export, and corrections
- [x] Local web dashboard (loopback + random session token)
- [x] Interactive tree drilldown and project inspection
- [x] In-dashboard corrections (rename, merge, split, move, add note, attach, detach)
- [x] Self-contained static HTML export (`tokentree report --html`)
- [x] Structured JSON and CSV exports (`tokentree export`)

## Phase 4 — Hardening and distribution
- [x] Cross-platform release pipeline with SHA-256 checksums, verifiable artifacts, SPDX 2.3 & CycloneDX 1.5 SBOMs, and scoped npm launcher (Criterion 36 PARTIAL pending live registry publication)
- [x] End-to-end beta validation (Acceptance criteria 1–36 audited in `docs/public-beta-evidence.md`: 32 PASS, 4 PARTIAL [Criteria 1, 2, 24, 36], 0 DEFERRED, 0 FAIL)
- [x] OpenAI Codex adapter (`crates/tokentree-codex` & `packages/adapters/codex`) with versioned real formats, strict precedence, covered counter suppression, durable append checkpoints, and adversarial tests (Criterion 24 PARTIAL pending official live daemon release)
- [x] Isolated Claude plugin automation, zero-config report, and cross-platform data preservation on uninstall (Criteria 1 & 2 PARTIAL, Criterion 18 PASS)

## First-Class Adapters & Provider Validation Suite
- [x] Grok CLI adapter (`crates/tokentree-grok` & `packages/adapters/grok`) with exact $10^{-9}$ USD tick preservation, turn-level precedence, zero-token billing failure handling, and durable ingestion checkpoints
- [x] Hermes / OpenRouter adapter (`crates/tokentree-hermes` & `packages/adapters/hermes`) with SQLite `state.db` and `--usage-file` parsing, subagent hierarchy correlation, and live verified non-zero token generation
- [x] Provider-neutral validation and diagnostics suite (`tokentree validate`, `docs/validation.md`) with automated discovery, configuration, import, integrity, reconciliation, and privacy auditing




