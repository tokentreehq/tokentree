// SPDX-License-Identifier: Apache-2.0
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

#[test]
fn test_validate_all_adapters_cli_passes() {
    let output = std::process::Command::new(bin_path())
        .args(["validate", "--all"])
        .output()
        .expect("execute tokentree validate --all");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "validate --all failed with code {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(stdout.contains("Overall Validation: PASS"));
    assert!(stdout.contains("[claude]"));
    assert!(stdout.contains("[codex]"));
    assert!(stdout.contains("[grok]"));
    assert!(stdout.contains("[hermes]"));
}

#[test]
fn test_validate_individual_adapters_cli() {
    for adapter in &["claude", "codex", "grok", "hermes"] {
        let output = std::process::Command::new(bin_path())
            .args(["validate", adapter])
            .output()
            .unwrap_or_else(|e| panic!("execute tokentree validate {adapter}: {e}"));

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "validate {adapter} failed: {stdout}"
        );
        assert!(stdout.contains(&format!("[{adapter}]")));
        assert!(stdout.contains("Overall Validation: PASS"));
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
fn test_validate_output_report_schema_and_privacy_assertions() {
    let dir = tempdir().unwrap();
    let report_path = dir.path().join("report.json");

    let output = std::process::Command::new(bin_path())
        .args([
            "validate",
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
    let val: serde_json::Value = serde_json::from_str(&content).expect("valid json");

    // Assert strictly allowlisted top-level schema
    assert_eq!(val["schema_version"], "1.0.0");
    assert!(
        val["generator"]
            .as_str()
            .unwrap()
            .starts_with("tokentree validate")
    );
    assert_eq!(val["overall_passed"], true);
    assert!(val["environment"]["os_family"].is_string());
    assert!(val["environment"]["platform"].is_string());
    assert!(val["environment"]["arch"].is_string());

    // Assert adapters present
    let adapters = val["adapters"].as_object().unwrap();
    assert!(adapters.contains_key("claude"));
    assert!(adapters.contains_key("codex"));
    assert!(adapters.contains_key("grok"));
    assert!(adapters.contains_key("hermes"));

    for (_name, ad) in adapters {
        assert!(ad["capabilities"]["cli_installed"].is_boolean());
        assert!(ad["capabilities"]["sessions_discovered"].is_boolean());
        assert!(ad["capabilities"]["config_present"].is_boolean());
        assert!(ad["capabilities"]["capture_available"].is_boolean());

        assert_eq!(ad["checks"]["overall_passed"], true);
        assert_eq!(ad["checks"]["discovery_passed"], true);
        assert_eq!(ad["checks"]["config_passed"], true);
        assert_eq!(ad["checks"]["import_passed"], true);
        assert_eq!(ad["checks"]["integrity_passed"], true);
        assert_eq!(ad["checks"]["reconciliation_passed"], true);
        assert_eq!(ad["checks"]["privacy_audit_passed"], true);

        assert!(ad["counters"]["sessions_evaluated"].as_u64().is_some());
        assert!(ad["counters"]["events_ingested"].as_u64().is_some());
        assert!(ad["counters"]["total_tokens"].as_u64().is_some());
        assert_eq!(ad["counters"]["duplicate_requests"], 0);
        assert_eq!(ad["counters"]["privacy_violations"], 0);

        // Assert local diagnostics
        let diag = &ad["local_diagnostics"];
        assert!(diag["models_observed"].is_array());
        assert!(diag["total_cost_micros"].as_u64().is_some());
        assert!(diag["measured_turns"].as_u64().is_some());
    }

    // Adversarial leak checks: assert report contains no sensitive tokens, keys, or home directories
    let lower = content.to_lowercase();
    assert!(!lower.contains("sk-ant-"));
    assert!(!lower.contains("sk-proj-"));
    assert!(!lower.contains("bearer "));
    assert!(!lower.contains("password"));
    assert!(!lower.contains("\\users\\"));
    assert!(!lower.contains("/home/"));
    assert!(!lower.contains("/users/"));
}
