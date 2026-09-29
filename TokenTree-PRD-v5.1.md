# TokenTree — Product Requirements Document

| Field | Value |
|---|---|
| Status | Draft v5.1 — TokenTree product specification, Rust-first architecture amendment |
| Date | 2026-09-29 |
| Product | TokenTree |
| Type | Open-source, local-first developer tool |
| Lineage | Merges PRD v3.1 and v4.0 into v5.0, then rebrands to TokenTree in v5.1. |
| License intent | Apache-2.0 for core and adapters (legal review before publish); MIT acceptable if counsel prefers Apache-2.0 for the patent grant |
| Primary distribution | Claude Code plugin + standalone CLI from a **scoped** npm package that exposes the `tokentree` binary |
| Additional hosts | Codex CLI, Grok and other Agent Skills hosts, Gemini CLI, OpenCode |
| Related prototype | `task-usage-tracker` skill (`scripts/track.py`, `~/.task-usage/ledger.json`) — migration source and behavioral reference only |

---

## 0. How to read this document

This is the product constitution, not a changelog of two earlier drafts.

- **Product rules** (unavailable, completeness, capture mode, naming, prompt policy) are binding on every surface.
- **Measurement rules** (official telemetry first, dedupe, causal-request, overhead, integer micros) are binding on every adapter.
- Later phases (GitHub, Jira, orgs, hosted sync) may extend the schema. They must not weaken these rules.

Two sentences the product is not allowed to violate:

1. Never present a missing measurement as a dollar, and never tell a skill-only user they are being tracked.
2. Measure the provider request once, then classify it. Classification never rewrites the event.

---

## 1. Executive summary

TokenTree automatically measures AI coding-agent usage and organizes it into an explorable hierarchy of **projects, work items (features, tasks, bug fixes, subtasks), sessions, and turns**.

Existing tools answer:

- How many tokens did I use today?
- How much did this session cost?
- Which model or folder consumed the most?

TokenTree answers:

- How much did it cost to **build this game**?
- How much did **adding multiplayer** cost?
- How much did **fixing the collision bug** cost?
- Which child of that feature consumed the most?
- How confident is that attribution?
- How complete is that number, and is it billed or only estimated?

Users install once and work normally. Lifecycle hooks and official request telemetry capture usage. A local classifier groups turns into projects and nested work items. Users inspect results through `/tokentree`, a local dashboard, terminal reports, static HTML, or natural-language questions. They can rename, merge, split, move, attach, detach, and verify anything the classifier got wrong.

Git and GitHub improve identity. They are not required. When they are absent, TokenTree uses working directories, manifests, file paths, session history, timing, and optional in-memory prompt signals.

**Measured usage is immutable. Classification is inferred and correctable.** The SQLite ledger is the only number any UI may show. Missing measurements are **unavailable**, never a fabricated `$0.00`. Incomplete rollups always carry a completeness chip.

Estimated API-equivalent cost is never presented as an invoice.

The measurement layer prefers official request-level telemetry: Claude Code OpenTelemetry request events and Codex app-server token events. Hooks provide project, prompt, turn, task, and session boundaries. Local transcript and rollout parsing is a historical and compatibility fallback, not the only source of truth.

Manual `start` / `stop` / `attach` remains available for hosts without hooks and for deliberate naming.

> **Canonical disclaimer.** Amounts are list-price estimates from public per-token rates unless another cost type is labeled. They are not your provider invoice, prepaid credit balance, or subscription allowance.

### 1.1 Canonical names

| Surface | Name |
|---|---|
| Product | TokenTree |
| People type | `/tokentree` · `tokentree` · optional short alias `/tuse` |
| CLI binary | `tokentree` (short alias: `tuse`; legacy aliases: `tokenusage`, `taskuse`) |
| Slash command | `/tokentree` (Claude Code) · `$tokentree` (Codex when `/` is reserved) |
| Config | `.tokentree.yml` (legacy aliases: `.tokenusage.yml`, `.taskuse.yml`) |
| Data directory | `~/.tokentree/` (`TOKENTREE_HOME`; legacy `TOKENUSAGE_HOME`, `TASKUSE_HOME`, `~/.tokenusage/`, `~/.task-usage/` are import aliases only) |
| GitHub | Org `tokentreehq` · repo `tokentree` · `github.com/tokentreehq/tokentree`. Do not use `github.com/tokentree` — archived blockchain org. |
| npm / crates / brew | `@tokentreehq/cli` exposing the `tokentree` binary. Never publish bare `tokenusage` or `tokenuse`. |
| Claude marketplace slug | `tokentreehq/tokentree` |
| Optional domain | `tokentree.dev` or `tokentree.app` if registrable |

These names are locked as of 2026-09-29. Do not reopen them unless a registry rejects the create.

---

## 2. Problem

Coding agents are used for whole projects, features, fixes, tests, and docs. Native reports and most third-party CLIs (ccusage, toktrack, tokimeter, Token Tracker, the existing npm packages named `tokenusage` / `tokenuse`, and similar) aggregate by day, session, model, or directory.

Those units do not match how people evaluate work.

A project such as “Space Game” can span weeks, many sessions, several branches, and more than one agent. Inside it: a scoring system, multiplayer, a collision bug. One session can contain several tasks. One task can span several sessions.

Users therefore cannot reliably answer:

1. What did the entire project consume?
2. What did a particular feature consume?
3. What did a particular bug fix consume?
4. Which subtasks drove the cost?
5. How confident is the attribution?
6. Is the dollar value billed, subscription-allocated, or only API-equivalent?
7. Is that number complete, or are some turns unavailable?

Manual `start` / `stop` labeling can work and must remain available, but it is forgotten. The default path is automatic capture plus lightweight correction.

---

## 3. Vision

> Install once, work normally, and understand the token usage and estimated cost of every project, feature, task, bug fix, and subtask — measured when the host can measure, marked unavailable when it cannot.

TokenTree is the local source of truth for the economics of AI-assisted engineering work, across agents, without requiring GitHub or a cloud account.

### 3.1 Principles

1. **Automatic by default** on hosts that expose hooks or an adapter watcher.
2. **Hierarchical** — explorable from project down to turn.
3. **Local-first** — prompts, code, and transcripts stay on the machine by default. No account required.
4. **Agent-neutral core** — Claude Code is the first adapter, not the schema.
5. **Measured facts stay immutable** — reclassification never rewrites source usage.
6. **Explainable attribution** — every assignment has a method, confidence, and reason.
7. **Honest cost language** — estimated is not billed and is not a subscription allowance.
8. **Correctable** — rename, merge, split, move, attach, detach, verify.
9. **Low overhead** — hooks enqueue; they do not parse transcripts or run the classifier inline.
10. **Useful without GitHub** — directories, manifests, and prompts are enough.
11. **Manual override always works** — CLI `start` / `stop` / `attach` for hosts without hooks and for people who want to name the task themselves.
12. **Outcome hierarchy is the wedge** — time and model rollups exist as views, not the product.
13. **Concurrent sessions are first-class** — no global active-task file.
14. **Unavailable is first-class** — never fabricate measured zeros.
15. **The SQLite ledger is the only source of displayed numbers.**
16. **Capture mode is visible per host** — automatic / logs / manual.
17. **Official telemetry preferred** over internal transcript reconstruction.
18. **Do not become another daily usage dashboard.**

---

## 4. Goals

### 4.1 Product goals

- Detect projects from local context with and without Git.
- Classify agent turns into features, tasks, bug fixes, subtasks, and related types.
- Maintain parent-child relationships and recursive totals (direct, descendant, inclusive).
- Measure usage at turn, session, work-item, project, model, and agent levels.
- Account for subagents and cache token categories without double-counting.
- Compute model-aware estimated costs with versioned price tables and integer micros.
- Ship `/tokentree` inside Claude Code and equivalent commands on other hosts.
- Answer natural-language cost questions from the ledger, never from chat memory.
- Provide a local clickable dashboard with drill-down and a static HTML export.
- Import and classify existing Claude Code and later Codex history, plus the prototype JSON ledger.
- Let users correct classification without editing transcripts.
- Show confidence, data-completeness, cost type, attribution policy, and overhead on every rollup.
- Store data locally by default; no account required.
- Support explicit CLI bookkeeping for Grok and other hosts that lack a hook surface.
- Export JSON and CSV for spreadsheets or warehouses.
- Document adapter, classifier, project-resolver, pricing-source, and report-exporter interfaces.
- Prefer official request telemetry over internal transcript formats.
- Report measurement capability, completeness, and cost-category coverage per host.
- Separate user work items from agent-internal execution tasks.
- Define a reproducible default attribution policy (`causal-request`) and explicit overhead categories.
- Remain useful when hooks, telemetry, or local logs are blocked by policy.
- Reconcile hook-captured events against parsed session files.
- Evaluate classification quality on a labeled fixture corpus before calling the tree good.

### 4.2 Claims the product will not make

- TokenTree is not the provider invoice.
- TokenTree is not `/usage` rate-limit windows.
- TokenTree does not guarantee perfect task boundaries.
- TokenTree does not treat a Pro/Max subscription as cash spent per task unless the user opts into an allocation method.
- TokenTree is not automatic tracking on a skill-only install.
- TokenTree is not the existing npm packages named `tokenusage` or `tokenuse`.

### 4.3 Later surfaces this architecture must not block

These ship after the core is real. They stay in this PRD so schema and packaging can grow into them.

- Codex, Gemini CLI, OpenCode, Grok session-log adapters
- Cross-agent identity for the same project and work item
- GitHub / GitLab issue and pull-request linkage
- Jira and Linear linkage
- CI ingestion and cost per merged PR / successful outcome
- Team aggregation, self-hosted org server, role-based access
- Billing reconciliation and contracted rates
- Budget alerts and anomaly detection
- Optional hosted cloud sync (off by default, never required)

---

