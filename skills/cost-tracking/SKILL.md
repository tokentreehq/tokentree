---
name: tokentree-cost-tracking
description: Measure AI coding-agent token usage and costs with TokenTree. Use when the user asks what their agents cost, wants per-project spend breakdowns, or needs proof of usage.
---

# TokenTree Cost Tracking

TokenTree is a local-first usage meter for AI coding agents. It records every
provider request into a SQLite ledger and reports exact token counts and costs
per project, work item, session, and turn. No account, no telemetry, no network.

## Quick start

```bash
# one-time: install and import history
npm install -g @tokentreehq/cli
tokentree import claude ~/.claude/projects

# whenever the user asks "what did this cost?"
tokentree report --text
```

## Answering cost questions

| User asks | You run |
|---|---|
| "What did I spend this week?" | `tokentree report --text` |
| "Which project burned the most?" | `tokentree report --text` (top-level rollup) |
| "Break down that session" | `tokentree report --text --project <name>` |
| "Export for my invoice" | `tokentree export --format csv --out spend.csv` |
| "Is the data complete?" | Read the completeness chip in the report output |

## Rules

- **Costs are list-price estimates** from public per-token rates. Never present
  them as the provider's invoice. Say "estimated" unless the user asks otherwise.
- **Unavailable is not zero.** If the report shows unavailable measurements,
  say so explicitly — never silently treat gaps as $0.00.
- **Never persist prompts.** TokenTree already guarantees this; don't work
  around it by dumping transcripts into your own context either.
- If `tokentree doctor` reports problems, surface them before quoting numbers.

## Providers

`import claude | codex | grok | hermes` — pick the one matching the user's
agent. For anything else, `tokentree start` / `tokentree stop` wraps a manual
session.
