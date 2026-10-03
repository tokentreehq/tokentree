// SPDX-License-Identifier: Apache-2.0
mod dashboard;
mod validate;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokentree_claude::{discover_sessions, parse_session};
use tokentree_core::{MeasurementSource, PriceSnapshot, token_completeness};
use tokentree_ledger::{
    Ledger, ManualCounts, ManualStartInput, add_note, apply_prototype, attach_session,
    detach_session, ensure_session_attribution, ensure_session_attribution_for_source, export_csv,
    export_html, export_json, load_project_trees, merge_work_items, move_work_item,
    preview_prototype, query_ledger, rename_work_item, render_project_trees, session_stable_id,
    split_work_item, start_manual, stop_manual,
};

const DISCLAIMER: &str = "Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.";
const DEFAULT_PRICES_JSON: &str = include_str!("../../../packages/pricing/data/prices.json");

#[derive(Parser)]
#[command(
    name = "tokentree",
    version,
    about = "Local-first AI coding-agent usage measurement"
)]
struct Cli {
    #[arg(long, global = true, env = "TOKENTREE_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum RepairAction {
    /// Repair historical Claude snapshot overcounting (C4): recompute
    /// per-session deltas for `snapshot_delta` rows written by pre-fix
    /// parsers. Backs up every touched row first; idempotent.
    SnapshotOvercount {
        /// Preview what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Required to actually modify the ledger (ignored with --dry-run).
        #[arg(long)]
        yes: bool,
        /// Restore a previous repair run's original rows. With no value,
        /// restores the most recent run.
        #[arg(long, value_name = "RUN_ID")]
        restore: Option<Option<String>>,
    },
    /// Reverse the source_kind vocabulary migration using its backup table.
    /// Backs up nothing further; the migration backup is kept auditable.
    RestoreSourceKindBackup {
        /// Preview how many rows would be restored without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Required to actually modify the ledger (ignored with --dry-run).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum Command {
    Doctor,
    Validate {
        #[arg(value_name = "ADAPTER")]
        adapter: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        self_test: bool,
        #[arg(long)]
        require_live: bool,
        #[arg(long, value_name = "SECONDS")]
        wait: Option<u64>,
        #[arg(long, value_name = "PATH")]
        fixture: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[arg(long)]
        local_details: bool,
    },
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    Report {
        #[arg(long)]
        text: bool,
        #[arg(long)]
        html: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Dashboard {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        no_open: bool,
    },
    Export {
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Query {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        work_item: Option<String>,
        #[arg(long)]
        include_descendants: bool,
    },
    Start {
        #[arg(long)]
        project: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        parent: Option<String>,
    },
    Stop {
        #[arg(long)]
        input: Option<u64>,
        #[arg(long)]
        output: Option<u64>,
        #[arg(long)]
        cache_read: Option<u64>,
        #[arg(long)]
        cache_write: Option<u64>,
        #[arg(long)]
        reasoning: Option<u64>,
        #[arg(long)]
        model: Option<String>,
    },
    Attach {
        #[arg(long)]
        session: String,
        #[arg(long)]
        task: String,
    },
    Detach {
        #[arg(long)]
        session: String,
    },
    Note {
        #[arg(long)]
        text: String,
        #[arg(long)]
        task: Option<String>,
    },
    Rename {
        #[arg(long)]
        task: String,
        #[arg(long)]
        title: String,
    },
    Move {
        #[arg(long)]
        task: String,
        #[arg(long)]
        parent: Option<String>,
    },
    Merge {
        #[arg(long)]
        source: String,
        #[arg(long)]
        target: String,
        /// Preview what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Required to actually modify the ledger (ignored with --dry-run).
        #[arg(long)]
        yes: bool,
    },
    Split {
        #[arg(long)]
        source: String,
        #[arg(long)]
        title: String,
        #[arg(long, value_delimiter = ',')]
        spans: Vec<String>,
        /// Preview what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Required to actually modify the ledger (ignored with --dry-run).
        #[arg(long)]
        yes: bool,
    },
    Classify,
    Reconcile,
    MigratePrototype {
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        apply: bool,
    },
    Repair {
        #[command(subcommand)]
        action: RepairAction,
    },
    OtlpServe {
        #[arg(long, default_value = "127.0.0.1:4318")]
        address: SocketAddr,
    },
    #[command(hide = true)]
    HookEnqueue,
}

#[derive(Subcommand)]
enum ImportSource {
    Claude { path: Option<PathBuf> },
    Codex { path: Option<PathBuf> },
    Grok { path: Option<PathBuf> },
    Hermes { path: Option<PathBuf> },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let home = cli.home.unwrap_or_else(default_home);

    let command = match cli.command {
        Some(cmd) => cmd,
        None => Command::Report {
            text: true,
            html: false,
            project: None,
            out: None,
        },
    };

    match command {
        Command::HookEnqueue => enqueue_hook(&home),
        Command::Doctor => doctor(&home),
        Command::Validate {
            adapter,
            all,
            self_test,
            require_live,
            wait,
            fixture,
            output,
            local_details,
        } => {
            let exit_code = validate::run_validation(validate::ValidateOptions {
                adapter,
                all,
                self_test,
                require_live,
                wait,
                fixture,
                output,
                local_details,
            })?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
            Ok(())
        }
        Command::Import {
            source: ImportSource::Claude { path },
        } => import_claude(&home, path.unwrap_or_else(default_claude_path)),
        Command::Import {
            source: ImportSource::Codex { path },
        } => import_codex(&home, path.unwrap_or_else(default_codex_path)),
        Command::Import {
            source: ImportSource::Grok { path },
        } => import_grok(&home, path.unwrap_or_else(default_grok_path)),
        Command::Import {
            source: ImportSource::Hermes { path },
        } => import_hermes(&home, path.unwrap_or_else(default_hermes_path)),
        Command::Report {
            text,
            html,
            project,
            out,
        } => {
            if html {
                html_report_cmd(&home, project.as_deref(), out)
            } else if text {
                report(&home, project.as_deref())
            } else {
                report(&home, project.as_deref())?;
                println!("\n  Interactive dashboard: run 'tokentree dashboard'");
                println!("  Static HTML report: run 'tokentree report --html'\n");
                Ok(())
            }
        }
        Command::Dashboard { port, no_open } => dashboard_cmd(&home, port, no_open),
        Command::Export {
            format,
            project,
            out,
        } => export_cmd(&home, &format, project.as_deref(), out),
        Command::Query {
            project,
            work_item,
            include_descendants,
        } => query(
            &home,
            project.as_deref(),
            work_item.as_deref(),
            include_descendants,
        ),
        Command::Start {
            project,
            task,
            parent,
        } => start(&home, &project, &task, parent.as_deref()),
        Command::Stop {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
            model,
        } => stop(
            &home,
            ManualCounts {
                input,
                output,
                cache_read,
                cache_write,
                reasoning,
                model,
            },
        ),
        Command::Attach { session, task } => attach(&home, &session, &task),
        Command::Detach { session } => detach(&home, &session),
        Command::Note { text, task } => note(&home, &text, task.as_deref()),
        Command::Rename { task, title } => rename(&home, &task, &title),
        Command::Move { task, parent } => move_item(&home, &task, parent.as_deref()),
        Command::Merge {
            source,
            target,
            dry_run,
            yes,
        } => merge(&home, &source, &target, dry_run, yes),
        Command::Split {
            source,
            title,
            spans,
            dry_run,
            yes,
        } => split(&home, &source, &title, &spans, dry_run, yes),
        Command::Classify => classify(&home),
        Command::Reconcile => reconcile_cmd(&home),
        Command::MigratePrototype {
            source,
            preview,
            apply,
        } => migrate_prototype(&home, source, preview, apply),
        Command::Repair { action } => match action {
            RepairAction::SnapshotOvercount {
                dry_run,
                yes,
                restore,
            } => repair_snapshot_overcount(&home, dry_run, yes, restore),
            RepairAction::RestoreSourceKindBackup { dry_run, yes } => {
                repair_restore_source_kind_backup(&home, dry_run, yes)
            }
        },
        Command::OtlpServe { address } => {
            // H2: the loopback receiver requires a per-run bearer secret so a
            // malicious local process cannot append poisoned rows to the
            // append-only ledger. Pin via TOKENTREE_OTLP_TOKEN for automation.
            let auth = match std::env::var("TOKENTREE_OTLP_TOKEN") {
                Ok(token) if !token.trim().is_empty() => tokentree_otel::OtlpAuth {
                    bearer_token: token,
                },
                _ => tokentree_otel::OtlpAuth::generate(),
            };
            println!("TokenTree OTLP receiver: http://{address}/v1/logs");
            println!("Bearer token (required on every ingest request):");
            println!("  {}\n", auth.bearer_token);
            println!("Point the sender at this endpoint with:");
            println!("  export OTEL_EXPORTER_OTLP_PROTOCOL=http/json");
            println!("  export OTEL_EXPORTER_OTLP_ENDPOINT=http://{address}");
            println!(
                "  export OTEL_EXPORTER_OTLP_HEADERS=\"Authorization=Bearer {}\"",
                auth.bearer_token
            );
            let ledger = ledger(&home)?;
            tokio::runtime::Runtime::new()?.block_on(tokentree_otel::serve(address, ledger, auth))
        }
    }
}

fn effective_home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_home() -> PathBuf {
    std::env::var_os("TOKENTREE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| effective_home_dir().join(".tokentree"))
}

fn default_claude_path() -> PathBuf {
    effective_home_dir().join(".claude").join("projects")
}

fn default_codex_path() -> PathBuf {
    effective_home_dir().join(".codex").join("sessions")
}

fn default_grok_path() -> PathBuf {
    effective_home_dir().join(".grok").join("sessions")
}

fn default_hermes_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") {
            let path = PathBuf::from(local_appdata).join("hermes");
            if path.exists() {
                return path;
            }
        }
    }
    effective_home_dir().join(".hermes")
}

fn ledger(home: &Path) -> Result<Ledger> {
    Ledger::open(home.join("ledger.db"))
}

fn load_snapshot(home: &Path) -> Result<PriceSnapshot> {
    let local = home.join("prices.json");
    if local.exists() {
        if let Ok(s) = PriceSnapshot::load_from_file(&local) {
            return Ok(s);
        }
    }
    PriceSnapshot::load_from_str(DEFAULT_PRICES_JSON)
}

fn doctor(home: &Path) -> Result<()> {
    let ledger = ledger(home)?;
    let claude = default_claude_path();
    let codex = default_codex_path();
    let grok = default_grok_path();
    let hermes = default_hermes_path();
    println!("database: {}", ledger.path().display());
    println!("Claude transcripts: {}", claude.display());
    println!("Codex sessions: {}", codex.display());
    println!("Grok sessions: {}", grok.display());
    println!("Hermes state/sessions: {}", hermes.display());
    println!("TokenTree home: {}", home.display());
    println!(
        "Claude capture mode: {}",
        if claude.exists() {
            "logs (fallback)"
        } else {
            "manual/unavailable"
        }
    );
    println!(
        "Codex capture mode: {}",
        if codex.exists() {
            "rollout/app-server logs"
        } else {
            "manual/unavailable"
        }
    );
    println!(
        "Grok capture mode: {}",
        if grok.exists() {
            "usage.json telemetry"
        } else {
            "manual/unavailable"
        }
    );
    println!(
        "Hermes capture mode: {}",
        if hermes.exists() {
            "state.db / usage-file reports"
        } else {
            "manual/unavailable"
        }
    );
    println!("database integrity: {}", ledger.integrity_check()?);

    // Audit prompt persistence
    println!("prompt persistence: schema forbids prompt/completion/reasoning/tool payload columns");
    let spool_dir = home.join("spool");
    let audit = ledger.audit_prompt_leakage(if spool_dir.exists() {
        Some(&spool_dir)
    } else {
        None
    })?;
    if audit.leaks_detected > 0 {
        eprintln!(
            "prompt leakage: {} leak(s) detected across database and spool records",
            audit.leaks_detected
        );
        for detail in &audit.leak_details {
            eprintln!("  - {detail}");
        }
        bail!("doctor security check failed: prompt/secret leakage detected");
    } else {
        println!("prompt leakage: 0 leaks detected across database and spool records");
    }

    let snapshot = load_snapshot(home);
    match snapshot {
        Ok(s) => {
            println!(
                "pricing status: active snapshot version {} ({} models, SHA-256 verified)",
                s.version,
                s.models.len()
            );
        }
        Err(e) => {
            println!("pricing status: unavailable ({e})");
        }
    }

    Ok(())
}

fn import_claude(home: &Path, root: PathBuf) -> Result<()> {
    let sessions = discover_sessions(&root);
    let mut ledger = ledger(home)?;
    let mut inserted = 0;
    let mut duplicates = 0;
    let mut unknown = 0;
    let mut malformed = 0;
    for path in &sessions {
        let parsed = parse_session(path)?;
        unknown += parsed.stats.unknown;
        malformed += parsed.stats.malformed;
        // Collect session identities before ingest moves the observations.
        let session_keys: HashSet<(String, String)> = parsed
            .observations
            .iter()
            .map(|obs| (obs.adapter.clone(), obs.provider_session_id.clone()))
            .collect();
        // Snapshot generation-mix warning: if the ledger already holds
        // snapshot_delta rows for a session written by a different parser
        // generation than this import, the old cumulative rows and the new
        // delta rows cannot dedup — totals silently double-count. Warn
        // loudly; do NOT block the import.
        {
            let mut snap_sessions: HashSet<String> = HashSet::new();
            let mut incoming_version: Option<&str> = None;
            for obs in parsed
                .observations
                .iter()
                .filter(|o| o.source == MeasurementSource::SnapshotDelta)
            {
                snap_sessions.insert(session_stable_id(&obs.adapter, &obs.provider_session_id));
                incoming_version = Some(obs.parser_version.as_str());
            }
            if let Some(version) = incoming_version {
                let ids: Vec<String> = snap_sessions.into_iter().collect();
                let conflicts = tokentree_ledger::snapshot_generation_conflicts(
                    ledger.connection(),
                    &ids,
                    version,
                )?;
                for session_id in conflicts {
                    eprintln!(
                        "WARNING: session {session_id} already has snapshot_delta rows \
                         from a different parser generation (importing: {version}). \
                         Old cumulative rows and new delta rows cannot dedup by identity, \
                         so re-importing will double-count this session. \
                         Run `tokentree repair snapshot-overcount --dry-run` to inspect \
                         instead of re-importing the file."
                    );
                }
            }
        }
        let summary = ledger.ingest(parsed.observations)?;
        inserted += summary.inserted;
        duplicates += summary.duplicates;
        // Mirror the TypeScript import flow: every imported session gets a
        // default project/work-item attribution so reports render trees
        // instead of "No projects".
        for (adapter, provider_session_id) in &session_keys {
            ensure_session_attribution(
                ledger.connection_mut(),
                &session_stable_id(adapter, provider_session_id),
            )?;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sessions": sessions.len(), "inserted": inserted, "duplicates": duplicates,
            "unknown": unknown, "malformed": malformed,
        }))?
    );
    Ok(())
}

