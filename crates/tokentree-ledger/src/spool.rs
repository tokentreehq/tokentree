// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, params};
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

    // If the checkpoint offset is past the current EOF, the spool file was
    // truncated externally since the last poll. (Rotation resets the
    // checkpoint to 0 itself in maybe_rotate_spool — V1 — so this is only a
    // backstop for out-of-band truncation.)
    if start_offset > file_size {
        start_offset = 0;
    }

    let mut file = File::open(spool_path)
        .with_context(|| format!("open spool at {}", spool_path.display()))?;
    file.seek(SeekFrom::Start(start_offset))?;

    let mut reader = BufReader::new(file);
    let quarantine_path = quarantine_path_for(spool_path);

    let mut transaction = connection.transaction()?;
    let mut summary = HookWorkerSummary::default();
    let outcome = ingest_spool_lines(
        &mut transaction,
        &mut reader,
        spool_path,
        &quarantine_path,
        start_offset,
        &mut summary,
        true, // M3: hold a torn tail so the next poll re-reads it
    )?;

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
            i64::try_from(outcome.end_offset)
                .context("spool offset exceeds SQLite integer range")?,
        ],
    )?;

    transaction.commit()?;

    // M4: rotate the spool (and quarantine) files if they exceeded the size
    // cap. Rotation only happens when the spool is fully ingested, and the
    // checkpoint is reset to 0 as part of rotation (V1).
    maybe_rotate_spool(connection, spool_path, spool_max_bytes())?;

    Ok(summary)
}

/// Result of ingesting lines from a spool reader.
struct IngestOutcome {
    /// Byte offset of the first unprocessed byte: the start of a held torn
    /// tail, or EOF when everything was consumed.
    end_offset: u64,
}

