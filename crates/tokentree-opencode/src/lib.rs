// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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

pub const ADAPTER_NAME: &str = "opencode";
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
pub struct OpencodeAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct OpencodeParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub last_message_id: Option<String>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<OpencodeAnomaly>,
    pub stats: ParseStats,
    pub final_state: OpencodeParserState,
}

pub type OpencodeImportResult = tokentree_core::AdapterImportResult;

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "opencode.db"
            || (name.ends_with(".db")
                && !name.contains("summaries")
                && !name.starts_with("data")
                && !name.starts_with("session-store")
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
            name == "opencode.db"
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

/// Convert USD float to microdollars (1 USD = 1,000,000 micros).
#[must_use]
pub fn dollars_to_micros(dollars: f64) -> u64 {
    if dollars <= 0.0 || !dollars.is_finite() {
        0
    } else {
        (dollars * 1_000_000.0).round() as u64
    }
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
    let mut state = OpencodeParserState::default();

    // Check user_version for unsupported schema guard
    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if user_version >= 9999 {
        stats.unsupported += 1;
        stats.anomalies += 1;
        anomalies.push(OpencodeAnomaly {
            anomaly_type: "unsupported_version".to_string(),
            session_id: None,
            turn_id: None,
            source_path: source_path_str.clone(),
            source_offset: 0,
            details: serde_json::json!({ "user_version": user_version }),
        });
        return Ok(ParseResult {
            observations,
            anomalies,
            stats,
            final_state: state,
        });
    }

    let mut message_count = 0usize;

    // Check whether message table exists
    let has_message_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='message'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if has_message_table {
        let query =
            "SELECT id, session_id, time_created, data FROM message ORDER BY time_created ASC";
        if let Ok(mut stmt) = conn.prepare(query) {
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            });

            if let Ok(rows) = rows {
                for r in rows {
                    let (id, session_id, time_created, data_json) = match r {
                        Ok(v) => v,
                        Err(e) => {
                            stats.malformed += 1;
                            stats.anomalies += 1;
                            anomalies.push(OpencodeAnomaly {
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

                    let data_str = match data_json {
                        Some(s) if !s.trim().is_empty() => s,
                        _ => continue,
                    };

                    let parsed: Value = match serde_json::from_str(&data_str) {
                        Ok(v) => v,
                        Err(e) => {
                            stats.malformed += 1;
                            stats.anomalies += 1;
                            anomalies.push(OpencodeAnomaly {
                                anomaly_type: "malformed_json".to_string(),
                                session_id: Some(session_id.clone()),
                                turn_id: Some(id.clone()),
                                source_path: source_path_str.clone(),
                                source_offset: 0,
                                details: serde_json::json!({ "error": e.to_string() }),
                            });
                            continue;
                        }
                    };

                    // Check if message has token usage
                    let tokens_val = parsed.get("tokens");
                    if tokens_val.is_none()
                        && parsed.get("role").and_then(Value::as_str) != Some("assistant")
                    {
                        continue;
                    }

                    stats.parsed += 1;
                    message_count += 1;
                    state.active_session_id = Some(session_id.clone());
                    state.last_message_id = Some(id.clone());

                    let timestamp = time_created
                        .and_then(DateTime::from_timestamp_millis)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| Utc::now().to_rfc3339());

                    let model = parsed
                        .get("modelID")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            parsed
                                .get("model")
                                .and_then(|m| m.get("modelID").or_else(|| m.get("id")))
                                .and_then(Value::as_str)
                        })
                        .map(|s| s.to_string());

                    let agent = parsed
                        .get("agent")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());

                    let token_usage = if let Some(t) = tokens_val {
                        let inp = t.get("input").and_then(Value::as_u64);
                        let out = t.get("output").and_then(Value::as_u64);
                        let reas = t.get("reasoning").and_then(Value::as_u64);
                        let (cache_read, cache_write) = if let Some(c) = t.get("cache") {
                            (
                                c.get("read").and_then(Value::as_u64),
                                c.get("write").and_then(Value::as_u64),
                            )
                        } else {
                            (None, None)
                        };
                        TokenUsage {
                            input_tokens: inp,
                            cached_input_tokens: cache_read,
                            cache_write_tokens: cache_write,
                            output_tokens: out,
                            reasoning_tokens: reas,
                        }
                    } else {
                        TokenUsage::default()
                    };

                    let cost_micros = parsed
                        .get("cost")
                        .and_then(Value::as_f64)
                        .map(dollars_to_micros);

                    let is_unmeasured = !token_usage.is_measured();
                    let (source, subtype) = if is_unmeasured {
                        stats.anomalies += 1;
                        anomalies.push(OpencodeAnomaly {
                            anomaly_type: "missing_provider_measurements".to_string(),
                            session_id: Some(session_id.clone()),
                            turn_id: Some(id.clone()),
                            source_path: source_path_str.clone(),
                            source_offset: 0,
                            details: serde_json::json!({ "message_id": id }),
                        });
                        (
                            MeasurementSource::Unavailable,
                            source_kind::OPENCODE_UNMEASURED.to_string(),
                        )
                    } else {
                        (
                            MeasurementSource::ProviderFields,
                            source_kind::OPENCODE_MESSAGE_USAGE.to_string(),
                        )
                    };

                    let obs = UsageObservation {
                        adapter: ADAPTER_NAME.to_string(),
                        source,
                        source_subtype: Some(subtype),
                        source_event_id: Some(format!("opencode:{session_id}:{id}")),
                        provider_session_id: session_id.clone(),
                        request_id: Some(id.clone()),
                        turn_id: Some(id.clone()),
                        agent_id: agent,
                        parent_agent_id: None,
                        source_timestamp: Some(timestamp.clone()),
                        observed_at: timestamp,
                        model,
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

    // Truth ladder fallback: if no message usage rows were parsed, check `session` table
    if message_count == 0 {
        let has_session_table: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='session'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if has_session_table {
            let query = "SELECT id, model, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, agent, time_created FROM session";
            if let Ok(mut stmt) = conn.prepare(query) {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                    ))
                });

                if let Ok(rows) = rows {
                    for r in rows.flatten() {
                        let (
                            id,
                            model_json,
                            cost_val,
                            in_tok,
                            out_tok,
                            reas_tok,
                            read_tok,
                            write_tok,
                            agent,
                            time_created,
                        ) = r;

                        stats.parsed += 1;
                        let timestamp = time_created
                            .and_then(DateTime::from_timestamp_millis)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| Utc::now().to_rfc3339());

                        let model_str = model_json.and_then(|mj| {
                            if let Ok(v) = serde_json::from_str::<Value>(&mj) {
                                v.get("id").and_then(Value::as_str).map(|s| s.to_string())
                            } else {
                                Some(mj)
                            }
                        });

                        let token_usage = TokenUsage {
                            input_tokens: in_tok.map(|v| v.max(0) as u64),
                            cached_input_tokens: read_tok.map(|v| v.max(0) as u64),
                            cache_write_tokens: write_tok.map(|v| v.max(0) as u64),
                            output_tokens: out_tok.map(|v| v.max(0) as u64),
                            reasoning_tokens: reas_tok.map(|v| v.max(0) as u64),
                        };

                        let cost_micros = cost_val.map(dollars_to_micros);

                        let obs = UsageObservation {
                            adapter: ADAPTER_NAME.to_string(),
                            source: MeasurementSource::ProviderFields,
                            source_subtype: Some(source_kind::OPENCODE_SESSION_USAGE.to_string()),
                            source_event_id: Some(format!("opencode:{id}:session_summary")),
                            provider_session_id: id.clone(),
                            request_id: Some(format!("opencode:{id}:session_summary")),
                            turn_id: None,
                            agent_id: agent,
                            parent_agent_id: None,
                            source_timestamp: Some(timestamp.clone()),
                            observed_at: timestamp,
                            model: model_str,
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

pub fn import_opencode_file(
    connection: &mut Connection,
    path: &Path,
) -> Result<OpencodeImportResult> {
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
            "SELECT last_offset, file_hash, parser_version, adapter_state_json, last_event_hash FROM ingestion_checkpoints WHERE adapter = 'opencode' AND source_path = ?1",
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
            return Ok(OpencodeImportResult {
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
            "opencode:{}:{}:{}:{}",
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
        ) VALUES('opencode', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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

    Ok(OpencodeImportResult {
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