fn import_codex(home: &Path, root: PathBuf) -> Result<()> {
    let sessions = tokentree_codex::discover_sessions(&root);
    let mut ledger = ledger(home)?;
    let mut inserted = 0;
    let mut duplicates = 0;
    let mut anomalies = 0;
    for path in &sessions {
        let res = tokentree_codex::import_codex_file(ledger.connection_mut(), path)?;
        inserted += res.inserted;
        duplicates += res.duplicates;
        anomalies += res.anomalies;
        // Default attribution for every imported session (mirrors the
        // TypeScript import flow and the import_claude path above).
        ensure_session_attribution_for_source(ledger.connection_mut(), "codex", path)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sessions": sessions.len(), "inserted": inserted, "duplicates": duplicates,
            "anomalies": anomalies,
        }))?
    );
    Ok(())
}

fn import_grok(home: &Path, root: PathBuf) -> Result<()> {
    let sessions = tokentree_grok::discover_sessions(&root);
    let mut ledger = ledger(home)?;
    let mut inserted = 0;
    let mut duplicates = 0;
    let mut anomalies = 0;
    for path in &sessions {
        let res = tokentree_grok::import_grok_file(ledger.connection_mut(), path)?;
        inserted += res.inserted;
        duplicates += res.duplicates;
        anomalies += res.anomalies;
        ensure_session_attribution_for_source(ledger.connection_mut(), "grok", path)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sessions": sessions.len(), "inserted": inserted, "duplicates": duplicates,
            "anomalies": anomalies,
        }))?
    );
    Ok(())
}

