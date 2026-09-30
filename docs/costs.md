# Cost Terminology & Pricing Engine

TokenTree computes cost estimates using exact integer arithmetic with deterministic public model pricing snapshots.

---

## The Mandatory Disclaimer

Every report, terminal summary, static HTML export, and dashboard view includes the following disclaimer:

> **Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.**

TokenTree measures underlying token consumption and applies public per-token list prices. It does not reflect negotiated enterprise volume discounts, prepaid organization credits, or personal subscription allowances (e.g. Claude Pro/Team flat tiers).

---

## Integer-Micro Precision

Floating-point roundoff errors are prohibited in TokenTree's accounting engine. All financial values in the SQLite ledger are stored as non-negative 64-bit integers representing **micros** (one-millionth of a US dollar, or $\$0.000001$):

$$\text{micros} = \text{dollars} \times 1{,}000{,}000$$

When calculating cost:
$$\text{Cost}(\mu) = \sum_{k \in \{\text{input}, \text{cache\_read}, \text{cache\_write}, \text{output}, \text{reasoning}\}} \text{Tokens}_k \times \text{Rate}_k$$

Where each $\text{Rate}_k$ is expressed in integer micros per token.

---

## Cache Category Rates

Modern models feature distinct pricing tiers for cached input:
- **Base Input Tokens**: Fresh prompt tokens submitted without cache hit.
- **Cache Read Tokens**: Tokens served directly from prompt cache (typically 10% of base rate).
- **Cache Write Tokens**: Tokens written into 5-minute ephemeral prompt cache (typically 125% of base rate).
- **Output Tokens**: Generated response tokens.
- **Reasoning Tokens**: Extended thinking or internal reasoning tokens.

TokenTree attributes each category with its exact rate rather than blending them into a single generic input rate.

---

## Price Resolution Order

When resolving price for a request observed at timestamp $T$:
1. **Explicit Custom Override**: If a project-level `.tokentree.yml` or manual input specifies a fixed rate.
2. **Historical Versioned Snapshot**: A snapshot active and valid at timestamp $T$.
3. **Current Verified Snapshot**: Bundled verified snapshot in `packages/pricing/data/prices.json`.
4. **Unavailable**: If the model is unlisted or missing, the cost is marked as **unavailable**; TokenTree never fabricates `$0.00`.

---

## Checksummed Price Snapshots

All snapshot files must include a matching SHA-256 digest:
```json
{
  "version": 1,
  "updated_at": "2026-03-01T00:00:00Z",
  "sha256": "47372d8a...",
  "models": {
    "claude-sonnet-4-6": {
      "input_per_million": 3.0,
      "cache_read_per_million": 0.30,
      "cache_write_per_million": 3.75,
      "output_per_million": 15.0
    }
  }
}
```
If a price file has been altered without a matching checksum, `tokentree` rejects the snapshot and reports the tampering during `tokentree doctor`.
