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
- `apps/cli`: scoped CLI package and `tokentree` binary
- `apps/dashboard`: local dashboard (Phase 3)
- `packages/core`: canonical types and architecture interfaces
- `packages/database`: SQLite schema and migrations
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
# Phase 1+: tokentree doctor; tokentree reconcile
# Phase 3+: pnpm --filter @tokentreehq/dashboard dev
```

## Architecture
`spool → adapter → ledger → resolver → classifier worker → cost engine → UI`.
The SQLite ledger is the only number any UI may show. Reports never read live JSONL.

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
Phase 0: schema, naming, fixtures, evaluation harness, and threat model. Next slice: Phase 1 streaming Claude discovery, normalization, and idempotent ingestion.

## Adding an adapter
Discover sessions; stream and normalize requests; declare capabilities including `subagent_tokens_already_in_parent`; add sanitized fixtures and golden tests; pass the compatibility matrix; only then claim support.

Source of truth: `TokenTree-PRD-v5.1.md`. Decisions: `docs/decisions.md`.