## 5. Users

### 5.1 Primary — individual AI-assisted developer

Needs cost of a project or feature, expensive-workflow visibility, no label maintenance, private prompts.

Example: an indie developer building a game in Claude Code for several weeks, sometimes switching to Codex or Grok.

### 5.2 Secondary — engineering lead

Needs cost by project, feature, or PR; comparison across models and agents; visibility into rework and failed attempts.

### 5.3 Secondary — platform / FinOps

Needs normalized export, custom rates, optional reconciliation, governance that does not vacuum source code into a vendor cloud.

### 5.4 Also served by the same ledger

Indie builder / consultant, open-source maintainer. The individual workflow ships first. Lead and FinOps views reuse the same ledger.

---

## 6. Jobs to be done

| Job | Success |
|---|---|
| Whole project | “Space Game used 18.4M tokens and an estimated $42.81 across 24 work items (98% complete).” |
| One feature | “Multiplayer used 4.8M tokens and an estimated $11.02.” |
| One bug fix | “The collision fix used 1.7M tokens and an estimated $3.84.” |
| Child inside a fix | “Regression tests accounted for $0.88.” |
| Compare work | “Player movement cost 31% more than enemy spawning.” |
| Compare agents | “Claude vs Codex on Space Game this month.” |
| Find expensive work | Top work items this month. |
| Inspect classification | Explanation from branch, files, continuity. |
| Correct classification | Move last three turns under Multiplayer in under a minute. |
| Attach history | Hang an older session on the collision fix. |
| No GitHub | Config, manifests, paths, history. |
| Non-code work | Named inbox project (`personal.*`). |
| See capture mode | `doctor` shows `claude=hooks`, `grok=manual`. |
| Privacy | No prompt / response / reasoning / source stored by default. |
| Incomplete data | Unavailable turns listed; parent dollar carries a completeness chip. |

---

## 7. Taxonomy

### 7.1 Hierarchy

```text
Workspace (this machine)
└── Project
    └── Work item
        └── Work item (child, recursive)
            └── Usage span
                └── Session + turn (+ subagent)
```

Work items use a recursive `parent_id` model. There is **no separate slice table**. The automatic classifier normally creates no more than two work-item levels below a project unless signals are strong. Users may create deeper structures. The UI collapses levels beyond the first two until expanded.

A project total includes all descendants. Every work item exposes **direct**, **descendant**, and **inclusive** totals. Direct-only is an explicit request (`--direct` or “show only direct usage”).

Types describe meaning; hierarchy describes containment.

### 7.2 Project

A persistent body of work, usually a root directory, repository, package manifest, workspace, or repeated file cluster.

Display name and machine id are separate. Moving a folder must not mint a duplicate project if identity still matches.

A project may have multiple aliases, roots, repositories, and historical paths. One work item has exactly one `project_id`. Multiple repositories belong on one project as **roots**, not as extra project ids on the item. Cross-project relationships use a later `related_work_items` table.

Examples: Space Game, Portfolio Website, Payments API, `personal.morning-brief` (no repo).

### 7.3 Work item

A meaningful unit of work inside a project.

Types are labels, not extra depth:

- Objective
- Feature
- Task
- Subtask
- Bug fix
- Refactor
- Test work
- Documentation
- Maintenance
- Investigation
- Review
- Deployment
- Uncategorized

Examples: Build initial playable game · Add multiplayer · Fix collision bug · Add collision regression tests.

Statuses: `active`, `stale`, `completed`, `abandoned`, `superseded`, `unknown`.

`Stop` ends an agent turn and never automatically completes a user work item. Internal `TaskCompleted` is supporting evidence, not authoritative user-work completion. Inactivity marks a work item **stale**, not completed. There is no auto-close.

### 7.4 Usage span

A measured range of agent activity associated with one turn or a bounded group of provider requests. One turn may contain several model requests, retries, compaction, auxiliary calls, and subagent calls.

The normal UI assigns a span entirely to one work item (10,000 basis points). The schema supports weighted many-to-many attribution from the beginning so advanced corrections do not require a destructive migration. Fractional allocation is never silently inferred.

### 7.5 Internal execution tasks

Claude `TaskCreated` records, Codex subagents, tool calls, plans, and internal checklist entries are execution metadata, not automatically user-visible work items. They become visible children only when they are meaningful to the user, consume substantial usage, persist across turns, match user language, or are explicitly promoted.

### 7.6 Overhead

Usage that cannot be fairly assigned to one work item is recorded separately and included in project totals without being silently charged to the first task:

- `overhead.project` — initialization and indexing
- `overhead.session` — session setup
- `overhead.compaction` — compaction and summarization
- `overhead.auxiliary` — auxiliary model requests
- `overhead.classification` — TokenTree classifier usage, including any optional external LLM

### 7.7 Measured vs inferred

**Measured** (immutable after ingest): token categories, model, session id, turn bounds, timestamps, cwd, transcript path, tool activity, files touched when observable, provider-reported cost if present.

**Inferred** (replaceable): project, title, type, parent, switch detection, completion, confidence.

---

## 8. User experience

### 8.1 Install

Claude Code:

```text
/plugin marketplace add tokentreehq/tokentree
/plugin install tokentree@tokentreehq
```

Standalone (illustrative scoped name; finalize in Phase 0):

```bash
npx @tokentreehq/cli@latest
npm install -g @tokentreehq/cli
tokentree --help
```

Agent Skills hosts that support the open skill format:

```bash
npx skills add tokentreehq/tokentree
```

Later: Homebrew formula matching the chosen package identity.

| Installation | Reports | Explicit bookkeeping | Automatic tracking |
|---|---|---|---|
| Standalone CLI | Yes | Yes | When an adapter watcher is supported |
| Skill only | Yes | Yes | No |
| Full host plugin with hooks | Yes | Yes | Yes, after compatibility tests |

The Claude plugin contains `/tokentree`, skill instructions, lifecycle hooks, a **bundled known-good CLI version**, database migrations, and the dashboard launcher. Codex distribution uses its documented plugin and skill formats.

No host is advertised as automatically tracked until its adapter and lifecycle integration pass compatibility tests. Skill-only installs never show a green “automatic tracking” badge.

Uninstall removes the plugin. Data remains in `~/.tokentree/` until the user purges it.

`tokentree doctor` and first-run Settings show per-host capture mode.

### 8.2 First run

Detect available history and offer:

```text
TokenTree installed.

Detected:
• Claude Code
• 6 possible projects
• 83 historical sessions
• 14.2M measurable tokens
• Prototype ledger at ~/.task-usage/ledger.json

TokenTree reads each new prompt in memory to name the task.
It does not save prompt text. Disable in Settings if you prefer.

Import history?
1. Import and classify everything
2. Import the last 30 days          ← default, after confirm
3. Track only new activity
```

State which local paths will be read (for example `~/.claude/projects/**/*.jsonl`) and that source files are not modified.

### 8.3 Automatic tracking

User:

```text
Build a top-down space shooter with player movement and enemy spawning.
```

TokenTree creates or resumes:

```text
Project: Top-Down Space Shooter
Work item: Build initial playable game
Confidence: 91%
```

Follow-ups attach automatically. Possible tree:

```text
Build initial playable game
├── Player movement
│   ├── Keyboard controls
│   └── Fix diagonal movement speed
└── Enemy spawning
```

Do not interrupt after every prompt. Notification modes:

- Only when project or top-level work item changes (default)
- After every classified turn
- Status line only
- Silent

Classifier lag must not block the agent. Pending turns show as “N turns pending classify” and remain Uncategorized until the worker finishes.

### 8.4 Cold start when tokens exist but no project does

Resolution still follows §9. If nothing reliable matches:

1. Prefer an existing verified mapping for this cwd.
2. Else create or resume a named inbox project (`personal.<derived-label>` or `personal.unassigned`) rather than inventing a fake repository.
3. Ask **once** with a single clarification: “Track this under [suggested title], or name it?” Remember the answer as a verified mapping.
4. Never ask again for that identity fingerprint unless the user later moves or splits the project.
5. Non-code recurring work stays in `personal.*` until the user promotes it.

### 8.5 Explicit bookkeeping

Always available:

```bash
tokentree start --project space-game --task "fix collision bug" --parent "build playable prototype"
tokentree note --text "wrap-around miss on right wall"
tokentree stop --input 11200 --output 3400 --cache-read 88000 --model claude-sonnet-4-6
tokentree attach --session <id> --task <id>
tokentree detach --session <id>
```

Used when the host has no hooks, when the user wants to name the work themselves, or when CI / `-p` runs should record a known task.

Hooks and explicit starts write the same ledger. Manual attribution has the highest confidence.

If a host exposes no counts, the row is **unavailable** under the named project, not `$0.00`. Completeness on the project drops. Capture mode still says `manual`.

### 8.6 `/tokentree`

Always prints a terminal tree. Opens the local dashboard when a display exists unless `--text`. `--html PATH` writes a static snapshot. **`--text` never opens a port.**

No arguments: current project tree (all time for that project), then other projects.

```text
Space Game — $21.70 est. API-equivalent · 1.8M tok · 14 items · 94% complete · conf 82%
├── Build initial playable game          $17.86
│   ├── Player movement                   $5.20
│   │   ├── Keyboard controls             $2.81
│   │   └── Fix diagonal speed            $2.39
│   ├── Enemy spawning                    $4.81
│   └── Scoring system                    $3.94
└── Fix collision bug                     $3.84
    └── Regression test                   $0.88

other projects
  Portfolio Website                       $4.10
  interview-prep                          $0.12
  personal.morning-brief                  $0.40
```

Incomplete trees never show a bare dollar amount without the completeness chip. Unavailable children are listed, not zeroed.

Accepted arguments:

