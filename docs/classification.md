# Classification Engine & Boundary Behavior

TokenTree organizes turns into nested work items using a conservative boundary classifier.

---

## The Four Decisions

At each user prompt or agent turn boundary, the classifier evaluates context signals to select one of four transitions:

| Decision | Meaning | Resulting Hierarchy |
|---|---|---|
| **CONTINUE** | The user is continuing the immediate work on the current work item (e.g. "that worked", "try fixing the typo"). | Turns attribute to the existing active work item. |
| **CHILD** | The user started a direct subtask, follow-up verification, or regression test under the active work item (e.g. "now add a regression test for that bug"). | A new child work item is created with `parent_id` pointing to the current work item. |
| **SWITCH** | The user shifted focus to an independent task or feature in the project (e.g. "now build the leaderboards"). | A new top-level or sibling work item is created. |
| **UNCERTAIN** | Context is ambiguous or insufficient to establish clear attribution. | Marked for user review in `tokentree review` or the dashboard; attributed provisionally without rewriting event history. |

---

## Conservative Defaults & CHILD Cutovers

TokenTree favors continuity over false fragmentation:
- Ambiguous or short responses ("looks good", "ok", "run it again") default to **CONTINUE**.
- Prompts that explicitly reference a parent objective or immediate fix ("add a regression test", "write tests for that", "document the changes") trigger **CHILD**.
- Radical subject shifts or new major capabilities ("start working on X", "clean up Y") trigger **SWITCH**.

---

## Privacy Policy for Classification

1. **In-Memory Analysis Only**: The raw prompt text is evaluated in volatile memory solely to calculate boundary signals and derived labels.
2. **Never Persisted**: Full prompt text, completion text, tool inputs, file diffs, and reasoning tokens are **never written to disk or the SQLite database**.
3. **Prompt Fingerprinting**: Only the SHA-256 digest of the prompt (`prompt_fingerprint`) is stored to detect duplicates and verify turn boundaries.
4. **Redacted Derived Labels**: A short 3–8 word derived label (e.g., `Fix collision bug in game loop`) is produced with regex redaction applied to tokens resembling API keys, secrets, email addresses, and hex hashes.
