// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokentree_core::token_completeness;
use tokentree_ledger::Ledger;

pub const VALIDATE_SCHEMA_VERSION: &str = "1.0.0";
pub const VALIDATE_GENERATOR: &str = "tokentree validate 0.2.0";

const FIXTURE_CLAUDE: &str =
    include_str!("../../../fixtures/parsers/claude/public-small-v2.1.80.jsonl");
const FIXTURE_CODEX: &str =
    include_str!("../../../fixtures/parsers/codex/public-small-codex.jsonl");
const FIXTURE_GROK: &str = include_str!("../../../fixtures/parsers/grok/multi-turn.json");
const FIXTURE_HERMES: &str = include_str!("../../../fixtures/parsers/hermes/oneshot-usage.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    Failed,
    Unavailable,
    Skipped,
}

impl std::fmt::Display for ValidationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "PASS"),
            Self::Failed => write!(f, "FAIL"),
            Self::Unavailable => write!(f, "UNAVAILABLE"),
            Self::Skipped => write!(f, "SKIPPED"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOverallStatus {
    Passed,
    Failed,
    Unavailable,
    SelfTestPassed,
}

impl std::fmt::Display for AdapterOverallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "PASS"),
            Self::Failed => write!(f, "FAIL"),
            Self::Unavailable => write!(f, "UNAVAILABLE"),
            Self::SelfTestPassed => write!(f, "SELF-TEST PASS"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMetadata {
    pub os_family: String,
    pub platform: String,
    pub arch: String,
}

impl EnvironmentMetadata {
    pub fn current() -> Self {
        Self {
            os_family: std::env::consts::FAMILY.to_string(),
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub cli_installed: bool,
    pub sessions_discovered: bool,
    pub config_present: bool,
    pub capture_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterChecks {
    pub self_test: ValidationStatus,
    pub discovery: ValidationStatus,
    pub configuration: ValidationStatus,
    pub host_telemetry: ValidationStatus,
    pub ledger_integrity: ValidationStatus,
    pub reconciliation: ValidationStatus,
    pub privacy_audit: ValidationStatus,
    pub live_capture: ValidationStatus,
    pub overall_status: AdapterOverallStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCounters {
    pub sessions_evaluated: u64,
    pub events_ingested: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub anomalies_detected: u64,
    pub duplicate_requests: u64,
    pub privacy_violations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalDiagnostics {
    pub models_observed: Vec<String>,
    pub total_cost_micros: u64,
    pub measured_turns: u64,
    pub unmeasured_turns: u64,
    pub completeness_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterValidationReport {
    pub capabilities: AdapterCapabilities,
    pub checks: AdapterChecks,
    pub counters: AdapterCounters,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_diagnostics: Option<LocalDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationSuiteReport {
    pub schema_version: String,
    pub generator: String,
    pub timestamp: String,
    pub mode: String,
    pub environment: EnvironmentMetadata,
    pub overall_passed: bool,
    pub adapters: BTreeMap<String, AdapterValidationReport>,
}

/// Assert that the validation report JSON parses strictly against typed structs
/// with `#[serde(deny_unknown_fields)]` and passes all semantic and privacy assertions.
pub fn assert_report_schema_and_privacy(report_json: &str) -> Result<ValidationSuiteReport> {
    // 1. Strict typed deserialization with deny_unknown_fields
    let report: ValidationSuiteReport = serde_json::from_str(report_json)
        .context("Validation report failed strict schema validation (unknown or invalid fields)")?;

    // 2. Semantic version assertions
    if report.schema_version != VALIDATE_SCHEMA_VERSION {
        bail!(
            "Invalid schema version: expected '{}', got '{}'",
            VALIDATE_SCHEMA_VERSION,
            report.schema_version
        );
    }
    if !report.generator.starts_with("tokentree validate") {
        bail!("Invalid generator: {}", report.generator);
    }
    if chrono::DateTime::parse_from_rfc3339(&report.timestamp).is_err() {
        bail!("Invalid timestamp format: {}", report.timestamp);
    }

    // 3. Privacy assertions: reject forbidden secret patterns and user home paths
    let forbidden_patterns = [
        "sk-ant-",
        "sk-proj-",
        "sk-",
        "bearer ",
        "xai-",
        "ghp_",
        "gho_",
        "token=",
        "password",
        "authorization",
        "/home/",
        "\\users\\",
        "/users/",
        "canary_prompt_leak",
    ];
    let lower = report_json.to_lowercase();
    for pat in forbidden_patterns {
        if lower.contains(pat) {
            bail!("Privacy audit failure: validation report contains forbidden pattern: {pat}");
        }
    }

    Ok(report)
}

fn which_cli(name: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            #[cfg(windows)]
            {
                let exts = [".exe", ".cmd", ".bat", ".ps1", ""];
                for ext in exts {
                    let candidate = dir.join(format!("{name}{ext}"));
                    if candidate.is_file() {
                        return true;
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

pub struct ValidateOptions {
    pub adapter: Option<String>,
    pub all: bool,
    pub self_test: bool,
    pub require_live: bool,
    pub fixture: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub local_details: bool,
}

pub fn run_validation(options: ValidateOptions) -> Result<bool> {
    let supported_adapters = ["claude", "codex", "grok", "hermes"];

    let target_adapters: Vec<String> = if options.all {
        supported_adapters.iter().map(|s| s.to_string()).collect()
    } else if let Some(a) = &options.adapter {
        let lower = a.trim().to_lowercase();
        if !supported_adapters.contains(&lower.as_str()) {
            bail!(
                "Unknown adapter '{a}'. Supported adapters: {}",
                supported_adapters.join(", ")
            );
        }
        vec![lower]
    } else {
        bail!(
            "Specify an adapter (claude, codex, grok, hermes) or pass --all to validate all adapters."
        );
    };

    let mode_str = if options.self_test {
        "self_test"
    } else if options.require_live {
        "require_live"
    } else {
        "host"
    };

    println!("TokenTree Provider Validation Suite");
    println!("Schema Version: {VALIDATE_SCHEMA_VERSION}");
    println!("Mode: {mode_str}");
    println!(
        "Platform: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Targets: {}\n", target_adapters.join(", "));

    let mut adapter_reports = BTreeMap::new();
    let mut any_failed = false;
    let mut any_unavailable = false;
    let mut any_passed = false;

    for adapter in &target_adapters {
        let rep = validate_single_adapter(adapter, &options)?;
        match rep.checks.overall_status {
            AdapterOverallStatus::Passed | AdapterOverallStatus::SelfTestPassed => {
                any_passed = true;
            }
            AdapterOverallStatus::Unavailable => {
                any_unavailable = true;
            }
            AdapterOverallStatus::Failed => {
                any_failed = true;
            }
        }
        print_adapter_terminal_report(adapter, &rep);
        adapter_reports.insert(adapter.clone(), rep);
    }

    let overall_success = if options.self_test {
        !any_failed
    } else if options.require_live {
        !any_failed && !any_unavailable && any_passed
    } else {
        // Default host mode: fail if any check failed, or if single requested adapter is unavailable,
        // or if --all was requested but 0 providers are present on this machine.
        if any_failed {
            false
        } else if options.adapter.is_some() {
            !any_unavailable
        } else {
            any_passed && !any_failed
        }
    };

    println!("============================================================");
    if overall_success {
        if options.self_test {
            println!(
                "Overall Validation: PASS (all adapter parsers & invariants certified offline)"
            );
        } else {
            println!("Overall Validation: PASS (all targeted host environments satisfied)");
        }
    } else if any_unavailable && !any_failed && !options.self_test {
        println!("Overall Validation: UNAVAILABLE (no active host provider detected)");
        println!(
            "Hint: Run with --self-test to verify adapter parsers and ledger pipelines offline."
        );
    } else {
        println!("Overall Validation: FAIL (one or more verification checks failed)");
    }
    println!("============================================================");

    let suite_report = ValidationSuiteReport {
        schema_version: VALIDATE_SCHEMA_VERSION.to_string(),
        generator: VALIDATE_GENERATOR.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        mode: mode_str.to_string(),
        environment: EnvironmentMetadata::current(),
        overall_passed: overall_success,
        adapters: adapter_reports,
    };

    if let Some(out_path) = &options.output {
        let json_str = serde_json::to_string_pretty(&suite_report)?;
        assert_report_schema_and_privacy(&json_str)?;
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(out_path, json_str)?;
        println!("\nValidation report written to: {}", out_path.display());
    }

    Ok(overall_success)
}

fn validate_single_adapter(
    adapter: &str,
    options: &ValidateOptions,
) -> Result<AdapterValidationReport> {
    let (cli_name, session_dir, config_path) = match adapter {
        "claude" => (
            "claude",
            dirs::home_dir().map(|h| h.join(".claude/projects")),
            dirs::home_dir().map(|h| h.join(".claude")),
        ),
        "codex" => (
            "codex",
            dirs::home_dir().map(|h| h.join(".codex/sessions")),
            dirs::home_dir().map(|h| h.join(".codex")),
        ),
        "grok" => (
            "grok",
            dirs::home_dir().map(|h| h.join(".grok/sessions")),
            dirs::home_dir().map(|h| h.join(".grok")),
        ),
        "hermes" => {
            #[cfg(windows)]
            let h_path = std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("hermes"))
                .or_else(|| dirs::home_dir().map(|h| h.join(".hermes")));
            #[cfg(not(windows))]
            let h_path = dirs::home_dir().map(|h| h.join(".hermes"));

            ("hermes", h_path.clone(), h_path)
        }
        _ => bail!("unsupported adapter: {adapter}"),
    };

    let cli_installed = which_cli(cli_name);
    let sessions_discovered = session_dir
        .as_ref()
        .is_some_and(|p| p.exists() && has_any_telemetry(p));
    let config_present = config_path.as_ref().is_some_and(|p| p.exists());
    let capture_available = cli_installed || sessions_discovered;

    let capabilities = AdapterCapabilities {
        cli_installed,
        sessions_discovered,
        config_present,
        capture_available,
    };

    // 1. Adapter Discovery Check
    let discovery = if cli_installed || sessions_discovered || config_present {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Unavailable
    };

    // 2. Configuration Check
    let configuration = if options.self_test {
        ValidationStatus::Skipped
    } else {
        verify_configuration(adapter, config_path.as_deref())
    };

    // 3. Isolated Ledger Ingestion Sandbox
    let temp_dir = tempdir().context("create temporary validation ledger directory")?;
    let db_path = temp_dir.path().join("validate.db");
    let mut ledger = Ledger::open(&db_path).context("open validation ledger")?;

    let mut sessions_evaluated = 0u64;
    let mut total_anomalies = 0u64;

    // Self-Test on embedded (or custom) fixture
    let (self_test_ok, fixture_anoms) = if let Some(custom) = &options.fixture {
        sessions_evaluated += 1;
        ingest_adapter_telemetry(adapter, &mut ledger, custom)
    } else {
        sessions_evaluated += 1;
        ingest_embedded_fixture(adapter, &mut ledger, temp_dir.path())
    };
    total_anomalies += fixture_anoms;

    let self_test = if self_test_ok {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };

    // 4. Host Telemetry Validation
    let host_telemetry = if options.self_test || options.fixture.is_some() {
        ValidationStatus::Skipped
    } else if !sessions_discovered {
        ValidationStatus::Unavailable
    } else {
        let found = discover_sample_sessions(adapter, session_dir.as_ref().unwrap());
        if found.is_empty() {
            ValidationStatus::Unavailable
        } else {
            let mut all_host_ok = true;
            for p in &found {
                sessions_evaluated += 1;
                let (ok, anoms) = ingest_adapter_telemetry(adapter, &mut ledger, p);
                if !ok {
                    all_host_ok = false;
                }
                total_anomalies += anoms;
            }
            if all_host_ok {
                ValidationStatus::Passed
            } else {
                ValidationStatus::Failed
            }
        }
    };

    // 5. Ledger Invariant & Integrity Checks
    let integrity_ok = check_ledger_integrity(&ledger, adapter);
    let ledger_integrity = if integrity_ok {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };

    // 6. Reconciliation Checks
    let recon = ledger.reconcile().context("run ledger reconciliation")?;
    let reconciliation_ok =
        recon.duplicate_request_ids == 0 && recon.duplicate_subagent_counters == 0;
    let reconciliation = if reconciliation_ok {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };

    // 7. Privacy & Leak Audit
    let prompt_audit = ledger
        .audit_prompt_leakage(None)
        .context("run prompt leakage audit")?;
    let db_leak_free = scan_ledger_strings_for_secrets(&ledger);
    let privacy_violations =
        (prompt_audit.leaks_detected + if db_leak_free { 0 } else { 1 }) as u64;
    let privacy_audit = if privacy_violations == 0 {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };

    // 8. Live Capture Capability State
    let live_capture = if options.self_test {
        ValidationStatus::Skipped
    } else if capture_available {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Unavailable
    };

    // Overall Adapter Status
    let has_any_failure = self_test == ValidationStatus::Failed
        || configuration == ValidationStatus::Failed
        || host_telemetry == ValidationStatus::Failed
        || ledger_integrity == ValidationStatus::Failed
        || reconciliation == ValidationStatus::Failed
        || privacy_audit == ValidationStatus::Failed;

    let overall_status = if has_any_failure {
        AdapterOverallStatus::Failed
    } else if options.self_test {
        AdapterOverallStatus::SelfTestPassed
    } else if options.require_live {
        if discovery == ValidationStatus::Passed
            && (host_telemetry == ValidationStatus::Passed || capture_available)
        {
            AdapterOverallStatus::Passed
        } else {
            AdapterOverallStatus::Failed
        }
    } else {
        // Default host validation
        if discovery == ValidationStatus::Passed
            && (host_telemetry == ValidationStatus::Passed || capture_available)
        {
            AdapterOverallStatus::Passed
        } else {
            AdapterOverallStatus::Unavailable
        }
    };

    let checks = AdapterChecks {
        self_test,
        discovery,
        configuration,
        host_telemetry,
        ledger_integrity,
        reconciliation,
        privacy_audit,
        live_capture,
        overall_status,
    };

    let counters = extract_counters(
        &ledger,
        adapter,
        sessions_evaluated,
        total_anomalies,
        recon.duplicate_request_ids,
        privacy_violations,
    );

    let local_diagnostics = if options.local_details {
        Some(extract_local_diagnostics(&ledger, adapter))
    } else {
        None
    };

    Ok(AdapterValidationReport {
        capabilities,
        checks,
        counters,
        local_diagnostics,
    })
}

fn has_any_telemetry(dir: &Path) -> bool {
    if dir.is_file() {
        return true;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext == "json" || ext == "jsonl" || ext == "db" {
                    return true;
                }
            } else if p.is_dir() {
                if let Ok(sub) = fs::read_dir(&p) {
                    if sub.count() > 0 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn discover_sample_sessions(adapter: &str, root: &Path) -> Vec<PathBuf> {
    match adapter {
        "claude" => tokentree_claude::discover_sessions(root),
        "codex" => tokentree_codex::discover_sessions(root),
        "grok" => tokentree_grok::discover_sessions(root),
        "hermes" => tokentree_hermes::discover_sessions(root),
        _ => Vec::new(),
    }
}

fn verify_configuration(adapter: &str, config_path: Option<&Path>) -> ValidationStatus {
    let path = match config_path {
        Some(p) if p.exists() => p,
        _ => return ValidationStatus::Unavailable,
    };

    match adapter {
        "claude" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return ValidationStatus::Failed; // World-writable config directory
                    }
                }
            }
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(path) {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        return ValidationStatus::Failed;
                    }
                    if content.contains("sk-ant-api") {
                        return ValidationStatus::Failed; // Plaintext secret in config
                    }
                }
            }
            ValidationStatus::Passed
        }
        "codex" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return ValidationStatus::Failed;
                    }
                }
            }
            let config_json = path.join("config.json");
            if config_json.is_file() {
                if let Ok(content) = fs::read_to_string(&config_json) {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        return ValidationStatus::Failed;
                    }
                }
            }
            ValidationStatus::Passed
        }
        "grok" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return ValidationStatus::Failed;
                    }
                }
            }
            ValidationStatus::Passed
        }
        "hermes" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return ValidationStatus::Failed;
                    }
                }
            }
            let state_db = path.join("state.db");
            if state_db.is_file()
                && tokentree_hermes::parse_hermes_state_db(&state_db, None).is_err()
            {
                return ValidationStatus::Failed;
            }
            ValidationStatus::Passed
        }
        _ => ValidationStatus::Unavailable,
    }
}

