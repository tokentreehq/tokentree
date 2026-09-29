# Price snapshot contract

`prices.json` hashes the canonical compact JSON of `{version,updated,currency,models}`; the `sha256` field itself is excluded. The Phase 0 snapshot intentionally contains no model rates: unverified or stale numbers must resolve to the `unavailable` cost type, never `$0.00`. Verified public rate snapshots arrive in Phase 1 with provenance fixtures.
