# TokenTree Provider Validation & Certification Suite

TokenTree provides a first-class, provider-neutral validation and diagnostics capability via `tokentree validate`. This command performs end-to-end verification of installed AI agent host adapters, local telemetry sources, ledger integrity invariants, causal reconciliation, and zero-secret privacy compliance.

---

## 1. Overview & Architecture

The `tokentree validate` command explicitly separates **offline parser certification** from **live host environment diagnostics**:

1. **Offline Parser Self-Testing (`--self-test`)**: Ingests embedded, certified provider fixtures (`FIXTURE_CLAUDE`, `FIXTURE_CODEX`, `FIXTURE_GROK`, `FIXTURE_HERMES`) into an isolated sandbox database, validating parser accuracy, ledger constraints, checkpoint tracking, monotonicity, and privacy compliance without requiring host CLI installations or live accounts.
2. **Host Environment Diagnostics (default mode)**: Probes the local workstation for active agent CLIs (Claude Code, OpenAI Codex, xAI Grok, Hermes / OpenRouter), discovers existing session trees, and validates host telemetry. If a provider is not installed or configured on the host, it is truthfully reported as `UNAVAILABLE` rather than misrepresenting absence as a pass.
3. **Live Capture Enforcement (`--require-live`)**: Strictly requires that targeted providers are installed in `PATH` and have active, parseable host telemetry. Fails closed with exit code `1` if any required provider is missing or cannot be validated.

### Key Architectural Principles

- **Distinct Verification States**: Parser self-testing, host discovery, configuration integrity, host telemetry parsing, and live capture capability are tracked as distinct, strongly-typed states. Embedded fixtures satisfy only `self_test`; they never imply that a provider is installed or live-verified on the machine.
- **Fail-Closed Execution**: If any check fails (such as malformed telemetry, an unhandled anomaly, an invariant violation, or a detected credential), validation fails immediately with exit code `1`.
- **Zero Discarded Results**: Every discovered host session file is parsed. If any host file fails to parse or ingest, host telemetry validation immediately fails.
- **Typed `deny_unknown_fields` Reporting**: The optional `--output <PATH>` report conforms to a strict, typed schema (`schema_version: "1.0.0"`). Deserialization strictly rejects unknown fields at any depth, and semantic scanners assert zero credential patterns, user home paths, or raw prompts.

---

## 2. Command Reference

### Basic Usage

```bash
# Validate offline parser & ledger invariants for all adapters (offline certification)
tokentree validate --self-test --all

# Validate offline parser for a specific adapter
tokentree validate --self-test grok

# Validate local host environment for a specific installed adapter
tokentree validate claude
tokentree validate codex
tokentree validate grok
tokentree validate hermes

# Validate all host environments (reports UNAVAILABLE for uninstalled providers)
tokentree validate --all

# Enforce that all providers are installed and live-verified on this host
tokentree validate --require-live --all

# Export a versioned allowlisted JSON report with typed schema validation
tokentree validate --self-test --all --output ./validation-report.json

# Include local non-sensitive token/cost diagnostics in report
tokentree validate --self-test --all --output ./validation-report.json --local-details

# Certify a custom fixture file (useful for contributors and test harnesses)
tokentree validate grok --fixture ./fixtures/parsers/grok/multi-turn.json
```

### Exit Codes

| Exit Code | Meaning | Description |
|:---:|:---|:---|
| `0` | **PASS** | In `--self-test` mode: all parser self-tests and ledger invariants passed. In default/live mode: all targeted host providers were present, all discovered telemetry was valid, and all checks passed. |
| `1` | **FAIL / UNAVAILABLE** | One or more verification checks failed, or targeted provider is unavailable when live verification is required. |
| `2` | **USAGE ERROR** | Invalid arguments, unknown adapter name, or conflicting options. |

---

## 3. Explicit Status Enums & Verification States

All validation results are expressed through strongly-typed status enums:

### Check Status (`ValidationStatus`)

- `passed`: The check executed and fully satisfied all validation invariants.
- `failed`: The check executed and encountered malformed data, schema mismatches, invariant violations, or privacy leaks.
- `unavailable`: The requested capability or resource is not present on this host machine (e.g. CLI binary not found in `PATH`, session log directory does not exist).
- `skipped`: The check was intentionally bypassed for the selected operational mode (e.g. host telemetry checks skipped during `--self-test`).

### Adapter Overall Status (`AdapterOverallStatus`)

- `passed`: The provider was discovered on the host, all discovered telemetry parsed with 0 errors, and all ledger, reconciliation, and privacy checks passed.
- `self_test_passed`: In `--self-test` mode, the parser, ledger, and reconciliation invariants were certified offline against certified provider fixtures.
- `unavailable`: The provider is not installed or configured on this workstation (default host mode).
- `failed`: Any executed check failed closed.

---

## 4. The Distinct Verification Pillars

Every adapter evaluated by `tokentree validate` progresses through distinct verification stages:

### Stage 1: Parser Self-Test (`self_test`)
Evaluates the adapter parser against embedded provider fixtures (`FIXTURE_CLAUDE`, `FIXTURE_CODEX`, `FIXTURE_GROK`, `FIXTURE_HERMES`):
- Verifies that zero malformed records or syntax errors occur.
- Asserts that all provider fields map into normalized `TokenUsage` observations.
- In `--self-test` mode, this certifies the adapter implementation offline.

### Stage 2: Provider Discovery (`discovery`)
Probes the host environment safely without executing untrusted code:
- **CLI Binary**: Checks if the adapter binary (`claude`, `codex`, `grok`, `hermes`) exists on the system `PATH`.
- **Telemetry Roots**: Probes standard session storage directories (`~/.claude/projects`, `~/.codex/sessions`, `~/.grok/sessions`, `%LOCALAPPDATA%\hermes` or `~/.hermes`).
- **Configuration**: Verifies that adapter configuration files or directories exist.
- Reports `passed` if discovered, or `unavailable` if not detected on host.

### Stage 3: Configuration Integrity (`configuration`)
Validates that adapter settings and directory permissions comply with security policies:
- Asserts that directory permissions prevent world-writable access.
- Confirms that local SQLite databases (e.g. Hermes `state.db`) are accessible in read-only mode (`PRAGMA query_only = ON;`).
- Confirms that configuration files contain no exposed plaintext API keys.
- Reports `skipped` in `--self-test` mode, `unavailable` if config is missing, or `failed` if insecure.

### Stage 4: Host Telemetry Validation (`host_telemetry`)
Discovers and ingests host telemetry files from the local filesystem:
- Discovers all candidate session logs in the session tree.
- Parses every discovered file into an isolated ledger sandbox.
- **Fail-Closed Guarantee**: Never discards parse or ingest booleans. If any discovered file fails to parse, host telemetry validation immediately fails with status `failed`.
- Reports `skipped` in `--self-test` mode, or `unavailable` if no host logs exist.

### Stage 5: Ledger Invariants & Checkpoint Verification (`ledger_integrity`)
Performs SQLite database integrity and invariant checks on the ingested data:
- Executes `PRAGMA integrity_check` to ensure B-tree and page consistency.
- Executes `PRAGMA foreign_key_check` to assert that all sessions, turns, and usage events maintain valid relational integrity.
- **Checkpoint Validation**: Verifies that `ingestion_checkpoints` records valid 64-character SHA-256 hashes, non-negative offsets, non-negative file sizes, and matching adapter identities.
- **Monotonicity & Non-Negative Invariants**: Verifies that `input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_tokens`, and `provider_reported_cost_micros` are non-negative across all rows.

### Stage 6: Causal Reconciliation (`reconciliation`)
Runs TokenTree's reconciliation engine across the temporary ledger:
- Asserts `duplicate_request_ids == 0`.
- Asserts `duplicate_subagent_counters == 0`.
- Verifies that all anomalies are recorded deterministically.

### Stage 7: Privacy & Leak Audit (`privacy_audit`)
Executes TokenTree's automated privacy scanner against the ledger database:
- Executes `ledger.audit_prompt_leakage(None)` to assert 0 prompt/completion payload columns or leaks.
- Scans all model names, request IDs, event IDs, turn titles, and agent IDs for credential patterns (`sk-ant-`, `sk-proj-`, `sk-`, `bearer `, `xai-`, `ghp_`, `gho_`, API keys) and canary tokens.
- Asserts zero privacy violations.

### Stage 8: Live Capture State (`live_capture`)
Records whether live generation or hook capture capability is active on this host:
- Reports `passed` if CLI and telemetry capture are available.
- Reports `unavailable` if the tool is not installed or unauthenticated.
- Reports `skipped` in `--self-test` mode.

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
  "overall_passed": true,
  "adapters": {
    "claude": {
      "capabilities": {
        "cli_installed": true,
        "sessions_discovered": true,
        "config_present": true,
        "capture_available": true
      },
      "checks": {
        "self_test": "passed",
        "discovery": "passed",
        "configuration": "skipped",
        "host_telemetry": "skipped",
        "ledger_integrity": "passed",
        "reconciliation": "passed",
        "privacy_audit": "passed",
        "live_capture": "skipped",
        "overall_status": "self_test_passed"
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
        "privacy_violations": 0
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

To certify a new or updated host adapter for TokenTree:

1. **Implement Adapter Module**: Implement the adapter crate (`crates/tokentree-<name>`) and TypeScript parser (`packages/adapters/<name>`). Ensure all failed-request semantics emit `MeasurementSource::Unavailable` with deterministic anomalies.
2. **Provide Certified Fixtures**: Place minimal, privacy-cleared JSON/JSONL fixtures in `fixtures/parsers/<name>/`. Ensure 0 credentials, 0 real usernames, and 0 prompt bodies exist in fixtures.
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
