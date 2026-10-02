# TokenTree Provider Validation & Diagnostic Suite

TokenTree provides a first-class, provider-neutral validation and diagnostics capability via `tokentree validate`. This command performs end-to-end verification of installed AI agent host adapters, local telemetry sources, ledger integrity invariants, causal reconciliation, and zero-secret privacy compliance.

---

## 1. Overview & Architecture

The `tokentree validate` command explicitly separates **offline parser regression testing** from **live host environment diagnostics**:

1. **Offline Parser Self-Testing (`--self-test`)**: Ingests embedded, versioned regression fixtures (`FIXTURE_CLAUDE`, `FIXTURE_CODEX`, `FIXTURE_GROK`, `FIXTURE_HERMES`) into an isolated sandbox database, validating parser accuracy, ledger constraints, checkpoint tracking, monotonicity, and privacy compliance without requiring host CLI installations or live accounts.
2. **Host Environment Diagnostics (default mode)**: Probes the local workstation for active agent CLIs (Claude Code, OpenAI Codex, xAI Grok, Hermes / OpenRouter), discovers existing session trees, and validates host telemetry. If a provider is not installed or configured on the host, it is truthfully reported as `unavailable` rather than misrepresenting absence as a pass.
3. **Live Capture Enforcement (`--require-live`)**: Strictly requires that targeted providers are installed in `PATH` and have active, parseable host telemetry. Fails closed with exit code `1` if any required provider is missing or cannot be validated.

### Key Architectural Principles

- **Distinct Verification States**: Parser self-testing, host discovery, configuration integrity, host telemetry parsing, and live capture capability are tracked as distinct, strongly-typed states. Embedded regression fixtures satisfy only `self_test`; they never imply that a provider is installed or live-verified on the machine.
- **Fail-Closed Execution**: If any check fails (such as malformed telemetry, an unhandled anomaly, an invariant violation, or a detected credential), validation fails immediately with exit code `1`.
- **Zero Discarded Results**: Every discovered host session file is parsed. If any host file fails to parse or ingest, host telemetry validation records the failure and prevents a false healthy status.
- **Typed `deny_unknown_fields` Reporting**: The optional `--output <PATH>` report conforms to a strict, typed schema (`schema_version: "1.0.0"`). Deserialization strictly rejects unknown fields at any depth, and semantic scanners assert zero credential patterns, user home paths, or raw prompts.

---

## 2. Command Reference

### Basic Usage

```bash
# Validate offline parser & ledger invariants for all adapters using versioned regression fixtures
tokentree validate --self-test --all

# Validate offline parser for a specific adapter
tokentree validate --self-test grok

# Validate local host environment for a specific installed adapter
tokentree validate claude
tokentree validate codex
tokentree validate grok
tokentree validate hermes

# Validate all host environments (reports unavailable for uninstalled providers)
tokentree validate --all

# Enforce that all providers are installed and live-verified on this host
tokentree validate --require-live --all

# Wait up to 30 seconds for a newly created live provider event
tokentree validate --require-live --wait 30 grok

# Export a versioned allowlisted JSON report with typed schema validation
tokentree validate --self-test --all --output ./validation-report.json

# Include local non-sensitive token/cost diagnostics in report
tokentree validate --self-test --all --output ./validation-report.json --local-details

# Test a custom fixture file (useful for contributors and test harnesses)
tokentree validate grok --fixture ./fixtures/parsers/grok/multi-turn.json
```

### Exit Codes

| Exit Code | Constant | Meaning | Description |
|:---:|:---|:---|:---|
| `0` | `EXIT_HEALTHY` | **HEALTHY / PASS** | In `--self-test` mode: all parser self-tests and ledger invariants passed. In default/live mode: targeted host providers were available, all discovered telemetry was valid, and all checks passed. |
| `1` | `EXIT_VALIDATION_FAILURE` | **VALIDATION FAILURE** | One or more verification checks failed, malformed telemetry encountered, or `--require-live` was unsatisfied. |
| `2` | `EXIT_INVOCATION_ERROR` | **INVOCATION ERROR** | Invalid arguments, unknown adapter name, or conflicting options (e.g. `--fixture` with `--all`). |
| `3` | `EXIT_UNAVAILABLE` | **UNAVAILABLE** | In default host mode: one or more targeted providers were not found on the local system. |

---

## 3. Explicit Status Enums & Verification States

All validation results are expressed through strongly-typed status enums:

### Self-Test Status (`SelfTestStatus`)

- `passed`: The embedded regression fixture parsed successfully, records ingested into the isolated ledger, and all invariant checks passed.
- `failed`: Regression fixture parsing or invariant assertion failed.
- `not_run`: Self-testing was not requested for this execution mode.

### Provider Status (`ProviderStatus`)

- `available`: The provider CLI binary was discovered on the system `PATH`.
- `unavailable`: The provider CLI binary was not found on the system `PATH`.
- `misconfigured`: The provider CLI binary was found but configuration/environment checks failed.