```text
/tokentree
/tokentree Space Game
/tokentree how much did building this game cost
/tokentree task collision
/tokentree --direct
/tokentree week
/tokentree compare "player movement" "enemy spawning"
/tokentree compare agents
/tokentree review
/tokentree --html
/tokentree --text
```

On Codex, the same command is `$tokentree` if `/` is reserved.

### 8.7 Work-item detail

```text
Fix collision bug
id            wi_8f3a
project       Space Game
parent        (project root)
type          bug fix
status        open
when          2026-09-26 → 2026-09-27
sessions      3
turns         11
unavailable   1 turn
anomalies     0
tokens        in 80,200  out 21,100  cache_r 410,000
est. cost     $3.84  Estimated API-equivalent cost
direct        $2.96
descendants   $0.88
inclusive     $3.84
models        claude-sonnet-4-6
source mix    otel 70% · session-parse 30%
policy        causal-request
overhead      $0.12 session + compaction (in project total)
confidence    0.86  method: prompt_continuity+files
completeness  94%
explanation   Same cwd, collision/physics files, follow-up wording after playable-game objective
notes         wrap-around miss on right wall
```

Canonical footer on every cost surface:

> Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.

### 8.8 Natural language

Examples:

- How much did building this game cost?
- How much did multiplayer cost?
- What did fixing the collision bug cost?
- How much did the regression tests inside that bug fix cost?
- Which task was most expensive this week?
- Show only direct usage for multiplayer.
- Compare player movement and enemy spawning.
- Why was this classified here?

The skill compiles a structured query and executes `tokentree query`. It must not invent numbers from the conversation.

```bash
tokentree query \
  --project current \
  --work-item "fixing the collision bug" \
  --include-descendants \
  --format json
```

If several items match, ask the user to choose.

Resolution order for “this game” / “the bug”: current project (cwd / config) → aliases in `.tokentree.yml` → fuzzy title match → disambiguate.

### 8.9 Corrections

Dashboard and natural language:

```text
The regression tests were part of the collision bug.
Merge “collision investigation” with “fix collision bug.”
Move the last three turns to “Add multiplayer.”
This entire session belongs to Space Game, not Portfolio Website.
```

Operations:

- Rename work item or project
- Change type or status (complete / reopen / abandon)
- Merge work items
- Split a work item at selected turns
- Move a work item to another parent or project
- Move usage spans between work items
- Move sessions between projects
- Attach / detach sessions
- Mark classification verified
- Ignore / leave uncategorized

Corrections write new attribution rows with `supersedes_group_id`. Usage events stay put.

**Time budget.** The three fastest paths must complete in under a minute without leaving the current view:

1. Rename the current work item.
2. Move the last N turns onto another item.
3. Attach this session to an existing item.

### 8.10 Folder config

```yaml
# .tokentree.yml  (.taskuse.yml remains a legacy alias)
project: space-game
title: Space Game
aliases:
  - snake
  - the game
  - this game
roots:
  - .
  - ../space-game-assets
privacy:
  analyze_prompts_locally: true
  store_prompt_text: false
  store_file_paths: true
classification:
  aggressiveness: balanced
ui:
  open_dashboard: auto
  notifications: project-change
```

`tokentree init` writes this file from the resolved project.

Project configuration is untrusted repository input. It may define project identity, aliases, approved roots, classification hints, and stricter privacy restrictions. It may not execute commands, configure hooks, weaken global privacy, enable network egress, change global pricing, or read files outside approved roots.

---

## 9. Project detection

Project resolution order. The first reliable match wins:

1. Explicit user or CLI override
2. Existing verified mapping
3. `.tokentree.yml` walking upward
4. Stable Git repository identity
5. Project or package manifest
6. IDE workspace metadata when available
7. Canonical working-directory root, excluding noisy names such as `src`, `app`, `backend`, `frontend`, `$HOME`, and temporary directories
8. Repeated file-path cluster
9. Session history
10. Prompt-derived friendly title (from derived label, not stored full text)
11. Named inbox (`personal.*`)
12. One-time clarification (§8.4)

Projects are first-class records.

### 9.1 Git present

Capture root, branch, commit, and worktree. Capture a remote identity only when remote collection is enabled. Local Git is enough; GitHub is not required.

Issue IDs in branch names (`fix/collision-123`) are signals without OAuth.

### 9.2 Git absent

Inspect manifests including `package.json`, `pyproject.toml`, `setup.py`, `Cargo.toml`, `go.mod`, `pom.xml`, `build.gradle`, `composer.json`, `Gemfile`, `*.sln`, and `*.csproj`. If none exists, use a canonical directory fingerprint, repeated file clusters, session history, and a friendly title inferred locally from meaningful work.

### 9.3 Multiple roots and repositories

One logical project may contain several roots or repositories, for example a client, server, and infrastructure repository. Work items belong to the logical project. Cross-project relationships are represented later without assigning one work item to several projects. Multi-root totals must not double-count shared events.

### 9.4 Moves and renames

Reconnect via verified aliases, Git identity, manifest identity, file fingerprints, historical roots, and recent sessions, then user confirmation. Suggest a merge when two project records appear to represent the same tree after a move.

---

## 10. Work-item classification

Every new span gets `project_id`, `work_item_id`, `attribution_method`, `confidence_score`, `explanation`, `classifier_version`.

**Hooks do not run the classifier.** `UserPromptSubmit` / `Stop` enqueue `{fingerprint, derived_label_candidate, cwd, session_id, snapshot}` and return within the 100 ms budget. A worker classifies. Pending turns remain Uncategorized until done.

### 10.1 Turn outcomes

- `CONTINUE` — same work item
- `CHILD` — create or resume a child
- `SWITCH` — different work item in the same or another project
- `UNCERTAIN` — Needs review / Uncategorized

### 10.2 Signals

Strong: explicit issue or ticket id, branch name, Claude TaskCreate id, verified user mapping, clear topic-switch language, manual correction.

Supporting: lexical or semantic prompt similarity (only if prompt analysis is enabled), derived-label overlap (always), same files or modules, same tests, same cwd, same session title, time proximity, agent task subject, commit/PR metadata.

Negative: different issue id, branch change, “new task” language, unrelated modules, long inactivity, prior user correction.

### 10.3 Initial scorer

Values are starting hypotheses and must be tuned on labeled fixtures.

```text
Same verified mapping                      +100
Same ticket / issue ID                     +100
Same branch                                 +50
Same internal task id                       +40
Same files or modules                       +25
High local prompt similarity                 +25
Follow-up wording                           +20
Activity within 30 minutes                  +10

Different issue ID                         -100
Branch changed                              -60
Explicit new-task wording                   -50
Unrelated modules                           -25
Long inactivity                             -10
```

Cutovers:

```text
≥ 60 and narrower-scope language or file subset     CHILD
≥ 60 otherwise                                      CONTINUE
30–59                                               CONTINUE provisionally, low confidence
< 30 + new-task / new-ticket / new-branch           SWITCH
< 30 otherwise                                      UNCERTAIN
```

Fixture requirement: “fix that and add a regression test” under an open collision-fix parent must create a **child**, not a sibling.

Do not ship the aggressive default until SWITCH and CHILD precision are measured on the boundary corpus.

### 10.4 Classifier stack

1. Deterministic identifiers
2. Git and directory context
3. Rules and local lexical similarity
4. File / module overlap
5. Optional local embeddings (downloaded separately when enabled)
6. Optional external LLM classification (off by default; its usage recorded as `overhead.classification`)

### 10.5 Prompt analysis (single policy)

- Default: **on, in memory only**.
- Full prompts are not written to disk, logs, or the ledger.
- Derived labels: 3–8 words, verb + object, user’s wording, keep ticket ids, never `misc` / `session` / `work`.
- Redact `sk-`, `ghp_`, `github_pat_`, `Bearer`, `AKIA`, and similar before persist.
- Never leave the machine unless the user enables an external classifier.
- First-run and README state this in one paragraph.
- Settings can disable it; UI warns that topic-switch quality will decrease.
- `doctor` **fails** if prompt text is found in logs or the DB.

### 10.6 Confidence UX

- High: classify silently
- Medium: classify provisionally; visible in dashboard
- Low: Uncategorized / Needs review
- User-set aggressiveness: conservative / balanced (default) / aggressive

Never present a low-confidence boundary as certain.

---

## 11. Measurement, telemetry, and attribution

### 11.1 Measurement principles

- Hooks identify lifecycle boundaries; they are not assumed to contain complete token totals.
- Official request-level telemetry is preferred over internal transcript reconstruction.
- The same request observed from several sources is deduplicated, not added several times.
- Missing or contradictory measurement remains unavailable and reduces completeness.
- Original observations and provenance are preserved.
- Reports never read JSONL live. The ledger is the only number.
- `tokentree reconcile` compares hook-event counts to parsed-event counts per session and reports drift.

### 11.2 Truth ladder

For each provider request, take the highest available rung. A lower rung may fill missing fields on the same identity. It must not create a second usage event.

```text
1. Official request-level telemetry
   Claude: OpenTelemetry API-request events and token/cost counters
   Codex:  app-server thread/tokenUsage/updated and stable request/turn events
2. Stable provider-reported usage fields on the request
3. Request-level transcript or rollout records
4. Cumulative session snapshot delta (snapshot_after − snapshot_before)
5. Explicit CLI counts for that same event
6. Unavailable — keep the row, tokens = null, never store measured zero
```

Explicit user-supplied counts override reconstructed counts for the same event. They do not override a higher-priority official request id that already has totals.

A high-fidelity local mode may run or connect to a loopback OTLP receiver. Enabling it requires explicit consent before changing telemetry configuration.

### 11.3 Claude Code measurement

Preferred source order follows the truth ladder.

Claude request telemetry can provide input, output, cache-read, cache-creation, model, query source, request ID, and estimated cost. Claude hooks provide `session_id`, `prompt_id`, transcript path, cwd, user prompts (in memory), tasks, subagents, turn stop, and session end.

