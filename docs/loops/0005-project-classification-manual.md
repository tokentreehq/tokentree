# Loop 0005 — Project resolution, boundary classification, manual capture

## Goal
Create projects without Git, classify conservative CONTINUE/CHILD/SWITCH boundaries, and make explicit `start`/`stop` produce correctly attributed measured or unavailable ledger rows.

## PRD sections
§§8.4–8.5, 9, 10, 11.11–11.14, 18.1–18.3, 21.5 items 1–6, 12, 16, 31, 40.

## Tests
Resolution precedence, no-Git manifest detection, required CHILD rule, secret-safe labels, manual measured/unavailable stop, 10,000-bp attribution, and Stop not completing work items.

## Out of scope
Prompt model inference, file clustering, dashboard corrections, and automatic plugin hooks.

## Audit
- PASS — override/config/Git/manifest/cwd precedence is deterministic and no-Git detection works.
- PASS — malicious capability keys in project config are rejected.
- PASS — required regression-test CHILD behavior and conservative SWITCH/CONTINUE rules are tested.
- PASS — labels redact common secret forms and remain 3–8 words.
- PASS — manual stop stores null/unavailable without counts and never completes the work item.
- PASS — 10,000-bp attribution, tests, typecheck, corpus, and lint pass.
