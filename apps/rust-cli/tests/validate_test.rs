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
    self_test: String,
    discovery: String,
    configuration: String,
    host_telemetry: String,
    ledger_integrity: String,
    reconciliation: String,
    privacy_audit: String,
    live_capture: String,
    overall_status: String,
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
    overall_passed: bool,
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
    assert!(stdout.contains("Overall Validation: PASS"));
    assert!(stdout.contains("SELF-TEST PASS"));
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
        assert!(stdout.contains(&format!("[{adapter}]")));
        assert!(stdout.contains("Status:                  SELF-TEST PASS"));
        assert!(stdout.contains("Overall Validation: PASS"));
    }
}

#[test]
fn test_validate_require_live_fails_when_uninstalled() {
    // When requiring live environment, nonexistent or uninstalled adapters must fail closed
    let output = std::process::Command::new(bin_path())
        .args(["validate", "--require-live", "codex"])
        .output()
        .expect("execute validate --require-live codex");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("CLI Installed:         no") {
        assert!(
            !output.status.success(),
            "--require-live must fail when CLI is not installed"
        );
        assert!(stdout.contains("FAIL") || stdout.contains("UNAVAILABLE"));
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
        assert!(
            !output.status.success(),
            "uninstalled provider in host mode must not succeed"
        );
        assert!(stdout.contains("Status:                  UNAVAILABLE"));
        assert!(stdout.contains("Overall Validation: UNAVAILABLE"));
    }
}

#[test]
fn test_validate_unknown_adapter_fails_closed() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "unsupported_llm_tool"])
        .output()
        .expect("execute validate with unknown adapter");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown adapter") || stderr.contains("unsupported_llm_tool"));
}

#[test]
fn test_validate_malformed_fixture_fails_closed() {
    let fixture = repo_root().join("fixtures/parsers/grok/adversarial/corrupted.json");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());

    let output = std::process::Command::new(bin_path())
        .args(["validate", "grok", "--fixture", fixture.to_str().unwrap()])
        .output()
        .expect("execute validate on corrupted fixture");

    assert!(
        !output.status.success(),
        "malformed fixture must fail closed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FAIL"));
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

    assert!(output.status.success());
    assert!(report_path.exists());

    let content = std::fs::read_to_string(&report_path).unwrap();

    // 1. Strict typed deserialization with #[serde(deny_unknown_fields)]
    let report: TestValidationSuiteReport = serde_json::from_str(&content)
        .expect("report must deserialize into typed deny_unknown_fields struct");

    assert_eq!(report.schema_version, "1.0.0");
    assert_eq!(report.mode, "self_test");
    assert!(report.generator.starts_with("tokentree validate"));
    assert!(report.overall_passed);
    assert_eq!(report.adapters.len(), 4);

    for (name, ad) in &report.adapters {
        assert_eq!(ad.checks.self_test, "passed");
        assert_eq!(ad.checks.overall_status, "self_test_passed");
        assert_eq!(ad.counters.duplicate_requests, 0);
        assert_eq!(ad.counters.privacy_violations, 0);
        assert!(ad.counters.total_tokens > 0);

        let diag = ad.local_diagnostics.as_ref().expect("local diagnostics");
        assert!(!diag.models_observed.is_empty(), "models for {name}");
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