fn import_hermes(home: &Path, root: PathBuf) -> Result<()> {
    let sessions = tokentree_hermes::discover_sessions(&root);
    let mut ledger = ledger(home)?;
    let mut inserted = 0;
    let mut duplicates = 0;
    let mut anomalies = 0;
    for path in &sessions {
        let res = tokentree_hermes::import_hermes_file(ledger.connection_mut(), path)?;
        inserted += res.inserted;
        duplicates += res.duplicates;
        anomalies += res.anomalies;
        ensure_session_attribution_for_source(ledger.connection_mut(), "hermes", path)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sources": sessions.len(), "inserted": inserted, "duplicates": duplicates,
            "anomalies": anomalies,
        }))?
    );
    Ok(())
}

fn report(home: &Path, project_filter: Option<&str>) -> Result<()> {
    let mut ledger = ledger(home)?;
    if let Ok(snapshot) = load_snapshot(home) {
        let _ = ledger.apply_price_snapshot(&snapshot);
    }

    let trees = load_project_trees(ledger.connection(), project_filter)?;
    let rendered_trees = render_project_trees(&trees);
    println!("{rendered_trees}");

    let usage = ledger.aggregate_usage()?;
    let completeness = token_completeness(usage.measured, usage.unavailable, usage.anomalous);
    println!("\nTokenTree ledger report");
    println!(
        "requests: {} measured {} unavailable {} anomalous {}",
        usage.requests, usage.measured, usage.unavailable, usage.anomalous
    );
    if usage.vocabulary_notices > 0 {
        println!(
            "vocabulary notices: {} unmapped source kinds (informational, not counted against completeness)",
            usage.vocabulary_notices
        );
    }
    println!(
        "tokens: input {} cache-read {} cache-write {} output {} reasoning {}",
        usage.input, usage.cache_read, usage.cache_write, usage.output, usage.reasoning
    );
    println!(
        "completeness: {}",
        completeness.map_or_else(|| "—".into(), |value| format!("{value:.1}%"))
    );
    println!("policy: causal-request");
    println!("\n{DISCLAIMER}");
    Ok(())
}

