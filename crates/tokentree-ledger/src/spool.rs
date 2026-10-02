// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use tokentree_core::{ResolveProjectInput, resolve_project};

#[derive(Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct HookWorkerSummary {
    pub processed: u64,
    pub skipped: u64,
    /// Events that failed to apply and were moved to the quarantine file (H3).
    pub quarantined: u64,
    pub projects: u64,
    pub sessions: u64,
    pub turns: u64,
    pub pending_classify: u64,
}

#[derive(Deserialize, Debug)]
pub struct HookEnvelope {
    pub version: u32,
    pub kind: String,
    #[serde(rename = "capturedAt")]
    pub captured_at: String,
    pub payload: Value,
}

pub fn process_claude_hook_spool(
    connection: &mut Connection,
    spool_path: &Path,
) -> Result<HookWorkerSummary> {
    if !spool_path.exists() {
        return Ok(HookWorkerSummary::default());
    }

    let metadata = fs::metadata(spool_path)
        .with_context(|| format!("stat spool at {}", spool_path.display()))?;
    let file_size = metadata.len();
    let modified_at = metadata
        .modified()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());

    let spool_str = spool_path.to_string_lossy().to_string();
    let start_offset: u64 = connection
        .query_row(
            "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook' AND source_path=?",
            params![spool_str],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let mut file = File::open(spool_path)
        .with_context(|| format!("open spool at {}", spool_path.display()))?;
    file.seek(SeekFrom::Start(start_offset))?;

    let mut reader = BufReader::new(file);
    let mut current_offset = start_offset;
    let mut summary = HookWorkerSummary::default();
    let quarantine_path = quarantine_path_for(spool_path);

    let mut transaction = connection.transaction()?;

    let mut line = String::new();
    while let Ok(bytes_read) = reader.read_line(&mut line) {
        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            current_offset += bytes_read as u64;
            line.clear();
            continue;
        }

        match serde_json::from_str::<HookEnvelope>(trimmed) {
            Ok(event) => {
                // Per-event savepoint: a poison event rolls back only its
                // own partial writes (dropped savepoint = ROLLBACK TO), then
                // is quarantined. The checkpoint still advances past it, so
                // one bad event can never stall the queue (H3).
                let savepoint = transaction.savepoint()?;
                match apply_hook_event(&savepoint, &event, spool_path, &mut summary) {
                    Ok(()) => {
                        savepoint.commit()?;
                        summary.processed += 1;
                    }
                    // H3: quarantine the poison event and keep going. The old
                    // code returned Err here, which rolled back the whole
                    // transaction *including the checkpoint* — the same bad
                    // event would then stall every future run forever.
                    Err(e) => {
                        drop(savepoint);
                        quarantine_poison_event(&quarantine_path, trimmed, &e);
                        summary.quarantined += 1;
                    }
                }
            }
            Err(_) => {
                summary.skipped += 1;
            }
        }

        current_offset += bytes_read as u64;
        line.clear();
    }

    transaction.execute(
        "INSERT INTO ingestion_checkpoints(adapter, source_path, file_size, modified_at, last_offset, last_event_hash)
         VALUES('claude-hook', ?, ?, ?, ?, NULL)
         ON CONFLICT(adapter, source_path) DO UPDATE SET
            file_size = excluded.file_size,
            modified_at = excluded.modified_at,
            last_offset = excluded.last_offset",
        params![
            spool_str,
            i64::try_from(file_size).context("spool file size exceeds SQLite integer range")?,
            modified_at,
            i64::try_from(current_offset)
                .context("spool offset exceeds SQLite integer range")?,
        ],
    )?;

    transaction.commit()?;
    Ok(summary)
}

/// Sibling path for quarantined spool events, e.g.
/// `spool/claude-hooks.jsonl` -> `spool/claude-hooks.quarantine.jsonl`.
/// Poison events are preserved here for forensic inspection instead of
/// being dropped or blocking the queue.
fn quarantine_path_for(spool_path: &Path) -> std::path::PathBuf {
    let file_name = spool_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("spool.jsonl");
    let stem = file_name.strip_suffix(".jsonl").unwrap_or(file_name);
    spool_path.with_file_name(format!("{stem}.quarantine.jsonl"))
}

