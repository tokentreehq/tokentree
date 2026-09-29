# TokenTree

TokenTree is an open-source, local-first developer tool that measures AI coding-agent usage and organizes immutable provider requests into projects → work items → sessions → turns. Its production engine is Rust; TypeScript is reserved for plugin glue, the dashboard, and extension tooling.

> **Status:** active pre-beta. The Phase 0 foundation and substantial Phase 1–2 core are implemented. Claude transcript fallback, enqueue-only hooks, manual capture, ledger reporting, recursive trees, structured query, and corrections are usable for development; public automatic-tracking and public-beta claims remain gated by official telemetry, release packaging, broader compatibility fixtures, and hardening.

## Implemented

- SQLite WAL ledger, transactional migrations, append-only usage events, idempotent ingest, and versioned attribution
- Rust measurement core, exact-cost arithmetic, SQLite owner, streaming Claude parser, CLI, and privacy-safe hook enqueue
- Loopback-only Rust OTLP/HTTP JSON receiver for official Claude API-request events
- Streaming Claude JSONL fallback parser with request dedupe, checkpoints, unknown/truncated tolerance, and negative-delta anomalies
- Claude Code marketplace/plugin structure with enqueue-only lifecycle hooks; prompts and tool payloads are not persisted
- Project resolution from explicit override, verified/config identity, Git, manifest, or cwd
- Conservative CONTINUE/CHILD/SWITCH classifier with the required regression-test CHILD fixture
- Integer-micro cost engine and a checksummed, sourced Claude Sonnet 4.6 price snapshot
- Recursive terminal project/work-item trees, structured ledger query, attach/detach/note, and explicit start/stop
- `doctor`, `reconcile`, `import claude`, and non-destructive prototype migration

## Not yet claimed

- Official Claude request-level OTLP receiver and full real-version compatibility matrix
- Transient in-memory prompt-classifier IPC for automatic labels
- Bundled release CLI and published npm/marketplace artifacts
- Local dashboard, static HTML, complete correction UI, or public-beta acceptance

See [`docs/status.md`](./docs/status.md) for the exact delivery truth.

## Architecture

Rust owns latency-, correctness-, and packaging-sensitive paths: the published CLI, SQLite writes and migrations, streaming parsers, correlation/deduplication, OTLP, exact costs, and report/query execution. TypeScript remains for host-required plugin files, dashboard/UI, and the community-adapter SDK. SQLite and normalized JSON are stable cross-language contracts.

The current TypeScript CLI remains as a migration/reference harness while commands move into `apps/rust-cli`; it is not the final published measurement engine.

## Development

Requires Rust 1.98.1, Node.js 22.13+, and pnpm 10.

```bash
corepack enable
pnpm install
pnpm check
pnpm lint
```

Rust-only verification:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Useful development commands:

```bash
cargo build --release --bin tokentree
target/release/tokentree doctor
target/release/tokentree import claude ~/.claude/projects
target/release/tokentree report --text
```

`report --text` never starts or binds a server. Reports read only the SQLite ledger, never live JSONL.

## Privacy and cost honesty

TokenTree is local-only by default, has no account or telemetry, and does not persist full prompts, completions, reasoning, source, diffs, or tool payloads. Missing measurement and missing pricing remain unavailable; they are never displayed as measured `$0.00`.

Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.

The product constitution is [`TokenTree-PRD-v5.1.md`](./TokenTree-PRD-v5.1.md). Engineering rules are in [`CLAUDE.md`](./CLAUDE.md).

## License

Apache-2.0 intent, pending final legal review before public release.
