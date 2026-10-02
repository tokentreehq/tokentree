// SPDX-License-Identifier: Apache-2.0
//! Repair historical Claude `usage_snapshot` overcounting (C4).
//!
//! Before the C4 fix, the Claude parser stored every `usage_snapshot` record
//! as a full cumulative counter instead of diffing it per session. Existing
//! databases therefore contain overcounted `snapshot_delta` rows. Re-importing
//! does not fix history (rows dedup by event id), so this module recomputes
//! per-session deltas in place, exactly mirroring the fixed parser:
//!
//! * first snapshot per session = baseline → superseded (token columns zeroed)
//! * subsequent snapshots = delta vs the previous row's *original* values
//! * a backward counter move = anomaly (row zeroed, `measurement_anomalies`
//!   entry) and the new value becomes the baseline
//!
//! Safety properties:
//! * every affected row is copied to `snapshot_repair_backups` before any
//!   write, and `--restore` puts the originals back;
//! * `--dry-run` computes the full plan without writing;
//! * the repair is idempotent: completion is recorded in `maintenance_log`
//!   and a second run is a no-op;
//! * all writes happen inside a single transaction.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params};

use crate::stable_id;
use tokentree_core::source_kind;

/// Maintenance-log kind for this repair.
pub const SNAPSHOT_OVERCOUNT_REPAIR_KIND: &str = "snapshot_overcount_repair";
/// Bump when the repair algorithm changes.
pub const SNAPSHOT_OVERCOUNT_REPAIR_VERSION: &str = "1";
/// Parser versions whose `snapshot_delta` rows store full cumulatives.
/// Must stay in sync with `tokentree_claude::PARSER_VERSION` history:
// `0.2.0-rust` predates the C4 per-session delta fix (`0.2.1-rust`).
pub const PRE_DELTA_FIX_PARSER_VERSIONS: &[&str] = &["0.2.0-rust"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotRepairPlan {
    /// Sessions with at least one candidate row.
    pub sessions: u64,
    /// Candidate rows that would be rewritten.
    pub rows: u64,
    /// First-row baselines (token columns zeroed).
    pub baseline_rows: u64,
    /// Rows rewritten to true deltas.
    pub delta_rows: u64,
    /// Backward counter moves (zeroed + anomaly recorded).
    pub anomaly_rows: u64,
}

#[derive(Debug, Clone)]
pub struct SnapshotRepairOutcome {
    pub run_id: String,
    pub plan: SnapshotRepairPlan,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
struct SnapshotRow {
    id: String,
    session_id: Option<String>,
    input: Option<i64>,
    cached_in: Option<i64>,
    cache_write: Option<i64>,
    output: Option<i64>,
    reasoning: Option<i64>,
}

fn ensure_repair_tables(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshot_repair_backups (
            repair_run_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            input_tokens INTEGER,
            cached_input_tokens INTEGER,
            cache_write_tokens INTEGER,
            output_tokens INTEGER,
            reasoning_tokens INTEGER,
            backed_up_at TEXT NOT NULL,
            PRIMARY KEY (repair_run_id, event_id)
        );
        CREATE TABLE IF NOT EXISTS maintenance_log (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            version TEXT NOT NULL,
            details_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn repair_already_completed(connection: &Connection, kind: &str, version: &str) -> Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM maintenance_log WHERE kind = ?1 AND version = ?2",
        params![kind, version],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn load_candidate_rows(connection: &Connection) -> Result<Vec<SnapshotRow>> {
    let version_list = PRE_DELTA_FIX_PARSER_VERSIONS
        .iter()
        .map(|v| format!("'{v}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, session_id,
                input_tokens, cached_input_tokens, cache_write_tokens,
                output_tokens, reasoning_tokens
         FROM usage_events
         WHERE source_kind = '{}' AND parser_version IN ({version_list})
         ORDER BY coalesce(session_id, ''), observed_at, source_offset",
        source_kind::SNAPSHOT_DELTA
    );
    let mut stmt = connection.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SnapshotRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                input: row.get(2)?,
                cached_in: row.get(3)?,
                cache_write: row.get(4)?,
                output: row.get(5)?,
                reasoning: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
    Ok(rows)
}

type TokenTuple = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn row_values(row: &SnapshotRow) -> TokenTuple {
    (
        row.input,
        row.cached_in,
        row.cache_write,
        row.output,
        row.reasoning,
    )
}

/// Mirror of the fixed parser's `snapshot_delta`: per-field `after - before`.
/// Returns `None` when any present-on-both field moved backwards (counter
/// reset or reorder) — the parser detects this via `u64` underflow; stored
/// rows are `i64`, so the comparison is explicit. A field missing on either
/// side stays missing.
fn delta_fields(before: TokenTuple, after: TokenTuple) -> Option<TokenTuple> {
    let diff = |b: Option<i64>, a: Option<i64>| -> Option<Option<i64>> {
        match (b, a) {
            (Some(b), Some(a)) if a < b => None,
            (Some(b), Some(a)) => Some(Some(a - b)),
            _ => Some(None),
        }
    };
    Some((
        diff(before.0, after.0)?,
        diff(before.1, after.1)?,
        diff(before.2, after.2)?,
        diff(before.3, after.3)?,
        diff(before.4, after.4)?,
    ))
}

const ZEROED: TokenTuple = (Some(0), Some(0), Some(0), Some(0), Some(0));

/// Drop the append-only guard triggers for the duration of a repair
/// transaction; the caller must recreate them before committing. Mirrors the
/// C2 vocabulary migration's approach: the repair is an explicit,
/// backed-up, operator-invoked correction, not silent history rewriting.
fn drop_append_only_triggers(tx: &rusqlite::Transaction) -> Result<()> {
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS usage_events_no_update;
         DROP TRIGGER IF EXISTS usage_events_no_delete;",
    )?;
    Ok(())
}

fn recreate_append_only_triggers(tx: &rusqlite::Transaction) -> Result<()> {
    tx.execute_batch(
        "CREATE TRIGGER usage_events_no_update BEFORE UPDATE ON usage_events
         BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;
         CREATE TRIGGER usage_events_no_delete BEFORE DELETE ON usage_events
         BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;",
    )?;
    Ok(())
}

/// Compute the repair plan (and, for `apply`, the per-row new values) without
/// writing anything. Returns the plan plus, per row id, the replacement
/// token tuple and whether the row is a backward-move anomaly.
fn compute_plan(rows: &[SnapshotRow]) -> (SnapshotRepairPlan, Vec<(String, TokenTuple, bool)>) {
    let mut plan = SnapshotRepairPlan::default();
    let mut rewrites: Vec<(String, TokenTuple, bool)> = Vec::new();
    // Rows arrive ordered by (session, observed_at, source_offset).
    let mut current_session: Option<&str> = None;
    let mut prior: Option<TokenTuple> = None;

    for row in rows {
        let session_key = row.session_id.as_deref().unwrap_or("");
        if current_session != Some(session_key) {
            current_session = Some(session_key);
            prior = None;
            plan.sessions += 1;
        }
        let current = row_values(row);
        let (new_values, is_anomaly) = match prior {
            // First snapshot per session: establishes the baseline; the
            // stored cumulative is superseded (zeroed), exactly as the fixed
            // parser emits nothing for it.
            None => (ZEROED, false),
            Some(prev) => match delta_fields(prev, current) {
                Some(delta) => (delta, false),
                // Backward move: re-baseline, contribute nothing, flag it.
                None => (ZEROED, true),
            },
        };
        plan.rows += 1;
        if prior.is_none() {
            plan.baseline_rows += 1;
        } else if is_anomaly {
            plan.anomaly_rows += 1;
        } else {
            plan.delta_rows += 1;
        }
        // Deltas are always diffed against the *original* cumulative values.
        prior = Some(current);
        rewrites.push((row.id.clone(), new_values, is_anomaly));
    }
    (plan, rewrites)
}

/// Read-only preview of what `apply_snapshot_overcount_repair` would change.
pub fn plan_snapshot_overcount_repair(connection: &Connection) -> Result<SnapshotRepairPlan> {
    ensure_repair_tables(connection)?;
    if repair_already_completed(
        connection,
        SNAPSHOT_OVERCOUNT_REPAIR_KIND,
        SNAPSHOT_OVERCOUNT_REPAIR_VERSION,
    )? {
        return Ok(SnapshotRepairPlan::default());
    }
    let rows = load_candidate_rows(connection)?;
    Ok(compute_plan(&rows).0)
}

/// Apply the repair: backup, rewrite, record. All in one transaction.
/// Returns an empty plan (no-op) when the repair already completed.
pub fn apply_snapshot_overcount_repair(
    connection: &mut Connection,
) -> Result<SnapshotRepairOutcome> {
    ensure_repair_tables(connection)?;
    if repair_already_completed(
        connection,
        SNAPSHOT_OVERCOUNT_REPAIR_KIND,
        SNAPSHOT_OVERCOUNT_REPAIR_VERSION,
    )? {
        return Ok(SnapshotRepairOutcome {
            run_id: String::new(),
            plan: SnapshotRepairPlan::default(),
            dry_run: false,
        });
    }
    let rows = load_candidate_rows(connection)?;
    let (plan, rewrites) = compute_plan(&rows);
    if plan.rows == 0 {
        // Nothing to repair: stay a no-op and do not record a completion.
        return Ok(SnapshotRepairOutcome {
            run_id: String::new(),
            plan,
            dry_run: false,
        });
    }
    let run_id = format!("repair_{}", Utc::now().format("%Y%m%dT%H%M%S%.3f"));
    let backed_up_at = Utc::now().to_rfc3339();

    let tx = connection.transaction()?;
    // The repair rewrites history by operator request: drop the append-only
    // guards for this transaction only; they are recreated before commit.
    // A rollback therefore always leaves the guards (and the data) intact.
    drop_append_only_triggers(&tx)?;
    // 1. Backup every affected row before touching it.
    {
        let mut backup = tx.prepare(
            "INSERT INTO snapshot_repair_backups
             (repair_run_id, event_id, input_tokens, cached_input_tokens,
              cache_write_tokens, output_tokens, reasoning_tokens, backed_up_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in &rows {
            backup.execute(params![
                run_id,
                row.id,
                row.input,
                row.cached_in,
                row.cache_write,
                row.output,
                row.reasoning,
                backed_up_at,
            ])?;
        }
    }
    // 2. Rewrite token columns to true per-session deltas.
    {
        let mut update = tx.prepare(
            "UPDATE usage_events
             SET input_tokens = ?1, cached_input_tokens = ?2, cache_write_tokens = ?3,
                 output_tokens = ?4, reasoning_tokens = ?5
             WHERE id = ?6",
        )?;
        for (id, values, _) in &rewrites {
            let changed = update.execute(params![
                values.0, values.1, values.2, values.3, values.4, id
            ])?;
            if changed != 1 {
                bail!("repair expected to rewrite exactly one row for {id}");
            }
        }
    }
    // 3. Record backward-move anomalies for traceability.
    {
        let mut anomaly = tx.prepare(
            "INSERT INTO measurement_anomalies
             (id, session_id, turn_id, type, source_values_json, created_at)
             VALUES (?1, ?2, NULL, 'snapshot_repair_negative_delta', ?3, ?4)",
        )?;
        for (id, _, is_anomaly) in &rewrites {
            if *is_anomaly {
                let row = rows
                    .iter()
                    .find(|r| &r.id == id)
                    .context("repair row vanished")?;
                let now = Utc::now().to_rfc3339();
                let anomaly_id = stable_id(
                    "anom",
                    &format!("snapshot_repair_negative_delta:{run_id}:{id}"),
                );
                let values_json = serde_json::json!({
                    "repair_run_id": run_id,
                    "event_id": id,
                    "original_values": {
                        "input_tokens": row.input,
                        "cached_input_tokens": row.cached_in,
                        "cache_write_tokens": row.cache_write,
                        "output_tokens": row.output,
                        "reasoning_tokens": row.reasoning,
                    },
                })
                .to_string();
                anomaly.execute(params![anomaly_id, row.session_id, values_json, now,])?;
            }
        }
    }
    // 4. Record completion so re-runs are no-ops.
    {
        let details = serde_json::json!({
            "run_id": run_id,
            "sessions": plan.sessions,
            "rows": plan.rows,
            "baseline_rows": plan.baseline_rows,
            "delta_rows": plan.delta_rows,
            "anomaly_rows": plan.anomaly_rows,
        })
        .to_string();
        tx.execute(
            "INSERT INTO maintenance_log (id, kind, version, details_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                stable_id(
                    "maint",
                    &format!("{SNAPSHOT_OVERCOUNT_REPAIR_KIND}:{run_id}")
                ),
                SNAPSHOT_OVERCOUNT_REPAIR_KIND,
                SNAPSHOT_OVERCOUNT_REPAIR_VERSION,
                details,
                Utc::now().to_rfc3339(),
            ],
        )?;
    }
    recreate_append_only_triggers(&tx)?;
    tx.commit()?;
    Ok(SnapshotRepairOutcome {
        run_id,
        plan,
        dry_run: false,
    })
}

/// Restore the most recent (or specified) repair run's original rows.
/// Also clears the completion record so a future repair may run again.
pub fn restore_snapshot_repair(
    connection: &mut Connection,
    run_id: Option<&str>,
) -> Result<SnapshotRepairOutcome> {
    ensure_repair_tables(connection)?;
    let target_run: Option<String> = match run_id {
        Some(id) => Some(id.to_string()),
        None => connection
            .query_row(
                "SELECT repair_run_id FROM snapshot_repair_backups
                 ORDER BY backed_up_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok(),
    };
    let Some(target_run) = target_run else {
        bail!("no snapshot-repair backup found to restore");
    };
    let rows: Vec<(String, TokenTuple)> = connection
        .prepare(
            "SELECT event_id, input_tokens, cached_input_tokens, cache_write_tokens,
                    output_tokens, reasoning_tokens
             FROM snapshot_repair_backups WHERE repair_run_id = ?1",
        )?
        .query_map([target_run.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ),
            ))
        })?
        .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
    if rows.is_empty() {
        bail!("no snapshot-repair backup found for run {target_run}");
    }

    let tx = connection.transaction()?;
    drop_append_only_triggers(&tx)?;
    {
        let mut update = tx.prepare(
            "UPDATE usage_events
             SET input_tokens = ?1, cached_input_tokens = ?2, cache_write_tokens = ?3,
                 output_tokens = ?4, reasoning_tokens = ?5
             WHERE id = ?6",
        )?;
        for (id, values) in &rows {
            update.execute(params![
                values.0, values.1, values.2, values.3, values.4, id
            ])?;
        }
    }
    // Clear the completion record: the DB is back to its pre-repair state,
    // so a future repair must be allowed to run again.
    tx.execute(
        "DELETE FROM maintenance_log WHERE kind = ?1 AND version = ?2",
        params![
            SNAPSHOT_OVERCOUNT_REPAIR_KIND,
            SNAPSHOT_OVERCOUNT_REPAIR_VERSION
        ],
    )?;
    recreate_append_only_triggers(&tx)?;
    tx.commit()?;
    Ok(SnapshotRepairOutcome {
        run_id: target_run,
        plan: SnapshotRepairPlan {
            rows: rows.len() as u64,
            ..SnapshotRepairPlan::default()
        },
        dry_run: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;

    fn insert_session(conn: &Connection, session: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO sessions
             (id, adapter, provider_session_id, started_at)
             VALUES (?1, 'claude', ?1, '2026-01-01T00:00:00Z')",
            [session],
        )
        .unwrap();
    }

    /// Insert a pre-fix `snapshot_delta` row storing full cumulatives.
    fn insert_snapshot_row(
        conn: &Connection,
        id: &str,
        session: &str,
        offset: i64,
        tokens: (i64, i64, i64, i64, i64),
        parser_version: &str,
    ) {
        insert_session(conn, session);
        conn.execute(
            "INSERT INTO usage_events
             (id, adapter, source_kind, session_id, observed_at, ingested_at,
              input_tokens, cached_input_tokens, cache_write_tokens,
              output_tokens, reasoning_tokens,
              source_offset, event_hash, adapter_version, parser_version)
             VALUES (?1, 'claude', 'snapshot_delta', ?2,
              '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
              ?3, ?4, ?5, ?6, ?7, ?8, ?9, '0.2.0-rust', ?10)",
            params![
                id,
                session,
                tokens.0,
                tokens.1,
                tokens.2,
                tokens.3,
                tokens.4,
                offset,
                format!("testhash_{id}"),
                parser_version,
            ],
        )
        .unwrap();
    }

    fn insert_transcript_row(conn: &Connection, id: &str, session: &str) {
        insert_session(conn, session);
        conn.execute(
            "INSERT INTO usage_events
             (id, adapter, source_kind, session_id, observed_at, ingested_at,
              input_tokens, source_offset, event_hash, adapter_version, parser_version)
             VALUES (?1, 'claude', 'transcript_request', ?2,
              '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
              42, 0, ?3, '0.2.0-rust', '0.2.0-rust')",
            params![id, session, format!("testhash_{id}")],
        )
        .unwrap();
    }

    fn token_sums(conn: &Connection, kind: &str) -> (i64, i64, i64, i64, i64) {
        conn.query_row(
            "SELECT coalesce(sum(input_tokens),0), coalesce(sum(cached_input_tokens),0),
                    coalesce(sum(cache_write_tokens),0), coalesce(sum(output_tokens),0),
                    coalesce(sum(reasoning_tokens),0)
             FROM usage_events WHERE source_kind = ?1",
            [kind],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    }

    fn row_tokens(conn: &Connection, id: &str) -> TokenTuple {
        conn.query_row(
            "SELECT input_tokens, cached_input_tokens, cache_write_tokens,
                    output_tokens, reasoning_tokens
             FROM usage_events WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    }

    /// Three cumulative snapshots: baseline, growth, then a backward move.
    fn seed_overcounted(conn: &Connection) {
        insert_snapshot_row(conn, "r1", "ses_a", 0, (100, 10, 5, 20, 0), "0.2.0-rust");
        insert_snapshot_row(conn, "r2", "ses_a", 1, (150, 15, 8, 30, 2), "0.2.0-rust");
        insert_snapshot_row(conn, "r3", "ses_a", 2, (130, 12, 6, 25, 1), "0.2.0-rust");
    }

    #[test]
    fn dry_run_reports_plan_without_writing() {
        let ledger = Ledger::open_memory().unwrap();
        seed_overcounted(ledger.connection());
        let before = token_sums(ledger.connection(), "snapshot_delta");

        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan.sessions, 1);
        assert_eq!(plan.rows, 3);
        assert_eq!(plan.baseline_rows, 1);
        assert_eq!(plan.delta_rows, 1);
        assert_eq!(plan.anomaly_rows, 1);

        // Dry run changed nothing.
        assert_eq!(token_sums(ledger.connection(), "snapshot_delta"), before);
        assert_eq!(before, (380, 37, 19, 75, 3));
    }

    #[test]
    fn apply_rewrites_to_deltas_and_is_idempotent() {
        let mut ledger = Ledger::open_memory().unwrap();
        seed_overcounted(ledger.connection());

        let outcome = apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        assert!(!outcome.run_id.is_empty());
        assert_eq!(outcome.plan.rows, 3);

        // r1: baseline superseded; r2: true delta; r3: backward move zeroed.
        assert_eq!(
            row_tokens(ledger.connection(), "r1"),
            (Some(0), Some(0), Some(0), Some(0), Some(0))
        );
        assert_eq!(
            row_tokens(ledger.connection(), "r2"),
            (Some(50), Some(5), Some(3), Some(10), Some(2))
        );
        assert_eq!(
            row_tokens(ledger.connection(), "r3"),
            (Some(0), Some(0), Some(0), Some(0), Some(0))
        );
        assert_eq!(
            token_sums(ledger.connection(), "snapshot_delta"),
            (50, 5, 3, 10, 2)
        );

        // Backward move recorded as an anomaly.
        let anomalies: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM measurement_anomalies WHERE type = 'snapshot_repair_negative_delta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(anomalies, 1);

        // Re-running after a completed repair is a no-op.
        let outcome2 = apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        assert!(outcome2.run_id.is_empty());
        assert_eq!(outcome2.plan, SnapshotRepairPlan::default());
        assert_eq!(
            token_sums(ledger.connection(), "snapshot_delta"),
            (50, 5, 3, 10, 2)
        );
        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan, SnapshotRepairPlan::default());
    }

    #[test]
    fn skips_rows_written_by_fixed_parser() {
        let mut ledger = Ledger::open_memory().unwrap();
        // Already deltas, written by the fixed parser: must not be touched.
        insert_snapshot_row(
            ledger.connection(),
            "n1",
            "ses_b",
            0,
            (0, 0, 0, 0, 0),
            "0.2.1-rust",
        );
        insert_snapshot_row(
            ledger.connection(),
            "n2",
            "ses_b",
            1,
            (50, 5, 3, 10, 2),
            "0.2.1-rust",
        );

        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan, SnapshotRepairPlan::default());
        let outcome = apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        assert!(outcome.run_id.is_empty());
        assert_eq!(
            token_sums(ledger.connection(), "snapshot_delta"),
            (50, 5, 3, 10, 2)
        );
    }

    #[test]
    fn backup_and_restore_roundtrip() {
        let mut ledger = Ledger::open_memory().unwrap();
        seed_overcounted(ledger.connection());

        let outcome = apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        let run_id = outcome.run_id.clone();
        assert!(!run_id.is_empty());

        // Backup holds the original cumulative values.
        let backed_up: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM snapshot_repair_backups WHERE repair_run_id = ?1",
                [&run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backed_up, 3);
        let backup_sum: i64 = ledger
            .connection()
            .query_row(
                "SELECT coalesce(sum(input_tokens),0) FROM snapshot_repair_backups WHERE repair_run_id = ?1",
                [&run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backup_sum, 380);

        // Restore puts the originals back and clears the completion record.
        let restored = restore_snapshot_repair(ledger.connection_mut(), None).unwrap();
        assert_eq!(restored.run_id, run_id);
        assert_eq!(
            token_sums(ledger.connection(), "snapshot_delta"),
            (380, 37, 19, 75, 3)
        );
        // A repair may run again after a restore.
        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan.rows, 3);
    }

    #[test]
    fn restore_unknown_run_fails() {
        let mut ledger = Ledger::open_memory().unwrap();
        let err = restore_snapshot_repair(ledger.connection_mut(), Some("nope")).unwrap_err();
        assert!(err.to_string().contains("no snapshot-repair backup"));
    }

    #[test]
    fn single_snapshot_session_zeroes_baseline() {
        let mut ledger = Ledger::open_memory().unwrap();
        insert_snapshot_row(
            ledger.connection(),
            "s1",
            "ses_single",
            0,
            (100, 10, 5, 20, 1),
            "0.2.0-rust",
        );

        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan.sessions, 1);
        assert_eq!(plan.baseline_rows, 1);
        assert_eq!(plan.delta_rows, 0);

        apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        assert_eq!(
            row_tokens(ledger.connection(), "s1"),
            (Some(0), Some(0), Some(0), Some(0), Some(0))
        );
    }

    #[test]
    fn interleaved_kinds_and_sessions_untouched() {
        let mut ledger = Ledger::open_memory().unwrap();
        seed_overcounted(ledger.connection());
        insert_transcript_row(ledger.connection(), "t1", "ses_a");
        insert_snapshot_row(
            ledger.connection(),
            "o1",
            "ses_other",
            0,
            (200, 20, 10, 40, 4),
            "0.2.0-rust",
        );
        insert_snapshot_row(
            ledger.connection(),
            "o2",
            "ses_other",
            1,
            (260, 26, 13, 52, 5),
            "0.2.0-rust",
        );

        let plan = plan_snapshot_overcount_repair(ledger.connection()).unwrap();
        assert_eq!(plan.sessions, 2);
        assert_eq!(plan.rows, 5);

        apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        // transcript_request row untouched.
        assert_eq!(
            token_sums(ledger.connection(), "transcript_request"),
            (42, 0, 0, 0, 0)
        );
        // Other session repaired independently: baseline + delta.
        assert_eq!(
            row_tokens(ledger.connection(), "o1"),
            (Some(0), Some(0), Some(0), Some(0), Some(0))
        );
        assert_eq!(
            row_tokens(ledger.connection(), "o2"),
            (Some(60), Some(6), Some(3), Some(12), Some(1))
        );
    }

    #[test]
    fn missing_fields_stay_missing_in_deltas() {
        let mut ledger = Ledger::open_memory().unwrap();
        insert_session(ledger.connection(), "ses_m");
        ledger
            .connection()
            .execute(
                "INSERT INTO usage_events
                 (id, adapter, source_kind, session_id, observed_at, ingested_at,
                  input_tokens, output_tokens,
                  source_offset, event_hash, adapter_version, parser_version)
                 VALUES ('m1', 'claude', 'snapshot_delta', 'ses_m',
                  '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                  100, 20, 0, 'testhash_m1', '0.2.0-rust', '0.2.0-rust')",
                [],
            )
            .unwrap();
        ledger
            .connection()
            .execute(
                "INSERT INTO usage_events
                 (id, adapter, source_kind, session_id, observed_at, ingested_at,
                  input_tokens, output_tokens,
                  source_offset, event_hash, adapter_version, parser_version)
                 VALUES ('m2', 'claude', 'snapshot_delta', 'ses_m',
                  '2026-01-01T00:00:01Z', '2026-01-01T00:00:01Z',
                  150, 30, 1, 'testhash_m2', '0.2.0-rust', '0.2.0-rust')",
                [],
            )
            .unwrap();

        apply_snapshot_overcount_repair(ledger.connection_mut()).unwrap();
        // NULL fields stay NULL through the delta, mirroring the parser.
        assert_eq!(
            row_tokens(ledger.connection(), "m2"),
            (Some(50), None, None, Some(10), None)
        );
    }
}
