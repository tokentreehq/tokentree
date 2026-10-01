// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::tempdir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn bin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join("tokentree");
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TestEnvironmentMetadata {
    os_family: String,
    platform: String,
    arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TestAdapterCapabilities {
    cli_installed: bool,
    sessions_discovered: bool,
    config_present: bool,
    capture_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TestAdapterChecks {
    self_test_status: String,
    provider_status: String,
    configuration_status: String,
    telemetry_status: String,
    ledger_integrity_status: String,
    reconciliation_status: String,
    privacy_audit_status: String,
    live_capture_status: String,
    overall_status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TestHostTelemetryFiles {
    attempted: u64,
    verified: u64,
    failed: u64,
    unsupported: u64,
    anomalous: u64,
    duplicate_only: u64,
    empty: u64,
    skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TestAdapterCounters {
    sessions_evaluated: u64,
    events_ingested: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    reasoning_tokens: u64,
    anomalies_detected: u64,
    duplicate_requests: u64,
    privacy_violations: u64,
    host_files: TestHostTelemetryFiles,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct TestLocalDiagnostics {
    models_observed: Vec<String>,
    total_cost_micros: u64,
    measured_turns: u64,
    unmeasured_turns: u64,
    completeness_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct TestAdapterValidationReport {
    capabilities: TestAdapterCapabilities,
    checks: TestAdapterChecks,
    counters: TestAdapterCounters,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_diagnostics: Option<TestLocalDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct TestValidationSuiteReport {
    schema_version: String,
    generator: String,
    timestamp: String,
    mode: String,
    environment: TestEnvironmentMetadata,
    overall_status: String,
    exit_code: i32,
    adapters: BTreeMap<String, TestAdapterValidationReport>,
}

#[test]
fn test_validate_self_test_all_passes() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "--self-test", "--all"])
        .output()
        .expect("execute tokentree validate --self-test --all");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "validate --self-test --all failed with code {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("Overall Status: HEALTHY"));
    assert!(stdout.contains("[claude]"));
    assert!(stdout.contains("[codex]"));
    assert!(stdout.contains("[grok]"));
    assert!(stdout.contains("[hermes]"));
}

#[test]
fn test_validate_self_test_individual_adapters() {
    for adapter in &["claude", "codex", "grok", "hermes"] {
        let output = std::process::Command::new(bin_path())
            .args(["validate", "--self-test", adapter])
            .output()
            .unwrap_or_else(|e| panic!("execute tokentree validate --self-test {adapter}: {e}"));

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "validate --self-test {adapter} failed: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(output.status.code(), Some(0));
        assert!(stdout.contains(&format!("[{adapter}]")));
        assert!(stdout.contains("Self-Test (Fixtures):  passed"));
        assert!(stdout.contains("Overall Status: HEALTHY"));
    }
}

#[test]
fn test_validate_require_live_fails_when_uninstalled() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "--require-live", "codex"])
        .output()
        .expect("execute validate --require-live codex");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("CLI Installed:         no") {
        assert_eq!(
            output.status.code(),
            Some(1),
            "--require-live must fail with code 1 when CLI is not installed"
        );
        assert!(
            stdout.contains("Overall Status: UNAVAILABLE")
                || stdout.contains("Overall Status: FAILED")
        );
    }
}

#[test]
fn test_validate_host_mode_distinguishes_unavailable_from_passed() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "codex"])
        .output()
        .expect("execute validate codex in host mode");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // If Codex CLI is not installed on this machine, host validation must report UNAVAILABLE, not PASS!
    if stdout.contains("CLI Installed:         no") && stdout.contains("Telemetry Discovered:  no")
    {
        assert_eq!(
            output.status.code(),
            Some(3),
            "uninstalled provider in host mode must exit with code 3 (unavailable)"
        );
        assert!(stdout.contains("Provider Status:       unavailable"));
        assert!(stdout.contains("Overall Status: UNAVAILABLE"));
    }
}

#[test]
fn test_validate_unknown_adapter_fails_closed() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "unsupported_llm_tool"])
        .output()
        .expect("execute validate with unknown adapter");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown adapter") || stderr.contains("unsupported_llm_tool"));
}

#[test]
fn test_validate_fixture_with_all_incompatible() {
    let fixture = repo_root().join("fixtures/parsers/grok/multi-turn.json");
    let output = std::process::Command::new(bin_path())
        .args(["validate", "--all", "--fixture", fixture.to_str().unwrap()])
        .output()
        .expect("execute validate --all with --fixture");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--fixture cannot be combined with --all"));
}

