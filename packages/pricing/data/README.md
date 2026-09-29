# Price snapshot contract

`prices.json` hashes the canonical compact JSON of `{version,updated,currency,models}`; `sha256` itself is excluded. Rates are decimal USD per million tokens and include source URLs plus effective dates.

The initial verified entry is Claude Sonnet 4.6: $3 input, $15 output, $3.75 five-minute cache write, and $0.30 cache read per million tokens, sourced from Anthropic's model documentation on 2026-09-29. Models without a verified matching entry remain `unavailable`, never `$0.00`.
