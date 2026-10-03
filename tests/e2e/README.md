# End-to-End Verification Suites

Repeatable shell runbooks that prove TokenTree's correctness properties
beyond unit tests. Each suite was executed green during the PR #6
pre-merge verification (2026-10-02); they are kept here so anyone can
re-run the proof.

All suites use isolated `TOKENTREE_HOME` directories under `/tmp` and a
release-built `tokentree` binary. They never touch your real ledger.

## Suites

### `upgrade/` — ledger upgrade compatibility

Proves a ledger created by an older binary opens cleanly under a newer
one: no duplicate accounting, no lost sessions/spans/corrections, exact
cross-batch truth-ladder supersession, idempotent migration replay,
custom `source_kind` preservation, byte-for-byte backup/restore, and
clean recovery from `kill -9` mid-migration.

- `01-populate-old.sh` — build a rich ledger with the OLD binary
- `02-baseline.sh <home> <outdir>` — capture report, counts, DB copy
- See `upgrade/README.md` for the full 9-step procedure.

### `repair-privacy/` — repair, providers, privacy

- `s1_repair.sh` — `repair snapshot-overcount` lifecycle: `--dry-run`
  writes nothing, refusal without `--yes`, apply fixes totals, second
  apply is a no-op, `--restore` is byte-for-byte, `kill -9` mid-repair
  leaves zero partial writes, missing transcripts don't break it.
- `s2_providers.sh` — every provider (Claude, Codex, Grok, Hermes) from
  a fresh home: sessions, usage events, spans, 10,000bp attribution
  groups, `report --text` trees, and re-import idempotency.
- `s3_privacy.sh` — poison hook canaries stay out of quarantine/DB/logs,
  dashboard token transport (401 without credentials, `?token=` stripped
  via `history.replaceState`), OTLP 401 with zero rows written.

## Running

```bash
# upgrade suite needs two binaries:
#   OLD: built from the previous release tag (CI does this via git worktree)
#   NEW: cargo build --release --bin tokentree @ HEAD
./tests/e2e/upgrade/01-populate-old.sh
./tests/e2e/upgrade/02-baseline.sh /tmp/e2e/home-old /tmp/e2e/baseline
./tests/e2e/upgrade/03-verify-upgrade.sh /tmp/e2e/home-old /tmp/e2e/baseline
# ...then follow tests/e2e/upgrade/README.md steps 4 and 7 manually
# (they need the ad-hoc crafted fixtures from the original run, never committed)

# repair/privacy suites need one release binary:
TOKENTREE_BIN=/path/to/tokentree ./tests/e2e/repair-privacy/s1_repair.sh
```

## CI

`.github/workflows/ci.yml` job `e2e` runs these on every push/PR:

- **repair-privacy** (`s1_repair.sh`, `s2_providers.sh`, `s3_privacy.sh`) — every run.
- **upgrade** (`01-populate-old.sh`, `02-baseline.sh`, `03-verify-upgrade.sh`) —
  only on `main` pushes and tags: it builds a second (old) binary from the
  previous release tag, which is too slow for every PR.

The remaining upgrade runbook steps (4: cross-batch truth ladder, 7:
`restore_source_kind_backup` round-trip) stay manual — they depend on
fixtures that were never committed. Keep all scripts executable and don't
modify their logic without re-running the full suite.
