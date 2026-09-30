# Multi-Agent Bookkeeping & Subagent Attribution

Modern coding agents frequently spawn child subagents (e.g., researcher agents, test execution agents, or code reviewer subagents) to perform delegated tasks in parallel.

---

## The Double-Counting Pitfall

When a parent agent spawns a subagent, agent telemetry often reports usage in two conflicting ways:
1. **Request-Level Streams**: Each individual API request made by the subagent has its own prompt/completion tokens.
2. **Cumulative Summary Counters**: When the subagent terminates, the host agent emits a summary message into the parent session reporting the total tokens consumed by the subagent.

If both streams are ingested naively, subagent tokens are double-counted.

---

## How TokenTree Solves This

### 1. Truth-Ladder Source Precedence
TokenTree establishes a clear hierarchy of measurement sources:
$$\text{Official Telemetry (OTLP)} > \text{Streaming Transcript Requests} > \text{Cumulative Session Counters}$$

Individual request-level events always take precedence over summary counters.

### 2. The `subagent_tokens_already_in_parent` Capability
Each host adapter declares its multi-agent capability in its manifest:
- If `subagent_tokens_already_in_parent: true`, the parent transcript's final token counters already incorporate the subagent's tokens. In this case, TokenTree subtracts the subagent's request totals from the parent session total to prevent inflation.
- If `subagent_tokens_already_in_parent: false`, subagents and parent agents operate on isolated request streams.

### 3. Causal Work-Item Attribution
Subagent turns are linked directly to the parent work item or assigned to a dedicated child work item (e.g., `Researching codebase` under `Fix collision bug`), ensuring that costs roll up hierarchically into the parent project tree.
