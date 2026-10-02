## What

<!-- Link the issue: Closes #NNN -->

## Why

<!-- One paragraph: the problem this solves -->

## How

<!-- Approach in a few bullets. Call out anything that touches measurement,
     dedup, cost math, or migrations — those paths need adversarial tests. -->

## Verification

- [ ] `cargo test --workspace` green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `pnpm test` green (if TS touched)
- [ ] New behavior covered by a test (unit or adversarial fixture)

## Checklist

- [ ] No prompts, completions, or secrets in tests/fixtures
- [ ] Docs updated if user-facing behavior changed
- [ ] CHANGELOG entry (if user-facing)
