// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tokentree_core::{
    MeasurementSource, TokenUsage, UsageObservation, canonical_source_kind, source_kind,
};
use walkdir::WalkDir;

pub const ADAPTER_NAME: &str = "copilot";
pub const ADAPTER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "-rust");
pub const PARSER_VERSION: &str = "0.2.0-rust";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    pub subagent_tokens_already_in_parent: bool,
}

#[must_use]
pub fn capabilities() -> AdapterCapabilities {
    AdapterCapabilities {
        subagent_tokens_already_in_parent: false,
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub unsupported: u64,
    pub anomalies: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CopilotParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub last_event_id: Option<String>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<CopilotAnomaly>,
    pub stats: ParseStats,
    pub final_state: CopilotParserState,
}

pub type CopilotImportResult = tokentree_core::AdapterImportResult;

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "session-store.db"
            || name == "data.db"
            || (name.ends_with(".db")
                && !name.contains("summaries")
                && !name.starts_with("opencode")
                && !name.starts_with("ledger"))
        {
            return vec![root.to_path_buf()];
        }
        return Vec::new();
    }

    let mut sessions: Vec<_> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            if !entry.file_type().is_file() {
                return false;
            }
            let name = entry.file_name().to_string_lossy();
            name == "session-store.db" || name == "data.db"
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

/// Convert nano AIU (10^-9 USD) to microdollars (10^-6 USD).
#[must_use]
pub const fn nano_aiu_to_micros(nano: u64) -> u64 {
    nano / 1000
}