/// Persist a poison spool event for later inspection (H3). Best-effort by
/// design: quarantining must never itself fail ingestion, so all IO errors
/// are swallowed after a stderr notice. The quarantine file gets the same
/// 0600 treatment as the spool file on unix since it may contain hook
/// payloads.
fn quarantine_poison_event(quarantine_path: &Path, raw_line: &str, error: &anyhow::Error) {
    use std::io::Write as _;
    if let Some(parent) = quarantine_path.parent() {
        if fs::create_dir_all(parent).is_err() {
            eprintln!("tokentree: cannot create quarantine dir for spool poison event: {error}");
            return;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(quarantine_path) {
        Ok(mut file) => {
            let record = serde_json::json!({
                "quarantined_at": chrono::Utc::now().to_rfc3339(),
                "error": error.to_string(),
                "raw_line": raw_line,
            });
            if writeln!(file, "{record}").is_err() {
                eprintln!("tokentree: failed to write quarantined spool event: {error}");
            }
        }
        Err(io_err) => {
            eprintln!(
                "tokentree: cannot open quarantine file {} for poison spool event ({io_err}): {error}",
                quarantine_path.display()
            );
        }
    }
    eprintln!(
        "tokentree: quarantined poison spool event -> {}: {error}",
        quarantine_path.display()
    );
}

fn apply_hook_event(
    connection: &rusqlite::Connection,
    event: &HookEnvelope,
    spool_path: &Path,
    summary: &mut HookWorkerSummary,
) -> Result<()> {
    let session_key = event
        .payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim);

    let Some(session_key) = session_key else {
        return Ok(());
    };
    if session_key.is_empty() {
        return Ok(());
    }

    let cwd = event
        .payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            spool_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        });

    let project = resolve_project(ResolveProjectInput {
        cwd: cwd.clone(),
        ..Default::default()
    })?;

    let project_id = stable_id("prj", &project.key);
    let session_id = stable_id("ses", &format!("claude:{session_key}"));

    // 1. Ensure project exists
    let prj_changed = connection.execute(
        "INSERT OR IGNORE INTO projects(
            id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at
        ) VALUES(?,?,?,?,?,?,?,?)",
        params![
            project_id,
            project.key,
            project.display_name,
            stable_id("identity", &format!("{}:{}", project.method.as_str(), project.root.display())),
            project.method.as_str(),
            project.confidence,
            event.captured_at,
            event.captured_at,
        ],
    )?;
    if prj_changed == 1 {
        summary.projects += 1;
    }

    // 2. Ensure project root exists
    let root_path_str = project.root.to_string_lossy().to_string();
    connection.execute(
        "INSERT OR IGNORE INTO project_roots(
            project_id, canonical_path, root_type, fingerprint, active, first_seen_at, last_seen_at
        ) VALUES(?,?,?,?,1,?,?)",
        params![
            project_id,
            root_path_str,
            project.method.as_str(),
            stable_id("root", &root_path_str),
            event.captured_at,
            event.captured_at,
        ],
    )?;

    // 3. Ensure session exists
    let transcript_path = event.payload.get("transcript_path").and_then(Value::as_str);
    let cwd_str = cwd.to_string_lossy().to_string();
    let ses_changed = connection.execute(
        "INSERT OR IGNORE INTO sessions(
            id, adapter, provider_session_id, project_id, source_path, cwd, started_at
        ) VALUES(?,'claude',?,?,?,?,?)",
        params![
            session_id,
            session_key,
            project_id,
            transcript_path,
            cwd_str,
            event.captured_at,
        ],
    )?;
    if ses_changed == 1 {
        summary.sessions += 1;
    }
    connection.execute(
        "UPDATE sessions SET project_id=?, cwd=? WHERE id=?",
        params![project_id, cwd_str, session_id],
    )?;

    // 4. Handle event kinds
    match event.kind.as_str() {
        "UserPromptSubmit" => {
            let sequence: i64 = connection.query_row(
                "SELECT count(*) FROM turns WHERE session_id=?",
                params![session_id],
                |row| row.get(0),
            )?;

            let prompt_id = event
                .payload
                .get("prompt_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| sequence.to_string());

            let turn_id = stable_id("turn", &format!("{session_id}:{prompt_id}"));
            let work_item_id = stable_id("wi", &format!("{project_id}:uncategorized"));

            connection.execute(
                "INSERT OR IGNORE INTO work_items(
                    id, project_id, type, title, status, confidence, classifier_version, created_at
                ) VALUES(?,?,'inbox','Uncategorized','open',0,'pending',?)",
                params![work_item_id, project_id, event.captured_at],
            )?;

            let fingerprint = event
                .payload
                .get("prompt_fingerprint")
                .and_then(Value::as_str);

            let turn_changed = connection.execute(
                "INSERT OR IGNORE INTO turns(
                    id, session_id, sequence_number, started_at, prompt_fingerprint, prompt_storage_mode
                ) VALUES(?,?,?,?,?,'fingerprint_only')",
                params![turn_id, session_id, sequence, event.captured_at, fingerprint],
            )?;

            if turn_changed == 1 {
                summary.turns += 1;
                connection.execute(
                    "INSERT INTO classification_events(
                        id, turn_id, outcome, signals_json, score, classifier_version, created_at
                    ) VALUES(?,?,'UNCERTAIN','[\"prompt_not_persisted\",\"pending_worker\"]',0,'pending',?)",
                    params![stable_id("class", &turn_id), turn_id, event.captured_at],
                )?;
                summary.pending_classify += 1;
            }
        }
        "Stop" | "StopFailure" => {
            connection.execute(
                "UPDATE turns SET ended_at=? WHERE id=(
                    SELECT id FROM turns WHERE session_id=? AND ended_at IS NULL ORDER BY sequence_number DESC LIMIT 1
                )",
                params![event.captured_at, session_id],
            )?;
        }
        "SessionEnd" => {
            connection.execute(
                "UPDATE sessions SET ended_at=? WHERE id=?",
                params![event.captured_at, session_id],
            )?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn processes_claude_hooks_safely() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");

        let mut file = File::create(&spool_path).unwrap();
        let ev1 = json!({
            "version": 1,
            "kind": "UserPromptSubmit",
            "capturedAt": "2026-09-29T10:00:00Z",
            "payload": {
                "session_id": "ses_test",
                "prompt_fingerprint": "abc12345",
                "cwd": dir.path().to_str().unwrap()
            }
        });
        let ev2 = json!({
            "version": 1,
            "kind": "Stop",
            "capturedAt": "2026-09-29T10:05:00Z",
            "payload": {
                "session_id": "ses_test",
                "cwd": dir.path().to_str().unwrap()
            }
        });
        writeln!(file, "{}", ev1).unwrap();
        writeln!(file, "{}", ev2).unwrap();
        drop(file);

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2);
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.turns, 1);

        let turn_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(turn_count, 1);

        // Interrupted/checkpoint test: re-running should process 0 new records
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 0);
    }

    #[test]
    fn quarantine_path_for_appends_quarantine_suffix() {
        let p = std::path::Path::new("/tmp/spool/claude-hooks.jsonl");
        assert_eq!(
            quarantine_path_for(p),
            std::path::Path::new("/tmp/spool/claude-hooks.quarantine.jsonl")
        );
    }

    /// H3: a poison event (here: a hook whose cwd contains a `.tokentree.yml`
    /// with a forbidden key, making project resolution fail) must be
    /// quarantined without stalling the events around it, and the checkpoint
    /// must advance past it so re-runs do not stall either.
    #[test]
    fn poison_spool_event_is_quarantined_not_stalling() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");

        let good_cwd = dir.path().join("good-proj");
        std::fs::create_dir_all(&good_cwd).unwrap();
        let poison_cwd = dir.path().join("poison-proj");
        std::fs::create_dir_all(&poison_cwd).unwrap();
        // Forbidden capability key -> resolve_project fails -> apply fails.
        std::fs::write(
            poison_cwd.join(".tokentree.yml"),
            "project: poison\nexec: /bin/true\n",
        )
        .unwrap();

        let hook = |session: &str, cwd: &std::path::Path| {
            json!({
                "version": 1,
                "kind": "UserPromptSubmit",
                "capturedAt": "2026-09-29T10:00:00Z",
                "payload": {
                    "session_id": session,
                    "prompt_fingerprint": "abc12345",
                    "cwd": cwd.to_str().unwrap()
                }
            })
        };

        let mut file = File::create(&spool_path).unwrap();
        writeln!(file, "{}", hook("ses_good_1", &good_cwd)).unwrap();
        writeln!(file, "{}", hook("ses_poison", &poison_cwd)).unwrap();
        writeln!(file, "{}", hook("ses_good_2", &good_cwd)).unwrap();
        drop(file);

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2);
        assert_eq!(summary.quarantined, 1);
        assert_eq!(summary.skipped, 0);

        // The good sessions after the poison event were still ingested.
        let session_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(session_count, 2);

        // The poison event is preserved in the quarantine file.
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");
        let quarantined = std::fs::read_to_string(&quarantine_path).unwrap();
        assert!(quarantined.contains("ses_poison"));
        assert!(quarantined.contains("forbidden capability"));

        // Re-running must not stall on the poison event: checkpoint advanced.
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 0);
        assert_eq!(summary2.quarantined, 0);
    }
}
