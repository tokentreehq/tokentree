# TokenTree

TokenTree is an open-source, local-first developer tool for measuring AI coding-agent usage and organizing immutable provider requests into projects → work items → sessions → turns.

> **Status:** Phase 0 foundation. No host adapter, plugin, CLI report, or dashboard is claimed usable yet.

The product constitution and requirements live in [`TokenTree-PRD-v5.1.md`](./TokenTree-PRD-v5.1.md). Engineering rules are in [`CLAUDE.md`](./CLAUDE.md), and current delivery truth is in [`docs/status.md`](./docs/status.md).

```bash
corepack enable
pnpm install
pnpm check
pnpm lint
```

Privacy defaults: local-only, no account, no telemetry, no network egress, and no prompt/completion/source/tool payload persistence.

License intent: Apache-2.0, pending final legal review before publication.
