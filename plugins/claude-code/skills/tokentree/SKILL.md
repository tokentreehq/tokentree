---
name: tokentree
description: Report AI coding-agent usage from TokenTree's local SQLite ledger. Use when the user asks about tokens, cost, projects, tasks, completeness, or attribution.
---

# TokenTree

Use only the `tokentree` CLI. Never infer usage numbers from conversation context and never read host JSONL directly for a report.

- For a terminal report, run `tokentree report --text`.
- For diagnostics, run `tokentree doctor`.
- For source drift, run `tokentree reconcile`.
- If several projects or work items match, ask the user to choose.
- Preserve the distinction between measured, unavailable, anomalous, and incomplete.
- Never describe skill-only installation as automatic tracking.
