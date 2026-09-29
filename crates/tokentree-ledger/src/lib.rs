// SPDX-License-Identifier: Apache-2.0
pub mod corrections;
pub mod manual;
pub mod pricing;
pub mod prototype;
pub mod spool;
pub mod tree;

pub use corrections::{
    add_note, attach_session, detach_session, ensure_session_attribution, move_work_item,
    reclassify_work_item, rename_work_item,
};
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
        self.connection.query_row(
            "SELECT count(*),
             coalesce(sum(CASE WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END),0),
             coalesce(sum(CASE WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END),0),
             coalesce(sum(input_tokens),0),coalesce(sum(cached_input_tokens),0),coalesce(sum(cache_write_tokens),0),coalesce(sum(output_tokens),0),coalesce(sum(reasoning_tokens),0)
             FROM usage_events",
            [],
            |row| Ok(AggregateUsage {
                requests: row.get::<_, i64>(0)? as u64,
                measured: row.get::<_, i64>(1)? as u64,
                unavailable: row.get::<_, i64>(2)? as u64,
                input: row.get::<_, i64>(3)? as u64,
                cache_read: row.get::<_, i64>(4)? as u64,
                cache_write: row.get::<_, i64>(5)? as u64,
                output: row.get::<_, i64>(6)? as u64,
                reasoning: row.get::<_, i64>(7)? as u64,
            }),
        ).map_err(Into::into)
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

    pub fn reconcile(&self) -> Result<ReconcileResult> {
        reconcile(&self.connection)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconcileResult {
    pub sessions: u64,
    pub duplicate_request_ids: u64,
    pub unresolved_anomalies: u64,
    pub subagent_reconciliation: String,
}

pub fn reconcile(connection: &Connection) -> Result<ReconcileResult> {
    let sessions: i64 =
        connection.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?;
    let duplicate_requests: i64 = connection.query_row(
        "SELECT count(*) FROM (SELECT request_id FROM usage_events WHERE request_id IS NOT NULL GROUP BY request_id HAVING count(*) > 1)",
        [],
        |row| row.get(0),
    )?;
    let unresolved_anomalies: i64 = connection.query_row(
        "SELECT count(*) FROM measurement_anomalies WHERE resolved_at IS NULL",
        [],
        |row| row.get(0),
    )?;

    Ok(ReconcileResult {
        sessions: sessions.max(0) as u64,
        duplicate_request_ids: duplicate_requests.max(0) as u64,
        unresolved_anomalies: unresolved_anomalies.max(0) as u64,
        subagent_reconciliation: "unavailable until adapter capability is verified".to_owned(),
    })
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
              id,adapter,source_kind,source_event_id,session_id,turn_id,request_id,source_timestamp,
              observed_at,ingested_at,model,service_tier,region,input_tokens,cached_input_tokens,
              cache_write_tokens,output_tokens,reasoning_tokens,provider_reported_cost_micros,
              source_path,source_offset,event_hash,adapter_version,parser_version
            ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                stable_id("evt", &observation.canonical_identity()),
                observation.adapter,
                observation.source.as_str(),
                observation.source_event_id,
                session_id,
                observation.turn_id,
                observation.request_id,
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

#[derive(Debug, Eq, PartialEq)]
pub struct AggregateUsage {
    pub requests: u64,
    pub measured: u64,
    pub unavailable: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
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
            source_event_id: None,
            provider_session_id: "s".into(),
            request_id: Some("r".into()),
            turn_id: None,
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
