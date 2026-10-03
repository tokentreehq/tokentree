# Corrections, Attributions & User Notes

TokenTree maintains an append-only ledger where raw usage observations are immutable, while human or classifier attributions are explicitly versioned.

---

## The Causal-Request Policy

- **Measurement is Forever**: When an AI agent issues a request to a provider, that measurement (tokens, model, timestamp) is recorded once and never modified.
- **Attribution is Versioned**: Attributing that request to a project, task, or subtask is a separate classification decision recorded in `attribution_groups` and `attributions`.
- **Transcripts are Never Touched**: Making corrections in TokenTree will never alter host transcript files (`.jsonl`).

---

## The 10,000 Basis Points Rule

Attribution weights are integer basis points where $10{,}000\text{ bp} = 100.00\%$.

For any active attribution group:
$$\sum_{a \in \text{Attributions}} \text{weight\_basis\_points}(a) = 10{,}000$$

When a user attaches or moves a session:
1. The existing attribution group is marked `active = 0`.
2. A new attribution group is inserted with `active = 1` and `weight_basis_points = 10000`.
3. The replacement is transactional, preserving full historical provenance.

---

## Correction Commands

### Rename Work Item
Updates the display title of a work item:
```bash
tokentree rename --work-item wrk_123 --title "Fix collision bug v2"
```

### Move Work Item
Reparents a work item under another task or promotes it to a root:
```bash
# Reparent under wrk_parent
tokentree move --work-item wrk_child --parent wrk_parent

# Promote to root
tokentree move --work-item wrk_child
```

### Attach Session
Explicitly attaches an unassigned or misattributed session to a work item:
```bash
tokentree attach --session ses_abc --work-item wrk_123
```

### Detach Session
Detaches a session from a work item:
```bash
tokentree detach --session ses_abc
```

### Add Explicit Note
Adds a human annotation (1–240 characters) to a work item:
```bash
tokentree note --work-item wrk_123 --text "Root cause was race condition in physics step"
```
Notes are strictly opt-in, explicitly entered by humans, and never scraped from prompt text.
