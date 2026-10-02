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
    let mut start_offset: u64 = connection
        .query_row(
            "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook' AND source_path=?",
            params![spool_str],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    // M4: if the checkpoint offset is past the current EOF, the spool file
    // was rotated (or truncated) since the last poll. Rotation only ever
    // happens after the old file was fully ingested, so restart from 0.
    if start_offset > file_size {
        start_offset = 0;
    }

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

        // M3: a final line without a trailing newline means the hook writer
        // is mid-append (writers always terminate lines with `\n`; `read_line`
        // only returns an unterminated chunk at EOF). Do NOT ingest it and do
        // NOT advance the checkpoint past it — break here so the next poll
        // re-reads the completed line. Advancing would silently lose the
        // event: the partial line fails JSON parse (counted `skipped`), and
        // the remainder read later starts mid-JSON (skipped again).
        if !line.ends_with('\n') {
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
                        // Quarantine metadata only: the raw hook line may
                        // contain prompt text and must never hit disk.
                        // `current_offset` is this line's start byte offset.
                        quarantine_poison_event(
                            &quarantine_path,
                            "claude-hook",
                            &event.kind,
                            current_offset,
                            &line,
                            &e,
                        );
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

    // M4: rotate the spool (and quarantine) files if they exceeded the size
    // cap. Rotation only happens when the spool is fully ingested, so the
    // next poll picks up the fresh file from offset 0 (see the
    // start_offset > file_size reset above).
    maybe_rotate_spool(connection, spool_path, spool_max_bytes())?;

    Ok(summary)
}

/// Maximum spool/quarantine file size before rotation, in bytes.
/// Overridable via `TOKENTREE_SPOOL_MAX_BYTES`; defaults to 100 MiB.
pub fn spool_max_bytes() -> u64 {
    std::env::var("TOKENTREE_SPOOL_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(100 * 1024 * 1024)
}

/// Generations of rotated spool files kept (`path.1` .. `path.N`).
const SPOOL_ROTATIONS: u32 = 3;

/// Rotate `spool_path` (and its quarantine sibling) when over `max_bytes`.
///
/// The spool file is renamed to `<name>.1` — with older generations shifted
/// to `.2`, `.3`, … and the oldest dropped — and ingestion restarts from
/// offset 0 on the fresh file (the poll detects `last_offset > file_size` and
/// resets). Rotation NEVER happens while un-ingested data remains: it
/// requires the checkpoint's `last_offset` to cover the whole file. (There is
/// an unavoidable check-then-rename race if the hook writer appends in the
/// microseconds between the size check and the rename; the writer appends
/// complete lines and the M3 hold-checkpoint rule means a torn tail is
/// re-read, not lost — but operators should treat rotation during active
/// writes as best-effort.)
///
/// The quarantine file is write-only forensic data (never re-read by
/// ingestion), so it rotates on size alone, independent of the spool.
fn maybe_rotate_spool(connection: &Connection, spool_path: &Path, max_bytes: u64) -> Result<()> {
    // The quarantine file is write-only forensic data (never re-read by
    // ingestion), so it rotates on size alone, independent of the spool.
    let quarantine_path = quarantine_path_for(spool_path);
    if fs::metadata(&quarantine_path).map(|m| m.len()).unwrap_or(0) > max_bytes {
        rotate_generations(&quarantine_path)?;
    }

    let file_size = fs::metadata(spool_path).map(|m| m.len()).unwrap_or(0);
    if file_size <= max_bytes {
        return Ok(());
    }
    let spool_str = spool_path.to_string_lossy().to_string();
    let last_offset: i64 = connection
        .query_row(
            "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook' AND source_path=?",
            params![spool_str],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if (last_offset.max(0) as u64) < file_size {
        // Un-ingested data remains (writer appended after our poll, or the
        // poll ingested only part of the file). Never rotate; try next poll.
        return Ok(());
    }
    rotate_generations(spool_path)?;
    Ok(())
}

/// Shift `<path>` -> `<path>.1`, `<path>.1` -> `<path>.2`, …, keeping
/// [`SPOOL_ROTATIONS`] generations and dropping the oldest.
fn rotate_generations(path: &Path) -> Result<()> {
    let base = path.to_string_lossy().to_string();
    for generation in (1..SPOOL_ROTATIONS).rev() {
        let from_name = format!("{base}.{generation}");
        let from = Path::new(&from_name);
        if from.exists() {
            fs::rename(from, format!("{base}.{}", generation + 1)).with_context(|| {
                format!(
                    "rotate spool generation {generation} for {}",
                    path.display()
                )
            })?;
        }
    }
    fs::rename(path, format!("{base}.1"))
        .with_context(|| format!("rotate spool file {}", path.display()))?;
    Ok(())
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

/// Persist a poison spool event's *sanitized metadata* for later inspection
/// (H3). Privacy constitution: hook payloads may contain prompt text, so the
/// raw line is NEVER persisted — only a SHA-256 fingerprint the operator can
/// match against their own spool/transcript files for diagnosis. Best-effort
/// by design: quarantining must never itself fail ingestion, so all IO errors
/// are swallowed after a stderr notice. The quarantine file keeps the 0600
/// treatment as defense-in-depth.
fn quarantine_poison_event(
    quarantine_path: &Path,
    adapter: &str,
    event_kind: &str,
    source_offset: u64,
    raw_line: &str,
    error: &anyhow::Error,
) {
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
                "adapter": adapter,
                "label": "poison hook event rejected",
                "event_kind": event_kind,
                "event_fingerprint": tokentree_core::sha256_hex(raw_line.as_bytes()),
                "source_offset": source_offset,
                "error": error.to_string(),
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
        "tokentree: quarantined poison spool event metadata -> {}: {error}",
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

    /// M3: a torn final line (writer mid-append, no trailing newline) must
    /// NOT be ingested and must NOT advance the checkpoint. The next poll
    /// re-reads the completed line and ingests it exactly once.
    #[test]
    fn torn_spool_line_holds_checkpoint_until_completed() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");

        let hook = |session: &str| {
            json!({
                "version": 1,
                "kind": "UserPromptSubmit",
                "capturedAt": "2026-09-29T10:00:00Z",
                "payload": {
                    "session_id": session,
                    "prompt_fingerprint": "abc12345",
                    "cwd": dir.path().to_str().unwrap()
                }
            })
        };

        // One complete event, then a partial line with no trailing newline,
        // as if the hook writer was mid-append when we polled.
        let line1 = format!("{}\n", hook("ses_torn_1"));
        let partial = format!("{}", hook("ses_torn_2"));
        fs::write(&spool_path, format!("{line1}{partial}")).unwrap();

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 1);
        assert_eq!(
            summary.skipped, 0,
            "torn line must be held, not counted as skipped"
        );

        // Checkpoint held at the torn line's start byte offset.
        let offset: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset as u64, line1.len() as u64);

        // Writer completes the line; the next poll ingests it exactly once.
        {
            use std::io::Write as _;
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&spool_path)
                .unwrap();
            writeln!(f).unwrap(); // terminates the partial line
        }
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 1);
        assert_eq!(summary2.skipped, 0);

        let turn_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(turn_count, 2, "torn event ingested exactly once");

        // And the checkpoint now covers the whole file.
        let offset2: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let file_len = fs::metadata(&spool_path).unwrap().len();
        assert_eq!(offset2 as u64, file_len);
    }

    /// M4: rotation never drops un-ingested data, and ingestion resumes from
    /// offset 0 on the fresh file with no duplicates.
    #[test]
    fn spool_rotates_only_when_fully_ingested() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");
        let rotated_path = dir.path().join("claude-hooks.jsonl.1");

        let hook = |session: &str| {
            json!({
                "version": 1,
                "kind": "UserPromptSubmit",
                "capturedAt": "2026-09-29T10:00:00Z",
                "payload": {
                    "session_id": session,
                    "prompt_fingerprint": "abc12345",
                    "cwd": dir.path().to_str().unwrap()
                }
            })
        };
        let append_line = |session: &str| {
            use std::io::Write as _;
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&spool_path)
                .unwrap();
            writeln!(f, "{}", hook(session)).unwrap();
        };

        let mut ledger = Ledger::open_memory().unwrap();
        append_line("ses_rot_1");
        append_line("ses_rot_2");
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2);

        // Un-ingested data present: append without polling, then rotation
        // must refuse (tiny cap forces the size check to trigger).
        append_line("ses_rot_3");
        maybe_rotate_spool(ledger.connection(), &spool_path, 10).unwrap();
        assert!(
            !rotated_path.exists(),
            "must not rotate while un-ingested data remains"
        );

        // Ingest everything, then rotation proceeds.
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 1);
        maybe_rotate_spool(ledger.connection(), &spool_path, 10).unwrap();
        assert!(rotated_path.exists(), "spool should have rotated");
        assert!(
            !spool_path.exists(),
            "original path renamed away by rotation"
        );

        // Fresh file: ingestion restarts from offset 0, no duplicates.
        append_line("ses_rot_4");
        let summary3 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary3.processed, 1);
        let turn_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(turn_count, 4);
    }

    /// M4: the quarantine file rotates on size alone (it is write-only).
    #[test]
    fn quarantine_file_rotates_on_size() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");

        // Spool stays tiny; quarantine exceeds the cap.
        fs::write(&spool_path, "").unwrap();
        fs::write(&quarantine_path, "x".repeat(500)).unwrap();

        let ledger = Ledger::open_memory().unwrap();
        maybe_rotate_spool(ledger.connection(), &spool_path, 100).unwrap();

        assert!(dir.path().join("claude-hooks.quarantine.jsonl.1").exists());
        assert!(!quarantine_path.exists());
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

        // The quarantine file holds sanitized metadata only: the raw hook
        // payload (including the canary session id) must never be persisted,
        // per the privacy constitution.
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");
        let quarantined = std::fs::read_to_string(&quarantine_path).unwrap();
        assert!(
            !quarantined.contains("ses_poison"),
            "raw hook payload leaked into quarantine file"
        );
        assert!(!quarantined.contains("ses_good_1"));
        assert!(quarantined.contains("poison hook event rejected"));
        assert!(quarantined.contains("\"adapter\":\"claude-hook\""));
        assert!(quarantined.contains("\"event_kind\":\"UserPromptSubmit\""));
        assert!(quarantined.contains("event_fingerprint"));
        assert!(quarantined.contains("source_offset"));
        // The error message is metadata, not payload content.
        assert!(quarantined.contains("forbidden capability"));
        // The fingerprint identifies the exact raw line for forensics.
        let poison_line = format!("{}\n", hook("ses_poison", &poison_cwd));
        let expected_fp = tokentree_core::sha256_hex(poison_line.as_bytes());
        assert!(
            quarantined.contains(&expected_fp),
            "quarantine must carry the raw line's fingerprint"
        );

        // Re-running must not stall on the poison event: checkpoint advanced.
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 0);
        assert_eq!(summary2.quarantined, 0);
    }

    #[test]
    fn quarantine_never_persists_prompt_like_or_huge_payloads() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");

        let poison_cwd = dir.path().join("poison-proj");
        std::fs::create_dir_all(&poison_cwd).unwrap();
        // Forbidden capability key -> resolve_project fails -> apply fails.
        std::fs::write(
            poison_cwd.join(".tokentree.yml"),
            "project: poison\nexec: /bin/true\n",
        )
        .unwrap();

        // Adversarial payloads: prompt-like text with a canary, and a 1 MiB
        // blob. Both must fail apply (poison cwd) and be quarantined without
        // their bytes ever touching the quarantine file.
        let canary = "CANARY_PROMPT_do_not_persist_9f8e7d6c";
        let huge_blob = format!("{canary}_{}", "_".repeat(1048576));
        let hook = |payload_extra: &str| {
            format!(
                "{{\"version\":1,\"kind\":\"UserPromptSubmit\",\"capturedAt\":\"2026-09-29T10:00:00Z\",\
                 \"payload\":{{\"session_id\":\"ses_adv\",\"cwd\":\"{}\",\"prompt\":{}}}}}",
                poison_cwd.to_str().unwrap().replace('\\', "\\\\"),
                payload_extra,
            )
        };
        let line1 = hook(&format!("\"please {canary} summarize my secrets\""));
        let line2 = hook(&serde_json::to_string(&huge_blob).unwrap());
        std::fs::write(&spool_path, format!("{line1}\n{line2}\n")).unwrap();

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.quarantined, 2);
        assert_eq!(summary.processed, 0);

        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");
        let quarantined = std::fs::read_to_string(&quarantine_path).unwrap();
        assert_eq!(quarantined.lines().count(), 2);
        assert!(
            !quarantined.contains(canary),
            "prompt canary leaked into quarantine file"
        );
        assert!(
            !quarantined.contains("summarize my secrets"),
            "prompt text leaked into quarantine file"
        );
        // Metadata survived.
        assert!(quarantined.contains("event_fingerprint"));
        assert!(quarantined.contains("\"event_kind\":\"UserPromptSubmit\""));
        // Fingerprints match the exact raw lines.
        for raw in [&format!("{line1}\n"), &format!("{line2}\n")] {
            let fp = tokentree_core::sha256_hex(raw.as_bytes());
            assert!(quarantined.contains(&fp), "missing fingerprint for a line");
        }
    }
}
