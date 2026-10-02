// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokentree_core::token_completeness;
use tokentree_ledger::Ledger;

pub const VALIDATE_SCHEMA_VERSION: &str = "1.0.0";
pub const VALIDATE_GENERATOR: &str = "tokentree validate 0.2.0";

pub const EXIT_HEALTHY: i32 = 0;
pub const EXIT_VALIDATION_FAILURE: i32 = 1;
pub const EXIT_INVOCATION_ERROR: i32 = 2;
pub const EXIT_UNAVAILABLE: i32 = 3;

const FIXTURE_CLAUDE: &str =
    include_str!("../../../fixtures/parsers/claude/public-small-v2.1.80.jsonl");
const FIXTURE_CODEX: &str =
    include_str!("../../../fixtures/parsers/codex/public-small-codex-clean.jsonl");
const FIXTURE_GROK: &str = include_str!("../../../fixtures/parsers/grok/multi-turn.json");
const FIXTURE_HERMES: &str = include_str!("../../../fixtures/parsers/hermes/oneshot-usage.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfTestStatus {
    Passed,
    Failed,
    NotRun,
}

impl std::fmt::Display for SelfTestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::NotRun => write!(f, "not_run"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Available,
    Unavailable,
    Misconfigured,
}

impl std::fmt::Display for ProviderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Available => write!(f, "available"),
            Self::Unavailable => write!(f, "unavailable"),
            Self::Misconfigured => write!(f, "misconfigured"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryStatus {
    Verified,
    NotFound,
    Unsupported,
    Failed,
    NotRun,
}

impl std::fmt::Display for TelemetryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified => write!(f, "verified"),
            Self::NotFound => write!(f, "not_found"),
            Self::Unsupported => write!(f, "unsupported"),
            Self::Failed => write!(f, "failed"),
            Self::NotRun => write!(f, "not_run"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveCaptureStatus {
    Verified,
    Unavailable,
    Failed,
    NotRun,
}

impl std::fmt::Display for LiveCaptureStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified => write!(f, "verified"),
            Self::Unavailable => write!(f, "unavailable"),
            Self::Failed => write!(f, "failed"),
            Self::NotRun => write!(f, "not_run"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Verified,
    Misconfigured,
    NotFound,
    Failed,
    NotRun,
}

impl std::fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified => write!(f, "verified"),
            Self::Misconfigured => write!(f, "misconfigured"),
            Self::NotFound => write!(f, "not_found"),
            Self::Failed => write!(f, "failed"),
            Self::NotRun => write!(f, "not_run"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallStatus {
    Healthy,
    Degraded,
    Unavailable,
    Failed,
}

impl std::fmt::Display for OverallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unavailable => write!(f, "unavailable"),
            Self::Failed => write!(f, "failed"),
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
    pub self_test_status: SelfTestStatus,
    pub provider_status: ProviderStatus,
    pub configuration_status: CheckStatus,
    pub telemetry_status: TelemetryStatus,
    pub ledger_integrity_status: CheckStatus,
    pub reconciliation_status: CheckStatus,
    pub privacy_audit_status: CheckStatus,
    pub live_capture_status: LiveCaptureStatus,
    pub overall_status: OverallStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryImportOutcome {
    Verified,
    DuplicateOnly,
    Malformed,
    UnsupportedVersion,
    Inaccessible,
    Empty,
    Skipped,
    Failed,
}

impl std::fmt::Display for TelemetryImportOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verified => write!(f, "verified"),
            Self::DuplicateOnly => write!(f, "duplicate_only"),
            Self::Malformed => write!(f, "malformed"),
            Self::UnsupportedVersion => write!(f, "unsupported_version"),
            Self::Inaccessible => write!(f, "inaccessible"),
            Self::Empty => write!(f, "empty"),
            Self::Skipped => write!(f, "skipped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestResult {
    pub outcome: TelemetryImportOutcome,
    pub anomalies: u64,
    #[allow(dead_code)]
    pub latest_timestamp: Option<chrono::DateTime<Utc>>,
    #[allow(dead_code)]
    pub latest_event_identity: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostTelemetryFiles {
    pub attempted: u64,
    pub verified: u64,
    pub failed: u64,
    pub unsupported: u64,
    pub anomalous: u64,
    pub duplicate_only: u64,
    pub empty: u64,
    pub skipped: u64,
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
    pub host_files: HostTelemetryFiles,
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
    pub overall_status: OverallStatus,
    pub exit_code: i32,
    pub adapters: BTreeMap<String, AdapterValidationReport>,
}

/// Assert that the validation report JSON parses strictly against typed structs
/// with `#[serde(deny_unknown_fields)]` and passes all semantic and privacy assertions.
pub fn assert_report_schema_and_privacy(report_json: &str) -> Result<ValidationSuiteReport> {
    // 1. Check byte size limit
    if report_json.len() > 1_000_000 {
        bail!("Report exceeds maximum size limit of 1MB");
    }

    // 2. Strict typed deserialization with deny_unknown_fields
    let report: ValidationSuiteReport = serde_json::from_str(report_json)
        .context("Validation report failed strict schema validation (unknown or invalid fields)")?;

    // 3. Semantic version assertions
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
    if !["self_test", "require_live", "host"].contains(&report.mode.as_str()) {
        bail!("Invalid validation mode: {}", report.mode);
    }

    // 4. Semantic field assertions on each adapter
    for (adapter_name, adapter_rep) in &report.adapters {
        if !["claude", "codex", "grok", "hermes"].contains(&adapter_name.as_str()) {
            bail!("Invalid adapter key in report: {adapter_name}");
        }
        if let Some(diag) = &adapter_rep.local_diagnostics {
            if diag.models_observed.len() > 100 {
                bail!("Models observed array exceeds maximum bound of 100");
            }
            for model in &diag.models_observed {
                if model.len() > 128 {
                    bail!("Model identifier exceeds 128 characters: {model}");
                }
            }
            if let Some(pct) = diag.completeness_pct {
                if !pct.is_finite() || !(0.0..=100.0).contains(&pct) {
                    bail!("Invalid completeness percentage: {pct}");
                }
            }
        }
    }

    // 5. Privacy assertions: reject forbidden secret patterns and user home paths
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
    pub wait: Option<u64>,
    pub fixture: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub local_details: bool,
}

pub fn run_validation(options: ValidateOptions) -> Result<i32> {
    let supported_adapters = ["claude", "codex", "grok", "hermes"];

    // Validate mutually exclusive or incompatible options
    if options.all && options.fixture.is_some() {
        eprintln!("Error: --fixture cannot be combined with --all; specify a single adapter.");
        return Ok(EXIT_INVOCATION_ERROR);
    }

    if let Some(fix_path) = &options.fixture {
        if !fix_path.is_file() {
            eprintln!("Error: Fixture file not found: {}", fix_path.display());
            return Ok(EXIT_INVOCATION_ERROR);
        }
    }

    let target_adapters: Vec<String> = if options.all {
        supported_adapters.iter().map(|s| s.to_string()).collect()
    } else if let Some(a) = &options.adapter {
        let lower = a.trim().to_lowercase();
        if !supported_adapters.contains(&lower.as_str()) {
            eprintln!(
                "Error: Unknown adapter '{a}'. Supported adapters: {}",
                supported_adapters.join(", ")
            );
            return Ok(EXIT_INVOCATION_ERROR);
        }
        vec![lower]
    } else {
        eprintln!(
            "Error: Specify an adapter (claude, codex, grok, hermes) or pass --all to validate all adapters."
        );
        return Ok(EXIT_INVOCATION_ERROR);
    };

    let mode_str = if options.self_test {
        "self_test"
    } else if options.require_live {
        "require_live"
    } else {
        "host"
    };

    println!("TokenTree Truthful Provider Validation Suite");
    println!("Schema Version: {VALIDATE_SCHEMA_VERSION}");
    println!("Mode: {mode_str}");
    println!(
        "Platform: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Targets: {}\n", target_adapters.join(", "));

    let validation_start_time = Utc::now();
    let mut adapter_reports = BTreeMap::new();
    let mut any_failed = false;
    let mut any_unavailable = false;
    let mut any_degraded = false;
    let mut any_healthy = false;
    let mut any_verified_live = false;

    for adapter in &target_adapters {
        let rep = validate_single_adapter(adapter, &options, validation_start_time)?;
        match rep.checks.overall_status {
            OverallStatus::Healthy => {
                any_healthy = true;
            }
            OverallStatus::Degraded => {
                any_degraded = true;
            }
            OverallStatus::Unavailable => {
                any_unavailable = true;
            }
            OverallStatus::Failed => {
                any_failed = true;
            }
        }
        if rep.checks.live_capture_status == LiveCaptureStatus::Verified {
            any_verified_live = true;
        }

        print_adapter_terminal_report(adapter, &rep);
        adapter_reports.insert(adapter.clone(), rep);
    }

    // Determine overall status and exit code
    let (overall_status, exit_code) = if options.self_test {
        if any_failed {
            (OverallStatus::Failed, EXIT_VALIDATION_FAILURE)
        } else {
            (OverallStatus::Healthy, EXIT_HEALTHY)
        }
    } else if options.require_live {
        if any_failed || !any_verified_live || any_unavailable || any_degraded {
            (OverallStatus::Failed, EXIT_VALIDATION_FAILURE)
        } else {
            (OverallStatus::Healthy, EXIT_HEALTHY)
        }
    } else {
        // Default host mode:
        if any_failed {
            (OverallStatus::Failed, EXIT_VALIDATION_FAILURE)
        } else if options.adapter.is_some() {
            // Specific single adapter requested:
            if any_healthy {
                (OverallStatus::Healthy, EXIT_HEALTHY)
            } else if any_degraded {
                (OverallStatus::Degraded, EXIT_UNAVAILABLE)
            } else {
                (OverallStatus::Unavailable, EXIT_UNAVAILABLE)
            }
        } else {
            // --all requested:
            if any_healthy && !any_degraded && !any_unavailable {
                (OverallStatus::Healthy, EXIT_HEALTHY)
            } else if any_healthy {
                (OverallStatus::Degraded, EXIT_HEALTHY)
            } else {
                (OverallStatus::Unavailable, EXIT_UNAVAILABLE)
            }
        }
    };

    println!("============================================================");
    match overall_status {
        OverallStatus::Healthy => {
            if options.self_test {
                println!(
                    "Overall Status: HEALTHY (all adapter parsers & invariants verified offline)"
                );
            } else {
                println!("Overall Status: HEALTHY (targeted environments fully verified)");
            }
        }
        OverallStatus::Degraded => {
            println!("Overall Status: DEGRADED (some optional providers unavailable on host)");
        }
        OverallStatus::Unavailable => {
            println!(
                "Overall Status: UNAVAILABLE (target host provider not detected or not verified)"
            );
            println!(
                "Hint: Run with --self-test to verify adapter parsers and ledger pipelines offline."
            );
        }
        OverallStatus::Failed => {
            println!("Overall Status: FAILED (one or more verification checks failed)");
        }
    }
    println!("Exit Code: {exit_code}");
    println!("============================================================");

    let suite_report = ValidationSuiteReport {
        schema_version: VALIDATE_SCHEMA_VERSION.to_string(),
        generator: VALIDATE_GENERATOR.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        mode: mode_str.to_string(),
        environment: EnvironmentMetadata::current(),
        overall_status,
        exit_code,
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

    Ok(exit_code)
}

fn effective_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

fn validate_single_adapter(
    adapter: &str,
    options: &ValidateOptions,
    validation_start_time: chrono::DateTime<Utc>,
) -> Result<AdapterValidationReport> {
    let home = effective_home_dir();
    let (cli_name, session_dir, config_path) = match adapter {
        "claude" => (
            "claude",
            home.as_ref().map(|h| h.join(".claude").join("projects")),
            home.as_ref().map(|h| h.join(".claude")),
        ),
        "codex" => (
            "codex",
            home.as_ref().map(|h| h.join(".codex").join("sessions")),
            home.as_ref().map(|h| h.join(".codex")),
        ),
        "grok" => (
            "grok",
            home.as_ref().map(|h| h.join(".grok").join("sessions")),
            home.as_ref().map(|h| h.join(".grok")),
        ),
        "hermes" => {
            #[cfg(windows)]
            let h_path = std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("hermes"))
                .or_else(|| home.as_ref().map(|h| h.join(".hermes")));
            #[cfg(not(windows))]
            let h_path = home.as_ref().map(|h| h.join(".hermes"));

            ("hermes", h_path.clone(), h_path)
        }
        _ => bail!("unsupported adapter: {adapter}"),
    };

    let cli_installed = which_cli(cli_name);
    let mut sessions_discovered = session_dir
        .as_ref()
        .is_some_and(|p| p.exists() && has_any_telemetry(p));
    let mut config_present = config_path.as_ref().is_some_and(|p| p.exists());

    // 1. Adapter Parser Self-Test on versioned regression fixture
    let temp_dir = tempdir().context("create temporary validation ledger directory")?;
    let db_path = temp_dir.path().join("validate.db");
    let mut ledger = Ledger::open(&db_path).context("open validation ledger")?;

    let mut sessions_evaluated = 0u64;
    let mut total_anomalies = 0u64;

    let (self_test_ok, fixture_anoms) = if let Some(custom) = &options.fixture {
        sessions_evaluated += 1;
        let r = ingest_adapter_telemetry(adapter, &mut ledger, custom);
        (
            r.outcome == TelemetryImportOutcome::Verified
                || r.outcome == TelemetryImportOutcome::DuplicateOnly,
            r.anomalies,
        )
    } else {
        sessions_evaluated += 1;
        let r = ingest_embedded_fixture(adapter, &mut ledger, temp_dir.path());
        (
            r.outcome == TelemetryImportOutcome::Verified
                || r.outcome == TelemetryImportOutcome::DuplicateOnly,
            r.anomalies,
        )
    };
    total_anomalies += fixture_anoms;

    let self_test_status = if self_test_ok {
        SelfTestStatus::Passed
    } else {
        SelfTestStatus::Failed
    };

    // 2. Provider Installation & Discovery Check
    let mut provider_status = if options.self_test {
        ProviderStatus::Unavailable
    } else if cli_installed || sessions_discovered {
        ProviderStatus::Available
    } else {
        ProviderStatus::Unavailable
    };

    // 3. Configuration Check
    let mut configuration_status = if options.self_test {
        CheckStatus::NotRun
    } else {
        verify_configuration(adapter, config_path.as_deref())
    };

    // Snapshot initial event identities before validation/polling
    let initial_identities = snapshot_event_identities(adapter, session_dir.as_deref());

    // 4. Host Telemetry Validation (never let fixture success override host telemetry failure!)
    let mut host_files = HostTelemetryFiles::default();
    let mut fresh_event_observed = false;
    let wait_secs = if options.require_live {
        options.wait.unwrap_or(0)
    } else {
        0
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);

    let should_poll_live = options.require_live && (cli_installed || sessions_discovered);

    let telemetry_status = if options.self_test || options.fixture.is_some() {
        TelemetryStatus::NotRun
    } else if (!options.require_live && !sessions_discovered)
        || (options.require_live && !should_poll_live)
    {
        TelemetryStatus::NotFound
    } else {
        loop {
            // Recheck directory existence and telemetry discovery every polling interval
            if let Some(s_dir) = &session_dir {
                if s_dir.exists() && has_any_telemetry(s_dir) {
                    sessions_discovered = true;
                }
            }
            if let Some(c_dir) = &config_path {
                if c_dir.exists() {
                    config_present = true;
                }
            }

            let found = if let Some(s_dir) = &session_dir {
                if s_dir.exists() {
                    discover_sample_sessions(adapter, s_dir)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            if found.is_empty() {
                if std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                break TelemetryStatus::NotFound;
            }

            host_files = HostTelemetryFiles::default();
            for p in &found {
                host_files.attempted += 1;
                sessions_evaluated += 1;
                let res = ingest_adapter_telemetry(adapter, &mut ledger, p);
                match res.outcome {
                    TelemetryImportOutcome::Verified => host_files.verified += 1,
                    TelemetryImportOutcome::DuplicateOnly => host_files.duplicate_only += 1,
                    TelemetryImportOutcome::Malformed => host_files.failed += 1,
                    TelemetryImportOutcome::UnsupportedVersion => host_files.unsupported += 1,
                    TelemetryImportOutcome::Inaccessible => host_files.failed += 1,
                    TelemetryImportOutcome::Empty => host_files.empty += 1,
                    TelemetryImportOutcome::Skipped => host_files.skipped += 1,
                    TelemetryImportOutcome::Failed => host_files.failed += 1,
                }
                if res.anomalies > 0 {
                    host_files.anomalous += 1;
                }
                total_anomalies += res.anomalies;

                // Inspect observations to verify a newly created provider event identity
                let current_events = match adapter {
                    "claude" => tokentree_claude::parse_session(p)
                        .ok()
                        .map(|r| r.observations),
                    "codex" => tokentree_codex::parse_session(p)
                        .ok()
                        .map(|r| r.observations),
                    "grok" => tokentree_grok::parse_session(p)
                        .ok()
                        .map(|r| r.observations),
                    "hermes" => tokentree_hermes::parse_session(p)
                        .ok()
                        .map(|r| r.observations),
                    _ => None,
                };

                if let Some(events) = current_events {
                    for obs in events {
                        let id = obs.canonical_identity();
                        if !initial_identities.contains(&id) {
                            let authoritative_ts = obs
                                .source_timestamp
                                .as_deref()
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|dt| dt.with_timezone(&Utc));

                            // File mtime alone must never prove live capture.
                            // Authoritative timestamp must be >= validation_start_time (allowing up to 30s bounded clock skew)
                            let is_fresh = match authoritative_ts {
                                Some(ts) => {
                                    ts >= validation_start_time - chrono::Duration::seconds(30)
                                }
                                None => false,
                            };
                            if is_fresh {
                                fresh_event_observed = true;
                                sessions_discovered = true;
                            }
                        }
                    }
                }
            }

            if fresh_event_observed || std::time::Instant::now() >= deadline {
                break if host_files.failed > 0 {
                    TelemetryStatus::Failed
                } else if host_files.unsupported > 0 {
                    TelemetryStatus::Unsupported
                } else if host_files.verified > 0 || host_files.duplicate_only > 0 {
                    TelemetryStatus::Verified
                } else if host_files.empty > 0 || host_files.skipped > 0 {
                    TelemetryStatus::NotFound
                } else {
                    TelemetryStatus::Failed
                };
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    };

    if !options.self_test {
        configuration_status = verify_configuration(adapter, config_path.as_deref());
        provider_status = if cli_installed || sessions_discovered {
            ProviderStatus::Available
        } else {
            ProviderStatus::Unavailable
        };
    }

    let capabilities = AdapterCapabilities {
        cli_installed,
        sessions_discovered,
        config_present,
        capture_available: cli_installed || sessions_discovered,
    };

    // 5. Ledger Invariant & Integrity Checks
    let integrity_ok = check_ledger_integrity(&ledger, adapter);
    let ledger_integrity_status = if integrity_ok {
        CheckStatus::Verified
    } else {
        CheckStatus::Failed
    };

    // 6. Reconciliation Checks
    let recon = ledger.reconcile().context("run ledger reconciliation")?;
    let reconciliation_ok =
        recon.duplicate_request_ids == 0 && recon.duplicate_subagent_counters == 0;
    let reconciliation_status = if reconciliation_ok {
        CheckStatus::Verified
    } else {
        CheckStatus::Failed
    };

    // 7. Privacy & Leak Audit
    let prompt_audit = ledger
        .audit_prompt_leakage(None)
        .context("run prompt leakage audit")?;
    let db_leak_free = scan_ledger_strings_for_secrets(&ledger);
    let privacy_violations =
        (prompt_audit.leaks_detected + if db_leak_free { 0 } else { 1 }) as u64;
    let privacy_audit_status = if privacy_violations == 0 {
        CheckStatus::Verified
    } else {
        CheckStatus::Failed
    };

    // 8. Live Capture Capability State
    let live_capture_status = if options.self_test {
        LiveCaptureStatus::NotRun
    } else if fresh_event_observed && telemetry_status == TelemetryStatus::Verified {
        LiveCaptureStatus::Verified
    } else if options.require_live {
        LiveCaptureStatus::Failed
    } else {
        LiveCaptureStatus::Unavailable
    };

    // Overall Adapter Status
    let has_any_failure = self_test_status == SelfTestStatus::Failed
        || configuration_status == CheckStatus::Failed
        || configuration_status == CheckStatus::Misconfigured
        || telemetry_status == TelemetryStatus::Failed
        || ledger_integrity_status == CheckStatus::Failed
        || reconciliation_status == CheckStatus::Failed
        || privacy_audit_status == CheckStatus::Failed
        || host_files.failed > 0; // Any malformed discovered telemetry prevents healthy!

    let overall_status = if has_any_failure {
        OverallStatus::Failed
    } else if options.self_test {
        OverallStatus::Healthy
    } else if options.require_live {
        if live_capture_status == LiveCaptureStatus::Verified
            && provider_status == ProviderStatus::Available
            && configuration_status == CheckStatus::Verified
            && telemetry_status == TelemetryStatus::Verified
        {
            OverallStatus::Healthy
        } else {
            OverallStatus::Failed
        }
    } else {
        // Default host validation
        if host_files.unsupported > 0 {
            // Mixed or unsupported telemetry must produce degraded
            OverallStatus::Degraded
        } else if provider_status == ProviderStatus::Available
            && configuration_status == CheckStatus::Verified
            && telemetry_status == TelemetryStatus::Verified
        {
            OverallStatus::Healthy
        } else if provider_status == ProviderStatus::Available {
            // CLI installed or provider available alone produces degraded
            OverallStatus::Degraded
        } else {
            OverallStatus::Unavailable
        }
    };

    let checks = AdapterChecks {
        self_test_status,
        provider_status,
        configuration_status,
        telemetry_status,
        ledger_integrity_status,
        reconciliation_status,
        privacy_audit_status,
        live_capture_status,
        overall_status,
    };

    let counters = extract_counters(
        &ledger,
        adapter,
        sessions_evaluated,
        total_anomalies,
        recon.duplicate_request_ids,
        privacy_violations,
        host_files,
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

fn snapshot_event_identities(
    adapter: &str,
    session_dir: Option<&Path>,
) -> std::collections::BTreeSet<String> {
    let mut identities = std::collections::BTreeSet::new();
    let dir = match session_dir {
        Some(d) if d.exists() => d,
        _ => return identities,
    };
    let files = discover_sample_sessions(adapter, dir);
    for p in files {
        match adapter {
            "claude" => {
                if let Ok(res) = tokentree_claude::parse_session(&p) {
                    for obs in res.observations {
                        identities.insert(obs.canonical_identity());
                    }
                }
            }
            "codex" => {
                if let Ok(res) = tokentree_codex::parse_session(&p) {
                    for obs in res.observations {
                        identities.insert(obs.canonical_identity());
                    }
                }
            }
            "grok" => {
                if let Ok(res) = tokentree_grok::parse_session(&p) {
                    for obs in res.observations {
                        identities.insert(obs.canonical_identity());
                    }
                }
            }
            "hermes" => {
                if let Ok(res) = tokentree_hermes::parse_session(&p) {
                    for obs in res.observations {
                        identities.insert(obs.canonical_identity());
                    }
                }
            }
            _ => {}
        }
    }
    identities
}

fn verify_configuration(adapter: &str, config_path: Option<&Path>) -> CheckStatus {
    let path = match config_path {
        Some(p) if p.exists() => p,
        _ => return CheckStatus::NotFound,
    };

    match adapter {
        "claude" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return CheckStatus::Misconfigured; // World-writable config directory
                    }
                }
            }
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(path) {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        return CheckStatus::Misconfigured;
                    }
                    if content.contains("sk-ant-api") {
                        return CheckStatus::Misconfigured; // Plaintext secret in config
                    }
                }
            }
            CheckStatus::Verified
        }
        "codex" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return CheckStatus::Misconfigured;
                    }
                }
            }
            let config_json = path.join("config.json");
            if config_json.is_file() {
                if let Ok(content) = fs::read_to_string(&config_json) {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        return CheckStatus::Misconfigured;
                    }
                }
            }
            CheckStatus::Verified
        }
        "grok" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return CheckStatus::Misconfigured;
                    }
                }
            }
            let config_json = path.join("config.json");
            if config_json.is_file() {
                if let Ok(content) = fs::read_to_string(&config_json) {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        return CheckStatus::Misconfigured;
                    }
                }
            }
            CheckStatus::Verified
        }
        "hermes" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(path) {
                    if meta.permissions().mode() & 0o002 != 0 {
                        return CheckStatus::Misconfigured;
                    }
                }
            }
            let state_db = path.join("state.db");
            if state_db.is_file()
                && tokentree_hermes::parse_hermes_state_db(&state_db, None).is_err()
            {
                return CheckStatus::Misconfigured;
            }
            CheckStatus::Verified
        }
        _ => CheckStatus::NotFound,
    }
}

