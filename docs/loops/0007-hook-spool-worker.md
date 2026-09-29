# Loop 0007 — Hook spool worker

## Goal
Consume sanitized Claude hook records off the hook clock, resolve projects, create sessions/turns, retain only prompt fingerprints, and make replay checkpoint-safe.

## PRD sections
§§8.3–8.4, 9, 10.5, 11.3, 17.3, 19–20.

## Tests
Project/session/turn creation, prompt-text absence, Stop-not-complete behavior, and checkpoint replay.

## Out of scope
Prompt-derived labels require a transient worker IPC path; fingerprint-only backlog remains Uncategorized rather than inventing a label.

## Audit
- PASS — hook records are processed only by the background command, never on the hook clock.
- PASS — prompt text is absent; only fingerprints are written to turns.
- PASS — checkpoint offsets make replay idempotent and leave incomplete trailing records for retry.
- PASS — Stop ends a turn but leaves work items open.
- PASS — projects resolve without Git and pending classification stays explicitly Uncategorized.
- PASS — tests, strict typecheck, corpus, and lint pass.