pub fn parse_session(path: &Path) -> Result<ParseResult> {
    tokentree_core::check_file_size(path).map_err(|e| anyhow::anyhow!(e))?;

    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")?;

    let source_path_str = path.to_string_lossy().to_string();
    let mut observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut state = CopilotParserState::default();

    // Check schema_version if present
    let schema_ver: Option<i64> = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .ok();
    if let Some(v) = schema_ver {
        if v >= 999 {
            stats.unsupported += 1;
            stats.anomalies += 1;
            anomalies.push(CopilotAnomaly {
                anomaly_type: "unsupported_version".to_string(),
                session_id: None,
                turn_id: None,
                source_path: source_path_str.clone(),
                source_offset: 0,
                details: serde_json::json!({ "schema_version": v }),
            });
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: state,
            });
        }
    }

    // Check whether assistant_usage_events table exists
    let has_usage_events: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='assistant_usage_events'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let mut event_count = 0usize;

    if has_usage_events {
        let query = "SELECT id, session_id, turn_index, agent_id, parent_tool_call_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, total_nano_aiu, created_at FROM assistant_usage_events ORDER BY rowid ASC";
        if let Ok(mut stmt) = conn.prepare(query) {
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            });

            if let Ok(rows) = rows {
                for r in rows {
                    let (
                        id,
                        session_id,
                        turn_index,
                        agent_id,
                        parent_tool_call_id,
                        model,
                        input_tokens,
                        output_tokens,
                        cache_read,
                        cache_write,
                        reasoning,
                        nano_aiu,
                        created_at,
                    ) = match r {
                        Ok(v) => v,
                        Err(e) => {
                            stats.malformed += 1;
                            stats.anomalies += 1;
                            anomalies.push(CopilotAnomaly {
                                anomaly_type: "malformed_row".to_string(),
                                session_id: None,
                                turn_id: None,
                                source_path: source_path_str.clone(),
                                source_offset: 0,
                                details: serde_json::json!({ "error": e.to_string() }),
                            });
                            continue;
                        }
                    };

                    stats.parsed += 1;
                    event_count += 1;
                    state.active_session_id = Some(session_id.clone());
                    state.last_event_id = Some(id.clone());

                    let turn_id_str = turn_index.map(|idx| format!("turn_{idx}"));
                    let timestamp = created_at.unwrap_or_else(|| Utc::now().to_rfc3339());

                    let token_usage = TokenUsage {
                        input_tokens: input_tokens.map(|v| v.max(0) as u64),
                        cached_input_tokens: cache_read.map(|v| v.max(0) as u64),
                        cache_write_tokens: cache_write.map(|v| v.max(0) as u64),
                        output_tokens: output_tokens.map(|v| v.max(0) as u64),
                        reasoning_tokens: reasoning.map(|v| v.max(0) as u64),
                    };

                    let is_unmeasured = !token_usage.is_measured();
                    let (source, subtype) = if is_unmeasured {
                        stats.anomalies += 1;
                        anomalies.push(CopilotAnomaly {
                            anomaly_type: "missing_provider_measurements".to_string(),
                            session_id: Some(session_id.clone()),
                            turn_id: turn_id_str.clone(),
                            source_path: source_path_str.clone(),
                            source_offset: 0,
                            details: serde_json::json!({ "event_id": id }),
                        });
                        (
                            MeasurementSource::Unavailable,
                            source_kind::COPILOT_UNMEASURED.to_string(),
                        )
                    } else {
                        (
                            MeasurementSource::ProviderFields,
                            source_kind::COPILOT_USAGE_EVENT.to_string(),
                        )
                    };

                    let cost_micros = nano_aiu.and_then(|n| {
                        if n > 0 {
                            Some(nano_aiu_to_micros(n as u64))
                        } else {
                            None
                        }
                    });

                    let obs = UsageObservation {
                        adapter: ADAPTER_NAME.to_string(),
                        source,
                        source_subtype: Some(subtype),
                        source_event_id: Some(format!("copilot:{session_id}:{id}")),
                        provider_session_id: session_id.clone(),
                        request_id: Some(id.clone()),
                        turn_id: turn_id_str,
                        agent_id,
                        parent_agent_id: parent_tool_call_id,
                        source_timestamp: Some(timestamp.clone()),
                        observed_at: timestamp,
                        model: model.or_else(|| Some("copilot-default".to_string())),
                        service_tier: None,
                        region: None,
                        usage: token_usage,
                        provider_reported_cost_micros: cost_micros,
                        source_path: source_path_str.clone(),
                        source_offset: 0,
                        adapter_version: ADAPTER_VERSION.to_string(),
                        parser_version: PARSER_VERSION.to_string(),
                    };

                    observations.push(obs);
                }
            }
        }
    }

    // Truth ladder fallback: if no assistant_usage_events exist, inspect `sessions` table
    if event_count == 0 {
        let has_sessions_table: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if has_sessions_table {
            // Check if sessions table has token columns
            let query = "SELECT id, model, total_input_tokens, total_output_tokens, total_cached_tokens, total_reasoning_tokens, total_nano_aiu, created_at FROM sessions WHERE total_input_tokens IS NOT NULL OR total_output_tokens IS NOT NULL";
            if let Ok(mut stmt) = conn.prepare(query) {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                });

                if let Ok(rows) = rows {
                    for r in rows.flatten() {
                        let (
                            id,
                            model,
                            in_tok,
                            out_tok,
                            cached_tok,
                            reas_tok,
                            nano_aiu,
                            created_at,
                        ) = r;
                        stats.parsed += 1;
                        let timestamp = created_at.unwrap_or_else(|| Utc::now().to_rfc3339());
                        let token_usage = TokenUsage {
                            input_tokens: in_tok.map(|v| v.max(0) as u64),
                            cached_input_tokens: cached_tok.map(|v| v.max(0) as u64),
                            cache_write_tokens: None,
                            output_tokens: out_tok.map(|v| v.max(0) as u64),
                            reasoning_tokens: reas_tok.map(|v| v.max(0) as u64),
                        };
                        let cost_micros = nano_aiu.and_then(|n| {
                            if n > 0 {
                                Some(nano_aiu_to_micros(n as u64))
                            } else {
                                None
                            }
                        });
                        let obs = UsageObservation {
                            adapter: ADAPTER_NAME.to_string(),
                            source: MeasurementSource::ProviderFields,
                            source_subtype: Some(source_kind::COPILOT_USAGE_EVENT.to_string()),
                            source_event_id: Some(format!("copilot:{id}:session_rollup")),
                            provider_session_id: id.clone(),
                            request_id: Some(format!("copilot:{id}:session_rollup")),
                            turn_id: None,
                            agent_id: None,
                            parent_agent_id: None,
                            source_timestamp: Some(timestamp.clone()),
                            observed_at: timestamp,
                            model: model.or_else(|| Some("copilot-default".to_string())),
                            service_tier: None,
                            region: None,
                            usage: token_usage,
                            provider_reported_cost_micros: cost_micros,
                            source_path: source_path_str.clone(),
                            source_offset: 0,
                            adapter_version: ADAPTER_VERSION.to_string(),
                            parser_version: PARSER_VERSION.to_string(),
                        };
                        observations.push(obs);
                    }
                }
            }
        }
    }

    Ok(ParseResult {
        observations,
        anomalies,
        stats,
        final_state: state,
    })
}

