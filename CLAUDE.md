# TokenTree engineering manual

## What TokenTree is
TokenTree is an open-source, local-first developer tool.
It measures AI coding-agent usage once at the provider-request boundary.
It organizes immutable measurements into projects → work items → sessions → turns.
It keeps inferred attribution versioned and correctable without rewriting usage.
It reports honest token, cost, provenance, and completeness data from a SQLite ledger.

## Constitution
1. Never present a missing measurement as a dollar, and never tell a skill-only user they are being tracked.
2. Measure the provider request once, then classify it. Classification never rewrites the event.

## Locked names
Product: TokenTree. CLI: `tokentree`. npm: `@tokentreehq/cli`. GitHub: `tokentreehq/tokentree`. Config: `.tokentree.yml`. Data: `~/.tokentree/` or `TOKENTREE_HOME`. Never publish bare `tokenusage` or `tokenuse` and never use `github.com/tokentree`.

## Repository map
- `apps/rust-cli`: canonical published `tokentree` binary
- `apps/cli`: temporary TypeScript migration/reference harness; not the production measurement engine
- `apps/dashboard`: local dashboard (Phase 3)
- `crates/tokentree-core`: canonical measurement, deduplication, completeness, and exact-cost logic
- `crates/tokentree-ledger`: production SQLite owner, migrations, ingest, and aggregation
- `crates/tokentree-claude`: bounded-memory Claude discovery/parser
- `packages/core`: TypeScript contract/reference tests during migration
- `packages/database`: schema compatibility harness; Rust owns production writes
- `packages/classifier`: boundary corpus and evaluation harness
- `packages/pricing`: checksummed price snapshots
- `packages/reports`: ledger-only reports
- `packages/adapters/claude`: Claude adapter when compatibility tests pass
- `plugins/claude-code`: enqueue-only plugin hooks (Phase 2)
- `fixtures/parsers`, `fixtures/boundaries`: sanitized compatibility and classification data

Empty adapter packages must not claim support. Codex is roadmap-only until Phase 5.

## Commands
```bash
pnpm install
pnpm test
pnpm typecheck
pnpm lint
pnpm fixture:boundaries
pnpm check
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# Phase 1+: tokentree doctor; tokentree reconcile
# Phase 3+: pnpm --filter @tokentreehq/dashboard dev
```

## Architecture
`spool → adapter → ledger → resolver → classifier worker → cost engine → UI`.
The SQLite ledger is the only number any UI may show. Reports never read live JSONL.

Rust is mandatory for the published CLI/hook enqueue, SQLite ownership, streaming parsers, OTLP, canonical measurement/deduplication, exact cost, and report/query aggregation. TypeScript is for host-required plugin glue, dashboard/UI, and the community-adapter SDK. Do not create two authoritative implementations.

Truth ladder: official OTel/app-server telemetry → provider fields → transcript request → snapshot delta → CLI counts → unavailable.

## Privacy
Default local-only; no telemetry or network. Do not persist prompts, completions, reasoning, source, diffs, tool payloads, or full transcripts. Persist only fingerprints and redacted 3–8 word derived labels. `doctor` must fail when prompt text appears in DB or logs. Project config is untrusted and may only provide identity and stricter privacy.

## Do not
- No live JSONL reports or measured zero for unavailable data.
- No classifier or transcript parsing on the hook path.
- No fake supported adapters or empty host packages.
- No prompt persistence or remote dashboard assets.
- No shell interpolation of project/work-item titles.
- `--text` must never bind a port.

## Current phase
Phase 2 in progress with a Rust-first engine migration. Rust core, ledger, Claude parser, loopback OTLP/HTTP JSON receiver, CLI doctor/import/report, and hook enqueue exist. Next slice: port recursive tree/query/corrections and pricing persistence to Rust, then package signed binaries. Phase 3 remains gated.

## Adding an adapter
Discover sessions; stream and normalize requests; declare capabilities including `subagent_tokens_already_in_parent`; add sanitized fixtures and golden tests; pass the compatibility matrix; only then claim support.

Source of truth: `TokenTree-PRD-v5.1.md`. Decisions: `docs/decisions.md`.
