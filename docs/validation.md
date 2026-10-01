# TokenTree Provider Validation & Certification Suite

TokenTree provides a first-class, provider-neutral validation and diagnostics capability via `tokentree validate`. This command performs end-to-end verification of installed AI agent host adapters, local telemetry sources, ledger integrity invariants, causal reconciliation, and zero-secret privacy compliance.

---

## 1. Overview & Architecture

The `tokentree validate` command is designed for two primary operational use cases:

1. **Host Environment Diagnostics**: Probing the host machine for active agent CLIs (Claude Code, OpenAI Codex, xAI Grok, Hermes / OpenRouter), discovering existing session logs, and verifying that local telemetry ingests into the TokenTree ledger cleanly and accurately without crashing or double counting.
2. **Adapter Certification**: Serving as the automated certification harness for contributors implementing or updating host adapters. The harness executes a six-pillar validation pipeline against both live sessions and certified provider fixtures.

### Key Architectural Principles

- **Fail-Closed Execution**: If any check fails (such as malformed telemetry, an unresolved anomaly, an attribution invariant violation, or a detected credential), validation immediately fails with exit code `1`.
- **Structured Internal Data**: All checks query strongly-typed data structures and SQLite queries directly. TokenTree never relies on fragile regex scraping of human-readable terminal output.
- **Privacy-Safe Versioned Reporting**: The optional `--output <PATH>` report conforms to a strict, recursively-validated allowlist schema (`schema_version: "1.0.0"`). It stores only platform metadata, boolean check results, and aggregate integer counters—**never** raw prompts, completions, command output, usernames, home paths, source paths, or API keys.

---

## 2. Command Reference

### Basic Usage

```bash
# Validate a specific installed adapter
tokentree validate claude
tokentree validate codex
tokentree validate grok
tokentree validate hermes

# Validate all four supported host adapters
tokentree validate --all

# Export a versioned allowlisted JSON report
tokentree validate --all --output ./validation-report.json

# Include local non-sensitive token/cost diagnostics in report
tokentree validate --all --output ./validation-report.json --local-details

# Certify a specific fixture file (useful for contributors and CI)
tokentree validate grok --fixture ./fixtures/parsers/grok/multi-turn.json
```

### Exit Codes

| Exit Code | Meaning | Description |
|:---:|:---|:---|
| `0` | **PASS** | All targeted adapters satisfied all six validation pillars. |
| `1` | **FAIL** | One or more validation checks failed (malformed telemetry, database corruption, duplicate requests, privacy violation). |
| `2` | **USAGE ERROR** | Invalid arguments, unknown adapter name, or invalid options. |

---

## 3. The Six Validation Pillars

Every adapter evaluated by `tokentree validate` must pass all six verification pillars:

### Pillar 1: Discovery & Capabilities (`capabilities` & `discovery_passed`)
Probes the host environment safely without running untrusted code:
- **CLI Binary**: Checks if the adapter binary (`claude`, `codex`, `grok`, `hermes`) exists on the system `PATH`.
- **Telemetry Discovery**: Probes standard session storage directories (`~/.claude/projects`, `~/.codex/sessions`, `~/.grok/sessions`, `%LOCALAPPDATA%\hermes` or `~/.hermes`).
- **Configuration**: Verifies that adapter configuration files or directories exist and are readable.
- **Capture Readiness**: Determines if live capture is possible on this machine.

### Pillar 2: Configuration Integrity (`config_passed`)
Validates that adapter settings, environment variables, and directory permissions comply with security policies:
- Asserts that prompt security is enabled.
- Asserts that directory permissions prevent unauthorized read access.
- Confirms that local database configurations (e.g. Hermes SQLite `state.db`) are accessible in read-only mode (`PRAGMA query_only = ON;`).

### Pillar 3: Telemetry Import Verification (`import_passed`)
Ingests telemetry into an isolated, temporary SQLite ledger (`validate.db`) in a sandbox directory:
- Verifies that the adapter's certified provider fixture parses with `malformed == 0`.
- Ingests recent discovered host sessions (if present) to confirm real-world host data compatibility.
- Confirms that duplicate ingestion is idempotent and that checkpoint hashes are preserved.

