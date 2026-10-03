pub mod audit;
pub mod corrections;
pub mod export;
pub mod manual;
pub mod pricing;
pub mod prototype;
pub mod repair;
pub mod spool;
pub mod tree;

pub use audit::{LeakageAuditResult, audit_prompt_leakage};

pub use corrections::{
    MergePlan, SplitPlan, add_note, attach_session, detach_session, ensure_session_attribution,
    ensure_session_attribution_for_source, merge_work_items, move_work_item, plan_merge_work_items,
    plan_split_work_item, reclassify_work_item, rename_work_item, split_work_item,
    validate_group_invariant,
};
pub use export::{csv_escape, export_csv, export_html, export_json, html_escape};
pub use manual::{
    ManualCounts, ManualStartInput, ManualStartResult, ManualStopResult, start_manual, stop_manual,
};
pub use pricing::{PricingSummary, apply_price_snapshot};
pub use prototype::{PrototypePreview, apply_prototype, preview_prototype};
pub use repair::{
    PRE_DELTA_FIX_PARSER_VERSIONS, SNAPSHOT_OVERCOUNT_REPAIR_KIND,
    SNAPSHOT_OVERCOUNT_REPAIR_VERSION, SnapshotRepairOutcome, SnapshotRepairPlan,
    apply_snapshot_overcount_repair, plan_snapshot_overcount_repair, restore_snapshot_repair,
    snapshot_generation_conflicts,
};
pub use spool::HookWorkerSummary;
pub use tree::{
    ProjectTree, UsageTotals, WorkTreeNode, format_totals, load_project_trees, query_ledger,
    render_project_trees,
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::fs;
use std::path::{Path, PathBuf};
use tokentree_core::{UsageObservation, deduplicate, source_kind};

const INITIAL_SCHEMA: &str = include_str!("../../../packages/database/migrations/0001_initial.sql");
const SCHEMA_VERSION: i64 = 1;

pub struct Ledger {
    connection: Connection,
    path: PathBuf,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct IngestSummary {
    pub inserted: u64,
    pub duplicates: u64,
    pub unavailable: u64,
    pub conflicts: u64,
    /// Rows superseded by truth-ladder replacement in this batch: a higher-
    /// precedence observation arrived for a request already recorded at a
    /// lower precedence. The old row stays in the ledger (marked via
    /// `usage_events.superseded_by`) and is excluded from aggregates.
    pub superseded: u64,
    /// L10: observations that failed to ingest and were quarantined as
    /// `measurement_anomalies` (type `poison_observation`) instead of
    /// aborting the batch.
    pub poisoned: u64,
}

impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
            set_mode(parent, 0o700)?;
        }
        let connection =
            Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        // PRAGMA journal_mode returns the resulting mode; execute_batch would
        // silently swallow a refusal (e.g. read-only FS, unsupported VFS),
        // leaving the ledger without WAL durability guarantees.
        let journal_mode: String =
            connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            bail!("PRAGMA journal_mode=WAL not honored (got '{journal_mode}')");
        }
        apply_migrations(&connection)?;
        set_mode(path, 0o600)?;
        // L11: WAL mode creates sidecar files (`-wal`, `-shm`) that inherit
        // the process umask, not the 0600 above. They carry the same
        // ledger bytes, so lock them down too when present.
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar = Path::new(&sidecar);
            if sidecar.exists() {
                set_mode(sidecar, 0o600)?;
            }
        }
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub fn open_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        apply_migrations(&connection)?;
        Ok(Self {
            connection,
            path: PathBuf::from(":memory:"),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub fn integrity_check(&self) -> Result<String> {
        Ok(self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?)
    }

    pub fn ingest(&mut self, observations: Vec<UsageObservation>) -> Result<IngestSummary> {
        ingest_observations(&mut self.connection, observations)
    }

    pub fn aggregate_usage(&self) -> Result<AggregateUsage> {
        aggregate_usage_with_policy(&self.connection)
    }

    pub fn apply_price_snapshot(
        &mut self,
        snapshot: &tokentree_core::PriceSnapshot,
    ) -> Result<PricingSummary> {
        pricing::apply_price_snapshot(&mut self.connection, snapshot)
    }

    pub fn process_claude_hook_spool(&mut self, spool_path: &Path) -> Result<HookWorkerSummary> {
        spool::process_claude_hook_spool(&mut self.connection, spool_path)
    }

    pub fn record_anomaly(
        &mut self,
        session_id: Option<&str>,
        turn_id: Option<&str>,
        anomaly_type: &str,
        source_values_json: &str,
    ) -> Result<()> {
        let resolved_session_id: Option<String> = if let Some(s) = session_id {
            self.connection
                .query_row(
                    "SELECT id FROM sessions WHERE id = ?1 OR provider_session_id = ?1 LIMIT 1",
                    [s],
                    |row| row.get(0),
                )
                .ok()
        } else {
            None
        };

        let resolved_turn_id: Option<String> = if let Some(t) = turn_id {
            self.connection
                .query_row("SELECT id FROM turns WHERE id = ?1 LIMIT 1", [t], |row| {
                    row.get(0)
                })
                .ok()
        } else {
            None
        };

        let now = chrono::Utc::now().to_rfc3339();
        let id = stable_id(
            "anom",
            &format!("{}:{}:{}", anomaly_type, now, source_values_json),
        );
        self.connection.execute(
            "INSERT INTO measurement_anomalies (id, session_id, turn_id, type, source_values_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, resolved_session_id, resolved_turn_id, anomaly_type, source_values_json, now],
        )?;
        Ok(())
    }

    pub fn reconcile(&self) -> Result<ReconcileResult> {
        reconcile(&self.connection)
    }

    pub fn audit_prompt_leakage(&self, spool_dir: Option<&Path>) -> Result<LeakageAuditResult> {
        audit_prompt_leakage(&self.connection, spool_dir)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconcileResult {
    pub sessions: u64,
    pub duplicate_request_ids: u64,
    pub unresolved_anomalies: u64,
    pub subagent_reconciliation: String,
    pub duplicate_subagent_counters: u64,
}

pub fn reconcile(connection: &Connection) -> Result<ReconcileResult> {
    let sessions: i64 =
        connection.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?;
    let duplicate_requests: i64 = connection.query_row(
        &format!(
            "SELECT count(*) FROM (
            SELECT request_id FROM usage_events
            WHERE request_id IS NOT NULL AND source_kind NOT IN ({})
              AND superseded_by IS NULL
            GROUP BY request_id HAVING count(*) > 1
        )",
            source_kind::SNAPSHOT_DELTA_SQL_LIST
        ),
        [],
        |row| row.get(0),
    )?;

    // Detect duplicate final-request / lifecycle counters vs request-level events:
    // Any turn or request where there is both a request-level event and a final-request counter event,
    // or multiple final-request counters for the same turn.
    // L6: a failing integrity query must surface, not silently report 0
    // duplicates. reconcile() returns Result precisely so callers see it.
    let duplicate_subagent_counters: i64 = connection.query_row(
        &format!(
            "SELECT count(*) FROM (
            SELECT session_id, turn_id FROM usage_events
            WHERE source_kind IN ({lifecycle})
              AND superseded_by IS NULL
            GROUP BY session_id, turn_id
            HAVING count(*) > 1
            UNION
            SELECT e1.session_id, e1.turn_id
            FROM usage_events e1
            JOIN usage_events e2 ON e1.session_id = e2.session_id
                AND ((e1.turn_id IS NOT NULL AND e1.turn_id = e2.turn_id) OR (e1.request_id IS NOT NULL AND e1.request_id = e2.request_id))
            WHERE e1.source_kind IN ({lifecycle})
              AND e2.source_kind NOT IN ({lifecycle}, {deltas})
              AND e1.superseded_by IS NULL
              AND e2.superseded_by IS NULL
        )",
            lifecycle = source_kind::LIFECYCLE_COUNTER_SQL_LIST,
            deltas = source_kind::SNAPSHOT_DELTA_SQL_LIST,
        ),
        [],
        |row| row.get(0),
    )?;

    let unresolved_anomalies: i64 = connection.query_row(
        "SELECT count(*) FROM measurement_anomalies WHERE resolved_at IS NULL",
        [],
        |row| row.get(0),
    )?;

    // Check adapter capability for subagent tokens
    let cap_row: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT state, detail FROM adapter_capabilities
             WHERE capability = 'subagent_tokens_already_in_parent'
             ORDER BY checked_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    let subagent_reconciliation = match cap_row {
        Some((state, detail)) if state == "available" => {
            let is_in_parent = detail
                .as_deref()
                .map(|d| {
                    let s = d.trim().to_ascii_lowercase();
                    s == "true"
                        || s.contains("\"already_in_parent\":true")
                        || s.contains("\"already_in_parent\": true")
                })
                .unwrap_or(false);
            if is_in_parent {
                "verified: child tokens included in parent (no double-counting)".to_string()
            } else {
                "verified: child tokens independent (rolled up to parent)".to_string()
            }
        }
        Some((state, _)) => format!("degraded: subagent capability state is {state}"),
        None => "degraded: capability unknown or missing".to_string(),
    };

    Ok(ReconcileResult {
        sessions: sessions.max(0) as u64,
        duplicate_request_ids: duplicate_requests.max(0) as u64,
        unresolved_anomalies: unresolved_anomalies.max(0) as u64,
        subagent_reconciliation,
        duplicate_subagent_counters: duplicate_subagent_counters.max(0) as u64,
    })
}

pub fn register_project_root(
    connection: &mut Connection,
    project_id: &str,
    canonical_path: &str,
    root_type: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let fingerprint = stable_id("root", canonical_path);
    connection.execute(
        "INSERT INTO project_roots (project_id, canonical_path, root_type, fingerprint, active, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
         ON CONFLICT (project_id, canonical_path) DO UPDATE SET active = 1, last_seen_at = ?5",
        params![project_id, canonical_path, root_type, fingerprint, now],
    )?;
    Ok(())
}

pub fn deactivate_project_root(connection: &mut Connection, canonical_path: &str) -> Result<()> {
    let changed = connection.execute(
        "UPDATE project_roots SET active = 0 WHERE canonical_path = ?1",
        params![canonical_path],
    )?;
    if changed == 0 {
        bail!("Project root {canonical_path} not found");
    }
    Ok(())
}

pub fn move_project_root(
    connection: &mut Connection,
    old_path: &str,
    new_path: &str,
) -> Result<()> {
    let project_id: String = connection
        .query_row(
            "SELECT project_id FROM project_roots WHERE canonical_path = ?1",
            [old_path],
            |row| row.get(0),
        )
        .with_context(|| format!("Project root {old_path} not found"))?;

    let now = chrono::Utc::now().to_rfc3339();
    let tx = connection.transaction()?;
    tx.execute(
        "UPDATE project_roots SET active = 0 WHERE canonical_path = ?1",
        params![old_path],
    )?;
    let new_fingerprint = stable_id("root", new_path);
    tx.execute(
        "INSERT INTO project_roots (project_id, canonical_path, root_type, fingerprint, active, first_seen_at, last_seen_at)
         VALUES (?1, ?2, 'moved', ?3, 1, ?4, ?4)
         ON CONFLICT (project_id, canonical_path) DO UPDATE SET active = 1, last_seen_at = ?4",
        params![project_id, new_path, new_fingerprint, now],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn ingest_observations(
    connection: &mut Connection,
    observations: Vec<UsageObservation>,
) -> Result<IngestSummary> {
    let mut transaction = connection.transaction()?;
    let summary = ingest_observations_tx(&mut transaction, &observations)?;
    transaction.commit()?;
    Ok(summary)
}

// Canonical session-ID derivation shared by ingest and the CLI import paths.
// (Defined in tokentree-core; re-exported here for compatibility.)
pub use tokentree_core::session_stable_id;

/// Ingest observations inside an already-open transaction. Callers that need
/// to bundle the ingest with further writes atomically (e.g. `stop_manual`)
/// use this directly and commit once; everyone else uses [`ingest_observations`].
pub fn ingest_observations_tx(
    transaction: &mut rusqlite::Transaction<'_>,
    observations: &[UsageObservation],
) -> Result<IngestSummary> {
    let deduped = deduplicate(observations.to_vec());
    let mut summary = IngestSummary {
        conflicts: deduped.conflicts as u64,
        ..IngestSummary::default()
    };

    for observation in &deduped.canonical {
        // L10: per-row savepoint. A poison row rolls back its own partial
        // writes, is quarantined as a measurement anomaly, and the batch
        // continues — one malformed row no longer kills the whole batch.
        let savepoint = transaction.savepoint()?;
        match ingest_one_observation(&savepoint, observation, &mut summary) {
            Ok(()) => savepoint.commit()?,
            Err(err) => {
                drop(savepoint); // rolls back the poison row's partial writes
                summary.poisoned = summary.poisoned.saturating_add(1);
                quarantine_poison_row(transaction, observation, &err)?;
            }
        }
    }
    Ok(summary)
}

/// Single observation ingest, run inside a per-row savepoint by
/// [`ingest_observations_tx`]. Any error aborts just this row.
fn ingest_one_observation(
    connection: &rusqlite::Connection,
    observation: &UsageObservation,
    summary: &mut IngestSummary,
) -> Result<()> {
    let transaction = connection;
    let session_id = session_stable_id(&observation.adapter, &observation.provider_session_id);
    transaction.execute(
        "INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,source_path,started_at) VALUES(?,?,?,?,?)",
        params![session_id, observation.adapter, observation.provider_session_id, observation.source_path, observation.source_timestamp.as_deref().unwrap_or(&observation.observed_at)],
    )?;
    let turn_db_id = if let Some(t_id) = &observation.turn_id {
        let turn_id = stable_id("turn", &format!("{}:{}", session_id, t_id));
        let seq: i64 = transaction
            .query_row(
                "SELECT coalesce(max(sequence_number) + 1, 0) FROM turns WHERE session_id = ?1",
                [&session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        transaction.execute(
            "INSERT OR IGNORE INTO turns (id, session_id, sequence_number, started_at, prompt_storage_mode)
             VALUES (?1, ?2, ?3, ?4, 'fingerprint_only')",
            params![
                turn_id,
                session_id,
                seq,
                observation
                    .source_timestamp
                    .as_deref()
                    .unwrap_or(&observation.observed_at)
            ],
        )?;
        Some(turn_id)
    } else {
        None
    };
    let input_tokens = sql_integer(observation.usage.input_tokens)?;
    let cached_input_tokens = sql_integer(observation.usage.cached_input_tokens)?;
    let cache_write_tokens = sql_integer(observation.usage.cache_write_tokens)?;
    let output_tokens = sql_integer(observation.usage.output_tokens)?;
    let reasoning_tokens = sql_integer(observation.usage.reasoning_tokens)?;
    let provider_cost = sql_integer(observation.provider_reported_cost_micros)?;
    let source_offset = i64::try_from(observation.source_offset)
        .context("source offset exceeds SQLite integer range")?;
    // Cross-batch truth-ladder replacement.
    //
    // Within one batch, `deduplicate()` already keeps the highest-precedence
    // observation per identity. Across batches the ledger used to be
    // first-write-wins: a transcript row ingested in batch 1 would shadow
    // official telemetry for the same request arriving in batch 2,
    // contradicting the documented truth ladder. Now, when the incoming
    // observation's identity matches an ACTIVE row recorded at a LOWER
    // precedence, the old row is marked superseded and the new row takes
    // its place — atomically, in this transaction, so a crash can never
    // leave a half-replaced pair behind.
    //
    // The ledger stays append-only: rows are never deleted or rewritten.
    // The old row keeps every byte of measurement data; only its
    // `superseded_by` tombstone is set (the one UPDATE the append-only
    // triggers permit). Aggregates exclude superseded rows, so exactly
    // one row per measured request is ever counted, and the old+new pair
    // remains as an audit trail.
    let identity = observation.canonical_identity();
    let hash_new = tokentree_core::sha256_hex(identity.as_bytes());
    // Backward compatibility (identity scheme v1): rows ingested before
    // the H6 fix carry `sha256({adapter}:request:{request_id})` as their
    // event hash. Looking both schemes up keeps re-imports duplicate-free
    // with zero accounting drift. See IDENTITY_SCHEME_VERSION.
    let hash_old = tokentree_core::sha256_hex(observation.canonical_identity_v1().as_bytes());
    let rank_new = observation.source.rank();

    // Active rows for this logical request under either identity scheme.
    // `logical_event_hash` is the base-identity hash of the row that
    // started the supersede chain; rows written before the column existed
    // fall back to `event_hash`. Prefer the current-scheme match on ties.
    //
    // V2: the v1-identity fallback (`hash_old`) is NOT session-scoped
    // (`{adapter}:request:{request_id}`), so without the session
    // predicate below a v1-era row from session s1 would falsely match
    // a genuinely new post-upgrade observation from session s2 on
    // request_id reuse — silently dropping it as a "duplicate" or
    // wrongly superseding s1's row. The v2 hash already embeds the
    // session, so scoping the whole match by session_id loses nothing:
    // true re-imports are always same-session.
    let matched: Vec<(String, String)> = transaction
        .prepare(
            "SELECT id, source_kind FROM usage_events
             WHERE superseded_by IS NULL
               AND session_id = ?3
               AND ((logical_event_hash IN (?1, ?2))
                 OR (logical_event_hash IS NULL AND event_hash IN (?1, ?2)))
             ORDER BY CASE WHEN (logical_event_hash = ?1 OR (logical_event_hash IS NULL AND event_hash = ?1)) THEN 0 ELSE 1 END",
        )?
        .query_map(params![hash_new, hash_old, &session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<_, _>>()?;

    // Insert one event row. Returns 1 when the row was new, 0 on conflict.
    let insert_event_row = |id: &str, event_hash: &str, logical_hash: &str| -> Result<usize> {
        transaction
                .execute(
                "INSERT OR IGNORE INTO usage_events(
                  id,adapter,source_kind,source_event_id,session_id,turn_id,request_id,agent_id,parent_agent_id,source_timestamp,
                  observed_at,ingested_at,model,service_tier,region,input_tokens,cached_input_tokens,
                  cache_write_tokens,output_tokens,reasoning_tokens,provider_reported_cost_micros,
                  source_path,source_offset,event_hash,adapter_version,parser_version,logical_event_hash
                ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    id,
                    observation.adapter,
                    // Canonical vocabulary choke point: never write raw
                    // provider-internal record types into source_kind.
                    tokentree_core::canonical_source_kind(observation),
                    observation.source_event_id,
                    session_id,
                    turn_db_id,
                    observation.request_id,
                    observation.agent_id,
                    observation.parent_agent_id,
                    observation.source_timestamp,
                    observation.observed_at,
                    observation.observed_at,
                    observation.model,
                    observation.service_tier,
                    observation.region,
                    input_tokens,
                    cached_input_tokens,
                    cache_write_tokens,
                    output_tokens,
                    reasoning_tokens,
                    provider_cost,
                    observation.source_path,
                    source_offset,
                    event_hash,
                    observation.adapter_version,
                    observation.parser_version,
                    logical_hash,
                ],
                )
                .map_err(anyhow::Error::from)
    };

    let mut inserted_rows = 0u64;
    let mut superseded_rows = 0u64;
    if matched.is_empty() {
        // No active row for this logical request: plain insert. (If a
        // SUPERSEDED row happens to hold this id, OR IGNORE keeps the
        // active superseding row authoritative and we count a duplicate.)
        inserted_rows +=
            insert_event_row(&stable_id("evt", &identity), &hash_new, &hash_new)? as u64;
    } else {
        for (old_id, old_kind) in &matched {
            if rank_new > tokentree_core::rank_of_source_kind(old_kind) {
                // Deterministic supersede identity: replaying this same
                // observation derives the same row id, so replacement is
                // idempotent and never double-inserts.
                let supersede_identity = format!("{identity}\x00supersedes\x00{old_id}");
                let new_id = stable_id("evt", &supersede_identity);
                let new_hash = tokentree_core::sha256_hex(supersede_identity.as_bytes());
                if insert_event_row(&new_id, &new_hash, &hash_new)? == 1 {
                    let marked = transaction.execute(
                        "UPDATE usage_events SET superseded_by = ?1
                         WHERE id = ?2 AND superseded_by IS NULL",
                        params![new_id, old_id],
                    )?;
                    // `marked == 1` always holds: we selected the row as
                    // active inside this same transaction, and the
                    // trigger permits exactly this tombstone transition.
                    debug_assert_eq!(marked, 1);
                    if marked == 1 {
                        inserted_rows += 1;
                        superseded_rows += 1;
                    }
                }
                // If the insert was ignored (id collision with an
                // unrelated row, e.g. the pathological (adapter,
                // source_kind, source_event_id) unique index), the old
                // row stays active: no partial replacement, no crash.
            }
        }
    }
    summary.inserted = summary.inserted.saturating_add(inserted_rows);
    summary.superseded = summary.superseded.saturating_add(superseded_rows);
    if inserted_rows == 0 {
        // Same-or-lower precedence re-ingest: idempotent dedup, and the
        // active row keeps its rank. A lower-precedence observation never
        // displaces a higher-precedence one.
        summary.duplicates = summary.duplicates.saturating_add(1);
    } else if !observation.usage.is_measured() {
        summary.unavailable = summary.unavailable.saturating_add(1);
    }
    Ok(())
}

/// L10: quarantine a poison observation as a measurement anomaly so the
/// failure is visible and auditable instead of killing the batch.
fn quarantine_poison_row(
    transaction: &rusqlite::Transaction<'_>,
    observation: &UsageObservation,
    err: &anyhow::Error,
) -> Result<()> {
    let session_id = session_stable_id(&observation.adapter, &observation.provider_session_id);
    // The per-row savepoint rolled back the poison row's session insert;
    // re-ensure it so the anomaly's FK holds.
    transaction.execute(
        "INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,source_path,started_at) VALUES(?,?,?,?,?)",
        params![
            session_id,
            observation.adapter,
            observation.provider_session_id,
            observation.source_path,
            observation
                .source_timestamp
                .as_deref()
                .unwrap_or(&observation.observed_at)
        ],
    )?;
    let anomaly_id = stable_id(
        "anom",
        &format!(
            "poison:{}:{}:{}",
            observation.adapter,
            observation.provider_session_id,
            observation.request_id.as_deref().unwrap_or("none")
        ),
    );
    let source_values = serde_json::json!({
        "adapter": observation.adapter,
        "provider_session_id": observation.provider_session_id,
        "request_id": observation.request_id,
        "source_kind": format!("{:?}", observation.source),
        "source_path": observation.source_path,
        "source_offset": observation.source_offset,
        "error": format!("{err:#}"),
    });
    transaction.execute(
        "INSERT OR IGNORE INTO measurement_anomalies
         (id, session_id, turn_id, type, source_values_json, created_at)
         VALUES (?1, ?2, NULL, 'poison_observation', ?3, ?4)",
        params![
            anomaly_id,
            session_id,
            source_values.to_string(),
            observation.observed_at,
        ],
    )?;
    Ok(())
}

fn sql_integer(value: Option<u64>) -> Result<Option<i64>> {
    value
        .map(|number| i64::try_from(number).context("value exceeds SQLite integer range"))
        .transpose()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubagentPolicy {
    /// Child/subagent tokens are already counted in parent totals → exclude child events
    AlreadyInParent,
    /// Child/subagent tokens are independent request streams → include all
    Independent,
    /// Capability unknown → include all but mark completeness as degraded
    Unknown,
}

/// Resolve the subagent accounting policy from the adapter_capabilities table.
pub fn resolve_subagent_policy(connection: &Connection) -> SubagentPolicy {
    let cap_row: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT state, detail FROM adapter_capabilities
             WHERE capability = 'subagent_tokens_already_in_parent'
             ORDER BY checked_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match cap_row {
        Some((state, detail)) if state == "available" => {
            let is_in_parent = detail
                .as_deref()
                .map(|d| {
                    let s = d.trim().to_ascii_lowercase();
                    s == "true"
                        || s.contains("\"already_in_parent\":true")
                        || s.contains("\"already_in_parent\": true")
                })
                .unwrap_or(false);
            if is_in_parent {
                SubagentPolicy::AlreadyInParent
            } else {
                SubagentPolicy::Independent
            }
        }
        _ => SubagentPolicy::Unknown,
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct AggregateUsage {
    pub requests: u64,
    pub measured: u64,
    pub unavailable: u64,
    pub anomalous: u64,
    /// Vocabulary/migration notices (e.g. `unmapped_source_kind`).
    /// Informational only — NOT counted against completeness.
    pub vocabulary_notices: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    /// True when subagent capability is unknown and totals may be inaccurate
    pub completeness_degraded: bool,
}

/// Anomaly types that are vocabulary/migration notices rather than
/// measurement gaps. A custom `source_kind` seen during migration is
/// operator information, not missing data — it must not depress the
/// completeness percentage.
pub const VOCABULARY_NOTICE_TYPES: &[&str] = &["unmapped_source_kind"];

/// True when an anomaly `type` is an informational vocabulary/migration
/// notice rather than a measurement-quality problem.
#[must_use]
pub fn is_vocabulary_notice(anomaly_type: &str) -> bool {
    VOCABULARY_NOTICE_TYPES.contains(&anomaly_type)
}

/// Compute aggregate usage, applying the subagent accounting policy.
///
/// - `AlreadyInParent`: exclude events from child sessions (those with
///   `root_session_id IS NOT NULL`, indicating they are sub-sessions) because
///   the parent session's events already include the child's tokens.
/// - `Independent`: include all events — child and parent streams are separate.
/// - `Unknown`: include all events but mark `completeness_degraded = true`.
pub fn aggregate_usage_with_policy(connection: &Connection) -> Result<AggregateUsage> {
    let policy = resolve_subagent_policy(connection);

    // Vocabulary/migration notices are informational, not measurement gaps:
    // they are counted separately and excluded from the completeness math.
    let notice_list = VOCABULARY_NOTICE_TYPES
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",");
    let count_anomalies = |notice_only: bool| -> Result<u64> {
        let query = format!(
            "SELECT count(*) FROM measurement_anomalies WHERE resolved_at IS NULL AND type {}IN ({notice_list})",
            if notice_only { "" } else { "NOT " },
        );
        Ok(connection
            .query_row(&query, [], |row| row.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as u64)
    };
    let anomalous: u64 = count_anomalies(false)?;
    let vocabulary_notices: u64 = count_anomalies(true)?;

    let is_subagent = "(ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id))";
    let is_covered_turn_counter = format!(
        "(ue.source_kind IN ({turn_counters}) AND EXISTS (SELECT 1 FROM usage_events d WHERE d.session_id = ue.session_id AND d.turn_id IS NOT NULL AND d.turn_id = ue.turn_id AND d.superseded_by IS NULL AND d.source_kind NOT IN ({turn_counters}, {lifecycle})))",
        turn_counters = source_kind::TURN_COUNTER_SQL_LIST,
        lifecycle = source_kind::LIFECYCLE_COUNTER_SQL_LIST,
    );

    match policy {
        SubagentPolicy::AlreadyInParent => {
            // Child/subagent events already represented in parent totals:
            // exclude child events so parent totals are not added again.
            let query = format!(
                "SELECT coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 ELSE 1 END), 0),
                 coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(input_tokens),0),coalesce(sum(cached_input_tokens),0),coalesce(sum(cache_write_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(reasoning_tokens),0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ({lifecycle})
                   AND ue.superseded_by IS NULL
                   AND NOT {is_subagent}
                   AND NOT {is_covered_turn_counter}",
                deltas = source_kind::SNAPSHOT_DELTA_SQL_LIST,
                lifecycle = source_kind::LIFECYCLE_COUNTER_SQL_LIST,
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
                        vocabulary_notices,
                        input: row.get::<_, i64>(3)? as u64,
                        cache_read: row.get::<_, i64>(4)? as u64,
                        cache_write: row.get::<_, i64>(5)? as u64,
                        output: row.get::<_, i64>(6)? as u64,
                        reasoning: row.get::<_, i64>(7)? as u64,
                        completeness_degraded: false,
                    })
                })
                .map_err(Into::into)
        }
        SubagentPolicy::Independent => {
            // Independent child request events roll up exactly once.
            let query = format!(
                "SELECT coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 ELSE 1 END), 0),
                 coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(input_tokens),0),coalesce(sum(cached_input_tokens),0),coalesce(sum(cache_write_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(reasoning_tokens),0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ({lifecycle})
                   AND ue.superseded_by IS NULL
                   AND NOT {is_covered_turn_counter}",
                deltas = source_kind::SNAPSHOT_DELTA_SQL_LIST,
                lifecycle = source_kind::LIFECYCLE_COUNTER_SQL_LIST,
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
                        vocabulary_notices,
                        input: row.get::<_, i64>(3)? as u64,
                        cache_read: row.get::<_, i64>(4)? as u64,
                        cache_write: row.get::<_, i64>(5)? as u64,
                        output: row.get::<_, i64>(6)? as u64,
                        reasoning: row.get::<_, i64>(7)? as u64,
                        completeness_degraded: false,
                    })
                })
                .map_err(Into::into)
        }
        SubagentPolicy::Unknown => {
            // When unknown/missing, report degraded completeness rather than silently selecting a policy.
            // Parent events are measured normally.
            // Child request events are unverified, counted in requests and unavailable, tokens not added to measured totals.
            let query = format!(
                "SELECT coalesce(sum(CASE WHEN ue.source_kind IN ({deltas}) THEN 0 ELSE 1 END), 0),
                 coalesce(sum(CASE
                     WHEN ue.source_kind IN ({deltas}) THEN 0
                     WHEN {is_subagent} THEN 0
                     WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1
                     ELSE 0 END), 0),
                 coalesce(sum(CASE
                     WHEN ue.source_kind IN ({deltas}) THEN 0
                     WHEN {is_subagent} THEN 1
                     WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1
                     ELSE 0 END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE input_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE cached_input_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE cache_write_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE output_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE reasoning_tokens END), 0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ({lifecycle})
                   AND ue.superseded_by IS NULL
                   AND NOT {is_covered_turn_counter}",
                deltas = source_kind::SNAPSHOT_DELTA_SQL_LIST,
                lifecycle = source_kind::LIFECYCLE_COUNTER_SQL_LIST,
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
                        vocabulary_notices,
                        input: row.get::<_, i64>(3)? as u64,
                        cache_read: row.get::<_, i64>(4)? as u64,
                        cache_write: row.get::<_, i64>(5)? as u64,
                        output: row.get::<_, i64>(6)? as u64,
                        reasoning: row.get::<_, i64>(7)? as u64,
                        completeness_degraded: true,
                    })
                })
                .map_err(Into::into)
        }
    }
}

fn apply_migrations(connection: &Connection) -> Result<()> {
    let has_metadata: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_metadata')",
        [],
        |row| row.get(0),
    )?;
    if has_metadata {
        let existing: Option<i64> = connection.query_row(
            "SELECT max(schema_version) FROM schema_metadata WHERE migration_state='applied'",
            [],
            |row| row.get(0),
        )?;
        if existing == Some(SCHEMA_VERSION) {
            let _ = connection.execute(
                "ALTER TABLE ingestion_checkpoints ADD COLUMN file_hash TEXT",
                [],
            );
            let _ = connection.execute(
                "ALTER TABLE ingestion_checkpoints ADD COLUMN parser_version TEXT",
                [],
            );
            let _ = connection.execute(
                "ALTER TABLE ingestion_checkpoints ADD COLUMN adapter_state_json TEXT",
                [],
            );
            ensure_ledger_evolution(connection)?;
            migrate_source_kind_vocabulary(connection)?;
            return Ok(());
        }
        bail!("unsupported or incomplete schema version {existing:?}");
    }
    connection.execute_batch("BEGIN EXCLUSIVE;")?;
    let result = (|| -> Result<()> {
        connection.execute_batch(INITIAL_SCHEMA)?;
        connection.execute(
            concat!(
                "INSERT INTO schema_metadata(schema_version,application_version,migration_state,created_at,updated_at) VALUES(1,'",
                env!("CARGO_PKG_VERSION"),
                "','applied',datetime('now'),datetime('now'))"
            ),
            [],
        )?;
        ensure_ledger_evolution(connection)?;
        connection.execute_batch("COMMIT;")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK;");
        return result;
    }
    // Runs in its own transaction; on a fresh database this is a no-op that
    // still records the version row for auditability. It must run outside the
    // `BEGIN EXCLUSIVE` block above (SQLite forbids nested transactions).
    migrate_source_kind_vocabulary(connection)?;
    Ok(())
}

/// Append-only triggers for `usage_events`, with one narrow carve-out: the
/// `superseded_by` tombstone may transition NULL -> non-NULL exactly once,
/// with every other column unchanged. Truth-ladder replacement marks the old
/// row instead of deleting it, so the audit trail survives while measurement
/// data itself stays immutable.
///
/// This SQL is the single definition used everywhere the triggers are
/// (re)created: fresh databases, the evolution step, and the vocabulary
/// migration. The TypeScript side never updates `superseded_by`, so the
/// carve-out is a no-op for it.
const USAGE_EVENTS_TRIGGERS_SQL: &str = "
CREATE TRIGGER usage_events_no_update BEFORE UPDATE ON usage_events
WHEN NOT (
  OLD.superseded_by IS NULL
  AND NEW.superseded_by IS NOT NULL
  AND NEW.id IS OLD.id
  AND NEW.adapter IS OLD.adapter
  AND NEW.source_kind IS OLD.source_kind
  AND NEW.source_event_id IS OLD.source_event_id
  AND NEW.source_process_id IS OLD.source_process_id
  AND NEW.source_sequence IS OLD.source_sequence
  AND NEW.session_id IS OLD.session_id
  AND NEW.prompt_id IS OLD.prompt_id
  AND NEW.turn_id IS OLD.turn_id
  AND NEW.request_id IS OLD.request_id
  AND NEW.agent_id IS OLD.agent_id
  AND NEW.parent_agent_id IS OLD.parent_agent_id
  AND NEW.parent_event_id IS OLD.parent_event_id
  AND NEW.source_timestamp IS OLD.source_timestamp
  AND NEW.observed_at IS OLD.observed_at
  AND NEW.ingested_at IS OLD.ingested_at
  AND NEW.model IS OLD.model
  AND NEW.service_tier IS OLD.service_tier
  AND NEW.region IS OLD.region
  AND NEW.input_tokens IS OLD.input_tokens
  AND NEW.cached_input_tokens IS OLD.cached_input_tokens
  AND NEW.cache_write_tokens IS OLD.cache_write_tokens
  AND NEW.output_tokens IS OLD.output_tokens
  AND NEW.reasoning_tokens IS OLD.reasoning_tokens
  AND NEW.provider_reported_cost_micros IS OLD.provider_reported_cost_micros
  AND NEW.source_path IS OLD.source_path
  AND NEW.source_offset IS OLD.source_offset
  AND NEW.event_hash IS OLD.event_hash
  AND NEW.adapter_version IS OLD.adapter_version
  AND NEW.parser_version IS OLD.parser_version
  AND NEW.logical_event_hash IS OLD.logical_event_hash
)
BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;
CREATE TRIGGER usage_events_no_delete BEFORE DELETE ON usage_events
BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;";

/// Idempotent additive evolution of the ledger schema. Deliberately NOT a
/// `schema_metadata` version bump: the schema is shared with the TypeScript
/// side at version 1, and both binaries refuse databases whose version they
/// do not recognize. New columns are nullable (old rows read as NULL, which
/// the queries treat as "active, base identity"), new tables use
/// `CREATE TABLE IF NOT EXISTS`, and the trigger replacement is a strict
/// superset of the old append-only behavior for any writer that never touches
/// `superseded_by`.
fn ensure_ledger_evolution(connection: &Connection) -> Result<()> {
    // `ALTER TABLE ... ADD COLUMN` fails when the column already exists;
    // that is the expected steady state — ignore it.
    let _ = connection.execute("ALTER TABLE usage_events ADD COLUMN superseded_by TEXT", []);
    let _ = connection.execute(
        "ALTER TABLE usage_events ADD COLUMN logical_event_hash TEXT",
        [],
    );
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS usage_events_superseded ON usage_events(superseded_by);
         CREATE INDEX IF NOT EXISTS usage_events_logical_hash ON usage_events(logical_event_hash)
           WHERE logical_event_hash IS NOT NULL;
         CREATE TABLE IF NOT EXISTS applied_migrations (
           name TEXT PRIMARY KEY,
           version INTEGER NOT NULL,
           applied_at TEXT NOT NULL,
           rows_affected INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE IF NOT EXISTS source_kind_migration_backup (
           event_id TEXT PRIMARY KEY,
           old_source_kind TEXT NOT NULL,
           new_source_kind TEXT NOT NULL,
           migrated_at TEXT NOT NULL
         ) STRICT;",
    )?;
    replace_usage_events_triggers(connection)?;
    Ok(())
}

/// Install [`USAGE_EVENTS_TRIGGERS_SQL`], tolerating concurrent openers.
///
/// Two threads/processes opening the same database can interleave their
/// DROP/CREATE pairs: A drops, B drops, A creates, B's CREATE then fails
/// with "already exists". Every installer writes the identical definition,
/// so retrying the whole DROP+CREATE pair converges: each iteration either
/// installs the triggers cleanly or proves another installer is making
/// progress toward the same end state.
///
/// This is the single source of truth for the append-only triggers,
/// including the truth-ladder `superseded_by` carve-out. `repair.rs` calls
/// it after dropping the guards for an operator-invoked correction; it must
/// never maintain its own stricter copy (V3).
pub(crate) fn replace_usage_events_triggers(connection: &Connection) -> Result<()> {
    for _ in 0..5 {
        connection.execute_batch(
            "DROP TRIGGER IF EXISTS usage_events_no_update;
             DROP TRIGGER IF EXISTS usage_events_no_delete;",
        )?;
        match connection.execute_batch(USAGE_EVENTS_TRIGGERS_SQL) {
            Ok(()) => return Ok(()),
            Err(e) if format!("{e:?}").contains("already exists") => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!("could not install usage_events triggers: lost repeated races with a concurrent opener");
}

// (Defined in tokentree-core; re-exported here for compatibility.)
pub use tokentree_core::stable_id;

/// Explicit allowlist of legacy `source_kind` values and their canonical
/// replacements. These are the only values the migration rewrites:
/// - `"provider_usage"`: the old Claude parser wrote the transcript record
///   type as the kind; this record type meant ProviderFields.
/// - `"assistant"`, `"user"`, `"summary"`: raw Claude transcript record types
///   that the old parser wrote as the kind; every such row was a
///   TranscriptRequest observation.
/// - `"claude_transcript"`: the old direct-write default kind.
///
/// Any value NOT on this list is left untouched and logged as a
/// `measurement_anomalies` row (`unmapped_source_kind`) for operator review.
/// This is deliberately conservative: the previous blanket rewrite mapped
/// *every* unrecognized value to `transcript_request`, which could silently
/// mislabel future vocabulary.
const SOURCE_KIND_ALLOWLIST: &[(&str, &str)] = &[
    (
        "provider_usage",
        tokentree_core::source_kind::PROVIDER_FIELDS,
    ),
    ("assistant", tokentree_core::source_kind::TRANSCRIPT_REQUEST),
    ("user", tokentree_core::source_kind::TRANSCRIPT_REQUEST),
    ("summary", tokentree_core::source_kind::TRANSCRIPT_REQUEST),
    (
        "claude_transcript",
        tokentree_core::source_kind::TRANSCRIPT_REQUEST,
    ),
];

const SOURCE_KIND_MIGRATION_NAME: &str = "normalize_source_kind_vocabulary";
const SOURCE_KIND_MIGRATION_VERSION: i64 = 1;

/// Allowlisted, backup-protected, versioned `source_kind` vocabulary migration.
///
/// - **Allowlisted**: only the values in [`SOURCE_KIND_ALLOWLIST`] are
///   rewritten; anything else is left in place and logged as an anomaly.
/// - **Backup-protected**: every rewritten row is copied to
///   `source_kind_migration_backup` (event id, old kind, new kind) before the
///   UPDATE, so [`restore_source_kind_backup`] can reverse the migration.
/// - **Versioned**: the run is recorded in `applied_migrations`; a second run
///   is a no-op returning 0.
///
/// The whole migration (backup, rewrite, anomaly logging, version record)
/// runs in a single transaction: a failure leaves the database exactly as it
/// was. Returns the number of rows rewritten.
pub fn migrate_source_kind_vocabulary(connection: &Connection) -> Result<u64> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS applied_migrations (
           name TEXT PRIMARY KEY,
           version INTEGER NOT NULL,
           applied_at TEXT NOT NULL,
           rows_affected INTEGER NOT NULL
         ) STRICT;
         CREATE TABLE IF NOT EXISTS source_kind_migration_backup (
           event_id TEXT PRIMARY KEY,
           old_source_kind TEXT NOT NULL,
           new_source_kind TEXT NOT NULL,
           migrated_at TEXT NOT NULL
         ) STRICT;",
    )?;
    let already: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM applied_migrations WHERE name = ?1 AND version = ?2)",
        params![SOURCE_KIND_MIGRATION_NAME, SOURCE_KIND_MIGRATION_VERSION],
        |row| row.get(0),
    )?;
    if already {
        return Ok(0);
    }

    connection.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<u64> {
        // The append-only triggers block UPDATE of source_kind, so drop them
        // for the duration of this transaction; they are reinstalled (carve-out
        // version) before COMMIT. The DROP itself is concurrency-safe
        // (IF EXISTS); the reinstall below goes through the race-tolerant
        // helper.
        connection.execute_batch(
            "DROP TRIGGER IF EXISTS usage_events_no_update;
             DROP TRIGGER IF EXISTS usage_events_no_delete;",
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut rows_affected = 0u64;
        for (legacy, canonical) in SOURCE_KIND_ALLOWLIST {
            // Backup BEFORE the rewrite: one row per affected event.
            let backed_up = connection.execute(
                "INSERT INTO source_kind_migration_backup(event_id, old_source_kind, new_source_kind, migrated_at)
                 SELECT id, source_kind, ?1, ?2 FROM usage_events WHERE source_kind = ?3",
                params![canonical, now, legacy],
            )?;
            let updated = connection.execute(
                "UPDATE usage_events SET source_kind = ?1 WHERE source_kind = ?2",
                params![canonical, legacy],
            )?;
            debug_assert_eq!(backed_up, updated);
            rows_affected += updated as u64;
        }
        // Values outside both the canonical vocabulary and the allowlist are
        // NOT rewritten; log them for operator review instead.
        let mut stmt = connection.prepare("SELECT DISTINCT source_kind FROM usage_events")?;
        let kinds: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        for kind in kinds {
            if tokentree_core::source_kind::is_recognized(&kind) {
                continue;
            }
            if SOURCE_KIND_ALLOWLIST
                .iter()
                .any(|(legacy, _)| *legacy == kind)
            {
                continue;
            }
            let detail = serde_json::json!({
                "source_kind": kind,
                "migration": SOURCE_KIND_MIGRATION_NAME,
            })
            .to_string();
            connection.execute(
                "INSERT OR IGNORE INTO measurement_anomalies(id, type, source_values_json, created_at)
                 VALUES(?1, 'unmapped_source_kind', ?2, datetime('now'))",
                params![
                    stable_id("anom", &format!("unmapped_source_kind:{kind}")),
                    detail
                ],
            )?;
        }
        replace_usage_events_triggers(connection)?;
        connection.execute(
            "INSERT INTO applied_migrations(name, version, applied_at, rows_affected)
             VALUES(?1, ?2, datetime('now'), ?3)",
            params![
                SOURCE_KIND_MIGRATION_NAME,
                SOURCE_KIND_MIGRATION_VERSION,
                rows_affected as i64
            ],
        )?;
        connection.execute_batch("COMMIT;")?;
        Ok(rows_affected)
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK;");
    }
    result
}

/// Reverse [`migrate_source_kind_vocabulary`] using the backup table.
/// Restores every backed-up row's original `source_kind` in a single
/// transaction and returns the number of rows restored. The backup table is
/// kept so the restore itself remains auditable (and repeatable).
pub fn restore_source_kind_backup(connection: &Connection) -> Result<u64> {
    connection.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<u64> {
        // Drop (not replace): the UPDATE below rewrites source_kind, which
        // even the carve-out trigger forbids. Triggers are reinstalled after.
        connection.execute_batch(
            "DROP TRIGGER IF EXISTS usage_events_no_update;
             DROP TRIGGER IF EXISTS usage_events_no_delete;",
        )?;
        let restored = connection.execute(
            "UPDATE usage_events
             SET source_kind = (SELECT old_source_kind FROM source_kind_migration_backup b
                                WHERE b.event_id = usage_events.id)
             WHERE id IN (SELECT event_id FROM source_kind_migration_backup)",
            [],
        )?;
        replace_usage_events_triggers(connection)?;
        connection.execute_batch("COMMIT;")?;
        Ok(restored as u64)
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK;");
    }
    result
}

/// Number of rows currently held in the `source_kind_migration_backup`
/// table (0 when the migration never ran). Used by `--dry-run` previews and
/// by the CLI to report a no-op instead of erroring on an empty backup.
pub fn count_source_kind_backup_rows(connection: &Connection) -> Result<u64> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'source_kind_migration_backup')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM source_kind_migration_backup",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
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
    use tokentree_core::{MeasurementSource, TokenUsage};

    /// L11: the WAL sidecar files must be 0600 like the main DB, not umask.
    #[cfg(unix)]
    #[test]
    fn wal_sidecars_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tt-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("ledger.db");
        {
            let mut ledger = Ledger::open(&db).unwrap();
            // Force WAL activity so the sidecars exist.
            ledger.ingest(vec![observation()]).unwrap();
        }
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db.as_os_str().to_os_string();
            p.push(suffix);
            let p = std::path::Path::new(&p);
            if p.exists() {
                let mode = std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{} has mode {mode:o}", p.display());
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn observation() -> UsageObservation {
        UsageObservation {
            adapter: "claude".into(),
            source: MeasurementSource::TranscriptRequest,
            source_subtype: None,
            source_event_id: None,
            provider_session_id: "s".into(),
            request_id: Some("r".into()),
            turn_id: None,
            agent_id: None,
            parent_agent_id: None,
            source_timestamp: None,
            observed_at: "2026-01-01T00:00:00Z".into(),
            model: Some("fixture".into()),
            service_tier: None,
            region: None,
            usage: TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..TokenUsage::default()
            },
            provider_reported_cost_micros: None,
            source_path: "fixture".into(),
            source_offset: 0,
            adapter_version: "test".into(),
            parser_version: "test".into(),
        }
    }

    #[test]
    fn migration_and_replay_are_idempotent() {
        let mut ledger = Ledger::open_memory().unwrap();
        assert_eq!(ledger.ingest(vec![observation()]).unwrap().inserted, 1);
        assert_eq!(ledger.ingest(vec![observation()]).unwrap().duplicates, 1);
        assert_eq!(ledger.aggregate_usage().unwrap().requests, 1);
        assert_eq!(ledger.integrity_check().unwrap(), "ok");
    }

    /// V2: the v1→v2 identity fallback must not deduplicate across sessions.
    /// A v1-era row (`event_hash = sha256("claude:request:req1")`, no
    /// `logical_event_hash`) from session-A must NOT match a post-upgrade
    /// v2 observation with the same request_id from session-B.
    #[test]
    fn v1_identity_fallback_does_not_dedup_across_sessions() {
        let mut ledger = Ledger::open_memory().unwrap();
        let session_a = tokentree_core::session_stable_id("claude", "session-A");
        let v1_hash = tokentree_core::sha256_hex("claude:request:req1".as_bytes());
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO sessions(id, adapter, provider_session_id, started_at)
                 VALUES(?1, 'claude', 'session-A', '2026-01-01T00:00:00Z')",
                [&session_a],
            )
            .unwrap();
        // Simulate a v1-era row: v1 identity hash, no logical_event_hash.
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_events(
                   id, adapter, source_kind, session_id, request_id,
                   observed_at, ingested_at, event_hash,
                   adapter_version, parser_version
                 ) VALUES('evt_v1', 'claude', 'transcript_request', ?1, 'req1',
                          '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?2,
                          '0.1.0', '0.1.0')",
                params![session_a, v1_hash],
            )
            .unwrap();

        // Same request_id, DIFFERENT session, post-upgrade observation.
        let mut obs_b = observation();
        obs_b.provider_session_id = "session-B".into();
        obs_b.request_id = Some("req1".into());
        let summary = ledger.ingest(vec![obs_b]).unwrap();
        assert_eq!(
            summary.inserted, 1,
            "session-B observation must not dedup against session-A's v1 row"
        );
        assert_eq!(summary.duplicates, 0);

        // Both rows survive as active.
        let active: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_events WHERE superseded_by IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 2);

        // Sanity: a true same-session re-import still dedups.
        let mut obs_a = observation();
        obs_a.provider_session_id = "session-A".into();
        obs_a.request_id = Some("req1".into());
        let summary2 = ledger.ingest(vec![obs_a]).unwrap();
        assert_eq!(summary2.duplicates, 1);
    }
}
