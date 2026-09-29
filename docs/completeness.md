# Completeness Formula & The Honest Unavailable State

TokenTree's guiding product constitution dictates:

> **Never present missing measurement as $0.00.**
> **Always display a completeness chip and unavailable count whenever numbers are incomplete.**

---

## The Completeness Formula

Completeness represents the percentage of known requests that yielded verifiable measurement:

$$\text{Completeness} = 100 \times \frac{\text{measured}}{\text{measured} + \text{unavailable}}$$

### Rules:
1. **Zero Requests**: If a work item or project has zero requests, completeness is undefined (`null` or `—`), never fabricated as $100\%$ or $0\%$.
2. **Missing Token Counts**: If an agent host or tool execution does not report tokens (e.g. an offline command or uninstrumented CLI run), the request is categorized as **unavailable**.
3. **Never Fake Zeros**: An unavailable row is never recorded or displayed as $\$0.00$. Presenting unmeasured work as free is a violation of TokenTree's accounting model.

---

## Completeness Chips in Reports and UI

Whenever completeness is below $100\%$ or any requests are unavailable, all displays surface prominent chips:

### Terminal (`tokentree report --text`):
```text
Fix collision bug v2 — $0.05 est. API-equivalent · 3000000 tok · 75% complete · 1 unavailable
```

### Dashboard & Static HTML:
- **100% Complete**: Green badge (`100% complete`).
- **Incomplete**: Amber badge (`75% complete (1 unavailable)`).
- **No Requests**: Muted badge (`no requests`).

---

## Anomalies & Negative Deltas

If a provider or host log reports a cumulative token count that drops below a previously observed checkpoint (a negative delta anomaly):
1. An anomaly entry is logged in `anomalies`.
2. The anomalous interval is marked as unmeasured.
3. The project's completeness score is automatically reduced.
4. `tokentree reconcile` flags the anomaly for human review.
