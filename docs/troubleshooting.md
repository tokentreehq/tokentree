# Troubleshooting, Diagnostics & Reconcile

TokenTree includes built-in diagnostic and reconciliation tools to verify integrity and resolve data drift.

---

## 1. Running `tokentree doctor`

`tokentree doctor` performs a comprehensive suite of health and security checks:

```bash
tokentree doctor
```

### What `doctor` Verifies:
1. **SQLite Database**: Verifies WAL mode, foreign keys, schema version, and connection responsiveness.
2. **Pricing Snapshot**: Validates SHA-256 digest of `prices.json` against known public rates.
3. **Filesystem Permissions**: Confirms `0700` directory permissions on `~/.tokentree` and `0600` file permissions on `ledger.db` where supported.
4. **Privacy & Leak Check**: Audits the `turns` table to guarantee that prompt text, tool arguments, completions, and diffs are absent. Verifies that derived labels conform to the 3–8 word bound.
5. **Path Matrix**: Verifies operating system transcript discovery paths (`darwin`, `linux`, `win32`).

If any check fails, `doctor` returns exit code `2` with actionable remediation steps.

---

## 2. Running `tokentree reconcile`

`tokentree reconcile` analyzes the ledger to identify duplicate canonical request IDs, anomalies, and multi-agent accounting:

```bash
tokentree reconcile
```

Output:
```text
sessions: 4
duplicate canonical request IDs: 0
unresolved anomalies: 0
subagent reconciliation: ok
```

### When to Reconcile:
- After importing historical transcripts.
- If you notice unexpected counts or overlapping sessions.
- To confirm that subagent requests are not being double-counted against the parent session.

---

## 3. Crash Recovery and Resumed Imports

TokenTree's ingestion engine is designed to survive crashes, sudden power losses, and process termination:
- **Crash-Safe Spool**: Hooks write compact JSON lines to `spool/claude-hooks.jsonl`.
- **Atomic File-Offset Checkpoints**: Checkpoints are stored transactionally in `ingestion_checkpoints`.
- **Canonical Request Deduplication**: Replaying the same transcript or spool file multiple times is completely idempotent.

If an import is interrupted, rerun:
```bash
tokentree import claude
```
TokenTree resumes from the exact byte offset without creating duplicate events.
