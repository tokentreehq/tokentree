# Loop 0012 — Interactive loopback dashboard, static HTML export, and multi-format exports

## User-visible goal
Provide a secure local loopback dashboard (`tokentree dashboard`), self-contained static HTML report generation (`tokentree report --html [path]`), and structured exports (`tokentree export --format json|csv`) allowing users to interactively drill down into projects, work items, and costs, review and apply corrections, and inspect status with zero external network dependencies and strict local privacy.

## PRD requirements covered
- §14.1, §14.2, §15, §17, §18.5, §28 (Acceptance requirements 8, 9, 10, 11, 12, 32, 34, 35).
- Loopback-only binding (`127.0.0.1`).
- Ephemeral random auth token passed on the URL and verified per request.
- Strict Content Security Policy (no remote scripts, styles, fonts, or CDNs).
- Never serves raw source transcripts.
- Self-contained static HTML export with embedded JSON, zero remote requests, and XSS escaping.
- Full drill-down: Projects -> Work items -> Children -> Attributions and notes.
- In-dashboard corrections: Rename, move, add note, attach, and detach.

## Architecture and components
- `apps/rust-cli`:
  - `dashboard.rs`: Axum web server bound strictly to `127.0.0.1` with ephemeral hex session token, security headers, and JSON API endpoints (`/api/projects`, `/api/status`, `/api/corrections/*`).
  - Embedded SPA dashboard HTML/CSS/JS with zero external CDNs, designed strictly under `landing-page-design` (Geist typography, dark mode palette `#181818` / `#1F1F1F` / `#272727`, flat cards, nested radius, Phosphor-style inline SVG icons, micro-motion).
  - Browser auto-open support with `--no-open` override.
  - Subcommands: `tokentree dashboard [--port <port>] [--no-open]`, `tokentree report --html [path]`, `tokentree export --format <json|csv> [--project <id>]`.
- `crates/tokentree-ledger`:
  - `html_report.rs`: Standalone static HTML generator embedding project trees, completeness chips, unavailable counts, and disclaimer.
  - `export.rs`: CSV and JSON serializers for usage events and hierarchical trees.

## Security and privacy checks
- Loopback bind only (`127.0.0.1`).
- Request authorization via random 32-character token.
- Strict Content Security Policy (`default-src 'self' 'unsafe-inline'; ... frame-ancestors 'none';`).
- Absolute paths outside the project root are redacted in static exports.
- All user-controlled strings (titles, notes, keys) are HTML-escaped.

## Acceptance commands
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `pnpm check`

## Audit & Verification
- `cargo test --workspace`: 28/28 tests passed across `tokentree-claude`, `tokentree-core`, `tokentree-ledger`, `tokentree-otel`, and `tokentree-cli`.
- `cargo clippy --workspace --all-targets -- -D warnings`: Clean, zero warnings.
- `cargo fmt --all -- --check`: Formatted and verified.
- `pnpm check`: 42/42 Vitest tests passing; boundary precision and recall at 100%.
- Dashboard: Axum server bound strictly to loopback (`127.0.0.1`), generates high-entropy random session token, enforces token on all routes, serves zero-CDN SPA with strict CSP, and handles in-app corrections via JSON API.
- Static HTML report: `tokentree report --html [path]` outputs self-contained HTML with embedded styles, inline Phosphor-style icons, real-time client search, completeness chips, unavailable indicators, and XSS escaping.
- Multi-format exports: `tokentree export --format json|csv` properly serializes full recursive project hierarchies.

