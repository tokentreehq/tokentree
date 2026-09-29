# TokenTree

TokenTree is an open-source, local-first developer tool that measures AI coding-agent usage and organizes immutable provider requests into projects → work items → sessions → turns. Its production engine is Rust; TypeScript is reserved for plugin glue, the dashboard, and extension tooling.

> **Status:** Public Beta (v0.2.0). Rust-first authoritative engine, SQLite WAL ledger, streaming Claude parser, loopback OTLP receiver, exact integer-micro cost arithmetic, conservative boundary classifier, recursive tree reports, interactive loopback dashboard, self-contained static HTML export, versioned attribution corrections, and multi-platform release packaging.

## Features

- **Authoritative Rust Engine**: Published `tokentree` CLI, hook enqueue, SQLite WAL owner, streaming parsers, and report/query execution.
- **SQLite WAL Ledger**: Transactional migrations, single-writer coordination, append-only usage events, and idempotent replay.
- **Truth-Ladder Ingestion**: Official OTLP request telemetry (`127.0.0.1:4318`) outranks transcript observations; canonical request ID deduplication.
- **Exact Integer-Micro Costs**: Zero floating-point rounding errors; distinct rates for base input, cache read, cache write, output, and reasoning tokens.
- **Honest Incompleteness**: Never presents missing measurements as `$0.00`; surfaces completeness chips and unavailable counts.
- **Conservative Boundary Classifier**: Groups turns into projects, work items, and child tasks (`CONTINUE`, `CHILD`, `SWITCH`) with 100% precision on boundary benchmarks.
- **Privacy by Design**: In-memory prompt analysis only; prompts, completions, tool inputs, and diffs are never persisted; derived labels are redacted.
- **Recursive Terminal Trees**: `tokentree report --text` renders hierarchical project/work-item rollups in under 50ms without opening ports.
- **Interactive Local Dashboard**: `tokentree dashboard` opens a loopback-only SPA with ephemeral random session tokens, strict CSP, and zero external CDNs.
- **Static HTML & Multi-Format Export**: `tokentree report --html` generates self-contained, XSS-safe static reports; `tokentree export` provides JSON and CSV.
- **Versioned Corrections**: Rename, move, add note, attach, and detach operations maintaining 10,000 basis points weight invariants.
- **Cross-Platform**: Binaries for macOS (Apple Silicon / Intel), Linux (x86_64, aarch64), and Windows (x64) with signed release workflows.

## Documentation

- [Installation & First Run](./docs/install.md)
- [Cost Terminology & Rates](./docs/costs.md)
- [Classification & Boundary Behavior](./docs/classification.md)
- [No-Git Project Detection](./docs/no-git.md)
- [Corrections & Attribution](./docs/corrections.md)
- [Privacy Model & Threat Model](./docs/privacy.md)
- [Completeness & Unavailable State](./docs/completeness.md)
- [Capture Modes](./docs/capture-modes.md)
- [Multi-Agent Bookkeeping](./docs/multi-agent.md)
- [Troubleshooting & Diagnostics](./docs/troubleshooting.md)
- [Public Roadmap](./docs/roadmap.md)


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
