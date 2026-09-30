# Loop 0010 — Rust pricing, project resolution, and classification engine

## User-visible goal
Enable the Rust engine to load checksummed price snapshots, price all ledger events with exact integer-micro calculations, resolve projects with and without Git, classify turn boundaries (CONTINUE, CHILD, SWITCH, UNCERTAIN) with redacted labels, and process Claude hook spools into sessions, turns, and work items with crash-safe SQLite checkpoints.

## PRD requirements covered
- §9, §10, §12.4, §15, §16, §17, §19, §20, §25, §28 (Requirements 2, 3, 4, 5, 12, 13, 14, 20, 22, 29, 30).

## Current behavior and gap
Currently, price snapshot loading, rate resolution, cost calculation on ledger events, project resolution, classification, and spool consumption exist only in the TypeScript reference harness (`packages/pricing`, `packages/core/src/project.ts`, `packages/classifier`, `apps/cli/src/pricing-ledger.ts`, `apps/cli/src/hooks-worker.ts`). Rust only had base measurement models, Claude JSONL parsing, SQLite ingest of raw usage events, and loopback OTLP. This loop brings pricing, project resolution, classification, and hook spool processing into Rust as authoritative components.

## Files/components involved
- `crates/tokentree-core`:
  - `pricing.rs`: Price snapshot model, SHA-256 verification, rate lookup, exact micro calculations.
  - `project.rs`: Project detection (override, config with forbidden key security checks, git root, manifest, cwd).
  - `classifier.rs`: Conservative boundary classifier (CONTINUE, CHILD, SWITCH, UNCERTAIN), secret redaction, 3–8 word label generator.
- `crates/tokentree-ledger`:
  - `pricing.rs`: Pricing version persistence and transactional cost calculations on usage events in SQLite.
  - `spool.rs`: Checkpointed hook spool processor ingesting Claude hooks into projects, project roots, sessions, turns, and work items.
- Tests in `crates/tokentree-core` and `crates/tokentree-ledger`.

## Tests and fixtures that will prove completion
- Price snapshot SHA-256 verification and rate resolution for Sonnet 4.6.
- Tampered snapshot rejection.
- Exact integer-micro cost calculation for input, output, cache-read, cache-write, reasoning.
- Project detection priority: override > config > git > manifest > cwd.
- Security rejection of `.tokentree.yml` containing command/exec/hook/egress capabilities.
- Non-Git manifest project detection (`package.json`, `Cargo.toml`).
- Safe Git detection that never treats `$HOME` as a Git project root.
- Classifier boundary evaluation matching required CHILD behavior on regression test prompts.
- Secret redaction and 3–8 word length constraint on derived labels.
- Spool processing creating projects, sessions, turns, and pending classifications with SQLite checkpoints.

## Security and privacy checks
- Pricing snapshots must match their cryptographic SHA-256 hashes.
- Untrusted `.tokentree.yml` files are rejected if they specify command execution, hooks, egress, or pricing.
- Prompt text is never persisted to SQLite; only SHA-256 `prompt_fingerprint` and redacted 3–8 word labels are stored.
- Secrets (tokens, api keys, gh keys) in prompts are redacted before label creation.

## Performance checks
- Bounded-memory streaming of hook spools.
- Transactional batch ingest into SQLite.

## Explicit non-goals
- Full tree display and interactive CLI commands (Loop 0011).
- Local dashboard (Loop 0012).

## Acceptance commands
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

## Audit
- PASS — Price snapshot loading and SHA-256 verification implemented and tested in Rust.
- PASS — Exact integer-micro cost calculation and rate resolution in Rust.
- PASS — Project resolution with override, config (forbidden key check), git, manifest, cwd.
- PASS — Boundary classification with secret redaction and 3-8 word labels in Rust.
- PASS — Spool worker with crash-safe checkpointing in SQLite implemented and tested in Rust.
- PASS — All quality gates pass: cargo test, cargo clippy (-D warnings), cargo fmt, pnpm test, pnpm fixture, pnpm lint.

