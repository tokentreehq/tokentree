# Contributing to TokenTree

We welcome community contributions to TokenTree! TokenTree is an open-source, local-first tool built to provide developers with honest, transparent, and private insights into AI coding-agent costs.

---

## Architecture Principles

When contributing code, please keep our binding engineering rules in mind:

1. **Rust-First Core**: Production measurement, parsing, SQLite database operations, cost arithmetic, and CLI subcommands are implemented in Rust.
2. **TypeScript for Glue & Web**: TypeScript is reserved for host plugin integration, browser dashboard tooling, and SDK interfaces.
3. **Never Fake Zeros**: Missing token measurements must always be recorded as `unavailable`, never as `$0.00`.
4. **Zero Prompt Retention**: Full prompt text, tool payloads, and diffs must never be persisted to disk or sent across the network.
5. **Quality Gates Must Pass**: All PRs must pass `cargo fmt`, `cargo test`, `cargo clippy -- -D warnings`, `pnpm typecheck`, `pnpm test`, and `pnpm fixture:boundaries`.

---

## Development Setup

### Prerequisites
- Rust 1.98.1+
- Node.js 22.13+
- pnpm 10.17+

### Install Dependencies & Verify
```bash
corepack enable
pnpm install
pnpm check
```

---

## Submitting Pull Requests

1. Fork the repository on GitHub (`tokentreehq/tokentree`).
2. Create a focused feature branch (`git checkout -b feature/my-feature`).
3. Write comprehensive tests covering new functionality or bug fixes.
4. Run `pnpm check` and ensure all unit tests, clippy checks, and formatting pass.
5. Open a Pull Request with a clear description of the problem and solution.
