# Loop 0006 — Claude enqueue-only hook plugin

## Goal
Add a current Claude Code plugin/marketplace structure whose lifecycle hooks sanitize and enqueue compact local events without storing prompts or invoking parsing/classification.

## Sources
Claude Code hooks, plugin manifest, and marketplace references loaded 2026-09-29 from `code.claude.com`.

## Tests
Prompt omission/fingerprint, selective tool metadata, permissions, valid plugin JSON, and in-process enqueue latency.

## Out of scope
Public automatic-tracking claim and marketplace release. The bundled full CLI artifact is a release-build task; this plugin remains pre-release until compatibility fixtures and packaging pass.

## Audit
- PASS — all required Claude lifecycle events enqueue only; selective PostToolUse stores file path only.
- PASS — prompt text and tool payloads are omitted; only a prompt fingerprint is spooled.
- PASS — spool file is mode 0600 where supported and in-process enqueue p95 is under 100 ms.
- PASS — hooks never parse transcripts, classify, bind a port, or access the network.
- PASS — current official plugin/marketplace layouts are used and tests, typecheck, corpus, and lint pass.
- N/A — public automatic-tracking claim remains blocked on real-version compatibility and a bundled release CLI.
