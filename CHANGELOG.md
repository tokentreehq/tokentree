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
