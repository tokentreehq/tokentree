# Loop 0004 — Prototype migration

## Goal
Provide previewable, non-destructive, resumable migration from `~/.task-usage/ledger.json` into the canonical ledger with provenance and 10,000-bp attribution.

## PRD sections
§§16.19, 20, 21.5 items 9 and 21, 25 Phase 1, 32.

## Tests
Preview writes nothing, apply creates a backup, source remains unchanged, unknown rows remain unavailable, and replay is idempotent.

## Out of scope
Legacy price conversion beyond preserving model/token observations; live classification remains separate.

## Audit
- PASS — preview writes nothing; apply backs up and never deletes or edits the source.
- PASS — replay is idempotent and every attribution is exactly 10,000 bp.
- PASS — missing token counts are imported as unavailable, not zero.
- PASS — tests, typecheck, and lint pass.
