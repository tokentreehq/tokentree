# Capture Modes & Capability States

TokenTree operates in three distinct capture modes depending on the host agent environment and installed components.

---

## 1. Automatic Lifecycle Capture (`automatic:hooks`)

The primary mode for Claude Code users:
- **How It Works**: Claude Code invokes TokenTree's lightweight hook script (`enqueue.sh` / `enqueue.mjs` / `tokentree hook-enqueue`) on lifecycle transitions.
- **Latency Budget**: Hooks only append a compact JSON line to `spool/claude-hooks.jsonl` and return immediately (typically under 10ms, p95 well under 100ms).
- **Background Worker**: A background worker ingests the spool into SQLite, derives project boundaries, and queues classifications off the critical hook clock.

---

## 2. Official OpenTelemetry Ingestion (`automatic:otlp`)

For environments where Claude Code or Codex is configured to emit OTLP traces:
- **Endpoint**: Loopback HTTP receiver listening on `127.0.0.1:4318/v1/logs`.
- **Precedence**: Official provider API request telemetry outranks transcript observations on the truth ladder.
- **Security**: The receiver strictly verifies loopback binding, enforces a 1 MiB body limit, and discards raw message bodies.
- **Authentication**: Every ingest request must present `Authorization: Bearer <secret>`. `tokentree otlp-serve` generates a fresh 256-bit secret from the OS CSPRNG on each run and prints it along with the `OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <secret>"` export the sender must set. Pin a stable secret for automation with the `TOKENTREE_OTLP_TOKEN` environment variable. Loopback binding alone is not enough: the ledger is append-only, so an unauthenticated local process could otherwise append poisoned rows that can never be removed.

---

## 3. Explicit Manual Runs (`manual`)

For agents without plugin hooks or for generic command-line workflows:
```bash
# Start an explicit work session
tokentree start --project space-game --task "Fix collision bug"

# Stop the session with reported counts (or stop with no counts for unavailable)
tokentree stop --input 50000 --output 2000 --model claude-sonnet-4-6
```

### Critical Invariants:
- Running `tokentree stop` records the usage span and marks the active run stopped.
- **`tokentree stop` does not automatically mark the work item as completed**; tasks remain open until explicitly resolved by the developer.

---

## 4. Skill-Only Mode (`skill-only`)

When only the conversational skill is installed without lifecycle hooks:
- The agent answers questions (e.g., `/tokentree` or "how much did task X cost?") by reading the local SQLite ledger.
- **Mandatory Guardrail**: TokenTree never tells a skill-only user they are being tracked automatically. In skill-only mode, the UI clearly displays `Capture Mode: Skill Only (No Automatic Tracking)`.