fn ingest_embedded_fixture(adapter: &str, ledger: &mut Ledger, temp_dir: &Path) -> (bool, u64) {
    let (fixture_content, file_name) = match adapter {
        "claude" => (FIXTURE_CLAUDE, "claude-fixture.jsonl"),
        "codex" => (FIXTURE_CODEX, "codex-fixture.jsonl"),
        "grok" => (FIXTURE_GROK, "grok-fixture.json"),
        "hermes" => (FIXTURE_HERMES, "hermes-fixture.json"),
        _ => return (false, 0),
    };

    let fixture_path = temp_dir.join(file_name);
    if fs::write(&fixture_path, fixture_content).is_err() {
        return (false, 0);
    }

    ingest_adapter_telemetry(adapter, ledger, &fixture_path)
}

fn ingest_adapter_telemetry(adapter: &str, ledger: &mut Ledger, path: &Path) -> (bool, u64) {
    match adapter {
        "claude" => match tokentree_claude::parse_session(path) {
            Ok(parsed) => {
                if parsed.stats.malformed > 0 {
                    return (false, 0);
                }
                match ledger.ingest(parsed.observations) {
                    Ok(summary) => (summary.inserted > 0 || summary.duplicates > 0, 0),
                    Err(_) => (false, 0),
                }
            }
            Err(_) => (false, 0),
        },
        "codex" => match tokentree_codex::import_codex_file(ledger.connection_mut(), path) {
            Ok(res) => (res.inserted > 0 || res.duplicates > 0, res.anomalies),
            Err(_) => (false, 0),
        },
        "grok" => match tokentree_grok::import_grok_file(ledger.connection_mut(), path) {
            Ok(res) => (res.inserted > 0 || res.duplicates > 0, res.anomalies),
            Err(_) => (false, 0),
        },
        "hermes" => match tokentree_hermes::import_hermes_file(ledger.connection_mut(), path) {
            Ok(res) => (res.inserted > 0 || res.duplicates > 0, res.anomalies),
            Err(_) => (false, 0),
        },
        _ => (false, 0),
    }
}