pub fn ingest_embedded_fixture(
    adapter: &str,
    ledger: &mut Ledger,
    temp_dir: &Path,
) -> IngestResult {
    let (fixture_content, file_name) = match adapter {
        "claude" => (FIXTURE_CLAUDE, "claude-fixture.jsonl"),
        "codex" => (FIXTURE_CODEX, "codex-fixture.jsonl"),
        "grok" => (FIXTURE_GROK, "grok-fixture.json"),
        "hermes" => (FIXTURE_HERMES, "hermes-fixture.json"),
        _ => {
            return IngestResult {
                outcome: TelemetryImportOutcome::Failed,
                anomalies: 0,
                latest_timestamp: None,
                latest_event_identity: None,
            };
        }
    };

    let fixture_path = temp_dir.join(file_name);
    if fs::write(&fixture_path, fixture_content).is_err() {
        return IngestResult {
            outcome: TelemetryImportOutcome::Failed,
            anomalies: 0,
            latest_timestamp: None,
            latest_event_identity: None,
        };
    }

    ingest_adapter_telemetry(adapter, ledger, &fixture_path)
}

pub fn ingest_adapter_telemetry(adapter: &str, ledger: &mut Ledger, path: &Path) -> IngestResult {
    // 1. Accessibility check
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => {
            return IngestResult {
                outcome: TelemetryImportOutcome::Inaccessible,
                anomalies: 0,
                latest_timestamp: None,
                latest_event_identity: None,
            };
        }
    };

    // 2. Empty check
    if meta.len() == 0 {
        return IngestResult {
            outcome: TelemetryImportOutcome::Empty,
            anomalies: 0,
            latest_timestamp: None,
            latest_event_identity: None,
        };
    }

    let is_sqlite = path.extension().is_some_and(|e| e == "db" || e == "sqlite");
    let before_hash = if !is_sqlite {
        fs::read(path).ok().map(|b| hex::encode(Sha256::digest(&b)))
    } else {
        None
    };
    let before_size = meta.len();

    let path_str = path.to_string_lossy().to_string();
    let had_checkpoint = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE source_path = ?1",
            [&path_str],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    let res = match adapter {
        "claude" => tokentree_claude::import_claude_file(ledger.connection_mut(), path),
        "codex" => tokentree_codex::import_codex_file(ledger.connection_mut(), path),
        "grok" => tokentree_grok::import_grok_file(ledger.connection_mut(), path),
        "hermes" => tokentree_hermes::import_hermes_file(ledger.connection_mut(), path),
        _ => {
            return IngestResult {
                outcome: TelemetryImportOutcome::Failed,
                anomalies: 0,
                latest_timestamp: None,
                latest_event_identity: None,
            };
        }
    };

    let mut result = match res {
        Ok(import_res) => {
            let outcome = if import_res.malformed > 0 {
                TelemetryImportOutcome::Malformed
            } else if import_res.unsupported > 0 {
                TelemetryImportOutcome::UnsupportedVersion
            } else if import_res.inserted > 0 {
                TelemetryImportOutcome::Verified
            } else if import_res.duplicates > 0 || had_checkpoint {
                TelemetryImportOutcome::DuplicateOnly
            } else if import_res.anomalies == 0 && import_res.start_offset == import_res.end_offset
            {
                TelemetryImportOutcome::Skipped
            } else {
                TelemetryImportOutcome::Verified
            };
            IngestResult {
                outcome,
                anomalies: import_res.anomalies,
                latest_timestamp: import_res.latest_authoritative_timestamp,
                latest_event_identity: import_res.latest_event_identity,
            }
        }
        Err(_) => IngestResult {
            outcome: TelemetryImportOutcome::Failed,
            anomalies: 0,
            latest_timestamp: None,
            latest_event_identity: None,
        },
    };

    // Verify source was not altered by ingestion for non-SQLite files
    if let Some(b_hash) = before_hash {
        let after_hash = fs::read(path).ok().map(|b| hex::encode(Sha256::digest(&b)));
        let after_size = fs::metadata(path).ok().map(|m| m.len());
        if after_hash != Some(b_hash) || after_size != Some(before_size) {
            result = IngestResult {
                outcome: TelemetryImportOutcome::Failed,
                anomalies: result.anomalies + 1,
                latest_timestamp: None,
                latest_event_identity: None,
            };
        }
    }

    result
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

    // Attribution invariants
    if !check_ledger_attribution_invariants(ledger) {
        return false;
    }

    // Deduplication check
    if !check_ledger_deduplication(ledger) {
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

fn check_ledger_attribution_invariants(ledger: &Ledger) -> bool {
    let missing_sessions: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE session_id IS NULL OR session_id = ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if missing_sessions != 0 {
        return false;
    }

    let orphan_subagents: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE parent_agent_id IS NOT NULL AND (agent_id IS NULL OR agent_id = '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if orphan_subagents != 0 {
        return false;
    }

    true
}

fn check_ledger_deduplication(ledger: &Ledger) -> bool {
    let duplicate_requests: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM (SELECT request_id FROM usage_events WHERE request_id IS NOT NULL AND source_kind NOT IN ('hermes_snapshot_delta', 'snapshot_delta') GROUP BY request_id HAVING count(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    duplicate_requests == 0
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
        "SELECT coalesce(model, ''), coalesce(request_id, ''), coalesce(turn_id, ''), coalesce(agent_id, ''), coalesce(parent_agent_id, '') FROM usage_events"
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
            r.get::<_, String>(4)?,
        ))
    });

    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let combined = format!("{} {} {} {} {}", r.0, r.1, r.2, r.3, r.4).to_lowercase();
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
    host_files: HostTelemetryFiles,
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
        host_files,
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
            "SELECT count(*) FROM usage_events WHERE adapter = ? AND source_kind NOT IN ('grok_turn_failed', 'grok_session_failed', 'hermes_failed_run', 'hermes_unmeasured')",
            [adapter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let unmeasured_turns: u64 = conn
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = ? AND source_kind IN ('grok_turn_failed', 'grok_session_failed', 'hermes_failed_run', 'hermes_unmeasured')",
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
        report.checks.self_test_status, report.counters.total_tokens
    );
    println!(
        "    Provider Status:       {}",
        report.checks.provider_status
    );
    println!(
        "    Configuration:         {}",
        report.checks.configuration_status
    );
    println!(
        "    Host Telemetry:        {} ({} attempted, {} verified, {} failed, {} duplicate, {} empty, {} skipped)",
        report.checks.telemetry_status,
        report.counters.host_files.attempted,
        report.counters.host_files.verified,
        report.counters.host_files.failed,
        report.counters.host_files.duplicate_only,
        report.counters.host_files.empty,
        report.counters.host_files.skipped
    );
    println!(
        "    Ledger Integrity:      {}",
        report.checks.ledger_integrity_status
    );
    println!(
        "    Reconciliation:        {} ({} duplicates, {} anomalies)",
        report.checks.reconciliation_status,
        report.counters.duplicate_requests,
        report.counters.anomalies_detected
    );
    println!(
        "    Privacy & Secret Audit:{} ({} violations)",
        report.checks.privacy_audit_status, report.counters.privacy_violations
    );
    println!(
        "    Live Capture:          {}",
        report.checks.live_capture_status
    );
    println!(
        "  Overall Status:          {}",
        report.checks.overall_status
    );
    println!();
}
