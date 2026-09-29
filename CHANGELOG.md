# Changelog

## Unreleased
### Added
- Phase 0 monorepo, CI, operating manual, decisions, status, and security threat model.
- Complete initial SQLite ledger schema and migration runner.
- Sanitized parser/boundary fixture contracts and evaluation harness.
- Checksummed, intentionally empty price snapshot pending verified public rates.
- Streaming Claude fallback discovery/parser with truth-ladder deduplication, anomalies, checkpoints, and idempotent ledger ingest.
- Integer-micro cost engine, completeness, doctor, reconcile, text report, and Claude import commands.
- Previewable, backed-up, non-destructive prototype migration.
- Safe project resolver, conservative boundary classifier, and explicit manual start/stop attribution.
- Pre-release Claude marketplace/plugin with enqueue-only privacy-safe lifecycle hooks and a ledger-only skill.
- Checkpointed background hook worker that resolves projects and creates fingerprint-only pending turns off the hook clock.
- Recursive project/work-item terminal trees and structured direct/descendant query.
- Attach/detach replacement attribution and explicit notes.
- Sourced, checksummed Claude Sonnet 4.6 pricing persisted as versioned integer-micro calculations.
- Public sanitized Claude Code 2.1.x compatibility fixture and camelCase/nested-usage parser support.
- Readable hook-worker record and event-processing modules; README synchronized with pre-beta reality.
- Adopted a Rust-first production architecture in the PRD and decisions.
- Added executable Rust measurement, SQLite ledger, Claude parser, CLI, and hook-enqueue vertical slice.
- Added Rust 1.98.1 pinning and rustfmt/test/Clippy gates to CI.
- Claude plugin launcher now prefers the bundled Rust binary with a development-only Node fallback.
- Added a loopback-only Rust OTLP/HTTP JSON receiver for official Claude `api_request` events, with bounded bodies, raw-body-event exclusion, and non-loopback rejection.
