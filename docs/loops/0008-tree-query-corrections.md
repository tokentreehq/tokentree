# Loop 0008 — Recursive tree, query, corrections, and compatibility

## Goal
Move beyond a flat token ledger: ship recursive project/work-item totals, structured query, attach/detach/note, verified pricing, realistic Claude fixtures, and readable hook-worker modules. Do not start the dashboard.

## PRD sections
§§8.6–8.9, 11.14, 12, 13, 15, 21.3–21.5, 25 Phase 2.

## Tests
Recursive parent totals, exact priced categories, unavailable cost, structured direct/recursive query, notes, attach/detach replacement attribution, public Claude 2.1.x camelCase/nested usage compatibility, and hook checkpoint replay.

## Audit
- PASS — recursive totals equal direct usage plus descendants and read only SQLite.
- PASS — structured query supports project/work-item resolution and direct vs descendant-inclusive totals.
- PASS — attach creates a superseding 10,000-bp group; detach deactivates attribution without rewriting usage.
- PASS — explicit notes are bounded to 240 characters and are never copied from prompts.
- PASS — shipped Sonnet 4.6 rates are non-empty, checksummed, source-attributed, and use cache-specific pricing.
- PASS — unknown models remain unavailable rather than zero.
- PASS — public sanitized Claude 2.1.x records exercise camelCase identities, nested usage, fallbacks, subagents, and unknown records.
- PASS — hook worker is split into readable record parsing and event/persistence functions.
- PASS — README matches current pre-beta truth; dashboard work did not start.
- PASS — 42 tests, strict typecheck, boundary fixtures, and lint pass.