fn html_report_cmd(home: &Path, project_filter: Option<&str>, out: Option<PathBuf>) -> Result<()> {
    let mut ledger = ledger(home)?;
    if let Ok(snapshot) = load_snapshot(home) {
        let _ = ledger.apply_price_snapshot(&snapshot);
    }
    let trees = load_project_trees(ledger.connection(), project_filter)?;
    let html = export_html(&trees, DISCLAIMER);
    let target = out.unwrap_or_else(|| {
        let reports_dir = home.join("reports");
        let _ = std::fs::create_dir_all(&reports_dir);
        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        reports_dir.join(format!("report-{ts}.html"))
    });
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&target, html)
        .with_context(|| format!("write HTML report to {}", target.display()))?;
    println!("Static HTML report generated: {}", target.display());
    Ok(())
}

fn dashboard_cmd(home: &Path, port: Option<u16>, no_open: bool) -> Result<()> {
    let mut ledger = ledger(home)?;
    if let Ok(snapshot) = load_snapshot(home) {
        let _ = ledger.apply_price_snapshot(&snapshot);
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(dashboard::run_dashboard(home, port, no_open))
}

fn export_cmd(
    home: &Path,
    format: &str,
    project_filter: Option<&str>,
    out: Option<PathBuf>,
) -> Result<()> {
    let mut ledger = ledger(home)?;
    if let Ok(snapshot) = load_snapshot(home) {
        let _ = ledger.apply_price_snapshot(&snapshot);
    }
    let trees = load_project_trees(ledger.connection(), project_filter)?;
    let content = match format.to_lowercase().as_str() {
        "json" => export_json(&trees)?,
        "csv" => export_csv(&trees)?,
        "html" => export_html(&trees, DISCLAIMER),
        other => bail!("unsupported export format '{other}'; supported: json, csv, html"),
    };
    if let Some(target) = out {
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&target, &content)
            .with_context(|| format!("write export to {}", target.display()))?;
        println!("Export written to: {}", target.display());
    } else {
        println!("{content}");
    }
    Ok(())
}

