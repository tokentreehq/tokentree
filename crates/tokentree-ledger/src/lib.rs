pub mod audit;
pub mod corrections;
pub mod export;
pub mod manual;
pub mod pricing;
pub mod prototype;
pub mod spool;
pub mod tree;

pub use audit::{LeakageAuditResult, audit_prompt_leakage};

pub use corrections::{
    add_note, attach_session, detach_session, ensure_session_attribution, merge_work_items,
    move_work_item, reclassify_work_item, rename_work_item, split_work_item,
    validate_group_invariant,
};
pub use export::{export_csv, export_html, export_json, html_escape};
pub use manual::{
    ManualCounts, ManualStartInput, ManualStartResult, ManualStopResult, start_manual, stop_manual,
};
pub use pricing::{PricingSummary, apply_price_snapshot};
pub use prototype::{PrototypePreview, apply_prototype, preview_prototype};
pub use spool::HookWorkerSummary;
pub use tree::{
    ProjectTree, UsageTotals, WorkTreeNode, format_totals, load_project_trees, query_ledger,
    render_project_trees,
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::fs;
use std::path::{Path, PathBuf};
use tokentree_core::{UsageObservation, deduplicate, sha256_hex};

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
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;",
        )?;
        apply_migrations(&connection)?;
        set_mode(path, 0o600)?;
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
        "SELECT count(*) FROM (SELECT request_id FROM usage_events WHERE request_id IS NOT NULL GROUP BY request_id HAVING count(*) > 1)",
        [],
        |row| row.get(0),
    )?;

    // Detect duplicate final-request / lifecycle counters vs request-level events:
    // Any turn or request where there is both a request-level event and a final-request counter event,
    // or multiple final-request counters for the same turn.
    let duplicate_subagent_counters: i64 = connection.query_row(
        "SELECT count(*) FROM (
            SELECT session_id, turn_id FROM usage_events
            WHERE source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
            GROUP BY session_id, turn_id
            HAVING count(*) > 1
            UNION
            SELECT e1.session_id, e1.turn_id
            FROM usage_events e1
            JOIN usage_events e2 ON e1.session_id = e2.session_id
                AND ((e1.turn_id IS NOT NULL AND e1.turn_id = e2.turn_id) OR (e1.request_id IS NOT NULL AND e1.request_id = e2.request_id))
            WHERE e1.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
              AND e2.source_kind NOT IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
        )",
        [],
        |row| row.get(0),
    ).unwrap_or(0);

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
    let deduped = deduplicate(observations);
    let transaction = connection.transaction()?;
    let mut summary = IngestSummary {
        conflicts: deduped.conflicts as u64,
        ..IngestSummary::default()
    };

    for observation in &deduped.canonical {
        let session_id = stable_id(
            "ses",
            &format!(
                "{}:{}",
                observation.adapter, observation.provider_session_id
            ),
        );
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
        let changed = transaction.execute(
            "INSERT OR IGNORE INTO usage_events(
              id,adapter,source_kind,source_event_id,session_id,turn_id,request_id,agent_id,parent_agent_id,source_timestamp,
              observed_at,ingested_at,model,service_tier,region,input_tokens,cached_input_tokens,
              cache_write_tokens,output_tokens,reasoning_tokens,provider_reported_cost_micros,
              source_path,source_offset,event_hash,adapter_version,parser_version
            ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                stable_id("evt", &observation.canonical_identity()),
                observation.adapter,
                observation
                    .source_subtype
                    .as_deref()
                    .unwrap_or(observation.source.as_str()),
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
                observation.event_hash(),
                observation.adapter_version,
                observation.parser_version,
            ],
        )?;
        if changed == 1 {
            summary.inserted += 1;
            if !observation.usage.is_measured() {
                summary.unavailable += 1;
            }
        } else {
            summary.duplicates += 1;
        }
    }
    transaction.commit()?;
    Ok(summary)
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
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    /// True when subagent capability is unknown and totals may be inaccurate
    pub completeness_degraded: bool,
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

    let anomalous: u64 = connection
        .query_row(
            "SELECT count(*) FROM measurement_anomalies WHERE resolved_at IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;

    let is_subagent = "(ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id))";
    let is_covered_turn_counter = "(ue.source_kind IN ('codex_turn_counter', 'turn_counter', 'turn_summary', 'cumulative_turn_counter') AND EXISTS (SELECT 1 FROM usage_events d WHERE d.session_id = ue.session_id AND d.turn_id IS NOT NULL AND d.turn_id = ue.turn_id AND d.source_kind NOT IN ('codex_turn_counter', 'turn_counter', 'turn_summary', 'cumulative_turn_counter', 'final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')))";

    match policy {
        SubagentPolicy::AlreadyInParent => {
            // Child/subagent events already represented in parent totals:
            // exclude child events so parent totals are not added again.
            let query = format!(
                "SELECT count(*),
                 coalesce(sum(CASE WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(CASE WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(input_tokens),0),coalesce(sum(cached_input_tokens),0),coalesce(sum(cache_write_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(reasoning_tokens),0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
                   AND NOT {is_subagent}
                   AND NOT {is_covered_turn_counter}"
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
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
                "SELECT count(*),
                 coalesce(sum(CASE WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(CASE WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END),0),
                 coalesce(sum(input_tokens),0),coalesce(sum(cached_input_tokens),0),coalesce(sum(cache_write_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(reasoning_tokens),0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
                   AND NOT {is_covered_turn_counter}"
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
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
                "SELECT count(*),
                 coalesce(sum(CASE
                     WHEN {is_subagent} THEN 0
                     WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1
                     ELSE 0 END), 0),
                 coalesce(sum(CASE
                     WHEN {is_subagent} THEN 1
                     WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1
                     ELSE 0 END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE input_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE cached_input_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE cache_write_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE output_tokens END), 0),
                 coalesce(sum(CASE WHEN {is_subagent} THEN 0 ELSE reasoning_tokens END), 0)
                 FROM usage_events ue
                 WHERE ue.source_kind NOT IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')
                   AND NOT {is_covered_turn_counter}"
            );
            connection
                .query_row(&query, [], |row| {
                    Ok(AggregateUsage {
                        requests: row.get::<_, i64>(0)? as u64,
                        measured: row.get::<_, i64>(1)? as u64,
                        unavailable: row.get::<_, i64>(2)? as u64,
                        anomalous,
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
            return Ok(());
        }
        bail!("unsupported or incomplete schema version {existing:?}");
    }
    connection.execute_batch("BEGIN EXCLUSIVE;")?;
    let result = (|| -> Result<()> {
        connection.execute_batch(INITIAL_SCHEMA)?;
        connection.execute(
            "INSERT INTO schema_metadata(schema_version,application_version,migration_state,created_at,updated_at) VALUES(1,'0.2.0','applied',datetime('now'),datetime('now'))", [],
        )?;
        connection.execute_batch("COMMIT;")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK;");
    }
    result
}

#[must_use]
pub fn stable_id(prefix: &str, value: &str) -> String {
    format!("{prefix}_{}", &sha256_hex(value.as_bytes())[..24])
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
}