#[test]
fn test_validate_malformed_fixture_fails_closed() {
    let fixture = repo_root().join("fixtures/parsers/grok/adversarial/corrupted.json");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());

    let output = std::process::Command::new(bin_path())
        .args(["validate", "grok", "--fixture", fixture.to_str().unwrap()])
        .output()
        .expect("execute validate on corrupted fixture");

    assert_eq!(
        output.status.code(),
        Some(1),
        "malformed fixture must fail with exit code 1"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FAILED"));
}

#[test]
fn test_validate_output_report_schema_and_deny_unknown_fields() {
    let dir = tempdir().unwrap();
    let report_path = dir.path().join("report.json");

    let output = std::process::Command::new(bin_path())
        .args([
            "validate",
            "--self-test",
            "--all",
            "--output",
            report_path.to_str().unwrap(),
            "--local-details",
        ])
        .output()
        .expect("execute validate with output report");

    assert_eq!(output.status.code(), Some(0));
    assert!(report_path.exists());

    let content = std::fs::read_to_string(&report_path).unwrap();

    // 1. Strict typed deserialization with #[serde(deny_unknown_fields)]
    let report: TestValidationSuiteReport = serde_json::from_str(&content)
        .expect("report must deserialize into typed deny_unknown_fields struct");

    assert_eq!(report.schema_version, "1.0.0");
    assert_eq!(report.mode, "self_test");
    assert!(report.generator.starts_with("tokentree validate"));
    assert_eq!(report.overall_status, "healthy");
    assert_eq!(report.exit_code, 0);
    assert_eq!(report.adapters.len(), 4);

    for (name, ad) in &report.adapters {
        assert_eq!(ad.checks.self_test_status, "passed");
        assert_eq!(ad.checks.overall_status, "healthy");
        assert_eq!(ad.counters.duplicate_requests, 0);
        assert_eq!(ad.counters.privacy_violations, 0);
        assert!(ad.counters.total_tokens > 0);

        let diag = ad.local_diagnostics.as_ref().expect("local diagnostics");
        assert!(!diag.models_observed.is_empty(), "models for {name}");
        if let Some(pct) = diag.completeness_pct {
            assert!((0.0..=100.0).contains(&pct));
        }
    }

    // 2. Adversarial unknown field injection test: deny_unknown_fields must reject unknown key
    let mut val: serde_json::Value = serde_json::from_str(&content).unwrap();
    val["unexpected_injected_field"] = serde_json::json!("adversarial_data");
    let injected_str = serde_json::to_string(&val).unwrap();
    let err = serde_json::from_str::<TestValidationSuiteReport>(&injected_str);
    assert!(
        err.is_err(),
        "typed report deserialization must reject unexpected fields"
    );

    // 3. Privacy assertions: report must never contain raw keys, prompts, or home paths
    let lower = content.to_lowercase();
    assert!(!lower.contains("sk-ant-"));
    assert!(!lower.contains("sk-proj-"));
    assert!(!lower.contains("bearer "));
    assert!(!lower.contains("password"));
    assert!(!lower.contains("\\users\\"));
    assert!(!lower.contains("/home/"));
    assert!(!lower.contains("/users/"));
}

#[test]
fn test_telemetry_outcomes_and_counters_verified_and_duplicate_only() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = tokentree_ledger::Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/hermes/oneshot-usage.json");

    // 1. First ingestion -> Outcome: Verified
    let res1 = tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &fixture);
    assert_eq!(
        res1.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::Verified
    );

    // 2. Second ingestion -> Outcome: DuplicateOnly
    let res2 = tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &fixture);
    assert_eq!(
        res2.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::DuplicateOnly
    );
}

#[test]
fn test_telemetry_outcomes_malformed_unsupported_inaccessible_empty() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = tokentree_ledger::Ledger::open(&db_path).unwrap();

    // 1. Inaccessible (non-existent path)
    let missing_path = dir.path().join("does_not_exist.json");
    let res_inacc =
        tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &missing_path);
    assert_eq!(
        res_inacc.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::Inaccessible
    );

    // 2. Empty (0-byte file)
    let empty_path = dir.path().join("empty.json");
    std::fs::write(&empty_path, b"").unwrap();
    let res_empty =
        tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &empty_path);
    assert_eq!(
        res_empty.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::Empty
    );

    // 3. Malformed (corrupted JSON)
    let malformed_path = dir.path().join("malformed.json");
    std::fs::write(&malformed_path, b"{ this is invalid json !!! }").unwrap();
    let res_malformed =
        tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &malformed_path);
    assert_eq!(
        res_malformed.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::Malformed
    );

    // 4. Unsupported Version
    let unsupported_path = dir.path().join("unsupported.json");
    std::fs::write(
        &unsupported_path,
        b"{\"schema_version\": \"99.0.0\", \"unsupported_version\": true}",
    )
    .unwrap();
    let res_unsupported =
        tokentree_cli::validate::ingest_adapter_telemetry("hermes", &mut ledger, &unsupported_path);
    assert_eq!(
        res_unsupported.outcome,
        tokentree_cli::validate::TelemetryImportOutcome::UnsupportedVersion
    );
}