fn query(
    home: &Path,
    project: Option<&str>,
    work_item: Option<&str>,
    include_descendants: bool,
) -> Result<()> {
    let mut ledger = ledger(home)?;
    if let Ok(snapshot) = load_snapshot(home) {
        let _ = ledger.apply_price_snapshot(&snapshot);
    }
    let res = query_ledger(ledger.connection(), project, work_item, include_descendants)?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}

fn start(home: &Path, project: &str, task: &str, parent: Option<&str>) -> Result<()> {
    let mut ledger = ledger(home)?;
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string();
    let res = start_manual(
        ledger.connection_mut(),
        ManualStartInput {
            project_key: project,
            project_title: None,
            task_title: task,
            parent_title: parent,
            cwd: &cwd,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}

fn stop(home: &Path, counts: ManualCounts) -> Result<()> {
    let mut ledger = ledger(home)?;
    let res = stop_manual(ledger.connection_mut(), counts)?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}

fn attach(home: &Path, session: &str, task: &str) -> Result<()> {
    let mut ledger = ledger(home)?;
    let changed = attach_session(ledger.connection_mut(), session, task)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "changed": changed }))?
    );
    Ok(())
}

fn detach(home: &Path, session: &str) -> Result<()> {
    let mut ledger = ledger(home)?;
    let changed = detach_session(ledger.connection_mut(), session)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "changed": changed }))?
    );
    Ok(())
}

