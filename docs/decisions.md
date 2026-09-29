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
Defer a public tap until a signed CLI artifact exists. Reserved target: `tokentreehq/tap/tokentree`; this is not a current installation claim.

## Open human decisions
Domain, default `tuse` alias, minimum Claude Code version after fixtures, and signed-release keyholder.

## 2026-09-29 — Rust-first production engine (locked)
The published CLI, hook enqueue, SQLite writer/migrations, streaming parsers, OTLP receiver, canonical measurement/deduplication, exact cost engine, and report/query aggregation are implemented in Rust. TypeScript remains for host-required plugin glue, dashboard/UI, and the community-adapter SDK. `@tokentreehq/cli` distributes signed Rust binaries and must not become a second JavaScript measurement implementation.

Reason: these paths benefit directly from fast startup, bounded memory, native concurrency, one-binary packaging, stable filesystem permissions, and compile-time measurement-state modeling. The SQLite and JSON contracts remain language-neutral so the migration does not rewrite user data.