#[test]
fn test_validate_historical_files_do_not_satisfy_require_live() {
    let fixture = repo_root().join("fixtures/parsers/grok/multi-turn.json");

    // Run validate with --require-live and --fixture on a historical fixture
    let output = std::process::Command::new(bin_path())
        .args([
            "validate",
            "--require-live",
            "grok",
            "--fixture",
            fixture.to_str().unwrap(),
        ])
        .output()
        .expect("execute validate --require-live with fixture");

    assert_eq!(
        output.status.code(),
        Some(1),
        "--require-live must fail with code 1 when only historical fixture is evaluated"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Live Capture:          unavailable")
            || stdout.contains("Live Capture:          not_run")
            || stdout.contains("Live Capture:          failed")
    );
    assert!(stdout.contains("Overall Status: FAILED"));
}

#[test]
fn test_validate_cli_installation_alone_produces_degraded_or_unavailable_not_healthy() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "claude"])
        .output()
        .expect("execute validate claude");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("CLI Installed:         yes")
        && stdout.contains("Host Telemetry:        not_found")
    {
        // CLI is installed but no telemetry is verified: status must be DEGRADED or UNAVAILABLE, never HEALTHY!
        assert_eq!(
            output.status.code(),
            Some(3),
            "CLI installed alone must exit with code 3 (unavailable/degraded)"
        );
        assert!(
            stdout.contains("Overall Status: DEGRADED")
                || stdout.contains("Overall Status: UNAVAILABLE")
        );
        assert!(!stdout.contains("Overall Status: HEALTHY"));
    }
}

#[test]
fn test_telemetry_outcomes_all_eight_variants_and_counter_mapping() {
    use tokentree_cli::validate::TelemetryImportOutcome;

    let all_outcomes = [
        TelemetryImportOutcome::Verified,
        TelemetryImportOutcome::DuplicateOnly,
        TelemetryImportOutcome::Malformed,
        TelemetryImportOutcome::UnsupportedVersion,
        TelemetryImportOutcome::Inaccessible,
        TelemetryImportOutcome::Empty,
        TelemetryImportOutcome::Skipped,
        TelemetryImportOutcome::Failed,
    ];

    let mut counters = TestHostTelemetryFiles::default();

    for outcome in all_outcomes {
        counters.attempted += 1;
        match outcome {
            TelemetryImportOutcome::Verified => counters.verified += 1,
            TelemetryImportOutcome::DuplicateOnly => counters.duplicate_only += 1,
            TelemetryImportOutcome::Malformed => counters.failed += 1,
            TelemetryImportOutcome::UnsupportedVersion => counters.unsupported += 1,
            TelemetryImportOutcome::Inaccessible => counters.failed += 1,
            TelemetryImportOutcome::Empty => counters.empty += 1,
            TelemetryImportOutcome::Skipped => counters.skipped += 1,
            TelemetryImportOutcome::Failed => counters.failed += 1,
        }
    }

    assert_eq!(counters.attempted, 8);
    assert_eq!(counters.verified, 1);
    assert_eq!(counters.duplicate_only, 1);
    assert_eq!(counters.failed, 3); // Malformed, Inaccessible, Failed
    assert_eq!(counters.unsupported, 1);
    assert_eq!(counters.empty, 1);
    assert_eq!(counters.skipped, 1);
}

#[test]
fn test_validate_configuration_not_found_prevents_healthy_status() {
    // When validating an adapter without verified host telemetry and configuration,
    // overall status must never be HEALTHY.
    let output = std::process::Command::new(bin_path())
        .args(["validate", "hermes"])
        .output()
        .expect("execute validate hermes");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("Configuration:         not_found") {
        assert!(
            !stdout.contains("Overall Status: HEALTHY"),
            "configuration not_found must never produce HEALTHY overall status"
        );
        assert!(
            stdout.contains("Overall Status: UNAVAILABLE")
                || stdout.contains("Overall Status: DEGRADED")
                || stdout.contains("Overall Status: FAILED")
        );
    }
}