fn note(home: &Path, text: &str, task: Option<&str>) -> Result<()> {
    let mut ledger = ledger(home)?;
    let id = add_note(ledger.connection_mut(), text, task)?;
    println!("{}", serde_json::to_string_pretty(&json!({ "id": id }))?);
    Ok(())
}

fn rename(home: &Path, task: &str, title: &str) -> Result<()> {
    let mut ledger = ledger(home)?;
    rename_work_item(ledger.connection_mut(), task, title)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "renamed": true }))?
    );
    Ok(())
}

fn move_item(home: &Path, task: &str, parent: Option<&str>) -> Result<()> {
    let mut ledger = ledger(home)?;
    move_work_item(ledger.connection_mut(), task, parent)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "moved": true }))?
    );
    Ok(())
}

fn merge(home: &Path, source: &str, target: &str, dry_run: bool, yes: bool) -> Result<()> {
    use tokentree_ledger::plan_merge_work_items;
    if dry_run {
        let ledger = ledger(home)?;
        let plan = plan_merge_work_items(ledger.connection(), source, target)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "source": plan.source_id,
                "target": plan.target_id,
                "attribution_groups": plan.attribution_groups,
                "child_work_items": plan.child_work_items,
                "notes": plan.notes,
            }))?
        );
        return Ok(());
    }
    if !yes {
        bail!(
            "refusing to modify the ledger without --yes; use --dry-run to preview the merge first"
        );
    }
    let mut ledger = ledger(home)?;
    let reattributed = merge_work_items(ledger.connection_mut(), source, target)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({ "merged": true, "spans_reattributed": reattributed })
        )?
    );
    Ok(())
}

fn split(
    home: &Path,
    source: &str,
    title: &str,
    spans: &[String],
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    use tokentree_ledger::plan_split_work_item;
    if dry_run {
        let ledger = ledger(home)?;
        let plan = plan_split_work_item(ledger.connection(), source, title, spans)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "source": plan.source_id,
                "new_title": plan.new_title,
                "spans": plan.spans,
            }))?
        );
        return Ok(());
    }
    if !yes {
        bail!(
            "refusing to modify the ledger without --yes; use --dry-run to preview the split first"
        );
    }
    let mut ledger = ledger(home)?;
    let new_id = split_work_item(ledger.connection_mut(), source, title, spans)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "split": true, "new_work_item_id": new_id }))?
    );
    Ok(())
}