### Pillar 4: Ledger Invariants & Integrity (`integrity_passed`)
Performs rigorous SQLite database checks on the ingested data:
- Executes `PRAGMA integrity_check` to ensure B-tree and index consistency.
- Executes `PRAGMA foreign_key_check` to assert that all sessions, turns, and usage events maintain valid relational integrity.
- Confirms that all attribution groups maintain the strict 10,000 basis points ($100.00\%$) invariant.

### Pillar 5: Reconciliation (`reconciliation_passed`)
Runs TokenTree's reconciliation engine across the temporary ledger:
- Asserts `duplicate_request_ids == 0`.
- Asserts that no duplicate subagent counters were double counted.
- Asserts that all measurement anomalies are recorded deterministically.

### Pillar 6: Privacy & Secret Audit (`privacy_audit_passed`)
Runs TokenTree's structured security scanner against the database:
- Executes `ledger.audit_prompt_leakage()` to assert 0 prompt/completion payload columns or leaks.
- Scans all model names, request IDs, event IDs, turn titles, and agent IDs for credential patterns (`sk-ant-`, `sk-`, `Bearer `, `xai-`, `ghp_`, API keys).
- Asserts zero privacy violations.

---

## 4. Allowlisted JSON Report Schema (`v1.0.0`)

When `--output <PATH>` is supplied, TokenTree generates a schema-compliant JSON document. The output is recursively validated prior to writing to disk to guarantee that no unauthorized keys or sensitive values are present.

### Schema Specification

```json
{
  "schema_version": "1.0.0",
  "generator": "tokentree validate 0.2.0",
  "timestamp": "2026-10-01T11:00:56.880Z",
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
        "discovery_passed": true,
        "config_passed": true,
        "import_passed": true,
        "integrity_passed": true,
        "reconciliation_passed": true,
        "privacy_audit_passed": true,
        "overall_passed": true
      },
      "counters": {
        "sessions_evaluated": 3,
        "events_ingested": 3,
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
        "models_observed": ["claude-opus-5"],
        "total_cost_micros": 0,
        "measured_turns": 3,
        "unmeasured_turns": 0,
        "completeness_pct": 50.0
      }
    }
  }
}
```

> [!NOTE]
> `local_diagnostics` is included only when the `--local-details` flag is explicitly requested. It contains strictly non-sensitive aggregated token metrics, cost totals, and model IDs.

---

## 5. Contributor Guidance: Certifying a New Adapter

To add and certify a new agent adapter in TokenTree:

1. **Implement Core Parsers**:
   - Provide an implementation in both Rust (`crates/tokentree-<adapter>`) and TypeScript (`packages/adapters/<adapter>`).
   - Implement `discover_sessions(root: &Path) -> Vec<PathBuf>`.
   - Implement idempotent ingestion with SHA-256 checkpoint hashing.
2. **Apply Measurement Truth Ladder**:
   - Prioritize detailed causal request events over cumulative turn/session counters.
   - Suppress cumulative counters when authoritative request events cover the turn.
   - For failed or zero-call requests, record measurements as unavailable (`None` / `null`) rather than false zeroes, emitting deterministic `missing_provider_measurements` anomalies.
3. **Exact Currency Conversion**:
   - Never use floating-point `f64` for dollar conversions.
   - Convert provider costs to integer microdollars ($10^{-6}$ USD) using exact decimal string parsing.
   - Distinguish `AuthoritativeZero` (free models), `MeasuredZero` (explicit zero-cost turns), `AuthoritativeProvider` (positive provider cost), and `UnavailableProviderCost` (paid models needing catalog fallback).
4. **Add Certified Fixtures**:
   - Place sanitized fixtures under `fixtures/parsers/<adapter>/`:
     - Clean multi-turn session.
     - Failed or zero-token error session.
     - Adversarial malformed JSON/JSONL.
     - Missing cost on paid model.
5. **Register in `tokentree validate`**:
   - Register the adapter in `apps/rust-cli/src/validate.rs`.
   - Add integration tests in `apps/rust-cli/tests/validate_test.rs`.
   - Run `tokentree validate <new-adapter>` and verify all six pillars pass with exit code `0`.