pub fn import_copilot_file(
    connection: &mut Connection,
    path: &Path,
) -> Result<CopilotImportResult> {
    let source_path_str = path.to_string_lossy().to_string();

    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let file_size = meta.len();

    let file_content = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let file_identity_hash = hex::encode(Sha256::digest(&file_content));

    let modified_at = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    struct CheckpointRow {
        #[allow(dead_code)]
        last_offset: i64,
        file_hash: Option<String>,
        parser_version: Option<String>,
        #[allow(dead_code)]
        adapter_state_json: Option<String>,
        last_event_hash: Option<String>,
    }

    let checkpoint: Option<CheckpointRow> = connection
        .query_row(
            "SELECT last_offset, file_hash, parser_version, adapter_state_json, last_event_hash FROM ingestion_checkpoints WHERE adapter = 'copilot' AND source_path = ?1",
            [&source_path_str],
            |row| {
                Ok(CheckpointRow {
                    last_offset: row.get(0)?,
                    file_hash: row.get(1)?,
                    parser_version: row.get(2)?,
                    adapter_state_json: row.get(3)?,
                    last_event_hash: row.get(4)?,
                })
            },
        )
        .ok();

    if let Some(cp) = &checkpoint {
        if cp.file_hash.as_deref() == Some(&file_identity_hash)
            && cp.parser_version.as_deref() == Some(PARSER_VERSION)
        {
            return Ok(CopilotImportResult {
                inserted: 0,
                duplicates: 0,
                malformed: 0,
                unsupported: 0,
                anomalies: 0,
                anomaly_types: Vec::new(),
                latest_event_identity: cp.last_event_hash.clone(),
                latest_authoritative_timestamp: None,
                start_offset: file_size,
                end_offset: file_size,
            });
        }
    }

    let parse_res = parse_session(path)?;

    let tx = connection.transaction()?;

    let mut inserted = 0_u64;
    let mut duplicates = 0_u64;
    let mut last_event_hash: Option<String> = None;

    for obs in &parse_res.observations {
        let session_id = tokentree_core::session_stable_id(&obs.adapter, &obs.provider_session_id);
        tx.execute(
            "INSERT OR IGNORE INTO sessions(id, adapter, provider_session_id, source_path, started_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![session_id, obs.adapter, obs.provider_session_id, obs.source_path, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
        )?;

        let turn_db_id = if let Some(t_id) = &obs.turn_id {
            let turn_id = tokentree_core::stable_id("turn", &format!("{session_id}:{t_id}"));
            let seq: i64 = tx
                .query_row(
                    "SELECT coalesce(max(sequence_number) + 1, 0) FROM turns WHERE session_id = ?1",
                    [&session_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            tx.execute(
                "INSERT OR IGNORE INTO turns (id, session_id, sequence_number, started_at, prompt_storage_mode) VALUES (?1, ?2, ?3, ?4, 'fingerprint_only')",
                params![turn_id, session_id, seq, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
            )?;
            Some(turn_id)
        } else {
            None
        };

        let evt_hash = obs.event_hash();
        last_event_hash = Some(evt_hash.clone());

        let changed = tx.execute(
            "INSERT OR IGNORE INTO usage_events(
              id, adapter, source_kind, source_event_id, session_id, turn_id, request_id, agent_id, parent_agent_id, source_timestamp,
              observed_at, ingested_at, model, service_tier, region, input_tokens, cached_input_tokens,
              cache_write_tokens, output_tokens, reasoning_tokens, provider_reported_cost_micros,
              source_path, source_offset, event_hash, adapter_version, parser_version
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                format!("evt_{}", &evt_hash[..16]),
                obs.adapter,
                canonical_source_kind(obs),
                obs.source_event_id,
                session_id,
                turn_db_id,
                obs.request_id,
                obs.agent_id,
                obs.parent_agent_id,
                obs.source_timestamp,
                obs.observed_at,
                obs.observed_at,
                obs.model,
                obs.service_tier,
                obs.region,
                obs.usage.input_tokens.map(|v| v as i64),
                obs.usage.cached_input_tokens.map(|v| v as i64),
                obs.usage.cache_write_tokens.map(|v| v as i64),
                obs.usage.output_tokens.map(|v| v as i64),
                obs.usage.reasoning_tokens.map(|v| v as i64),
                obs.provider_reported_cost_micros.map(|v| v as i64),
                obs.source_path,
                obs.source_offset as i64,
                evt_hash,
                obs.adapter_version,
                obs.parser_version,
            ],
        )?;

        if changed == 1 {
            inserted += 1;
        } else {
            duplicates += 1;
        }
    }

    for anom in &parse_res.anomalies {
        let file_id = if anom.source_path.is_empty() {
            &source_path_str
        } else {
            &anom.source_path
        };
        let stable_file_id = hex::encode(&Sha256::digest(file_id.as_bytes())[..8]);
        let raw_key = format!(
            "copilot:{}:{}:{}:{}",
            stable_file_id, anom.source_offset, anom.anomaly_type, PARSER_VERSION
        );
        let anom_id = format!(
            "anom_{}",
            &hex::encode(Sha256::digest(raw_key.as_bytes()))[..16]
        );
        let resolved_session: Option<String> = if let Some(s) = &anom.session_id {
            tx.query_row(
                "SELECT id FROM sessions WHERE id = ?1 OR provider_session_id = ?1 LIMIT 1",
                [s],
                |row| row.get(0),
            )
            .ok()
        } else {
            None
        };
        let resolved_turn: Option<String> = if let Some(t) = &anom.turn_id {
            tx.query_row("SELECT id FROM turns WHERE id = ?1 LIMIT 1", [t], |row| {
                row.get(0)
            })
            .ok()
        } else {
            None
        };

        tx.execute(
            "INSERT OR IGNORE INTO measurement_anomalies (id, session_id, turn_id, type, source_values_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                anom_id,
                resolved_session,
                resolved_turn,
                anom.anomaly_type,
                anom.details.to_string(),
                Utc::now().to_rfc3339(),
            ],
        )?;
    }

    let adapter_state_json = serde_json::to_string(&parse_res.final_state)?;
    tx.execute(
        "INSERT INTO ingestion_checkpoints(
          adapter, source_path, file_size, modified_at, last_offset, last_event_hash, file_hash, parser_version, adapter_state_json
        ) VALUES('copilot', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(adapter, source_path) DO UPDATE SET
          file_size = excluded.file_size,
          modified_at = excluded.modified_at,
          last_offset = excluded.last_offset,
          last_event_hash = excluded.last_event_hash,
          file_hash = excluded.file_hash,
          parser_version = excluded.parser_version,
          adapter_state_json = excluded.adapter_state_json",
        params![
            source_path_str,
            file_size as i64,
            modified_at,
            file_size as i64,
            last_event_hash,
            file_identity_hash,
            PARSER_VERSION,
            adapter_state_json,
        ],
    )?;

    tx.commit()?;

    let mut anomaly_types = BTreeSet::new();
    for anom in &parse_res.anomalies {
        anomaly_types.insert(anom.anomaly_type.clone());
    }
    let unsupported = parse_res
        .anomalies
        .iter()
        .filter(|a| a.anomaly_type == "unsupported_version")
        .count() as u64;

    let latest_event_identity = parse_res
        .observations
        .last()
        .map(|o| o.canonical_identity())
        .or(last_event_hash);

    let latest_authoritative_timestamp = parse_res
        .observations
        .iter()
        .filter_map(|o| {
            o.source_timestamp
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        })
        .map(|dt| dt.with_timezone(&Utc))
        .max();

    Ok(CopilotImportResult {
        inserted,
        duplicates,
        malformed: parse_res.stats.malformed,
        unsupported,
        anomalies: parse_res.anomalies.len() as u64,
        anomaly_types: anomaly_types.into_iter().collect(),
        latest_event_identity,
        latest_authoritative_timestamp,
        start_offset: 0,
        end_offset: file_size,
    })
}