### Telemetry Status (`TelemetryStatus`)

- `verified`: Discovered host telemetry files were parsed and ingested into the temporary ledger without error.
- `not_found`: No candidate telemetry logs or databases were discovered for this provider on the host.
- `unsupported`: The telemetry format or schema version is unsupported.
- `failed`: One or more discovered telemetry files failed to parse or violated ingestion invariants.
- `not_run`: Host telemetry discovery and ingestion was skipped (e.g. during `--self-test`).

### Live Capture Status (`LiveCaptureStatus`)

- `verified`: Live generation or hook capture capability is active and verified on this host.
- `unavailable`: The tool is not installed, unauthenticated, or has no verified live capture.
- `failed`: Live capture execution failed.
- `not_run`: Live capture evaluation was not run.

### Check Status (`CheckStatus`)

- `verified`: The check executed and fully satisfied all validation invariants.
- `misconfigured`: Configuration files or directory permissions violated security requirements.
- `not_found`: Necessary files or directories were not present on the host.
- `failed`: The check executed and encountered malformed data, schema mismatches, invariant violations, or privacy leaks.
- `not_run`: The check was bypassed for the selected operational mode.

### Overall Status (`OverallStatus`)

- `healthy`: All targeted capabilities and checks passed completely.
- `degraded`: Telemetry was not found or optional checks were incomplete, but no checks failed.
- `unavailable`: Targeted provider was not found on the system.
- `failed`: Any executed check failed closed.

---

## 4. The Distinct Verification Pillars

Every adapter evaluated by `tokentree validate` progresses through distinct verification stages:

### Stage 1: Parser Self-Test (`self_test_status`)
Evaluates the adapter parser against embedded versioned regression fixtures (`FIXTURE_CLAUDE`, `FIXTURE_CODEX`, `FIXTURE_GROK`, `FIXTURE_HERMES`):
- Verifies that zero malformed records or syntax errors occur.
- Asserts that all provider fields map into normalized `TokenUsage` observations.
- In `--self-test` mode, this certifies the adapter parser offline.

### Stage 2: Provider Discovery (`provider_status`)
Probes the host environment safely without executing untrusted code:
- **CLI Binary**: Checks if the adapter binary (`claude`, `codex`, `grok`, `hermes`) exists on the system `PATH`.
- **Telemetry Roots**: Probes standard session storage directories (`~/.claude/projects`, `~/.codex/sessions`, `~/.grok/sessions`, `%LOCALAPPDATA%\hermes` or `~/.hermes`).
- **Configuration**: Verifies that adapter configuration files or directories exist.
- Reports `available` if discovered, or `unavailable` if not detected on host.

### Stage 3: Configuration Integrity (`configuration_status`)
Validates that adapter settings and directory permissions comply with security policies:
- Asserts that directory permissions prevent world-writable access.
- Confirms that local SQLite databases (e.g. Hermes `state.db`) are accessible in read-only mode (`PRAGMA query_only = ON;`).
- Confirms that configuration files contain no exposed plaintext API keys.
- Reports `not_run` in `--self-test` mode, `not_found` if config is missing, or `failed` if insecure.

### Stage 4: Host Telemetry Validation (`telemetry_status`)
Discovers and ingests host telemetry files from the local filesystem:
- Discovers all candidate session logs in the session tree.
- Parses every discovered file into an isolated ledger sandbox.
- **Fail-Closed Guarantee**: Never discards parse or ingest outcomes. Every file is mapped to a structured `TelemetryImportOutcome` (`verified`, `duplicate_only`, `malformed`, `unsupported_version`, `inaccessible`, `empty`, `skipped`, `failed`). If any discovered file fails to parse or violates ingestion invariants, host telemetry validation records the failure and prevents a false healthy status.
- Tracks file-level counters: `attempted`, `verified`, `failed`, `unsupported`, `anomalous`, `duplicate_only`, `empty`, `skipped`.
- Reports `not_run` in `--self-test` mode, or `not_found` if no host logs exist.

### Stage 5: Ledger Invariants & Checkpoint Verification (`ledger_integrity_status`)
Performs SQLite database integrity and invariant checks on the ingested data:
- Executes `PRAGMA integrity_check` to ensure B-tree and page consistency.
- Executes `PRAGMA foreign_key_check` to assert that all sessions, turns, and usage events maintain valid relational integrity.
- **Checkpoint Validation**: Verifies that `ingestion_checkpoints` records valid 64-character SHA-256 hashes, non-negative offsets, non-negative file sizes, and matching adapter identities.
- **Monotonicity & Non-Negative Invariants**: Verifies that `input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_tokens`, and `provider_reported_cost_micros` are non-negative across all rows.

### Stage 6: Causal Reconciliation (`reconciliation_status`)
Runs TokenTree's reconciliation engine across the temporary ledger:
- Asserts `duplicate_request_ids == 0`.
- Asserts `duplicate_subagent_counters == 0`.
- Verifies that all anomalies are recorded deterministically.

