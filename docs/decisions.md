# Decisions

## 2026-09-29 — Product and distribution names (locked)
- Product: TokenTree
- Commands: `/tokentree`, `tokentree`; optional alias `/tuse`
- GitHub: `tokentreehq/tokentree`
- npm: `@tokentreehq/cli`, binary `tokentree`
- Marketplace: `tokentreehq/tokentree`
- Config/data: `.tokentree.yml`, `~/.tokentree/`, `TOKENTREE_HOME`
- Never publish bare `tokenusage` or `tokenuse`; never use `github.com/tokentree`.

## 2026-09-29 — License intent
Apache-2.0 for core and adapters, pending final legal review. Source files use SPDX identifiers. MIT remains an acceptable counsel-directed fallback.

## 2026-09-29 — Homebrew
Defer a public tap until official release binaries with code signing exist. Reserved target: `tokentreehq/tap/tokentree`; this is not a current installation claim.

## Open human decisions
Domain, default `tuse` alias, minimum Claude Code version after fixtures, and signed-release keyholder.

## 2026-09-29 — Rust-first production engine (locked)
The published CLI, hook enqueue, SQLite writer/migrations, streaming parsers, OTLP receiver, canonical measurement/deduplication, exact cost engine, and report/query aggregation are implemented in Rust. TypeScript remains for host-required plugin glue, dashboard/UI, and the community-adapter SDK. `@tokentreehq/cli` distributes SHA-256 checksummed Rust binaries and must not become a second JavaScript measurement implementation.

Reason: these paths benefit directly from fast startup, bounded memory, native concurrency, one-binary packaging, stable filesystem permissions, and compile-time measurement-state modeling. The SQLite and JSON contracts remain language-neutral so the migration does not rewrite user data.

## 2026-09-29 — Codex integration implemented for public beta candidate (updated)
OpenAI Codex integration is fully implemented in `crates/tokentree-codex` and `packages/adapters/codex` for the public beta candidate. It parses rollout and app-server JSONL streams, correlates hook turn boundaries (`turn/started`) with token events, calculates cumulative deltas, detects negative deltas and counter resets as anomalies that penalize completeness, deduplicates against summary counters, preserves subagent parent/child relationships, strictly forbids prompt/completion storage, and integrates into the CLI (`tokentree import codex`), doctor, and reconcile. Acceptance Criterion 24 is marked PASS.
