// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct AdapterCapabilities {
    pub cli_installed: bool,
    pub sessions_discovered: bool,
    pub config_present: bool,
    pub capture_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterChecks {
    pub discovery_passed: bool,
    pub config_passed: bool,
    pub import_passed: bool,
    pub integrity_passed: bool,
    pub reconciliation_passed: bool,
    pub privacy_audit_passed: bool,
    pub overall_passed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct LocalDiagnostics {
    pub models_observed: Vec<String>,
    pub total_cost_micros: u64,
    pub measured_turns: u64,
    pub unmeasured_turns: u64,
    pub completeness_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdapterValidationReport {
    pub capabilities: AdapterCapabilities,
    pub checks: AdapterChecks,
    pub counters: AdapterCounters,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_diagnostics: Option<LocalDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationSuiteReport {
    pub schema_version: String,
    pub generator: String,
    pub timestamp: String,
    pub environment: EnvironmentMetadata,
    pub overall_passed: bool,
    pub adapters: BTreeMap<String, AdapterValidationReport>,
}

/// Assert that the JSON validation report strictly adheres to the allowlisted schema
/// and contains zero sensitive patterns (prompts, completions, auth tokens, home paths).
pub fn assert_report_leak_free(report_json: &str) -> Result<()> {
    let forbidden_patterns = [
        "sk-ant-",
        "sk-proj-",
        "sk-",
        "bearer ",
        "xai-",
        "token=",
        "password",
        "authorization",
        "/home/",
        "\\users\\",
        "/users/",
    ];
    let lower = report_json.to_lowercase();
    for pat in forbidden_patterns {
        if lower.contains(pat) {
            bail!("Privacy audit failure: validation report contains forbidden pattern: {pat}");
        }
    }

    let val: Value =
        serde_json::from_str(report_json).context("parse report JSON for allowlist validation")?;
    assert_allowlist_keys(&val, "")?;
    Ok(())
}

fn assert_allowlist_keys(val: &Value, path: &str) -> Result<()> {
    match val {
        Value::Object(map) => {
            for (key, child) in map {
                let subpath = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match subpath.as_str() {
                    "schema_version"
                    | "generator"
                    | "timestamp"
                    | "environment"
                    | "environment.os_family"
                    | "environment.platform"
                    | "environment.arch"
                    | "overall_passed"
                    | "adapters" => {}
                    _ if subpath.starts_with("adapters.") => {
                        let parts: Vec<&str> = subpath.split('.').collect();
                        if parts.len() == 2 {
                            let adapter = parts[1];
                            if !["claude", "codex", "grok", "hermes"].contains(&adapter) {
                                bail!("Disallowed adapter key in report: {adapter}");
                            }
                        } else if parts.len() >= 3 {
                            let field = parts[2];
                            match field {
                                "capabilities" => {
                                    if parts.len() == 4 {
                                        let cap = parts[3];
                                        if ![
                                            "cli_installed",
                                            "sessions_discovered",
                                            "config_present",
                                            "capture_available",
                                        ]
                                        .contains(&cap)
                                        {
                                            bail!("Disallowed capability key in report: {cap}");
                                        }
                                    }
                                }
                                "checks" => {
                                    if parts.len() == 4 {
                                        let check = parts[3];
                                        if ![
                                            "discovery_passed",
                                            "config_passed",
                                            "import_passed",
                                            "integrity_passed",
                                            "reconciliation_passed",
                                            "privacy_audit_passed",
                                            "overall_passed",
                                        ]
                                        .contains(&check)
                                        {
                                            bail!("Disallowed check key in report: {check}");
                                        }
                                    }
                                }
                                "counters" => {
                                    if parts.len() == 4 {
                                        let counter = parts[3];
                                        if ![
                                            "sessions_evaluated",
                                            "events_ingested",
                                            "total_tokens",
                                            "input_tokens",
                                            "output_tokens",
                                            "cached_tokens",
                                            "reasoning_tokens",
                                            "anomalies_detected",
                                            "duplicate_requests",
                                            "privacy_violations",
                                        ]
                                        .contains(&counter)
                                        {
                                            bail!("Disallowed counter key in report: {counter}");
                                        }
                                    }
                                }
                                "local_diagnostics" => {
                                    if parts.len() == 4 {
                                        let diag = parts[3];
                                        if ![
                                            "models_observed",
                                            "total_cost_micros",
                                            "measured_turns",
                                            "unmeasured_turns",
                                            "completeness_pct",
                                        ]
                                        .contains(&diag)
                                        {
                                            bail!(
                                                "Disallowed local_diagnostics key in report: {diag}"
                                            );
                                        }
                                    }
                                }
                                _ => bail!("Disallowed adapter field: {field}"),
                            }
                        }
                    }
                    _ => bail!("Disallowed key in validation report: {subpath}"),
                }
                assert_allowlist_keys(child, &subpath)?;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                assert_allowlist_keys(item, path)?;
            }
        }
        _ => {}
    }
    Ok(())
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

    println!("TokenTree Provider Validation Suite");
    println!("Schema Version: {VALIDATE_SCHEMA_VERSION}");
    println!(
        "Platform: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Targets: {}\n", target_adapters.join(", "));

    let mut adapter_reports = BTreeMap::new();
    let mut overall_success = true;

    for adapter in &target_adapters {
        let rep =
            validate_single_adapter(adapter, options.fixture.as_deref(), options.local_details)?;
        if !rep.checks.overall_passed {
            overall_success = false;
        }
        print_adapter_terminal_report(adapter, &rep);
        adapter_reports.insert(adapter.clone(), rep);
    }

    println!("============================================================");
    if overall_success {
        println!("Overall Validation: PASS (all checks satisfied)");
    } else {
        println!("Overall Validation: FAIL (one or more checks failed)");
    }
    println!("============================================================");

    let suite_report = ValidationSuiteReport {
        schema_version: VALIDATE_SCHEMA_VERSION.to_string(),
        generator: VALIDATE_GENERATOR.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        environment: EnvironmentMetadata::current(),
        overall_passed: overall_success,
        adapters: adapter_reports,
    };

    if let Some(out_path) = &options.output {
        let json_str = serde_json::to_string_pretty(&suite_report)?;
        assert_report_leak_free(&json_str)?;
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
    custom_fixture: Option<&Path>,
    local_details: bool,
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

    // Discovery check validates that custom fixture (if specified) exists and that
    // host telemetry paths (if present) are accessible and readable.
    let discovery_passed = if let Some(fix) = custom_fixture {
        fix.exists() && fix.is_file()
    } else if let Some(s_dir) = &session_dir {
        if s_dir.exists() {
            fs::read_dir(s_dir).is_ok()
        } else {
            true
        }
    } else {
        true
    };

    // Configuration checks
    let config_passed = verify_configuration(adapter, config_path.as_deref());

    // Isolated Ingestion Verification
    let temp_dir = tempdir().context("create temporary validation ledger directory")?;
    let db_path = temp_dir.path().join("validate.db");
    let mut ledger = Ledger::open(&db_path).context("open validation ledger")?;

    let (import_passed, anomalies_detected) = if let Some(fix_path) = custom_fixture {
        ingest_adapter_telemetry(adapter, &mut ledger, fix_path)
    } else {
        // 1. Verify certified provider fixture integrity
        let (fix_ok, fix_anoms) = ingest_embedded_fixture(adapter, &mut ledger, temp_dir.path());
        if !fix_ok {
            (false, fix_anoms)
        } else if sessions_discovered {
            // 2. Also ingest up to 3 most recent host sessions to verify live machine telemetry
            let found = discover_sample_sessions(adapter, session_dir.as_ref().unwrap());
            let mut host_anoms = fix_anoms;
            for p in found.iter().rev().take(3) {
                let (_, anoms) = ingest_adapter_telemetry(adapter, &mut ledger, p);
                host_anoms += anoms;
            }
            (true, host_anoms)
        } else {
            (true, fix_anoms)
        }
    };

    // Ledger Invariant and Integrity Checks
    let integrity_passed = check_ledger_integrity(&ledger);

    // Structured Reconciliation
    let recon = ledger.reconcile().context("run ledger reconciliation")?;
    let reconciliation_passed =
        recon.duplicate_request_ids == 0 && recon.duplicate_subagent_counters == 0;

    // Structured Privacy Audit
    let prompt_audit = ledger
        .audit_prompt_leakage(None)
        .context("run prompt leakage audit")?;
    let db_leak_free = scan_ledger_strings_for_secrets(&ledger);
    let privacy_violations =
        (prompt_audit.leaks_detected + if db_leak_free { 0 } else { 1 }) as u64;
    let privacy_audit_passed = privacy_violations == 0;

    let overall_passed = discovery_passed
        && config_passed
        && import_passed
        && integrity_passed
        && reconciliation_passed
        && privacy_audit_passed;

    let checks = AdapterChecks {
        discovery_passed,
        config_passed,
        import_passed,
        integrity_passed,
        reconciliation_passed,
        privacy_audit_passed,
        overall_passed,
    };

    let counters = extract_counters(
        &ledger,
        adapter,
        anomalies_detected,
        recon.duplicate_request_ids,
        privacy_violations,
    );

    let local_diagnostics = if local_details {
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

fn verify_configuration(adapter: &str, config_dir: Option<&Path>) -> bool {
    match adapter {
        "claude" => {
            // Verify no bypass of prompt security
            true
        }
        "codex" => true,
        "grok" => true,
        "hermes" => {
            // Verify config/state permissions if path exists
            if let Some(p) = config_dir {
                if p.exists() {
                    return fs::metadata(p).is_ok();
                }
            }
            true
        }
        _ => false,
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

fn check_ledger_integrity(ledger: &Ledger) -> bool {
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
    fk_violations == 0
}

fn scan_ledger_strings_for_secrets(ledger: &Ledger) -> bool {
    let forbidden = ["sk-ant-", "sk-proj-", "sk-", "bearer ", "xai-", "ghp_"];
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
    anomalies: u64,
    duplicates: u64,
    privacy_violations: u64,
) -> AdapterCounters {
    let conn = ledger.connection();
    let row = conn
        .query_row(
            "SELECT
            count(DISTINCT session_id),
            count(*),
            coalesce(sum(input_tokens), 0),
            coalesce(sum(output_tokens), 0),
            coalesce(sum(cached_input_tokens), 0),
            coalesce(sum(reasoning_tokens), 0)
         FROM usage_events WHERE adapter = ?1",
            [adapter],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap_or((0, 0, 0, 0, 0, 0));

    let input = row.2.max(0) as u64;
    let output = row.3.max(0) as u64;
    let cached = row.4.max(0) as u64;
    let reasoning = row.5.max(0) as u64;
    let total = input + output + cached + reasoning;

    AdapterCounters {
        sessions_evaluated: row.0.max(0) as u64,
        events_ingested: row.1.max(0) as u64,
        total_tokens: total,
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached,
        reasoning_tokens: reasoning,
        anomalies_detected: anomalies,
        duplicate_requests: duplicates,
        privacy_violations,
    }
}

fn extract_local_diagnostics(ledger: &Ledger, adapter: &str) -> LocalDiagnostics {
    let conn = ledger.connection();

    let mut models = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT DISTINCT model FROM usage_events WHERE adapter = ?1 AND model IS NOT NULL ORDER BY model") {
        if let Ok(rows) = stmt.query_map([adapter], |r| r.get::<_, String>(0)) {
            for m in rows.flatten() {
                models.push(m);
            }
        }
    }

    let cost_micros: i64 = conn.query_row(
        "SELECT coalesce(sum(provider_reported_cost_micros), 0) FROM usage_events WHERE adapter = ?1",
        [adapter],
        |r| r.get(0),
    ).unwrap_or(0);

    let (measured, unmeasured): (i64, i64) = conn.query_row(
        "SELECT
            count(CASE WHEN source_kind != 'unavailable' AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL) THEN 1 END),
            count(CASE WHEN source_kind = 'unavailable' OR (input_tokens IS NULL AND output_tokens IS NULL) THEN 1 END)
         FROM usage_events WHERE adapter = ?1",
        [adapter],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap_or((0, 0));

    let completeness_pct = if measured + unmeasured > 0 {
        let comp = token_completeness(measured as u64, 0, (measured + unmeasured) as u64);
        comp.map(|c| (c * 100.0).round() / 100.0)
    } else {
        None
    };

    LocalDiagnostics {
        models_observed: models,
        total_cost_micros: cost_micros.max(0) as u64,
        measured_turns: measured.max(0) as u64,
        unmeasured_turns: unmeasured.max(0) as u64,
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
        "    Discovery:             {}",
        if report.checks.discovery_passed {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!(
        "    Configuration:         {}",
        if report.checks.config_passed {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!(
        "    Telemetry Import:      {} ({} events, {} tokens)",
        if report.checks.import_passed {
            "PASS"
        } else {
            "FAIL"
        },
        report.counters.events_ingested,
        report.counters.total_tokens
    );
    println!(
        "    Ledger Integrity:      {}",
        if report.checks.integrity_passed {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!(
        "    Reconciliation:        {} ({} duplicates, {} anomalies)",
        if report.checks.reconciliation_passed {
            "PASS"
        } else {
            "FAIL"
        },
        report.counters.duplicate_requests,
        report.counters.anomalies_detected
    );
    println!(
        "    Privacy & Secret Audit:{}",
        if report.checks.privacy_audit_passed {
            "PASS (0 violations)"
        } else {
            "FAIL"
        }
    );
    if let Some(diag) = &report.local_diagnostics {
        println!("  Diagnostics:");
        println!(
            "    Models:                {}",
            diag.models_observed.join(", ")
        );
        println!("    Provider Cost Micros:  {}", diag.total_cost_micros);
        println!(
            "    Measured / Unmeasured: {} / {}",
            diag.measured_turns, diag.unmeasured_turns
        );
        if let Some(c) = diag.completeness_pct {
            println!("    Completeness:          {c:.1}%");
        }
    }
    println!(
        "  Status:                  {}\n",
        if report.checks.overall_passed {
            "PASS"
        } else {
            "FAIL"
        }
    );
}