fn check_ledger_integrity(ledger: &Ledger, adapter: &str) -> bool {
    let check = ledger
        .integrity_check()
        .unwrap_or_else(|_| "err".to_string());
    if check != "ok" {
        return false;
    }

    // Foreign keys verification
    let fk_violations: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check()", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    if fk_violations != 0 {
        return false;
    }

    // Checkpoint validation
    if !check_ledger_checkpoints(ledger, adapter) {
        return false;
    }

    // Monotonicity & non-negative counter checks
    if !check_ledger_monotonicity(ledger) {
        return false;
    }

    true
}

fn check_ledger_checkpoints(ledger: &Ledger, adapter: &str) -> bool {
    let mut stmt = match ledger.connection().prepare(
        "SELECT adapter, source_path, file_size, last_offset, file_hash, parser_version FROM ingestion_checkpoints WHERE adapter = ?"
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = match stmt.query_map([adapter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for r in rows.flatten() {
        if r.0 != adapter || r.1.is_empty() || r.2 < 0 || r.3 < 0 {
            return false;
        }
        if let Some(hash) = r.4 {
            if hash.len() != 64 {
                return false;
            }
        }
    }
    true
}

fn check_ledger_monotonicity(ledger: &Ledger) -> bool {
    let mut stmt = match ledger.connection().prepare(
        "SELECT input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, provider_reported_cost_micros FROM usage_events"
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return false,
    };
    for r in rows.flatten() {
        if let Some(t) = r.0 {
            if t < 0 {
                return false;
            }
        }
        if let Some(t) = r.1 {
            if t < 0 {
                return false;
            }
        }
        if let Some(t) = r.2 {
            if t < 0 {
                return false;
            }
        }
        if let Some(t) = r.3 {
            if t < 0 {
                return false;
            }
        }
        if let Some(c) = r.4 {
            if c < 0 {
                return false;
            }
        }
    }
    true
}

fn scan_ledger_strings_for_secrets(ledger: &Ledger) -> bool {
    let forbidden = [
        "sk-ant-",
        "sk-proj-",
        "sk-",
        "bearer ",
        "xai-",
        "ghp_",
        "gho_",
        "canary_prompt_leak",
    ];
    let mut stmt = match ledger.connection().prepare(
        "SELECT coalesce(model, ''), coalesce(request_id, ''), coalesce(turn_id, ''), coalesce(agent_id, '') FROM usage_events"
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    });

    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let combined = format!("{} {} {} {}", r.0, r.1, r.2, r.3).to_lowercase();
            for f in &forbidden {
                if combined.contains(f) {
                    return false;
                }
            }
        }
    }
    true
}

