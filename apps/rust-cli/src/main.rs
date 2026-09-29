// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokentree_claude::{discover_sessions, parse_session};
use tokentree_core::token_completeness;
use tokentree_ledger::Ledger;

const DISCLAIMER: &str = "Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.";

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
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Doctor,
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    Report {
        #[arg(long)]
        text: bool,
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
    match cli.command {
        Command::HookEnqueue => enqueue_hook(&home),
        Command::Doctor => doctor(&home),
        Command::Import {
            source: ImportSource::Claude { path },
        } => import_claude(&home, path.unwrap_or_else(default_claude_path)),
        Command::Report { text } => {
            if !text {
                bail!("use --text; it never binds a port")
            }
            report(&home)
        }
        Command::OtlpServe { address } => {
            println!("TokenTree OTLP receiver: http://{address}/v1/logs");
            println!(
                "Set OTEL_EXPORTER_OTLP_PROTOCOL=http/json and point Claude Code logs to this loopback endpoint."
            );
            let ledger = ledger(&home)?;
            tokio::runtime::Runtime::new()?.block_on(tokentree_otel::serve(address, ledger))
        }
    }
}

fn default_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokentree")
}
fn default_claude_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude/projects")
}
fn ledger(home: &Path) -> Result<Ledger> {
    Ledger::open(home.join("ledger.db"))
}

fn doctor(home: &Path) -> Result<()> {
    let ledger = ledger(home)?;
    let claude = default_claude_path();
    println!("database: {}", ledger.path().display());
    println!("Claude transcripts: {}", claude.display());
    println!("TokenTree home: {}", home.display());
    println!(
        "Claude capture mode: {}",
        if claude.exists() {
            "logs (fallback)"
        } else {
            "manual/unavailable"
        }
    );
    println!("database integrity: {}", ledger.integrity_check()?);
    println!("prompt persistence: schema forbids prompt/completion/reasoning/tool payload columns");
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
        let summary = ledger.ingest(parsed.observations)?;
        inserted += summary.inserted;
        duplicates += summary.duplicates;
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

fn report(home: &Path) -> Result<()> {
    let ledger = ledger(home)?;
    let usage = ledger.aggregate_usage()?;
    let completeness = token_completeness(usage.measured, usage.unavailable, 0);
    println!("TokenTree ledger report");
    println!(
        "requests: {} measured {} unavailable {}",
        usage.requests, usage.measured, usage.unavailable
    );
    println!(
        "tokens: input {} cache-read {} cache-write {} output {} reasoning {}",
        usage.input, usage.cache_read, usage.cache_write, usage.output, usage.reasoning
    );
    println!(
        "completeness: {}",
        completeness.map_or_else(|| "—".into(), |value| format!("{value:.1}%"))
    );
    println!("policy: causal-request");
    println!("cost: unavailable unless a verified versioned calculation exists");
    println!("\n{DISCLAIMER}");
    Ok(())
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
    #[test]
    fn hook_sanitizer_drops_prompt_and_payload() {
        let input = json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"secret full prompt","tool_input":{"content":"source code"}});
        let output = sanitize_hook(&input).to_string();
        assert!(!output.contains("secret full prompt"));
        assert!(!output.contains("source code"));
        assert!(output.contains("prompt_fingerprint"));
    }
}
