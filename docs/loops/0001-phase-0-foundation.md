# Loop 0001 — Phase 0 foundation

## Goal
Create a tested, honest foundation: names, monorepo, full v5.1 schema, fixtures, evaluation harness, pricing snapshot contract, and threat model.

## PRD sections
§§0–1, 12.3–12.7, 16, 17.4, 18, 21, 25 Phase 0, 27–28.

## Files touched
Root agent/docs/config files; `packages/database`; `packages/pricing`; `packages/classifier`; `fixtures`; `schemas`; CI.

## Tests
Schema migration and required tables/constraints; migration idempotence; price hash acceptance/rejection; CHILD/SWITCH scorer metrics and zero-denominator behavior.

## Out of scope
No host is claimed supported. No hooks, live ingestion, CLI reports, dashboard, classifier inference, cost calculation, or publication.

## Audit
### Product honesty
- PASS — unavailable is nullable in schema; no dollar UI exists.
- PASS — capture mode is not claimed; skill-only tracking is not claimed.
- N/A — cost surfaces and `--text` are not implemented.
### Measurement
- PASS — unique event hash/source identity and append-only trigger support idempotence/immutability.
- PASS — nullable usage and anomaly tables exist; money uses integer micros.
- PASS — adapter capabilities include the subagent double-count flag contract.
- PASS — active attribution rows are constrained to basis points; transactional sum validation is Phase 3.
- N/A — ingest, truth ladder, reports, and source-log access are Phase 1.
### Privacy/security
- PASS — schema has fingerprints/labels, not prompt bodies; threat model documents local-only defaults.
- PASS — no network or dashboard code exists.
- PASS — project config cannot execute because no config evaluator exists.
- N/A — filesystem permissions are enforced in Phase 1 database opening.
### Engineering
- PASS — tests, strict typecheck, fixtures, CLAUDE.md, and status are included.
- PASS — no adapter support claims.

## Result
PASS. Phase 0 foundation is mergeable. Next: Phase 1 Claude discovery and normalized, idempotent ingest.