/// Ingest complete lines from `reader` starting at `start_offset`, applying
/// each event inside `tx` (the caller's transaction; the caller commits).
///
/// - Complete lines: JSON-parse → apply under a per-event savepoint; poison
///   events are quarantined (metadata only) and the offset still advances
///   past them, so one bad event can never stall the queue (H3).
/// - `hold_torn_tail` (M3): an unterminated final line means the hook writer
///   is mid-append — stop WITHOUT advancing past it, so the next poll
///   re-reads the completed line. When false (rotation catch-up on a file
///   that is never re-polled), the partial line is parse-attempted and
///   counted skipped instead of being held forever.
/// - Invalid UTF-8 line (V9): the raw bytes are fingerprinted into quarantine
///   (never persisted), the line is counted skipped, and ingestion continues.
/// - I/O error: stop; the caller commits up to `end_offset` and a later poll
///   retries from there.
#[allow(clippy::too_many_arguments)]
fn ingest_spool_lines(
    tx: &mut Transaction,
    reader: &mut impl BufRead,
    spool_path: &Path,
    quarantine_path: &Path,
    start_offset: u64,
    summary: &mut HookWorkerSummary,
    hold_torn_tail: bool,
) -> Result<IngestOutcome> {
    // V9: read raw bytes per line so one invalid-UTF-8 line can be skipped
    // with exact offset accounting instead of abandoning the rest of the
    // file (BufRead::read_line would return Err and end the loop, leaving
    // the checkpoint stuck at the bad bytes forever — a replay livelock).
    let mut raw_buf: Vec<u8> = Vec::new();
    let mut current_offset = start_offset;

    loop {
        raw_buf.clear();
        // L9: bounded line reads — a degenerate multi-hundred-MB hook line
        // must not OOM the worker. Oversize lines are skipped (fingerprinted
        // to quarantine) with exact offset accounting.
        let (consumed, truncated) = match tokentree_core::read_capped_line(reader, &mut raw_buf) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                eprintln!("tokentree: spool read error at offset {current_offset}: {e}");
                break;
            }
        };
        let line_start = current_offset;
        current_offset += consumed;

        if truncated {
            quarantine_oversize_line(quarantine_path, line_start, &raw_buf);
            summary.quarantined += 1;
            continue;
        }

        // M3: a final line without a trailing newline means the hook writer
        // is mid-append (writers always terminate lines with `\n`;
        // `read_until` only returns an unterminated chunk at EOF). Hold it:
        // do NOT ingest and do NOT advance the checkpoint past it, so the
        // next poll re-reads the completed line. Advancing would silently
        // lose the event: the partial line fails JSON parse (counted
        // `skipped`), and the remainder read later starts mid-JSON (skipped
        // again). Rotation catch-up (hold_torn_tail=false) has no next poll,
        // so the partial line is parse-attempted and counted skipped instead.
        if !raw_buf.ends_with(b"\n") && hold_torn_tail {
            current_offset = line_start;
            break;
        }

        let line = match String::from_utf8(std::mem::take(&mut raw_buf)) {
            Ok(s) => s,
            Err(e) => {
                // V9: fingerprint the raw bytes for forensics, count the
                // line skipped, and keep going — never stall the queue.
                quarantine_malformed_bytes(quarantine_path, line_start, &e.into_bytes());
                summary.skipped += 1;
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<HookEnvelope>(trimmed) {
            Ok(event) => {
                // Per-event savepoint: a poison event rolls back only its
                // own partial writes (dropped savepoint = ROLLBACK TO), then
                // is quarantined. The checkpoint still advances past it, so
                // one bad event can never stall the queue (H3).
                let savepoint = tx.savepoint()?;
                match apply_hook_event(&savepoint, &event, spool_path, summary) {
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
                        // `line_start` is this line's start byte offset.
                        quarantine_poison_event(
                            quarantine_path,
                            "claude-hook",
                            &event.kind,
                            line_start,
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
    }

    Ok(IngestOutcome {
        end_offset: current_offset,
    })
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
/// to `.2`, `.3`, … and the oldest dropped.
///
/// Rotation NEVER happens while un-ingested data remains: it requires the
/// checkpoint's `last_offset` to cover the whole file.
///
/// Two rotation hazards are handled here:
/// - V1: the checkpoint row is keyed by `(adapter, source_path)` and would
///   otherwise survive rotation with the OLD file's offset. If the fresh
///   file grew past that offset before the next poll, the reader would seek
///   into the new file at the stale offset and silently skip new events.
///   The checkpoint is therefore reset to 0 as part of rotation (the
///   `start_offset > file_size` check in the poll loop remains as a backstop
///   for external truncation).
/// - V4: an event appended between the size check and the rename would land
///   in `<spool>.1`, which no poll will ever read again. After renaming, the
///   tail of `.1` from the pre-rotation offset is catch-up ingested
///   (complete lines only).
///
/// The quarantine file is write-only forensic data (never re-read by
/// ingestion), so it rotates on size alone, independent of the spool.
/// Note (S1): quarantine forensic retention is bounded — generations rotate
/// on size alone and the oldest past `.3` is dropped.
fn maybe_rotate_spool(
    connection: &mut Connection,
    spool_path: &Path,
    max_bytes: u64,
) -> Result<()> {
    // S2 — rotation identity contract. The spool file has no stable identity
    // (no inode tracking); instead rotation is gated on *full ingestion*:
    // it only fires when the checkpoint's last_offset >= the file's current
    // size, i.e. every byte has been ingested and the checkpoint committed.
    // After the rename the checkpoint resets to offset 0 for the fresh file
    // (V1), so a new file is always read from the start. This gate is
    // airtight for the append-only hook writer. Known limitation: an
    // out-of-band truncate+rewrite of the spool file between the poll's
    // checkpoint commit and this size check could rotate un-ingested
    // content; only the hook writer touches this file, so this is not
    // reachable in normal operation.
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
    let last_offset = last_offset.max(0) as u64;
    if last_offset < file_size {
        // Un-ingested data remains (writer appended after our poll, or the
        // poll ingested only part of the file). Never rotate; try next poll.
        return Ok(());
    }

    rotate_generations(spool_path)?;

    // V4: catch-up ingest the tail of the rotated file. Anything the hook
    // writer appended between the size check above and the rename now lives
    // in `<spool>.1`, which no future poll will ever read.
    let rotated_path = std::path::PathBuf::from(format!("{}.1", spool_path.to_string_lossy()));
    if rotated_path.exists() {
        catch_up_rotated_spool(connection, &rotated_path, &quarantine_path, last_offset)?;
    }

    // V1: reset the checkpoint to the fresh file's start. Rotation only
    // happens on a fully-ingested file (plus the catch-up above), so
    // offset 0 is exactly right.
    connection.execute(
        "UPDATE ingestion_checkpoints SET last_offset=0, file_size=0 WHERE adapter='claude-hook' AND source_path=?",
        params![spool_str],
    )?;

    Ok(())
}

/// V4: ingest the tail of a rotated spool file (`<spool>.1`) starting at
/// `from_offset` — the bytes a hook writer may have appended between the
/// rotation size-check and the rename, which no future poll will ever read.
///
/// Complete lines are ingested transactionally via the shared
/// [`ingest_spool_lines`] helper. A torn tail is parse-attempted and counted
/// skipped rather than held: unlike the live spool there is no next poll
/// that could re-read it, so holding would lose the bytes just as surely.
/// (In practice the writer always terminates lines, so a torn tail here
/// means we raced a mid-write at microsecond scale.)
fn catch_up_rotated_spool(
    connection: &mut Connection,
    rotated_path: &Path,
    quarantine_path: &Path,
    from_offset: u64,
) -> Result<HookWorkerSummary> {
    let mut summary = HookWorkerSummary::default();
    let mut file = File::open(rotated_path)
        .with_context(|| format!("open rotated spool at {}", rotated_path.display()))?;
    file.seek(SeekFrom::Start(from_offset))?;
    let mut reader = BufReader::new(file);
    let mut tx = connection.transaction()?;
    ingest_spool_lines(
        &mut tx,
        &mut reader,
        rotated_path,
        quarantine_path,
        from_offset,
        &mut summary,
        false, // no next poll for .1: don't hold a torn tail, count it skipped
    )?;
    tx.commit()?;
    Ok(summary)
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
/// P2: scrub an error string before persisting it to the quarantine file.
/// Hook payloads may contain prompt text, and a future
/// `with_context(|| format!(...payload...))` on the `apply_hook_event` path
/// would otherwise land that text on disk. Any token from the raw hook line
/// that leaked into the error message is redacted; the full error still goes
/// to stderr (ephemeral), and the quarantine record keeps the raw line's
/// SHA-256 fingerprint for forensics.
fn scrub_error_for_quarantine(error: &anyhow::Error, raw_line: &str) -> String {
    use std::collections::HashSet;
    let msg = error.to_string();
    // Index the raw line's tokens: anything in the error that also appears
    // in the raw hook line is treated as potential payload leakage.
    let raw_tokens: HashSet<&str> = raw_line
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 4)
        .collect();
    let scrubbed: Vec<String> = msg
        .split_whitespace()
        .map(|word| {
            let core = word.trim_matches(|c: char| !c.is_alphanumeric());
            if core.len() >= 4 && raw_tokens.contains(core) {
                "[redacted]".to_string()
            } else {
                word.to_string()
            }
        })
        .collect();
    let mut result = scrubbed.join(" ");
    if result.len() > 300 {
        result.truncate(300);
        result.push_str("…[truncated]");
    }
    result
}

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
                "error": scrub_error_for_quarantine(error, raw_line),
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

/// V9: quarantine the fingerprint of raw spool bytes that failed UTF-8
/// decoding. The raw bytes are NEVER persisted (they may contain prompt
/// text) — only a SHA-256 fingerprint for forensics, plus the offset.
/// Unlike poison events there is no structured error to record.
fn quarantine_malformed_bytes(quarantine_path: &Path, source_offset: u64, raw_bytes: &[u8]) {
    use std::io::Write as _;
    if let Some(parent) = quarantine_path.parent() {
        if fs::create_dir_all(parent).is_err() {
            eprintln!("tokentree: cannot create quarantine dir for malformed spool bytes");
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
                "adapter": "claude-hook",
                "label": "malformed spool bytes skipped",
                "event_kind": "unknown",
                "event_fingerprint": tokentree_core::sha256_hex(raw_bytes),
                "source_offset": source_offset,
            });
            if writeln!(file, "{record}").is_err() {
                eprintln!("tokentree: failed to write quarantined malformed spool bytes");
            }
        }
        Err(io_err) => {
            eprintln!(
                "tokentree: cannot open quarantine file {} ({io_err})",
                quarantine_path.display()
            );
        }
    }
}

/// L9: quarantine the fingerprint of an oversize spool line that was skipped
/// by the bounded reader. Only the first MAX_LINE_BYTES are fingerprinted
/// (the remainder was discarded without buffering); the raw content is
/// NEVER persisted.
fn quarantine_oversize_line(quarantine_path: &Path, source_offset: u64, partial: &[u8]) {
    use std::io::Write as _;
    if let Some(parent) = quarantine_path.parent() {
        if fs::create_dir_all(parent).is_err() {
            eprintln!("tokentree: cannot create quarantine dir for oversize spool line");
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
                "adapter": "claude-hook",
                "label": "oversize spool line skipped",
                "event_kind": "unknown",
                "event_fingerprint": tokentree_core::sha256_hex(partial),
                "source_offset": source_offset,
            });
            if writeln!(file, "{record}").is_err() {
                eprintln!("tokentree: failed to write quarantined oversize spool line");
            }
        }
        Err(io_err) => {
            eprintln!(
                "tokentree: cannot open quarantine file {} ({io_err})",
                quarantine_path.display()
            );
        }
    }
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

    /// P2 canary: prompt text that leaks into an error string must not reach
    /// the quarantine file — only the fingerprint and a scrubbed error.
    #[test]
    fn quarantine_error_scrubs_raw_line_content() {
        let raw_line = r#"{"kind":"SessionStart","payload":{"session_id":"abc123","prompt":"my secret prompt text here"}}"#;
        // Simulate a future with_context! that interpolates payload data.
        let err = anyhow::anyhow!("hook apply failed for payload my secret prompt text here: db busy");
        let scrubbed = scrub_error_for_quarantine(&err, raw_line);
        assert!(!scrubbed.contains("my secret prompt text here"), "raw line content leaked: {scrubbed}");
        assert!(scrubbed.contains("[redacted]"));
        assert!(scrubbed.contains("db busy"));
        // Long errors are bounded.
        let long_err = anyhow::anyhow!("{}", "x".repeat(1000));
        let scrubbed = scrub_error_for_quarantine(&long_err, raw_line);
        assert!(scrubbed.len() <= 320);
        assert!(scrubbed.ends_with("[truncated]"));
    }

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
        maybe_rotate_spool(ledger.connection_mut(), &spool_path, 10).unwrap();
        assert!(
            !rotated_path.exists(),
            "must not rotate while un-ingested data remains"
        );

        // Ingest everything, then rotation proceeds.
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 1);
        maybe_rotate_spool(ledger.connection_mut(), &spool_path, 10).unwrap();
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

    /// V1: rotation must reset the checkpoint to 0. The old code left the
    /// stale offset in place, so a fresh spool file that grew past the old
    /// offset before the next poll had its head silently skipped.
    #[test]
    fn rotation_resets_checkpoint_so_fresh_file_ingests_fully() {
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
        append_line("ses_v1_1");
        append_line("ses_v1_2");
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2);
        let old_offset: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(old_offset > 0);

        // Force rotation with a tiny cap.
        maybe_rotate_spool(ledger.connection_mut(), &spool_path, 10).unwrap();
        assert!(dir.path().join("claude-hooks.jsonl.1").exists());

        // The checkpoint must be reset — a stale offset here is the V1 bug.
        let offset: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset, 0);

        // Grow the fresh file well past the old offset, then poll: every
        // event must be ingested (the old bug skipped the first old_offset
        // bytes of the new file).
        for i in 0..10 {
            append_line(&format!("ses_v1_new_{i}"));
        }
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 10);
        assert_eq!(summary2.skipped, 0);
        let turn_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(turn_count, 12);
    }

    /// V4: the rotation catch-up ingests bytes appended between the rotation
    /// size-check and the rename. The race itself isn't deterministically
    /// testable, so this drives `catch_up_rotated_spool` directly on a
    /// rotated file whose tail the checkpoint doesn't cover.
    #[test]
    fn catch_up_rotated_spool_ingests_tail() {
        let dir = tempdir().unwrap();
        let rotated_path = dir.path().join("claude-hooks.jsonl.1");
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");

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
        let line1 = format!("{}\n", hook("ses_v4_1"));
        let line2 = format!("{}\n", hook("ses_v4_2"));
        let line3 = format!("{}\n", hook("ses_v4_3"));
        fs::write(&rotated_path, format!("{line1}{line2}{line3}")).unwrap();

        // Simulate: the checkpoint covered only line1 when rotation happened;
        // lines 2-3 are the "in-window" appends living in .1.
        let mut ledger = Ledger::open_memory().unwrap();
        let summary = catch_up_rotated_spool(
            ledger.connection_mut(),
            &rotated_path,
            &quarantine_path,
            line1.len() as u64,
        )
        .unwrap();
        assert_eq!(summary.processed, 2);
        assert_eq!(summary.skipped, 0);

        let turn_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        assert_eq!(turn_count, 2);
    }

    /// V9: a spool line with invalid UTF-8 must be fingerprinted into
    /// quarantine and skipped — not livelock the queue. Events after the
    /// bad bytes must still be ingested, and the checkpoint must reach EOF.
    #[test]
    fn invalid_utf8_spool_line_is_skipped_without_livelock() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");

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
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(format!("{}\n", hook("ses_u8_1")).as_bytes());
        let bad_start = raw.len();
        raw.extend_from_slice(b"\xff\xfe not valid utf-8 \x80\n");
        let bad_end = raw.len();
        raw.extend_from_slice(format!("{}\n", hook("ses_u8_2")).as_bytes());
        let bad_bytes = raw[bad_start..bad_end].to_vec();
        fs::write(&spool_path, &raw).unwrap();

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2);
        assert_eq!(summary.skipped, 1);

        // Checkpoint at EOF: the next poll must not re-process the tail
        // (the old livelock re-read everything on every poll).
        let offset: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset as u64, raw.len() as u64);
        let summary2 = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary2.processed, 0);
        assert_eq!(summary2.skipped, 0);

        // Quarantine holds the raw bytes' fingerprint — never the bytes.
        let quarantined = fs::read_to_string(&quarantine_path).unwrap();
        let expected_fp = tokentree_core::sha256_hex(&bad_bytes);
        assert!(
            quarantined.contains(&expected_fp),
            "quarantine must carry the bad bytes' fingerprint"
        );
        assert!(quarantined.contains("malformed spool bytes skipped"));
    }

    /// L9: a spool line over MAX_LINE_BYTES is skipped (fingerprinted to
    /// quarantine) without buffering the whole line; the lines around it
    /// still ingest and the checkpoint advances past the oversize bytes.
    #[test]
    fn oversize_spool_line_is_skipped_without_oom() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("claude-hooks.jsonl");
        let quarantine_path = dir.path().join("claude-hooks.quarantine.jsonl");

        let hook = |session: &str| {
            format!(
                "{}\n",
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
            )
        };
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(hook("ses_big_1").as_bytes());
        // One line just over the 16 MiB cap: valid JSON prefix, then padding.
        let mut big = b"{\"kind\":\"UserPromptSubmit\",\"payload\":{\"session_id\":\"ses_big_mid\",\"pad\":\"".to_vec();
        big.extend(std::iter::repeat(b'x').take(17 * 1024 * 1024));
        big.extend_from_slice(b"\"}}\n");
        let big_start = raw.len();
        raw.extend_from_slice(&big);
        raw.extend_from_slice(hook("ses_big_2").as_bytes());
        fs::write(&spool_path, &raw).unwrap();

        let mut ledger = Ledger::open_memory().unwrap();
        let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
        assert_eq!(summary.processed, 2, "lines around the oversize one must ingest");
        assert_eq!(summary.quarantined, 1, "oversize line must be quarantined");

        // Checkpoint at EOF: no re-processing, no stall.
        let offset: i64 = ledger
            .connection()
            .query_row(
                "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(offset as u64, raw.len() as u64);

        // Quarantine holds a fingerprint label — never the 17 MiB line.
        let quarantined = fs::read_to_string(&quarantine_path).unwrap();
        assert!(quarantined.contains("oversize spool line skipped"));
        assert!(quarantined.len() < 1024 * 1024, "quarantine must stay small");
        let _ = big_start;
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

        let mut ledger = Ledger::open_memory().unwrap();
        maybe_rotate_spool(ledger.connection_mut(), &spool_path, 100).unwrap();

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