### Stage 7: Privacy & Leak Audit (`privacy_audit_status`)
Executes TokenTree's automated privacy scanner against the ledger database:
- Executes `ledger.audit_prompt_leakage(None)` to assert 0 prompt/completion payload columns or leaks.
- Scans all model names, request IDs, event IDs, turn titles, and agent IDs for credential patterns (`sk-ant-`, `sk-proj-`, `sk-`, `bearer `, `xai-`, `ghp_`, `gho_`, API keys) and canary tokens.
- Asserts zero privacy violations.

### Stage 8: Live Capture State (`live_capture_status`)
Records whether live generation or hook capture capability is active on this host:
- **Freshness Requirement**: `live_capture_status` becomes `verified` ONLY when a fresh provider event or session created after validation start time is observed. Historical telemetry files satisfy only `telemetry_status` and will NEVER satisfy `--require-live`.
- **Usable Live Polling (`--wait <seconds>`)**: Pre-snapshots stable provider event and session identities before polling begins. Continuously inspects newly created events against authoritative internal timestamps (with bounded 30s clock-skew tolerance) until a fresh event is observed or deadline expires. File `mtime` modification alone, touched old files, and copied historical files are strictly rejected.
- **Default HEALTHY Requirements**: Default `HEALTHY` requires provider available, required configuration verified, real host telemetry verified, and all integrity/reconciliation/privacy checks verified.
- **Degraded Semantics**: CLI installation alone without host telemetry, or configuration `not_found` for providers requiring configuration, produces `degraded` or `unavailable`, never `healthy`.
- Reports `unavailable` if the tool is not installed, unauthenticated, or has no fresh live capture.
- Reports `not_run` in `--self-test` mode.

---

## 5. Allowlisted JSON Report Schema (`v1.0.0`)

When exported with `--output <PATH>`, the report is validated against typed Rust structures with `#[serde(deny_unknown_fields)]`:

```json
{
  "schema_version": "1.0.0",
  "generator": "tokentree validate 0.2.0",
  "timestamp": "2026-10-01T17:00:00Z",
  "mode": "self_test",
  "environment": {
    "os_family": "windows",
    "platform": "windows",
    "arch": "x86_64"
  },
  "overall_status": "healthy",
  "adapters": {
    "claude": {
      "capabilities": {
        "cli_installed": true,
        "sessions_discovered": true,
        "config_present": true,
        "capture_available": true
      },
      "checks": {
        "self_test_status": "passed",
        "provider_status": "available",
        "configuration_status": "not_run",
        "telemetry_status": "not_run",
        "ledger_integrity_status": "verified",
        "reconciliation_status": "verified",
        "privacy_audit_status": "verified",
        "live_capture_status": "not_run",
        "overall_status": "healthy"
      },
      "counters": {
        "sessions_evaluated": 1,
        "events_ingested": 2,
        "total_tokens": 15,
        "input_tokens": 10,
        "output_tokens": 5,
        "cached_tokens": 0,
        "reasoning_tokens": 0,
        "anomalies_detected": 0,
        "duplicate_requests": 0,
        "privacy_violations": 0,
        "host_files": {
          "attempted": 0,
          "verified": 0,
          "failed": 0,
          "unsupported": 0,
          "anomalous": 0,
          "duplicate_only": 0,
          "empty": 0,
          "skipped": 0
        }
      },
      "local_diagnostics": {
        "models_observed": [
          "claude-3-5-sonnet-20241022"
        ],
        "total_cost_micros": 45,
        "measured_turns": 2,
        "unmeasured_turns": 0,
        "completeness_pct": 100.0
      }
    }
  }
}
```

---

## 6. Contributor Certification Guidance

To add or update a host adapter for TokenTree:

1. **Implement Adapter Module**: Implement the adapter crate (`crates/tokentree-<name>`) and TypeScript parser (`packages/adapters/<name>`). Ensure all failed-request semantics emit `MeasurementSource::Unavailable` with deterministic anomalies.
2. **Provide Versioned Regression Fixtures**: Place minimal, privacy-cleared JSON/JSONL fixtures in `fixtures/parsers/<name>/`. Ensure 0 credentials, 0 real usernames, and 0 prompt bodies exist in fixtures.
3. **Execute Offline Certification**:
   ```bash
   cargo test -p tokentree-cli --test validate_test
   cargo run -p tokentree-cli --bin tokentree -- validate --self-test <adapter>
   ```
4. **Execute Live / Host Diagnostics**:
   ```bash
   cargo run -p tokentree-cli --bin tokentree -- validate <adapter>
   ```
5. **Verify Fail-Closed Rejection**: Run validation on malformed or adversarial fixtures:
   ```bash
   cargo run -p tokentree-cli --bin tokentree -- validate <adapter> --fixture ./fixtures/corrupted.json
   ```
   Assert that validation fails closed with exit code `1`.
