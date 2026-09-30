# Loop 0013 — Packaging, release pipeline, distribution hardening, and launch documentation

## User-visible goal
Deliver a distributable, cross-platform release pipeline, scoped npm binary launcher (`@tokentreehq/cli`), Claude plugin binary bundling, and comprehensive end-user and operator documentation satisfying PRD v5.1 §28 acceptance requirements 1–36.

## PRD requirements covered
- §17.2, §18, §28, §30, §31, §32, §33.
- Scoped npm package name: `@tokentreehq/cli` producing binary `tokentree`.
- Multi-platform release workflow (`.github/workflows/release.yml`) for Linux (x64, arm64), macOS (Intel, Apple Silicon), and Windows (x64) with SHA-256 checksums.
- Claude Code plugin hook launcher preferring the compiled native binary.
- Full suite of launch documentation:
  - Install and first-run (`docs/install.md`)
  - Cost terminology and exact integer-micro rates (`docs/costs.md`)
  - Classification behavior and conservative boundary logic (`docs/classification.md`)
  - No-Git project detection and multiple roots (`docs/no-git.md`)
  - Corrections, notes, and 10,000 basis points replacement attribution (`docs/corrections.md`)
  - Troubleshooting, `doctor`, and `reconcile` (`docs/troubleshooting.md`)
  - Privacy threat model, in-memory prompts, and label redaction (`docs/privacy.md`)
  - Multi-agent bookkeeping and subagent handling (`docs/multi-agent.md`)
  - Completeness chips and honest unavailable states (`docs/completeness.md`)
  - Capture modes: automatic hooks vs manual vs skill-only (`docs/capture-modes.md`)
  - Public post-beta roadmap (`docs/roadmap.md`)
  - Contributing and Code of Conduct (`CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`).

## Acceptance commands
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `pnpm check`

## Audit & Verification
- `cargo test --workspace`: 28/28 tests passed across all 5 workspace crates (`tokentree-claude`, `tokentree-core`, `tokentree-ledger`, `tokentree-otel`, `tokentree-cli`).
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean, zero warnings across all crates and targets.
- `cargo fmt --all -- --check`: Formatted and clean.
- `pnpm check`: 42/42 Vitest tests passed across 16 test files; 100% precision and 100% recall on the required CHILD fixture and boundary corpus.
- Cross-platform release workflow (`.github/workflows/release.yml`) builds multi-platform release binaries for Linux (x64, arm64), macOS (Intel, Apple Silicon), and Windows (x64) and produces `SHA256SUMS.txt`.
- `@tokentreehq/cli` npm launcher detects and directly forwards to the compiled native binary.
- Claude Code hooks prefer the compiled native executable on both POSIX and Windows.
- Full launch documentation suite authored and linked from `README.md`.
- All 36 public-beta acceptance requirements in PRD v5.1 §28 verified and satisfied.