fn extract_counters(
    ledger: &Ledger,
    adapter: &str,
    sessions_evaluated: u64,
    anomalies_detected: u64,
    duplicate_requests: u64,
    privacy_violations: u64,
) -> AdapterCounters {
    let conn = ledger.connection();

    let events_ingested: u64 = conn
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let input_tokens: u64 = conn
        .query_row(
            "SELECT coalesce(sum(input_tokens), 0) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let output_tokens: u64 = conn
        .query_row(
            "SELECT coalesce(sum(output_tokens), 0) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let cached_tokens: u64 = conn
        .query_row(
            "SELECT coalesce(sum(cached_input_tokens), 0) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let reasoning_tokens: u64 = conn
        .query_row(
            "SELECT coalesce(sum(reasoning_tokens), 0) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let total_tokens = input_tokens + output_tokens + cached_tokens + reasoning_tokens;

    AdapterCounters {
        sessions_evaluated,
        events_ingested,
        total_tokens,
        input_tokens,
        output_tokens,
        cached_tokens,
        reasoning_tokens,
        anomalies_detected,
        duplicate_requests,
        privacy_violations,
    }
}

fn extract_local_diagnostics(ledger: &Ledger, adapter: &str) -> LocalDiagnostics {
    let conn = ledger.connection();

    let mut models = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT model FROM usage_events WHERE adapter = ? AND model IS NOT NULL ORDER BY model"
    ) {
        if let Ok(rows) = stmt.query_map([adapter], |r| r.get::<_, String>(0)) {
            for m in rows.flatten() {
                models.push(m);
            }
        }
    }

    let total_cost_micros: u64 = conn
        .query_row(
            "SELECT coalesce(sum(provider_reported_cost_micros), 0) FROM usage_events WHERE adapter = ?",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let measured_turns: u64 = conn
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = ? AND source != 'unavailable'",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let unmeasured_turns: u64 = conn
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = ? AND source = 'unavailable'",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let completeness_pct = token_completeness(measured_turns, unmeasured_turns, 0);

    LocalDiagnostics {
        models_observed: models,
        total_cost_micros,
        measured_turns,
        unmeasured_turns,
        completeness_pct,
    }
}

fn print_adapter_terminal_report(adapter: &str, report: &AdapterValidationReport) {
    println!("[{adapter}]");
    println!("  Capabilities:");
    println!(
        "    CLI Installed:         {}",
        if report.capabilities.cli_installed {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "    Telemetry Discovered:  {}",
        if report.capabilities.sessions_discovered {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "    Config Present:        {}",
        if report.capabilities.config_present {
            "yes"
        } else {
            "no"
        }
    );
    println!("  Checks:");
    println!(
        "    Self-Test (Fixtures):  {} ({} tokens)",
        report.checks.self_test, report.counters.total_tokens
    );
    println!("    Discovery:             {}", report.checks.discovery);
    println!("    Configuration:         {}", report.checks.configuration);
    println!(
        "    Host Telemetry:        {}",
        report.checks.host_telemetry
    );
    println!(
        "    Ledger Integrity:      {}",
        report.checks.ledger_integrity
    );
    println!(
        "    Reconciliation:        {} ({} duplicates, {} anomalies)",
        report.checks.reconciliation,
        report.counters.duplicate_requests,
        report.counters.anomalies_detected
    );
    println!(
        "    Privacy & Secret Audit:{} ({} violations)",
        report.checks.privacy_audit, report.counters.privacy_violations
    );
    println!("    Live Capture:          {}", report.checks.live_capture);
    println!(
        "  Status:                  {}",
        report.checks.overall_status
    );
    println!();
}