fn classify(home: &Path) -> Result<()> {
    let mut ledger = ledger(home)?;
    let spool = home.join("spool/claude-hooks.jsonl");
    let summary = ledger.process_claude_hook_spool(&spool)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn reconcile_cmd(home: &Path) -> Result<()> {
    let ledger = ledger(home)?;
    let res = ledger.reconcile()?;
    println!("sessions: {}", res.sessions);
    println!(
        "duplicate canonical request IDs: {}",
        res.duplicate_request_ids
    );
    println!("unresolved anomalies: {}", res.unresolved_anomalies);
    println!("subagent reconciliation: {}", res.subagent_reconciliation);
    println!(
        "duplicate subagent counters: {}",
        res.duplicate_subagent_counters
    );
    Ok(())
}

fn repair_snapshot_overcount(
    home: &Path,
    dry_run: bool,
    yes: bool,
    restore: Option<Option<String>>,
) -> Result<()> {
    use tokentree_ledger::{
        apply_snapshot_overcount_repair, plan_snapshot_overcount_repair, restore_snapshot_repair,
    };
    let mut ledger = ledger(home)?;
    if let Some(restore_arg) = restore {
        // --restore or --restore <RUN_ID>: put the original rows back.
        let run_id: Option<&str> = restore_arg.as_deref();
        let outcome = restore_snapshot_repair(ledger.connection_mut(), run_id)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "restored": true,
                "run_id": outcome.run_id,
                "rows_restored": outcome.plan.rows,
            }))?
        );
        return Ok(());
    }
    if dry_run {
        let plan = plan_snapshot_overcount_repair(ledger.connection())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "sessions": plan.sessions,
                "rows": plan.rows,
                "baseline_rows": plan.baseline_rows,
                "delta_rows": plan.delta_rows,
                "anomaly_rows": plan.anomaly_rows,
            }))?
        );
        return Ok(());
    }
    if !yes {
        anyhow::bail!(
            "refusing to modify the ledger without --yes; use --dry-run to preview the repair first"
        );
    }
    let outcome = apply_snapshot_overcount_repair(ledger.connection_mut())?;
    if outcome.run_id.is_empty() {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "repaired": false,
                "reason": "snapshot-overcount repair already completed; nothing to do",
            }))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "repaired": true,
                "run_id": outcome.run_id,
                "sessions": outcome.plan.sessions,
                "rows": outcome.plan.rows,
                "baseline_rows": outcome.plan.baseline_rows,
                "delta_rows": outcome.plan.delta_rows,
                "anomaly_rows": outcome.plan.anomaly_rows,
            }))?
        );
    }
    Ok(())
}

fn repair_restore_source_kind_backup(home: &Path, dry_run: bool, yes: bool) -> Result<()> {
    use tokentree_ledger::{count_source_kind_backup_rows, restore_source_kind_backup};
    if dry_run {
        let ledger = ledger(home)?;
        let rows = count_source_kind_backup_rows(ledger.connection())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "rows": rows,
            }))?
        );
        return Ok(());
    }
    if !yes {
        bail!(
            "refusing to modify the ledger without --yes; use --dry-run to preview the restore first"
        );
    }
    let backed_up = {
        let ledger = ledger(home)?;
        count_source_kind_backup_rows(ledger.connection())?
    };
    if backed_up == 0 {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "restored": false,
                "reason": "no source_kind migration backup found; nothing to do",
            }))?
        );
        return Ok(());
    }
    let mut ledger = ledger(home)?;
    let restored = restore_source_kind_backup(ledger.connection_mut())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "restored": true,
            "rows_restored": restored,
        }))?
    );
    Ok(())
}

fn migrate_prototype(
    home: &Path,
    source: Option<PathBuf>,
    preview: bool,
    apply: bool,
) -> Result<()> {
    let src = source.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".task-usage/ledger.json")
    });
    if preview {
        let res = preview_prototype(&src)?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        return Ok(());
    }
    if apply {
        let mut ledger = ledger(home)?;
        let backup_dir = home.join("backups");
        let res = apply_prototype(ledger.connection_mut(), &src, &backup_dir)?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        return Ok(());
    }
    bail!("Choose --preview or --apply")
}

fn enqueue_hook(home: &Path) -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin().take(1_048_577).read_to_end(&mut input)?;
    if input.len() > 1_048_576 {
        bail!("hook input exceeds 1 MiB")
    }
    let value: Value = serde_json::from_slice(&input).context("parse hook JSON")?;
    let payload = sanitize_hook(&value);
    let envelope = json!({"version":1,"kind":value.get("hook_event_name").and_then(Value::as_str).unwrap_or("Unknown"),"capturedAt":Utc::now().to_rfc3339(),"payload":payload});
    let path = home.join("spool/claude-hooks.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    serde_json::to_writer(&mut file, &envelope)?;
    file.write_all(b"\n")?;
    set_mode(&path, 0o600)?;
    Ok(())
}

