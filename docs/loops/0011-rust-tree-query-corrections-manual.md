# Loop 0011 — Rust recursive tree, query, corrections, manual runs, and CLI parity

## User-visible goal
Enable the Rust engine and CLI to render recursive project/work-item trees (`tokentree report --text`), execute structured ledger queries (`tokentree query`), run manual sessions (`tokentree start`, `tokentree stop`), and perform versioned corrections (`tokentree note`, `tokentree attach`, `tokentree detach`, `tokentree rename`, `tokentree move`).

## PRD requirements covered
- §8.6, §9, §10, §12.4, §15, §16, §17, §18, §19, §20, §25, §28 (Requirements 4, 6, 7, 9, 10, 16, 26, 27, 31, 32, 34, 35).

## Current behavior and gap
Recursive project/work-item tree rollup, formatted output, structured queries, manual start/stop runs, and corrections (attach, detach, notes, renames) existed only in the TypeScript CLI (`apps/cli`). `apps/rust-cli` only supported `doctor`, `import claude`, `report --text` (flat summary), and `otlp-serve`. This loop brings full tree hierarchy, structured querying, manual session tracking, and versioned attribution corrections to the Rust engine and CLI.

## Files/components involved
- `crates/tokentree-ledger`:
  - `tree.rs`: Recursive project tree loading, child-into-parent usage rollups, and formatted tree rendering.
  - `query.rs`: Structured ledger queries by project and work item with descendant inclusion.
  - `manual.rs`: Explicit `start` and `stop` manual runs, sessions, usage events, and attributions.
  - `corrections.rs`: `attach_session`, `detach_session`, `add_note`, `rename_work_item`, `move_work_item` with versioned replacement attribution groups summing to 10,000 basis points.
- `apps/rust-cli`:
  - Subcommands: `query`, `start`, `stop`, `attach`, `detach`, `note`, `classify`, and updated `report --text` rendering the full recursive tree.
- Tests in `crates/tokentree-ledger` and `apps/rust-cli`.

## Tests and fixtures that will prove completion
- Test recursive rollups: children roll up into parent work items and roots correctly.
- Test incomplete numbers: tree never displays bare dollar without completeness chip; unavailable counts surfaced.
- Test structured query: project and work item exact lookup with and without descendants.
- Test manual start/stop: start creates session and active run; stop creates usage event, attribution group, and marks run stopped without auto-completing the work item.
- Test versioned attribution: attach supersedes previous active group, new group active, weights sum to exactly 10,000 basis points.
- Test detach: marks attribution group inactive, removes project from session.
- Test notes: creates note record tied to work item and project.
- Test rename and move: updates title and parent_id safely.

## Security and privacy checks
- CLI arguments are strictly parsed and sanitized.
- Notes must be 1–240 characters.
- Token counts in manual stop must be non-negative safe integers.
- Attribution groups use transactional updates to maintain integrity.

## Performance checks
- Hierarchical rollups use indexed queries and in-memory tree building in O(N).
- Terminal tree prints in under 50ms.

## Acceptance commands
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `pnpm check`

## Audit & Verification
- `cargo test --workspace`: 24/24 tests passed across `tokentree-claude`, `tokentree-core`, `tokentree-ledger`, `tokentree-otel`, and `tokentree-cli`.
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean, zero warnings.
- `cargo fmt --all -- --check`: Formatted and clean across all crates.
- `pnpm check`: 42/42 tests passing across 16 test files; boundary evaluation at 100% precision and 100% recall with 0 rows moved.
- Recursive trees: Inclusive child-into-parent rollup verified.
- Manual start/stop: Explicit manual sessions created and stopped with token usage, with work items remaining open.
- Attribution corrections: Attach, detach, rename, and move operations maintain 10,000 basis points weight invariants and supersede inactive groups.
- Prototype migration: Non-destructive preview, backup creation, and idempotent application verified.

