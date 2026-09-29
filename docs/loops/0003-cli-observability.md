# Loop 0003 — CLI observability

## Goal
Make the ledger usable end-to-end: `import claude`, `doctor`, `reconcile`, and ledger-only `report --text`, with resolved OS paths and prompt-leak checks.

## PRD sections
§§8.1–8.2, 11.1, 11.5–11.10, 12.7–12.8, 15, 18, 20, 21.5.

## Tests
Path matrix, import replay, doctor leak detection, ledger-only report completeness, and `--text` no-server behavior.

## Out of scope
Dashboard, HTML, external pricing refresh, real-version Claude compatibility claim, and classification.

## Audit
- PASS — reports read SQLite only and unavailable cost has no bare dollar amount.
- PASS — `report --text` has no dashboard/server code path.
- PASS — doctor prints resolved paths, capture mode, integrity, prices, and prompt-leak status.
- PASS — reconcile reports duplicate identities and unresolved anomalies without modifying sources.
- PASS — tests, typecheck, and lint pass.
