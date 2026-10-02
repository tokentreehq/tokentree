# Upgrade E2E — PR #6 (runbook)

Verifies opening a pre-PR#6 ledger (base `1089845`) with the PR#6 binary.

## Binaries
- OLD: `/tmp/tokentree-old-bin` — built from `main` @ `1089845`
- NEW: `/tmp/tokentree-release-bin` — PR tip (`0997332`)

## Procedure
1. `01-populate-old.sh` — old binary imports (claude snapshots+transcript,
   codex, grok, hermes; 2nd batch provider_fields for same request), manual
   session + rename/note/attach/split, custom `my_custom_kind` row.
2. `02-baseline.sh /tmp/e2e/home-old /tmp/e2e/baseline` — report, counts, kinds, DB copy.
3. Open same home with NEW binary: `doctor`, `reconcile`, `report --text`
   (migrations run at open). Compare against baseline.
4. Cross-batch truth ladder (new binary): import batch3 (transcript req_sup_1,
   otel req_sup_2), then batch4 (provider req_sup_1 → supersedes; transcript
   req_sup_2 → no-op). Assert exactly-once accounting.
5. Re-open → assert no-op (counts, migration version, totals unchanged).
6. Custom kind: assert byte-identical + `unmapped_source_kind` anomaly.
7. Backup/restore: mirror `restore_source_kind_backup` on a copy; assert
   byte-identical on all pre-existing columns.
8. Kill test: 15× `kill -9` during `doctor` on a baseline copy → open cleanly,
   `integrity_check` ok, migration exactly-once.
9. Re-import idempotency: transcript/codex/grok/hermes re-imports → all duplicates.

## Inputs
`/tmp/e2e/inputs/` — crafted JSONL fixtures (see generation in runbook).
Key cases:
- `claude/batch1/snapshots.jsonl` — cumulative usage_snapshot (sessions
  e2e_snap_a ×3, e2e_snap_b ×2) — overcounted by old parser.
- `claude/batch1/transcript.jsonl` — req_tl_1, req_tl_2.
- `claude/batch2/provider.jsonl` — provider_usage for req_tl_1 (truth ladder).
- `claude/batch3/a.jsonl`, `batch4/b.jsonl` — cross-batch supersede cases.

## Findings (see final report for evidence)
- PASS: no duplicate accounting on upgrade; no data loss; idempotent migrations.
- PASS: cross-batch supersede exactly-once; lower-precedence cannot displace.
- PASS: custom kinds preserved + anomaly logged; backup/restore byte-identical.
- PASS: kill -9 recovery clean, migration exactly-once.
- FIXED (this branch): `HERMES_ONESHOT_USAGE` missing from `source_kind`
  vocabulary — raw string write site in hermes adapter; added constant +
  `is_recognized()` coverage.
- FLAGGED: re-importing snapshot files after upgrade double-counts (old
  cumulative rows + new delta rows); needs a maintainer decision on the
  re-import story (repair command is the correct path).
- OBSERVED: migration logs `unmapped_source_kind` anomalies for non-allowlisted
  kinds, which lowers the reported completeness % (78.9 → 68.2 in the test
  ledger). By design, but noisy.