Hook set (enqueue and return, p95 under 100 ms excluding provider):

`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `TaskCreated`, `TaskCompleted`, `SubagentStart`, `SubagentStop`, `Stop`, `StopFailure`, `CwdChanged`, selective `PostToolUse` for files-touched only.

TokenTree correlates them without storing full prompt content.

Subagent lifecycle events identify parentage and duration. They must not be treated as complete token totals when they expose only the final request. Request-level token/cost events are aggregated by subagent query source and agent identifiers.

### 11.4 Codex measurement

Preferred source order follows the truth ladder.

Codex hooks provide session, turn, cwd, model, prompt (in memory), tool, subagent, stop, interrupt, and session-end boundaries. Rollout JSONL provides historical import and fallback. A Codex plugin may bundle hooks, but automatic tracking is not active until the user reviews and trusts them.

### 11.5 Other hosts

| Host | Capture |
|---|---|
| Claude Code | Hooks + official OTel + `~/.claude/projects/**/*.jsonl` fallback |
| Codex | Documented plugin/skill + app-server events + `~/.codex/sessions/**` fallback |
| Grok | Explicit start/stop; logs when a stable path exists |
| Gemini CLI / OpenCode / others | Adapter + known local logs |

Every adapter declares boundary, measurement, historical, subagent, tool-cost, and automatic-tracking capabilities. Unsupported hosts can use explicit bookkeeping and explicit token counts. A work record with unknown usage remains useful; its usage and cost fields are null.

`doctor` prints resolved paths per OS, including Windows.

### 11.6 Correlation and source precedence

Canonical correlation fields include:

- adapter and source type
- source event/request ID
- source process ID and sequence when available
- session ID and root session ID
- prompt ID
- turn ID
- request ID
- subagent ID and parent agent ID
- source timestamp
- observed timestamp
- ingestion timestamp
- source file and offset
- event hash

Source precedence is the truth ladder. A lower-priority observation can fill missing fields but cannot duplicate a higher-priority request. Conflicts create a measurement anomaly.

### 11.7 Ordering

Do not assume timestamps or one global sequence are monotonic across resume, fork, compaction, or another process. Order by explicit parent/turn/request relationships first, then source offset and timestamp, and use ingestion time only as a fallback.

### 11.8 Turn deltas and anomalies

When only cumulative values exist:

```text
turn usage = snapshot after − snapshot before
```

Handle reset, resume, fork, compaction, duplicates, out-of-order events, truncation, subagents, interruption, and missing stop events.

For a negative delta:

1. Preserve both snapshots.
2. Attempt to prove a reset or reconstruct request-level usage.
3. If reconstruction fails, create a `negative_delta` anomaly.
4. Mark the interval unavailable.
5. Reduce completeness.
6. Never convert unknown usage into measured zero.

Compaction that drops early usage objects reduces completeness. Do not invent tokens from a summary.

### 11.9 Deduplication

Use request IDs or stable provider/message IDs when available. Otherwise use a documented composite identity from adapter, session, turn, model, token fields, timestamp, source offset, and payload hash. Importing the same source repeatedly is idempotent.

### 11.10 Subagents

Subagent usage rolls up to its parent turn and active user work item while retaining identity.

Adapters **must declare** `subagent_tokens_already_in_parent: true | false`. If the parent turn already includes subagent tokens, do not add `SubagentStop` usage on top.

Reports expose inclusive, main-agent direct, subagent-only, and by-subagent-type views.

`reconcile` must detect injected duplicate subagent tokens.

### 11.11 Attribution policy

Default policy: `causal-request`.

All usage generated by a request is attributed to the work item active when that request began, including context tokens carried from earlier turns. This matches marginal request activity, remains reproducible, and avoids content-level token inspection.

Usage not fairly attributable to one work item is assigned to the overhead categories in §7.6.

Reports must name the active policy. Optional future policies may allocate shared context proportionally.

### 11.12 Weighted attribution

The default span assigns 10,000 basis points to one work item. The schema permits several active attribution rows whose weights sum to 10,000. Fractional allocation is advanced functionality and never silently inferred.

### 11.13 Capability and coverage manifest

Each adapter reports states such as `supported`, `partial`, `unavailable`, `blocked_by_policy`, `not_configured`, and `unsupported_version` for:

- prompt and turn boundaries
- request tokens
- cache tokens
- subagents
- historical import
- tool activity
- external tool cost
- automatic tracking
- provider cost

Each project report shows measurement coverage for model tokens, cache tokens, subagent tokens, server tools, MCP/external services, runtime charges, and TokenTree classifier overhead.

Managed-policy or trust-blocked hooks produce a clear degraded-capability report. They are never shown as automatic tracking.

### 11.14 Attribution ledger

Usage events are append-only. Attribution is versioned separately. Corrections create a replacement attribution group that supersedes earlier active rows while preserving history.

---

## 12. Cost and pricing

### 12.1 Categories

When available: uncached input, cached input, cache writes by tier, output, reasoning, separately priced tools, service tier, context tier, region, runtime, and other provider charges.

### 12.2 Formula

```text
estimated cost =
  uncached input × input rate
+ cached input × cached-input rate
+ cache writes × cache-write rate
+ output × output rate
+ reasoning/tool/runtime charges
+ applicable multipliers
```

Adapters normalize semantics before multiplication. Cache reads are never priced as full input unless the provider’s documented billing explicitly requires it.

### 12.3 Cost types

| Type | Meaning |
|---|---|
| `reconciled_billed_cost` | Matched to authoritative billing data |
| `provider_reported_cost` | Provider or gateway supplied a cost value |
| `configured_rate_estimate` | Calculated with user or contracted rates |
| `api_equivalent_estimate` | Public list prices; default |
| `allocated_subscription_cost` | Optional seat-cost accounting allocation, not a provider charge |
| `unavailable` | Measurement or pricing is insufficient |

### 12.4 Precision

- Tokens are integers.
- Money is stored as integer currency micros or exact decimal values, never binary floating point.
- Rates are decimal strings or rationals per documented unit.
- Attribution weights are integer basis points.
- Display rounding never changes stored values.
- Currency is USD in first public releases; the field exists for later.
- Timestamps are stored UTC; display uses configured TZ (default local).

### 12.5 Pricing provenance

Every calculation stores provider, resolved model snapshot, original model alias, price source, source hash/signature, effective date, retrieval date, currency, context tier, service tier, region, multiplier, attribution policy, and calculation version.

Users can reprice history at historical, current, or configured rates without changing token measurements.

### 12.6 Phase 1 price file (not vapor)

Ship `prices.json` with `updated` and `sha256`.

`tokentree doctor` warns if `updated` is older than 90 days. Stale prices still compute, with a banner.

`tokentree pricing refresh` downloads a GitHub release asset and verifies the hash. No anonymous CDN. Network refresh is opt-in.

Resolution order for a model:

1. User or contracted custom rates
2. Versioned snapshot matching the event time (historical reprice)
3. Current shipped snapshot
4. `unavailable` cost type if no rate exists

Custom rates:

```toml
[pricing.claude-sonnet-4-6]
input = "3.00"
cached_input = "0.30"
cache_write_5m = "3.75"
output = "15.00"
```

### 12.7 Completeness formula

Completeness is computed after deduplication.

```text
measured_requests     = requests with non-null token totals and no unresolved anomaly
unavailable_requests  = requests with null tokens, or intervals marked unavailable
anomalous_requests    = unresolved measurement anomalies on this rollup

token_completeness %  = 100 × measured_requests
                        / (measured_requests + unavailable_requests + anomalous_requests)
```

If the denominator is 0, completeness is null (no requests yet), not 100%.

A cost view must also show:

- pricing coverage (share of measured tokens that have a matching rate)
- external-tool coverage
- cache-category coverage
- overhead share
- unavailable turn count and anomaly count

**Rule.** If any child is unavailable or anomalous, the parent dollar figure **must** show completeness and the unavailable count. No bare `$3.84` on an incomplete tree.

A cost view must not call a total complete when known categories are unavailable.

### 12.8 Disclaimer

Every dollar surface includes the canonical disclaimer in §1.

---

## 13. Information architecture

Navigation:

- Overview
- Projects
- Work items
- Sessions
- Agents and models
- Needs review
- Pricing
- Settings

**Overview:** totals for day / week / month / custom range (timezone = machine TZ, stored UTC); project breakdown; most expensive work items; uncategorized; pending classify; classification health; ingest health; models and agents; capture-mode table.

**Project:** summary, aliases, roots, detection source, tree with completeness, timeline, sessions, models, confidence distribution.

**Work item:** type, status, parent/children; direct, descendant, and inclusive totals; timeline; token categories; models and agents; measured and unavailable turn counts; anomaly count; measurement-source mix; cost-category coverage; attribution policy; overhead; classification confidence; completeness; turn list; attribution explanation; related files and Git metadata; correction controls.

**Needs review:** low-confidence spans, uncategorized sessions, possible duplicate projects or tasks, ambiguous switches, unknown prices, unparsed events, anomalies. Bulk merge / move / verify / ignore / reclassify.

**Settings:** privacy toggles, notification mode, classifier aggressiveness, custom rates, import, purge, export, capture-mode table.

---

## 14. Dashboard and static HTML

### 14.1 Interactive dashboard

`tokentree dashboard` and `/tokentree` when a display exists.

- Bind `127.0.0.1` only
- Random session token in the URL
- Token dies when the process exits
- Never serve transcripts
- Confirm by typed phrase before non-loopback binding
- CSP; no remote scripts or fonts
- Escape all user-controlled HTML
- Live updates; full corrections

### 14.2 Static HTML

Self-contained HTML, no remote scripts or fonts, expandable tree, click-through, search, disclaimer, escaped strings, embedded data.

```bash
tokentree report --html
```

Default directory `~/.tokentree/reports/`. Static reports redact absolute paths by default and warn before writing into a repository.

`--text` never opens a server.

---

## 15. CLI

```bash
tokentree report [--period] [--project] [--tree] [--json] [--html] [--text] [--direct]
tokentree dashboard
tokentree projects
tokentree project <id-or-name>
tokentree tasks
tokentree task <id-or-name>
tokentree show <id-or-name>
tokentree query <question-or-flags>
tokentree compare <a> <b>
tokentree compare agents
tokentree start ...
tokentree stop ...
tokentree note --text
tokentree attach --session <id> --task <id>
tokentree detach --session <id>
tokentree status
tokentree import claude|codex|prototype
tokentree classify
tokentree review
tokentree rename | merge | move | split | verify | reclassify
tokentree export --format json|csv
tokentree init
tokentree doctor
tokentree reconcile
tokentree config
tokentree pricing
tokentree pricing refresh
tokentree sources
tokentree watch
tokentree snapshot
tokentree migrate prototype --preview|--apply
tokentree verify
tokentree backup
tokentree restore
tokentree purge
```

Formats: human tables, JSON, CSV.

Filters: time range, project, work item, model, agent, confidence, direct vs recursive, verified vs inferred.

Exit codes: `0` ok, `1` usage / nothing to do, `2` IO or parse with data left consistent.

`--period today|week` uses configured TZ (default local).

---

## 16. Data model

SQLite with WAL mode. Measured data and inferred organization are stored separately. No global `active.json`.

### 16.1 `projects`

`id`, `key`, `display_name`, `description`, `identity_hash`, `detection_method`, `confidence`, `verified_at`, `created_at`, `updated_at`

### 16.2 `project_aliases`

`project_id`, `alias_type`, `alias_value`, `verified`

### 16.3 `project_roots`

`project_id`, `canonical_path`, `root_type`, `fingerprint`, `active`, `first_seen_at`, `last_seen_at`

### 16.4 `work_items`

`id`, `project_id`, `parent_id`, `type`, `title`, `description`, `external_id`, `status`, `confidence`, `classifier_version`, `created_at`, `completed_at`, `verified_at`

Recursive `parent_id`. Single `project_id`. The UI may collapse depth; the database does not impose a three-level limit.

### 16.5 `sessions`

`id`, `adapter`, `provider_session_id`, `root_session_id`, `project_id`, `source_path`, `cwd`, `git_branch`, `git_commit`, `started_at`, `ended_at`

### 16.6 `turns`

`id`, `session_id`, `sequence_number`, `started_at`, `ended_at`, `prompt_fingerprint`, `derived_label`, `prompt_storage_mode`

Full prompt text is absent by default. Explicit user notes are stored separately.

### 16.7 `usage_events`

`id`, `adapter`, `source_kind`, `source_event_id`, `source_process_id`, `source_sequence`, `session_id`, `prompt_id`, `turn_id`, `request_id`, `agent_id`, `parent_agent_id`, `parent_event_id`, `source_timestamp`, `observed_at`, `ingested_at`, `model`, `service_tier`, `region`, `input_tokens`, `cached_input_tokens`, `cache_write_tokens`, `output_tokens`, `reasoning_tokens`, `provider_reported_cost_micros`, `source_path`, `source_offset`, `event_hash`, `adapter_version`, `parser_version`

Token fields are nullable. Usage events are append-only and idempotent. Documented repair migrations may supersede an event without erasing provenance.

### 16.8 `usage_spans`

`id`, `session_id`, `start_turn_id`, `end_turn_id`, `measurement_status`, `measured_usage_json`, `completeness`

### 16.9 `attribution_groups` and `attributions`

Attribution group: `id`, `usage_span_id`, `policy`, `supersedes_group_id`, `active`, `created_at`.

Attribution row: `group_id`, `project_id`, `work_item_id`, `role`, `weight_basis_points`, `method`, `confidence`, `explanation`, `classifier_version`, `verified_by_user`.

Active weights for a span sum to 10,000. Corrections create a replacement group and preserve decision history.

### 16.10 `classification_events`

`id`, `turn_id`, `outcome`, `signals_json`, `score`, `classifier_version`, `created_at`

Supports “why was this classified here?”, offline classifier evaluation, and safe reclassification with a new model.

### 16.11 `pricing_versions`

`id`, `provider`, `model_pattern`, `effective_from`, `effective_to`, `rates_json`, `source`, `retrieved_at`, `signature_or_hash`

### 16.12 `cost_calculations`

`usage_event_id`, `pricing_version_id`, `amount_micros`, `currency`, `cost_type`, `attribution_policy`, `coverage_json`, `calculated_at`

### 16.13 `ingestion_checkpoints`

`adapter`, `source_path`, `file_size`, `modified_at`, `last_offset`, `last_event_hash`

### 16.14 `measurement_anomalies`

`id`, `session_id`, `turn_id`, `type`, `source_values_json`, `resolution`, `created_at`, `resolved_at`

### 16.15 `adapter_capabilities`

`adapter`, `adapter_version`, `host_version`, `capability`, `state`, `detail`, `checked_at`

### 16.16 `coverage_manifests`

`adapter`, `version`, `model_tokens`, `cache_tokens`, `reasoning_tokens`, `subagent_tokens`, `server_tools`, `external_costs`, `runtime_costs`, `classifier_overhead`

### 16.17 `schema_metadata`

`schema_version`, `application_version`, `migration_state`, `created_at`, `updated_at`

### 16.18 `notes`

`id`, `project_id`, `work_item_id`, `text`, `created_by`, `created_at`

Notes are explicit, max 240 characters, purgeable, and never copied automatically from prompts.

### 16.19 Later

`related_work_items` for cross-project links. Not required for public beta.

Default data directory: `~/.tokentree/`; override with `TOKENTREE_HOME`.

Permissions where supported: `0700` for the data directory, `0600` for the database and sensitive exports.

Migration from `~/.task-usage/ledger.json` and the prototype price file is a first-class, previewable, non-destructive importer.

```bash
tokentree migrate prototype --preview
tokentree migrate prototype --apply
```

Migration creates a backup, imports idempotently, records provenance, and never deletes the source JSON automatically.

---

## 17. Architecture

```text
Host hooks / official request telemetry / explicit CLI / historical files
        → crash-safe local spool and correlation queue
        → adapter and source resolver
        → normalized canonical usage events
        → SQLite ledger
        → project resolver (async)
        → work-item classifier (async worker)
        → versioned cost engine
        → CLI / skills / dashboard / static HTML / exports
```

### 17.1 Extension interfaces

```ts
interface AgentAdapter {
  id: string;
  detect(): Promise<DetectionResult>;
  discoverSessions(options?: DiscoverOptions): AsyncIterable<SessionRef>;
  parseSession(ref: SessionRef): AsyncIterable<UsageEvent>;
  watch?(callback: (event: UsageEvent) => void): Promise<Watcher>;
  capabilities(): AdapterCapabilities; // includes subagent_tokens_already_in_parent
}

interface WorkClassifier {
  classify(input: ClassificationInput): Promise<ClassificationResult>;
  explain(resultId: string): Promise<ClassificationExplanation>;
}

interface PricingSource {
  resolve(model: string, at: Date, context?: PricingContext): Promise<PricingRecord | null>;
}
```

Also document interfaces for project resolvers and report exporters.

### 17.2 Stack

TokenTree uses a **Rust-first local engine with a TypeScript UI and extension layer**.

Rust is mandatory for:

- the published `tokentree` CLI and hook-enqueue executable
- SQLite WAL ownership, migrations, writer coordination, recovery, backup, and integrity checks
- streaming host-log parsers, canonical event normalization, truth-ladder correlation, and deduplication
- the embedded loopback OTLP receiver
- exact integer-micro cost calculations, completeness, recursive aggregation, and report/query execution
- filesystem permissions, atomic writes, bounded-memory imports, and cross-platform path resolution

TypeScript and Node.js LTS are used for:

- Claude/Codex plugin manifests, skills, and host-facing glue where the host requires JavaScript
- the local dashboard and static HTML UI
- a typed community-adapter SDK that communicates with the Rust engine through versioned JSON/JSONL or a loopback protocol
- web-focused tests and UI tooling

The Rust engine uses `rusqlite` with bundled SQLite, WAL mode, busy timeout, short transactions, and read-only report connections. The release pipeline produces signed native binaries for supported platforms. The scoped npm package `@tokentreehq/cli` is a thin installer/launcher for those binaries; it is not a second measurement implementation.

The SQLite schema, normalized event JSON schema, adapter capability schema, and CLI JSON output are language-neutral public contracts. TypeScript must not reimplement authoritative measurement, deduplication, cost, or aggregation logic once the corresponding Rust path ships.

Testing uses Rust unit/integration tests, Clippy, rustfmt, Vitest for TypeScript surfaces, golden parser fixtures, schema compatibility tests, and the labeled boundary corpus.

The Python prototype is a migration source and behavioral reference only.

### 17.3 Concurrency and write coordination

Support several agent sessions, overlapping hooks, import during live tracking, and process termination during ingest.

- Hooks append to a crash-safe local spool.
- A single writer queue commits normalized events.
- SQLite uses WAL, busy timeout, short transactions, and read-only report connections.
- Schema migration uses an exclusive migration lock and pre-migration backup.
- File-offset checkpoints and canonical request IDs make replay idempotent.
- Database integrity checks run after recovery and migration.
- Never use one global active-task file.

### 17.4 Repository layout

```text
tokentree/
├── apps/
│   ├── rust-cli/
│   ├── cli/            # temporary TypeScript migration/reference harness; not the published engine
│   └── dashboard/
├── crates/
│   ├── tokentree-core/
│   ├── tokentree-ledger/
│   ├── tokentree-claude/
│   ├── tokentree-otel/       # added only with working receiver tests
│   └── tokentree-reports/
├── packages/
│   ├── core/           # TypeScript contract/reference tests during migration
│   ├── database/       # schema compatibility harness; Rust owns production writes
│   ├── classifier/
│   ├── pricing/
│   ├── reports/
│   └── adapters/
│       ├── claude/
│       └── community/
├── plugins/
│   ├── claude-code/
│   └── codex/          # only after compatibility tests
├── fixtures/
│   ├── parsers/
│   └── boundaries/
├── schemas/
├── docs/
└── examples/
```

Only adapters that pass compatibility tests are presented as supported. Planned adapters are documented in the roadmap rather than represented as complete packages.

---

## 18. Privacy and security

### 18.1 Defaults

- Local storage only
- No account
- No telemetry unless explicitly enabled
- Local transient prompt analysis enabled for classification
- No full prompt, completion, reasoning, source-code, diff, tool-argument, or tool-output storage by default
- Read-only transcript access
- Dashboard bound to loopback only
- External classifier and network price refresh disabled by default

### 18.2 Persisted metadata

The default build may store token counts, model, timestamps, session identifiers, working directory, permitted Git metadata, prompt fingerprints, extracted issue IDs, redacted derived labels, classifier signals, and optional explicit notes.

### 18.3 User controls

Users can independently disable prompt analysis, file paths, Git remotes, branch data, commit data, and file-cluster analysis. Disabling signals must explain the expected classification-quality effect.

Controls include export all, purge all, purge one project, purge notes, purge derived labels, rebuild classifications, remove source references, backup, and restore.

### 18.4 External classification

If enabled, explain exactly what content is sent, support redaction, identify the provider, permit per-project disablement, and record classifier tokens and cost separately as `overhead.classification`.

### 18.5 Dashboard and process security

- Bind to `127.0.0.1` by default
- Use a random session token that expires when the process exits
- Never serve source transcript files
- Confirm before non-loopback binding
- Escape all user-controlled HTML
- Never interpolate project or work-item titles into a shell command
- Use secure temporary files and atomic writes

### 18.6 Threat model and filesystem security

Protected against by design:

- Accidental network upload by TokenTree
- Default full-prompt retention
- Dashboard exposure beyond localhost
- HTML and shell injection
- Malicious project configuration weakening global privacy
- Sensitive fields appearing in default exports
- Unsigned or tampered release artifacts
- Prompt text leaking into logs or the DB in default mode

Not protected against by default:

- Another process with the same operating-system user privileges
- A compromised machine or administrator/root access
- Provider-side data retention
- Original Claude/Codex transcript storage
- External classification explicitly enabled by the user

Project configuration is untrusted data as specified in §8.10.

### 18.7 Supply-chain and lifecycle security

- Signed or checksummed releases
- Dependency scanning
- Transactional migrations
- Secure temporary files and atomic writes
- Security policy and responsible disclosure process
- Documented retention and deletion behavior
- Plugin uninstall never deletes user data unless explicitly requested
- Plugin and adapter updates preserve compatibility metadata and require migration checks

---

## 19. Performance

- Hook p95 under 100 ms excluding provider work
- Hooks enqueue compact events and never parse complete transcripts or run the classifier inline
- Official telemetry ingestion is incremental and non-blocking
- Appended JSONL and rollout files are parsed incrementally
- Terminal `/tokentree` summary under 500 ms on a normal local database
- Dashboard initial load under 2 seconds for 100,000 usage events on a typical developer laptop
- Historical import shows progress, supports cancellation, and resumes safely
- Memory remains bounded through streaming ingestion
- Correlation and deduplication do not add the same request twice
- Classifier backlog is visible and never on the hook clock
- Every supported adapter publishes a measurement-coverage manifest

---

## 20. Reliability

- Never modify source agent logs
- Idempotent imported events
- Unknown event types skipped and counted
- Parser errors name the file and adapter version
- Costs recalculable without reparse
- Classifications recalculable without touching measured events
- Migrations transactional
- Interrupted import recoverable
- Live tracking and import may run concurrently
- `doctor` explains unknown log formats, stale prices, blocked hooks, unavailable telemetry, capability degradation, path resolution per OS, capture mode, and prompt leakage
- `reconcile` reports hook vs parse drift and duplicate subagent tokens
- Canonical request provenance prevents OTel, transcript, and delta observations from double-counting
- Plugin/adapter version compatibility is recorded for every import

Every project and work-item view shows: completeness percent, classification confidence, unknown-event count, missing-model or missing-price warnings, unavailable tokens, anomalies, last successful ingest, adapter/parser versions, attribution policy, overhead.

---

## 21. Testing strategy

### 21.1 Unit tests

Project resolution, manifests, aliases, skip-list, recursive rollups, CHILD/SWITCH scoring, token normalization, price matching, cache calculations, confidence behavior, HTML escaping, CLI parsing, migrations, unavailable render, secret redaction, completeness formula, subagent double-count flag, integer-micro money.

### 21.2 Human-labeled classification corpus

Maintain labeled examples for same-task follow-ups, task switches in the same files, cross-session continuation, no-Git work, issue-ID changes, non-code projects, subagent-heavy runs, and cross-agent continuation.

Track project precision/recall, task-switch precision/recall, child-detection precision/recall, cluster agreement, confidence calibration, high-confidence error rate, and Uncategorized rate.

Initial quality gates:

```text
Project assignment precision      ≥ 95%
Task-switch precision             ≥ 85%
Task-switch recall                ≥ 75%
CHILD precision                   measured before shipping aggressive default
High-confidence error rate        < 5%
```

Required fixture: “fix that and add a regression test” under an open collision-fix parent creates a child.

### 21.3 Sanitized compatibility fixtures

Maintain fixtures for new, resumed, forked, compacted, duplicated, out-of-order, truncated, and interrupted sessions; missing terminal events; subagents; model changes; unknown schema fields; unavailable usage; Windows paths.

### 21.4 Eval harness as a product artifact

Phase 0 publishes:

- Sanitized fixture format
- Boundary-label schema (project / work item / child / switch)
- Scoring script that prints SWITCH and CHILD precision/recall and % of high-confidence rows later moved
- Parser compatibility matrix template

“The tree is good” is a harness result, not a vibe.

### 21.5 Normative behavior tests

1. Automatic prompt tracking creates a project and work item without manual `start`.
2. A follow-up continues the current work item.
3. Explicit topic-switch language creates or resumes a sibling.
4. “Fix that and add a regression test” creates a CHILD when parent evidence is strong.
5. Explicit override beats verified mapping; verified mapping beats config; config beats Git; Git beats manifest; manifest beats directory inference.
6. Project detection works without Git.
7. Multiple roots resolve to one project without doubled totals.
8. Concurrent sessions do not overwrite one another.
9. Imported events are idempotent.
10. Duplicate events do not double-count.
11. Recursive totals equal direct usage plus descendants.
12. Reclassification changes attribution, not measured usage.
13. Negative deltas become anomalies rather than silent zeroes.
14. Missing usage displays as unavailable, never measured $0.00.
15. Cached tokens use the appropriate rate.
16. Cost output contains the canonical disclaimer and completeness when incomplete.
17. Ambiguous natural-language matches ask for clarification.
18. Historical import resumes after interruption.
19. HTML and shell inputs are safely escaped.
20. Plugin uninstall preserves user data.
21. Prototype migration is previewable, idempotent, and non-destructive.
22. Skill-only installation is not reported as automatic tracking.
23. The same Claude request observed through OTel and transcript is counted once.
24. Claude subagent totals come from request telemetry, not final-request counters.
25. Codex hook turn IDs correlate with app-server token events.
26. Causal-request attribution assigns context tokens to the active work item.
27. Project/session/classifier overhead remains visible and separate.
28. Weighted attribution groups always sum to 10,000 basis points.
29. Managed-policy hook blocking appears in capability diagnostics.
30. Malicious project config cannot execute commands or weaken privacy.
31. Money calculations reproduce exact integer-micro totals.
32. Event ordering remains valid across process resume and repeated source sequences.
33. Prompt text is absent from DB and logs in default mode.
34. Derived labels are redacted.
35. `--text` never opens a server.
36. `reconcile` detects injected duplicate subagent tokens.
37. Pending classify does not block the agent turn.
38. Completeness uses the formula in §12.7.
39. `doctor` fails if prompt text is persisted.
40. `Stop` does not automatically complete a work item.

### 21.6 Privacy tests

Verify no full prompt, response, reasoning, code, tool output, or external network request occurs in the default mode. Verify purge and export behavior field by field. Verify derived-label redaction.

### 21.7 Cross-platform tests

macOS, Linux, Windows, Unicode paths, long paths, symlinks, worktrees, concurrent writers, and interrupted migrations.

`doctor` path matrix (minimum):

| OS | Claude transcripts | Codex sessions | TokenTree home |
|---|---|---|---|
| macOS | `~/.claude/projects` | `~/.codex/sessions` | `~/.tokentree` |
| Linux | `~/.claude/projects` | `~/.codex/sessions` | `~/.tokentree` |
| Windows | `%USERPROFILE%\.claude\projects` | `%USERPROFILE%\.codex\sessions` | `%USERPROFILE%\.tokentree` |

Also resolve roaming vs local AppData when the host documents it. Print the resolved path, not a guess.

---

## 22. Open-source distribution

```text
/plugin install tokentree@tokentreehq
npx @tokentreehq/cli@latest
npm install -g @tokentreehq/cli
npx skills add tokentreehq/tokentree
```

`@tokentreehq/cli` installs and launches a checksummed, signed Rust binary for the current supported platform. It must fail clearly on an unsupported platform and must never silently fall back to a separate JavaScript measurement engine. The Claude plugin bundles the same known-good Rust binary version for hook enqueue and local commands.

Later: Homebrew, matching the scoped identity and distributing the same signed Rust release artifacts.

Provide: adapter, classifier, pricing-source, resolver, and exporter guides; sanitized fixture format; schema compatibility tests; eval harness; good-first issues; security policy; responsible disclosure; release and deprecation policy; public roadmap; and code of conduct.

Contribution bar for a new adapter: discover sessions, emit normalized events, declare capabilities including `subagent_tokens_already_in_parent`, add sanitized fixtures, and pass normalization, cost, idempotency, privacy, and compatibility tests.

Codex supports skills, native `.codex-plugin/plugin.json` packages, plugin-bundled hooks, and repository or personal marketplaces. Public self-serve directory publishing is treated as unavailable until officially released. A skill-only installation must not be described as automatic tracking unless hooks or an adapter watcher are active.

---

## 23. Success metrics

No product telemetry in the default build. Maintainers measure on dogfood, pilots, and optional anonymous usage (off).

Activation: installs that ingest at least one session; installs that produce a classified project; time to first useful report.

Quality: project and task-switch precision/recall on labeled fixtures; confidence calibration; share of usage outside Uncategorized; classifications accepted without correction; merge/split/move rate; high-confidence rows later corrected; measurement and pricing coverage.

Engagement: weekly `/tokentree`; natural-language queries per active user; dashboard return; work items viewed.

Trust: complete token data; pricing coverage; parser error rate by agent version; historical-import opt-in rate; zero accidental full-prompt persist; zero measured-$0 for unavailable rows.

Public-beta hypotheses to revise after pilots. “Kept without correction” is behavioral evidence, not ground truth; the labeled evaluation corpus remains the primary classifier-quality measure:

- First useful report in under 5 minutes for 80 percent of successful installs
- At least 80 percent of usage assigned to a project
- At least 70 percent of work-item classifications kept without correction
- Under 1 percent ingest failure on supported Claude fixtures
- Hook p95 under 100 ms
- Project assignment precision at least 95% on labeled fixtures
- Task-switch precision at least 85% on labeled fixtures
- No double-counting across official telemetry and transcript fallback
- Every supported adapter publishes a measurement-coverage manifest
- Correction of rename / move-last-N / attach-session in under a minute

---

## 24. Validation

Before calling the tree good enough, collect labeled sessions from real users and answer:

1. What do they consider a project, feature, task, subtask?
2. How often does one session contain several tasks?
3. How often does one task span several sessions?
4. Which boundary signals survive contact with real work?
5. How often is Git or GitHub missing?
6. Will they enable local prompt analysis?
7. Which corrections dominate?
8. Is task cost actionable or merely interesting?
9. Do they prefer automatic-and-imperfect over manual-and-empty?
10. What confidence should force review?

Useful evidence: sanitized fixtures, pilot teams, requests for issue or PR attribution, later demand for an org aggregate.

---

## 25. Development phases

Phases are delivery order for a complete open-source tool. Later phases extend the same architecture. Nothing here is discarded work.

### Phase 0 — Fixtures, schema, and naming

Labeled Claude sessions, parser fixtures, boundary labels, data model, threat model, scorer on paper, identity rules, cost fixtures, eval harness format, **package-name / GitHub org / marketplace slug / Homebrew decision**.

### Phase 1 — Measurement core

Claude discovery + streaming parser, official request-level OTel ingestion, correlation/deduplication engine, normalized events, SQLite, `prices.json` + sha256, capability diagnostics, project/session terminal reports, idempotent checkpoints, `doctor`, `reconcile`, prototype-ledger importer, unavailable + anomalies, completeness formula.

### Phase 2 — Plugin and classification

Claude plugin, enqueue-only hooks, project detector (Git and no-Git), worker classifier with CHILD rules, confidence and explanations, `/tokentree` terminal and natural-language bridge, explicit `start` / `stop` / `attach`, capture-mode UI, first-run prompt-analysis sentence, completeness chips.

### Phase 3 — Dashboard and corrections

Local web dashboard (loopback + URL token), drill-down, merge / split / move / attach / detach, Needs review, historical classify, static HTML export, notification modes, backup/restore.

### Phase 4 — Hardening and public repo

Cross-platform path matrix, parser + boundary matrices, performance, privacy review, docs, install and upgrade tests, public GitHub launch, marketplace listing under the chosen scoped name.

### Phase 5 — More agents

Codex adapter, Grok log adapter when a stable path exists, Gemini CLI, OpenCode, cross-agent project identity, community adapters.

### Phase 6 — Adjacent systems

GitHub / GitLab issues and PRs, Jira, Linear, CI ingestion, cost-per-merge views. Branch-name issue IDs already work without OAuth.

### Phase 7 — Organizations

Self-hosted aggregate, roles, contracted rates, billing reconciliation, budget alerts, optional hosted sync off by default.

---

## 26. Risks

| Risk | Mitigation |
|---|---|
| Bad automatic boundaries | Confidence, Needs review, easy corrections, conservative default, published evaluation corpus |
| Official telemetry unavailable | Transcript/request fallback, capability state, completeness reduction |
| Enterprise policy blocks hooks | Graceful degradation, `doctor`, explicit/manual mode, managed deployment documentation |
| Same request appears in telemetry and logs | Request-ID correlation, truth ladder, idempotent deduplication |
| Log format churn | Versioned adapters, tolerant parse, fixture matrix, `doctor` |
| Incumbents add task grouping | Hierarchy, explainability, corrections, cross-agent identity |
| Users read estimates as invoices | Required cost labels, footer, completeness chips |
| Hook latency | Enqueue only; budget 100 ms |
| Prompt analysis concern | First-run sentence, in-memory only, no logs, disable path, `doctor` leak check |
| Path change duplicates a project | Fingerprints, aliases, Git id, merge suggestions |
| Work spans many repositories | One logical project, many roots |
| One turn serves two tasks | Default dominant attribution; weighted basis-point schema |
| Shared conversational context distorts task cost | Declare causal-request policy and show overhead separately |
| Internal agent tasks pollute user hierarchy | Treat them as evidence; promote only when meaningful |
| Estimate does not match invoice | Versioned prices, custom rates, reconciliation warnings |
| Name collision on npm | Publish `@tokentreehq/cli` only; never publish bare `tokenusage` / `tokenuse` |
| Hook vs parse drift | `reconcile` |
| Subagent double-count | Adapter capability flag + request-level totals |
| Skill-only users think they are tracked | Capture-mode badge |
| Local dashboard abuse | Loopback, URL token, no transcript routes |
| Stale prices | `updated` + sha256 + 90-day banner |

---

## 27. Decisions

Closed unless evidence from fixtures or pilots changes them.

| Item | Resolution |
|---|---|
| Product name | TokenTree |
| CLI binary | `tokentree` (aliases: `tuse`; legacy `tokenusage`, `taskuse`) |
| npm / crates / brew name | `@tokentreehq/cli` exposing `tokentree`. Never publish bare `tokenusage` or `tokenuse`. Never use `github.com/tokentree`. |
| Config / data | `.tokentree.yml`, `~/.tokentree/`, `TOKENTREE_HOME` |
| License | Apache-2.0 preferred |
| Default import | Last 30 days, after confirmation |
| Plugin packaging | Bundle a known CLI version; `npx` for standalone |
| Classifier default | Balanced |
| Embeddings | Downloaded when enabled |
| Prompt analysis | On, in memory only; not stored; `doctor` fails on leakage |
| Derived labels | On demand for display; persist only the redacted 3–8 word label |
| `/tokentree` UI | Print tree; open dashboard if display; `--text` suppresses and never binds a port |
| Multi-repo | One project, many roots |
| Work item × project | Single `project_id` |
| Auto-close items | No; mark stale |
| Fractional attribution | Not inferred in v1; schema allows 10,000 bp groups |
| Attribution policy | `causal-request` default; named on reports |
| Subscription allocation | Off until enabled |
| Currency | USD first; field ready |
| Timezone | Stored UTC; display local |
| Notes | Max 240 characters; never copied from prompts |
| Price updates | Shipped `prices.json` + sha256; opt-in GitHub release refresh |
| Money storage | Integer micros |
| Telemetry vs transcripts | Official request telemetry preferred; transcripts are fallback |
| Internal agent tasks | Execution metadata unless promoted |
| `Stop` vs completion | `Stop` ends a turn; it does not complete a work item |
| Static reports | `~/.tokentree/reports/` |
| Paths / remotes | Paths on; remotes off |
| Completeness | Formula in §12.7; chips required on incomplete money surfaces |

Still open (do not block schema):

- Domain (`tokentree.dev` vs `.app`) and whether to enable the `tuse` short alias by default
- Minimum Claude Code version after the fixture matrix
- Signed-release keyholder

---

## 28. Acceptance — public beta

The public beta is ready when all of the following are true:

1. A user can install the Claude Code plugin and get a useful report with no config file.
2. New Claude sessions are detected automatically on a full plugin install.
3. Projects are detected with and without Git.
4. Usage is measured at turn and session level.
5. The classifier builds nested project → work item → child structures, including the regression-test CHILD fixture.
6. `/tokentree` shows projects, recursive totals, and completeness in the terminal.
7. A user can ask how much a project, feature, bug fix, or subtask consumed and get ledger numbers.
8. The local dashboard supports project and work-item drill-down.
9. Users can rename, merge, split, move, attach, and detach classifications.
10. Reports show token categories, cost type, confidence, completeness, unavailable counts, attribution policy, and overhead.
11. Source transcripts are never modified.
12. Prompt and source content are not sent externally by default; prompt text is absent from DB and logs.
13. Historical imports resume after interruption.
14. Unknown log records do not crash ingest.
15. Automated tests cover supported events, cost math, and boundary-corpus metrics.
16. Explicit `start` / `stop` records work on a host without hooks.
17. `doctor` names missing logs, stale prices, DB path, capture mode, resolved OS paths, and prompt leakage.
18. Uninstall does not delete user data unless asked.
19. Skill-only and full-plugin capability states are shown accurately.
20. A negative usage delta is surfaced as an anomaly and reduces completeness.
21. Prototype JSON migration is previewable and non-destructive.
22. A project can have multiple roots without duplicating its totals.
23. Claude request telemetry and transcript observations of the same request deduplicate correctly.
24. Codex hook boundaries correlate with app-server or rollout token events.
25. Subagent totals use request-level events rather than final-request counters, and `subagent_tokens_already_in_parent` is honored.
26. Reports name the active attribution policy and overhead.
27. Active attribution weights sum to 10,000 basis points.
28. Managed-policy or trust-blocked hooks produce a clear degraded-capability report.
29. Project configuration cannot execute commands or weaken global privacy.
30. Money calculations use integer micros or exact decimals.
31. `Stop` does not automatically complete a work item.
32. `--text` never opens a server.
33. `reconcile` runs clean on the supported Claude fixtures.
34. Incomplete trees never show a bare dollar amount without a completeness chip.
35. Unavailable rows are never stored or displayed as measured `$0.00`.
36. Phase 0 package-name decision is recorded and the published package is scoped.

---

## 29. End-to-end scenario

Alex has `/Users/alex/Desktop/space-game`, no Git, a `package.json`.

```text
Build a top-down space shooter with player movement, enemies, and scoring.
```

TokenTree detects the root from cwd and manifest, creates project `Space Game` and objective `Build initial playable game`, correlates the prompt boundary with request telemetry, and attributes the request usage under the causal-request policy.

```text
Add WASD controls.
Fix diagonal movement being too fast.
```

Creates `Player movement` with two children.

```text
Enemies occasionally pass through the player. Fix that and add a regression test.
```

Creates `Fix collision bug` and child `Add collision regression test`.

`/tokentree` shows the tree in §8.6, with API-equivalent totals and completeness.

```text
How much did fixing the collision bug cost?
```

Returns recursive total, categories, sessions, models, confidence, completeness.

```text
The regression test belongs under the collision bug.
```

Attribution updates. Claude JSONL is untouched.

Alex later uses Grok in the same folder:

```bash
tokentree start --task "add sound" --project space-game
tokentree stop
```

If Grok exposes no counts, the row is **unavailable** under Space Game, not `$0.00`. Completeness on the project drops. Capture mode in Settings still says `grok: manual`.

---

## 30. Positioning

**One line.** TokenTree automatically organizes AI coding-agent usage into projects, features, tasks, and subtasks so you can see what each piece of work consumed.

**Short pitch.** Install the plugin and work normally. TokenTree detects the project and task, measures tokens and estimated cost, and builds a tree from the whole game down to one bug fix. Run `/tokentree` or ask how much any piece cost. No GitHub required. Nothing leaves the machine by default. Incomplete numbers say so.

**Not.** A daily session dashboard. An invoice. A Jira replacement. A prompt store. The existing npm packages named `tokentree` or `tokenuse`.

**Differentiation.** Automatic hierarchy · cross-session work-item identity · project-to-subtask drill-down · explainable and correctable attribution · works without GitHub · local-first · adapter interface for more than one agent · official telemetry preferred · honest unavailable state.

**Promise.** From “How much did this entire project cost?” to “How much did this specific bug fix cost?” — one local ledger, automatically classified, honestly incomplete when the data is incomplete.

---

## 31. Launch checklist

### Product

- [ ] Recursive work items + types
- [ ] Claude ingest, official telemetry, enqueue-only hooks
- [ ] Classifier with CHILD/SWITCH cutovers
- [ ] Boundary corpus metrics
- [ ] `/tokentree` + `$tokentree` + NL
- [ ] Completeness chips + unavailable state
- [ ] Capture-mode UI
- [ ] Dashboard + static HTML
- [ ] Corrections including attach/detach
- [ ] Historical + prototype import
- [ ] Cost labels + disclaimer
- [ ] Explicit start/stop
- [ ] No-Git detection + inbox
- [ ] First-run prompt-analysis sentence
- [ ] Scoped package name reserved

### Engineering

- [ ] Migrations
- [ ] Idempotent ingest
- [ ] Truth-ladder dedupe
- [ ] Parser + boundary fixtures
- [ ] Pricing file + sha256
- [ ] Integer-micro money
- [ ] `doctor` + `reconcile`
- [ ] Cross-platform path matrix
- [ ] Performance budgets
- [ ] Crash recovery
- [ ] Single-writer queue and migration lock
- [ ] Subagent capability flag
- [ ] Backup/restore

### Privacy

- [ ] In-memory analysis, no persist
- [ ] Redacted derived labels
- [ ] `doctor` leak check
- [ ] Loopback dashboard + URL token
- [ ] Export/delete
- [ ] Security policy
- [ ] Checksummed releases

### Open source

- [ ] Scoped package name reserved
- [ ] License
- [ ] Contributing and code of conduct
- [ ] Adapter + fixture + eval-harness guides
- [ ] Release process
- [ ] Public roadmap

### Docs

- [ ] Install and first-run
- [ ] Cost terminology
- [ ] Classification behavior
- [ ] No-Git detection
- [ ] Corrections
- [ ] Troubleshooting
- [ ] Privacy model
- [ ] Multi-agent bookkeeping
- [ ] Completeness and unavailable
- [ ] Capture modes

---

## 32. Prototype map

| Prototype | Product home |
|---|---|
| `scripts/track.py` | `tokentree` CLI and explicit start / stop |
| `scripts/prices.json` | versioned `prices.json` + sha256 |
| `~/.task-usage/ledger.json` | `tokentree migrate prototype` |
| `SKILL.md` | plugin skill and natural language |
| Claude Stop hook | full hook set, enqueue only |
| flat task name | work item + parent + project |
| HTML table | dashboard + static export |
| `active.json` | removed |

```bash
tokentree migrate prototype --preview
tokentree migrate prototype --apply
```

---

## 33. Changelog

### v5.1 architecture amendment — Rust-first hybrid stack (2026-09-29)

- Makes Rust the production implementation for the CLI, hook enqueue, SQLite ownership, streaming parsers, OTLP receiver, measurement/cost core, and report/query execution.
- Retains TypeScript for host-required plugin glue, dashboard/UI, and the community-adapter SDK.
- Defines `@tokentreehq/cli` as a signed native-binary installer/launcher rather than an independent JavaScript measurement engine.
- Keeps SQLite and normalized JSON contracts language-neutral during the migration.

### v5.1 — TokenTree name lock (2026-09-29)

- Product display name is **TokenTree**.
- Users type `/tokentree` and `tokentree`. Optional short alias `tuse`.
- GitHub: `tokentreehq/tokentree`. Not `github.com/tokentree` (archived crypto org).
- npm: `@tokentreehq/cli` → binary `tokentree`.
- Config/data: `.tokentree.yml`, `~/.tokentree/`, `TOKENTREE_HOME`.
- `tokenusage`, `tokenuse`, `taskuse`, `taskusage` are import/legacy aliases only.
- Do not tell users to run `npx tokenuse` or `npx tokenusage`.

### v5.0 — merge of v3.1 and v4.0


v5.0 is the merge, not a third parallel product.

**Kept from v3.1**

- Unavailable as a first-class state; never fabricate measured zeros
- Ledger-only numbers
- Capture-mode badge; skill-only ≠ automatic
- Completeness chips on incomplete money surfaces
- Scoped npm naming and collision warning
- In-memory prompt policy with `doctor` leak check and redacted derived labels
- CHILD cutover + regression-test fixture
- `reconcile` and `subagent_tokens_already_in_parent`
- Jobs-to-be-done, closed decisions, `--text` never binds a port
- First-run prompt-analysis sentence
- Notes cap, timezone/currency defaults, `prices.json` + sha256

**Kept from v4.0**

- Official request telemetry preferred over transcripts
- Truth-ladder correlation, event hashes, ingestion checkpoints
- Causal-request policy and named overhead categories
- Internal execution tasks vs user work items
- Weighted attribution groups (10,000 bp) from day one
- Adapter capability and coverage manifests
- Integer micros and cost-calculation provenance
- Richer data model (`attribution_groups`, anomalies, schema metadata)
- `Stop` does not complete a work item; inactivity marks stale
- Broader CLI and measurement-normative tests

**Added in v5.0**

- Binding two-sentence product constitution
- Completeness formula
- Explicit truth ladder
- Cold-start inbox + one-time clarification
- Correction time budget (rename / move last N / attach session)
- Price resolution order (custom > historical snapshot > current > unavailable)
- Eval harness as a Phase 0 product artifact
- Windows / macOS / Linux `doctor` path matrix
- Deduplicated public-beta acceptance list
- Phase 0 package-name ship gate
- This changelog

---

## 34. References

- Claude Code hooks: https://code.claude.com/docs/en/hooks
- Claude Code monitoring and OpenTelemetry: https://code.claude.com/docs/en/monitoring-usage
- Claude Code plugins: https://code.claude.com/docs/en/plugins
- Claude Code skills: https://code.claude.com/docs/en/skills
- Claude Code costs: https://code.claude.com/docs/en/costs
- Anthropic pricing: https://docs.anthropic.com/en/docs/about-claude/pricing
- Codex hooks: https://developers.openai.com/codex/hooks
- Codex app server: https://developers.openai.com/codex/app-server
- Codex plugin building: https://developers.openai.com/codex/plugins/build
- Codex skills: https://developers.openai.com/codex/skills
- OpenAI API pricing: https://developers.openai.com/api/docs/pricing
- ccusage guide: https://ccusage.com/guide
- ccusage session reports: https://ccusage.com/guide/session-reports

Note: npm packages `tokenusage` and `tokenuse` are unrelated existing tools. This product must not ship under those bare names. Use TokenTree / `tokentree` / `@tokentreehq/cli`.

---

*End of PRD v5.0*