fn sanitize_hook(value: &Value) -> Value {
    let mut payload = Map::new();
    for key in [
        "session_id",
        "prompt_id",
        "transcript_path",
        "cwd",
        "permission_mode",
        "agent_id",
        "parent_agent_id",
        "task_id",
        "tool_name",
        "tool_use_id",
        "source",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            payload.insert(key.into(), Value::String(text.into()));
        }
    }
    if let Some(prompt) = value.get("prompt").and_then(Value::as_str) {
        payload.insert(
            "prompt_fingerprint".into(),
            Value::String(format!("{:x}", Sha256::digest(prompt.as_bytes()))),
        );
    }
    let event = value.get("hook_event_name").and_then(Value::as_str);
    let tool = value.get("tool_name").and_then(Value::as_str);
    if event == Some("PostToolUse") && matches!(tool, Some("Write" | "Edit" | "MultiEdit")) {
        if let Some(path) = value
            .pointer("/tool_input/file_path")
            .and_then(Value::as_str)
        {
            payload.insert("file_path".into(), Value::String(path.into()));
        }
    }
    Value::Object(payload)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hook_sanitizer_drops_prompt_and_payload() {
        let input = json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"secret full prompt","tool_input":{"content":"source code"}});
        let payload = sanitize_hook(&input);
        let stringified = payload.to_string();
        assert!(!stringified.contains("secret full prompt"));
        assert!(!stringified.contains("source code"));
        assert!(payload.get("prompt_fingerprint").is_some());
    }

    #[test]
    fn full_cli_vertical_slice_workflow() {
        let dir = tempdir().unwrap();
        let home = dir.path().to_path_buf();

        // 1. Doctor runs clean
        doctor(&home).unwrap();

        // 2. Start manual run
        let start_res = start_manual(
            ledger(&home).unwrap().connection_mut(),
            ManualStartInput {
                project_key: "game",
                project_title: Some("Space Game"),
                task_title: "Fix collision bug",
                parent_title: Some("Build playable game"),
                cwd: "/tmp/game",
            },
        )
        .unwrap();
        assert!(!start_res.project_id.is_empty());

        // 3. Stop manual run with counts
        let stop_res = stop_manual(
            ledger(&home).unwrap().connection_mut(),
            ManualCounts {
                input: Some(1_000_000),
                output: Some(1_000_000),
                cache_read: Some(1_000_000),
                cache_write: Some(1_000_000),
                reasoning: None,
                model: Some("claude-sonnet-4-6".into()),
            },
        )
        .unwrap();
        assert_eq!(stop_res.measurement_status, "measured");

        // 4. Query ledger
        let q = query_ledger(
            ledger(&home).unwrap().connection(),
            Some("game"),
            Some("Build playable"),
            true,
        )
        .unwrap();
        assert!(q.get("inclusive").is_some());

        // 5. Note
        let note_res = add_note(
            ledger(&home).unwrap().connection_mut(),
            "Collision bug reproducible on Windows",
            Some(&start_res.work_item_id),
        )
        .unwrap();
        assert!(note_res.starts_with("note_"));

        // 6. Rename
        rename_work_item(
            ledger(&home).unwrap().connection_mut(),
            &start_res.work_item_id,
            "Fix collision bug v2",
        )
        .unwrap();

        // 7. Reconcile
        let rec = ledger(&home).unwrap().reconcile().unwrap();
        assert_eq!(rec.sessions, 1);
        assert_eq!(rec.duplicate_request_ids, 0);

        // 8. Report
        report(&home, Some("game")).unwrap();

        // 9. Static HTML report
        let html_out = home.join("reports/test-report.html");
        html_report_cmd(&home, Some("game"), Some(html_out.clone())).unwrap();
        assert!(html_out.exists());
        let html_content = fs::read_to_string(&html_out).unwrap();
        assert!(html_content.contains("TokenTree"));
        assert!(html_content.contains("Fix collision bug v2"));
        assert!(html_content.contains("Space Game"));
        assert!(html_content.contains(DISCLAIMER));

        // 10. Exports (JSON and CSV)
        let json_out = home.join("export.json");
        export_cmd(&home, "json", Some("game"), Some(json_out.clone())).unwrap();
        assert!(json_out.exists());
        let json_content = fs::read_to_string(&json_out).unwrap();
        assert!(json_content.contains("Space Game"));

        let csv_out = home.join("export.csv");
        export_cmd(&home, "csv", Some("game"), Some(csv_out.clone())).unwrap();
        assert!(csv_out.exists());
        let csv_content = fs::read_to_string(&csv_out).unwrap();
        assert!(csv_content.contains("project_id,project_key"));
        assert!(csv_content.contains("Space Game"));
    }
}
