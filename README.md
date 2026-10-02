```
  ______      __            ______
 /_  __/___  / /_____  ____/_  __/_______  ___
  / / / __ \/ //_/ _ \/ __ \/ / / ___/ _ \/ _ \
 / / / /_/ / ,< /  __/ / / / / / /  /  __/  __/
/_/  \____/_/|_|\___/_/ /_/_/ /_/   \___/\___/

  the meter for the agent economy

──────────────────────────────────────────────────────────

$ tokentree report --text

my-project                              1,248,300 tok      $4.21
├─ auth refactor                          842,100 tok      $2.84
│  ├─ session 2026-10-01                  512,400 tok      $1.73
│  └─ session 2026-10-02                  329,700 tok      $1.11
└─ bugfix login loop                      406,200 tok      $1.37
```

**Quickstart** · **[Docs](./docs/install.md)** · **[Plugin](./plugins/claude-code)** · **[npm](https://www.npmjs.com/package/@tokentreehq/cli)** · **[X](https://x.com/0xshrikar)**

[![CI](https://github.com/tokentreehq/tokentree/actions/workflows/ci.yml/badge.svg)](https://github.com/tokentreehq/tokentree/actions/workflows/ci.yml)
[![Release](https://github.com/tokentreehq/tokentree/actions/workflows/release.yml/badge.svg)](https://github.com/tokentreehq/tokentree/actions/workflows/release.yml)
[![npm](https://img.shields.io/npm/v/@tokentreehq/cli)](https://www.npmjs.com/package/@tokentreehq/cli)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)

# TokenTree is the meter for the agent economy.

Open-source, local-first usage measurement for AI coding agents. Every request captured, every token counted, every dollar accounted for — organized into projects → work items → sessions → turns.

**If your agents are employees, TokenTree is payroll.**

TokenTree sits underneath your agents and records what they actually cost: tokens in, tokens out, cache hits, reasoning spend — per project, per session, per turn. No cloud account. No telemetry. No subscription. Your data never leaves your machine.

## TokenTree is right for you if

- ✅ You run Claude Code / Codex / Cursor for hours and have **no idea what it costs**
- ✅ You want **per-project cost breakdowns**, not a single scary invoice at month-end
- ✅ You've been burned by **"estimated" usage dashboards** that don't match reality
- ✅ You need **proof of spend** — for clients, for teams, for yourself
- ✅ You believe measurement tools should be **local-first and open-source**
- ✅ You want your agent's work **organized automatically** into something you can actually read

## The Problem

AI coding agents burn money invisibly. A single long Claude Code session can chew through millions of tokens across input, cache reads, cache writes, output, and reasoning — each billed at a different rate. The provider dashboard shows you a monthly total. Your `~/.claude/projects` folder holds raw JSONL you'll never read. In between: nothing.

| Without TokenTree | With TokenTree |
|---|---|
| ❌ Monthly invoice arrives; you guess which project burned it | ✅ Per-project, per-session, per-turn cost trees |
| ❌ Transcript parsers double-count the same request | ✅ Truth-ladder dedup: official telemetry always wins |
| ❌ Cache hits billed wrong; "estimates" drift from reality | ✅ Integer-micro arithmetic per token tier — exact |
| ❌ Missing data silently shows as $0.00 | ✅ Gaps surface as *unavailable*, never $0.00 |
| ❌ Your usage data lives on someone else's server | ✅ SQLite ledger on your disk. No account, no network |

## How TokenTree solves it

1. **Capture everything** — Lifecycle hooks, transcript imports, and official OTLP telemetry feed one append-only ledger. Four providers supported out of the box.
2. **Deduplicate ruthlessly** — Canonical request identities + a versioned truth ladder: official provider telemetry outranks transcript observations, across batches. The same request is never counted twice.
3. **Count exactly** — Integer-micro cost arithmetic per token tier. No floats, no rounding drift, no "approximately $4.20".
4. **Organize automatically** — A conservative boundary classifier groups turns into projects → work items → sessions with 100% precision on benchmarks. You get a tree, not a spreadsheet.
5. **Stay honest** — Missing measurements and missing prices are *unavailable*, never $0.00. Completeness is reported, not assumed.

```
$ tokentree report --text

my-project                              1,248,300 tok      $4.21
├─ auth refactor                          842,100 tok      $2.84
│  ├─ session 2026-10-01                  512,400 tok      $1.73
│  │  ├─ turn 1                            48,200 tok      $0.16
│  │  └─ turn 2                           464,200 tok      $1.57
│  └─ session 2026-10-02                  329,700 tok      $1.11
└─ bugfix login loop                      406,200 tok      $1.37

completeness: 98.2% measured · 1.8% unavailable (2 sessions missing cache-write telemetry)
```

## Features

| 🎯 Truth-Ladder Ingestion | 💰 Exact Cost Math | 🔒 Private by Design |
|---|---|---|
| Official OTLP telemetry outranks transcript parsing. Canonical request IDs dedup across re-imports and providers. The same request is never counted twice. | Integer-micro arithmetic per token tier — input, cache read, cache write, output, reasoning. Zero floating-point drift. What you see is what you'd be billed. | Prompts, completions, diffs, and tool payloads are analyzed in memory and **never persisted**. There is nothing to leak. |

| 🌳 Auto-Organization | 🛡️ Honest Gaps | 📊 Three Ways to Read It |
|---|---|---|
| Turns become projects → work items → sessions automatically. Rename, merge, split, and annotate — all transactional, all attribution-safe. | Missing measurements show as *unavailable*, never `$0.00`. Every report carries a completeness chip so you know what you're not seeing. | Terminal trees (`report --text`), a local loopback dashboard (`dashboard`), and self-contained static HTML exports (`report --html`). |

| 🔌 Four Providers | 🧰 Repair Toolkit | 📦 Real Distribution |
|---|---|---|
| Claude Code (hooks/plugin/transcripts/OTLP), Codex (app-server + rollout), Grok, Hermes. Any other CLI via `tokentree start` / `stop`. | `doctor` for health, `reconcile` for dedup audits, `repair snapshot-overcount` for legacy data — dry-run first, backups always. | Native binaries for macOS (ARM/Intel), Linux (x64/ARM64), Windows (x64) via npm or GitHub Releases. SHA-256 checksummed. |

## Why TokenTree is different

| | TokenTree | Provider dashboards | Transcript scripts |
|---|---|---|---|
| **Granularity** | Per turn, per session, per project | Monthly totals | Whatever you grep |
| **Cost math** | Integer-micro, per tier, exact | Rounded, opaque | Float drift |
| **Dedup** | Truth ladder across providers & batches | N/A | Double-counts |
| **Missing data** | Surfaced as *unavailable* | Silent | Silent |
| **Privacy** | Prompts never touch disk | Your data on their servers | Your scripts, your risk |
| **Offline** | Fully local SQLite | Requires login | Local but fragile |
| **Open source** | Apache-2.0, audited | Closed | — |

## The truth ladder

When two sources describe the same request, the more authoritative one wins. Always.

```
  ┌─────────────────────────────────────────────────┐
  │              AUTHORITY (highest first)          │
  ├─────────────────────────────────────────────────┤
  │  1. official_telemetry   OTLP from the provider │
  │  2. provider_fields      app-server / rollout   │
  │  3. transcript_request   parsed JSONL           │
  │  4. turn_counter         cumulative fallback    │
  │  5. manual               you typed it           │
  └─────────────────────────────────────────────────┘
         ▲ higher precedence supersedes lower,
         │ exactly once, transactionally —
         │ even across import batches.
```

## What's under the hood

```
 providers ──► hooks / transcripts / OTLP ──► ┌──────────────────────┐
                                              │   SQLite WAL ledger  │
                                              │  (append-only,       │
                                              │   single writer)     │
                                              └──────────┬───────────┘
                        ┌────────────────────────────────┼────────────────────────────────┐
                        ▼                                ▼                                ▼
              ┌──────────────────┐            ┌──────────────────┐            ┌──────────────────┐
              │  terminal trees  │            │ local dashboard  │            │ static exports   │
              │  report --text   │            │ loopback only,   │            │ --html / --json  │
              │  <50ms, no ports │            │ token auth, CSP  │            │ --csv, XSS-safe  │
              └──────────────────┘            └──────────────────┘            └──────────────────┘
```

**Rust owns everything correctness-sensitive** — parsing, dedup, cost math, migrations, reports. One implementation, no drift. TypeScript exists only where the host requires it: Claude Code plugin hooks, the dashboard UI, the npm launcher.

## Cost math, concretely

| Token tier | Float math (typical) | TokenTree (integer-micro) |
|---|---|---|
| 1,000,000 input @ $3.00/MTok | `$3.0000000000000004` | `$3.000000` |
| 500,000 cache read @ $0.30/MTok | `$0.14999999999999997` | `$0.150000` |
| Mixed 5-tier session | drifts by cents per session | exact to the micro-dollar |

Cents-per-session drift sounds small until you multiply by ten thousand sessions.

## Providers

| Provider | Capture |
|---|---|
| **Claude Code** | Lifecycle hooks · plugin · transcript import · OTLP telemetry |
| **Codex** | App-server + rollout records (strict precedence, no double-count) |
| **Grok** | Session usage import |
| **Hermes** | Session + oneshot usage import |
| **Anything else** | `tokentree start` / `tokentree stop` manual sessions |

## Privacy

- **Prompts, completions, reasoning traces, diffs, and tool inputs are never written to disk.** Classification happens in memory; only derived labels persist.
- **Dashboard** binds to loopback only · single-use random tokens · strict CSP · zero external CDNs.
- **OTLP** requires a bearer token. Unauthenticated requests get 401 and write zero rows.
- **No account. No analytics. No network egress.** One Rust workspace — go read it.

Full model: [docs/privacy.md](./docs/privacy.md) · [docs/security.md](./docs/security.md)

## Quickstart

**Claude Code (recommended)**

```bash
# Ask your agent — it knows the drill:
# "Install TokenTree: add the tokentree plugin and the npm launcher,
#  then import my last 7 days of usage."

claude plugin add tokentree@tokentreehq/tokentree
npm install -g @tokentreehq/cli
tokentree report --text
```

**npm (any provider)**

```bash
npm install -g @tokentreehq/cli
tokentree import claude ~/.claude/projects
tokentree import codex ~/.codex/sessions
tokentree report --text
```

**GitHub Releases** — SHA-256 checksummed binaries for macOS (ARM/Intel), Linux (x64/ARM64), Windows (x64). See [Releases](https://github.com/tokentreehq/tokentree/releases).

**From source**

```bash
cargo build --release --bin tokentree
./target/release/tokentree doctor
```

Then: `tokentree dashboard` for the local web UI.

## Command reference

| Command | What it does |
|---|---|
| `doctor` | Installation & ledger health check |
| `import <provider> <dir>` | Import transcripts (claude / codex / grok / hermes) |
| `report --text` | Hierarchical cost tree, <50ms, no server |
| `report --html` | Self-contained static report |
| `dashboard` | Interactive local dashboard (loopback, token auth) |
| `export --format csv` | JSON / CSV export |
| `rename` `merge` `split` `move` `note` `attach` | Corrections — transactional, attribution-safe |
| `reconcile` | Dedup audit across the ledger |
| `repair snapshot-overcount --dry-run` | Preview fixing legacy overcounted snapshots |
| `otlp-serve` | Loopback OTLP receiver for official telemetry |
| `classify` | Re-run the boundary classifier |

## What TokenTree is not

- **Not a cost *optimizer*.** It won't tell you which model to use. It tells you what you spent, exactly.
- **Not a cloud product.** There is no hosted version, no team sync, no API. Local-first is the point.
- **Not an agent framework.** It measures agents; it doesn't run them.
- **Not exact to your invoice.** Amounts are list-price estimates from public per-token rates. Your provider invoice, credits, and subscription allowances differ.

## Documentation

[Install & first run](./docs/install.md) · [Cost model](./docs/costs.md) · [Classification](./docs/classification.md) · [Corrections](./docs/corrections.md) · [Capture modes](./docs/capture-modes.md) · [Multi-agent](./docs/multi-agent.md) · [Privacy](./docs/privacy.md) · [Troubleshooting](./docs/troubleshooting.md) · [Roadmap](./docs/roadmap.md) · [Beta evidence](./docs/public-beta-evidence.md)

## Contributing

TokenTree is early and opinionated. Highest-leverage contributions: new provider adapters, classifier eval cases from **real** transcripts, dashboard polish.

```bash
corepack enable && pnpm install
cargo test --workspace     # Rust — 37 suites
pnpm test                  # TS — 67 tests
```

Read [CONTRIBUTING.md](./CONTRIBUTING.md) and [CLAUDE.md](./CLAUDE.md) before opening a PR.

## Star history

[![Star History](https://api.star-history.com/svg?repos=tokentreehq/tokentree&type=date&legend=top-left)](https://www.star-history.com/#tokentreehq/tokentree&type=date&legend=top-left)

## License

[Apache-2.0](./LICENSE) — copyright 2026 TokenTree contributors.
