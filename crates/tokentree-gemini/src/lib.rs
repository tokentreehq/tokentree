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

pub const ADAPTER_NAME: &str = "gemini";
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
pub struct GeminiAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GeminiParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub last_step_index: Option<i64>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<GeminiAnomaly>,
    pub stats: ParseStats,
    pub final_state: GeminiParserState,
}

pub type GeminiImportResult = tokentree_core::AdapterImportResult;

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        let is_db = root.extension().is_some_and(|ext| ext == "db");
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_db
            && !name.contains("summaries")
            && !name.starts_with("data")
            && !name.starts_with("session-store")
            && !name.starts_with("opencode")
            && !name.starts_with("ledger")
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
            let path = entry.path();
            let is_db = path.extension().is_some_and(|ext| ext == "db");
            if !is_db {
                return false;
            }
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name.contains("summaries")
                || file_name == "data.db"
                || file_name == "session-store.db"
                || file_name == "opencode.db"
                || file_name == "ledger.db"
            {
                return false;
            }
            true
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

/// Helper for bounded protobuf varint parsing without external crates.
fn parse_varint(data: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut val: u64 = 0;
    let mut shift = 0;
    while pos < data.len() {
        let b = data[pos];
        pos += 1;
        val |= u64::from(b & 0x7f) << shift;
        if (b & 0x80) == 0 {
            return Some((val, pos));
        }
        shift += 7;
        if shift > 64 {
            return None;
        }
    }
    None
}

/// Lightweight decoded protobuf field.
#[allow(dead_code)]
enum ProtoValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed64([u8; 8]),
    Fixed32([u8; 4]),
}

fn parse_proto_fields<'a>(data: &'a [u8]) -> Vec<(u32, ProtoValue<'a>)> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let (key, next_pos) = match parse_varint(data, pos) {
            Some(v) => v,
            None => break,
        };
        pos = next_pos;
        let field_num = (key >> 3) as u32;
        let wire_type = (key & 7) as u8;
        match wire_type {
            0 => {
                if let Some((val, next_pos)) = parse_varint(data, pos) {
                    pos = next_pos;
                    fields.push((field_num, ProtoValue::Varint(val)));
                } else {
                    break;
                }
            }
            1 => {
                if pos + 8 <= data.len() {
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&data[pos..pos + 8]);
                    pos += 8;
                    fields.push((field_num, ProtoValue::Fixed64(b)));
                } else {
                    break;
                }
            }
            2 => {
                if let Some((len, next_pos)) = parse_varint(data, pos) {
                    let len = len as usize;
                    pos = next_pos;
                    if pos + len <= data.len() {
                        fields.push((field_num, ProtoValue::Bytes(&data[pos..pos + len])));
                        pos += len;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            5 => {
                if pos + 4 <= data.len() {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&data[pos..pos + 4]);
                    pos += 4;
                    fields.push((field_num, ProtoValue::Fixed32(b)));
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    fields
}

/// Extract timestamp from metadata proto field 1 (submessage: field 1 = secs, field 2 = nanos).
fn extract_timestamp(proto_bytes: &[u8]) -> Option<String> {
    let fields = parse_proto_fields(proto_bytes);
    let mut secs: Option<i64> = None;
    let mut nanos: u32 = 0;
    for (f_num, val) in fields {
        if f_num == 1 {
            if let ProtoValue::Varint(s) = val {
                secs = Some(s as i64);
            }
        } else if f_num == 2 {
            if let ProtoValue::Varint(n) = val {
                nanos = (n as u32).min(999_999_999);
            }
        }
    }
    secs.and_then(|s| DateTime::from_timestamp(s, nanos))
        .map(|dt| dt.to_rfc3339())
}

#[derive(Default)]
struct DecodedUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    agent_id: Option<String>,
    request_id: Option<String>,
}

/// Decode generation usage from metadata proto field 9.
fn decode_generation_usage(proto_bytes: &[u8]) -> DecodedUsage {
    let mut usage = DecodedUsage::default();
    let fields = parse_proto_fields(proto_bytes);
    for (f_num, val) in fields {
        match f_num {
            2 => {
                if let ProtoValue::Varint(v) = val {
                    usage.input_tokens = Some(v);
                }
            }
            3 => {
                if let ProtoValue::Varint(v) = val {
                    usage.output_tokens = Some(v);
                }
            }
            5 => {
                if let ProtoValue::Varint(v) = val {
                    usage.cached_input_tokens = Some(v);
                }
            }
            7 => {
                if let ProtoValue::Bytes(b) = val {
                    if let Ok(s) = std::str::from_utf8(b) {
                        usage.agent_id = Some(s.to_string());
                    }
                }
            }
            9 => {
                if let ProtoValue::Varint(v) = val {
                    usage.reasoning_tokens = Some(v);
                }
            }
            11 => {
                if let ProtoValue::Bytes(b) = val {
                    if let Ok(s) = std::str::from_utf8(b) {
                        usage.request_id = Some(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    usage
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
    let session_file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown_session");

    let mut observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut state = GeminiParserState::default();

    // Check user_version for unsupported schema guard
    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if user_version >= 9999 {
        stats.unsupported += 1;
        stats.anomalies += 1;
        anomalies.push(GeminiAnomaly {
            anomaly_type: "unsupported_version".to_string(),
            session_id: Some(session_file_stem.to_string()),
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

    // Determine model name from gen_metadata if available, or fallback
    let mut detected_model: Option<String> = None;
    if let Ok(mut stmt) = conn.prepare("SELECT data FROM gen_metadata WHERE size > 0 LIMIT 5") {
        let rows = stmt.query_map([], |row| row.get::<_, Vec<u8>>(0));
        if let Ok(rows) = rows {
            for r in rows.flatten() {
                if let Ok(s) = std::str::from_utf8(&r) {
                    if let Some(pos) = s.find("gemini-") {
                        let candidate = &s[pos..];
                        let end = candidate
                            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '.')
                            .unwrap_or(candidate.len());
                        let m = candidate[..end].trim_end_matches("-control");
                        if !m.is_empty() {
                            detected_model = Some(m.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }
    let default_model = detected_model.unwrap_or_else(|| "gemini-3.8-flash".to_string());

    // Resolve provider_session_id from trajectory_meta if present, otherwise file stem
    let provider_session_id: String = conn
        .query_row(
            "SELECT cascade_id FROM trajectory_meta LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| session_file_stem.to_string());
    state.active_session_id = Some(provider_session_id.clone());

    // Query steps table
    let mut stmt =
        match conn.prepare("SELECT idx, step_type, status, metadata FROM steps ORDER BY idx ASC") {
            Ok(s) => s,
            Err(e) => {
                stats.malformed += 1;
                stats.anomalies += 1;
                anomalies.push(GeminiAnomaly {
                    anomaly_type: "malformed_schema".to_string(),
                    session_id: Some(provider_session_id.clone()),
                    turn_id: None,
                    source_path: source_path_str.clone(),
                    source_offset: 0,
                    details: serde_json::json!({ "error": e.to_string() }),
                });
                return Ok(ParseResult {
                    observations,
                    anomalies,
                    stats,
                    final_state: state,
                });
            }
        };

    let step_rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<Vec<u8>>>(3)?,
        ))
    })?;

    for r in step_rows {
        let (idx, step_type, status, metadata) = match r {
            Ok(v) => v,
            Err(e) => {
                stats.malformed += 1;
                stats.anomalies += 1;
                anomalies.push(GeminiAnomaly {
                    anomaly_type: "malformed_row".to_string(),
                    session_id: Some(provider_session_id.clone()),
                    turn_id: None,
                    source_path: source_path_str.clone(),
                    source_offset: 0,
                    details: serde_json::json!({ "error": e.to_string() }),
                });
                continue;
            }
        };

        state.last_step_index = Some(idx);
        let turn_id_str = format!("step_{idx}");

        // step_type == 15 is model generation / planner response
        // step_type == 17 is error message / failed turn
        if step_type != 15 && step_type != 17 {
            continue;
        }

        let is_failed = step_type == 17 || status != 3;

        let meta_bytes = metadata.unwrap_or_default();
        let meta_fields = parse_proto_fields(&meta_bytes);

        // Field 1 is timestamp
        let timestamp = meta_fields
            .iter()
            .find(|(f, _)| *f == 1)
            .and_then(|(_, v)| match v {
                ProtoValue::Bytes(b) => extract_timestamp(b),
                _ => None,
            })
            .unwrap_or_else(|| Utc::now().to_rfc3339());

        // Field 9 is generation usage
        let usage_data = meta_fields
            .iter()
            .find(|(f, _)| *f == 9)
            .and_then(|(_, v)| match v {
                ProtoValue::Bytes(b) => Some(decode_generation_usage(b)),
                _ => None,
            });

        stats.parsed += 1;

        let (source, subtype, token_usage, request_id, agent_id) = match (is_failed, usage_data) {
            (true, _) => {
                stats.anomalies += 1;
                anomalies.push(GeminiAnomaly {
                    anomaly_type: "turn_failed".to_string(),
                    session_id: Some(provider_session_id.clone()),
                    turn_id: Some(turn_id_str.clone()),
                    source_path: source_path_str.clone(),
                    source_offset: idx as u64,
                    details: serde_json::json!({
                        "step_index": idx,
                        "step_type": step_type,
                        "status": status,
                    }),
                });
                (
                    MeasurementSource::Unavailable,
                    source_kind::GEMINI_TURN_FAILED.to_string(),
                    TokenUsage::default(),
                    None,
                    None,
                )
            }
            (false, None) => {
                stats.anomalies += 1;
                anomalies.push(GeminiAnomaly {
                    anomaly_type: "missing_provider_measurements".to_string(),
                    session_id: Some(provider_session_id.clone()),
                    turn_id: Some(turn_id_str.clone()),
                    source_path: source_path_str.clone(),
                    source_offset: idx as u64,
                    details: serde_json::json!({
                        "step_index": idx,
                        "step_type": step_type,
                    }),
                });
                (
                    MeasurementSource::Unavailable,
                    source_kind::GEMINI_TURN_UNMEASURED.to_string(),
                    TokenUsage::default(),
                    None,
                    None,
                )
            }
            (false, Some(u)) => {
                let tu = TokenUsage {
                    input_tokens: u.input_tokens,
                    cached_input_tokens: u.cached_input_tokens,
                    cache_write_tokens: None,
                    output_tokens: u.output_tokens,
                    reasoning_tokens: u.reasoning_tokens,
                };
                (
                    MeasurementSource::ProviderFields,
                    source_kind::GEMINI_TURN_USAGE.to_string(),
                    tu,
                    u.request_id,
                    u.agent_id,
                )
            }
        };

        let req_id_str =
            request_id.unwrap_or_else(|| format!("gemini:{}:{turn_id_str}", provider_session_id));

        let obs = UsageObservation {
            adapter: ADAPTER_NAME.to_string(),
            source,
            source_subtype: Some(subtype),
            source_event_id: Some(format!("gemini:{}:{turn_id_str}", provider_session_id)),
            provider_session_id: provider_session_id.clone(),
            request_id: Some(req_id_str),
            turn_id: Some(turn_id_str),
            agent_id,
            parent_agent_id: None,
            source_timestamp: Some(timestamp.clone()),
            observed_at: timestamp,
            model: Some(default_model.clone()),
            service_tier: None,
            region: None,
            usage: token_usage,
            provider_reported_cost_micros: None,
            source_path: source_path_str.clone(),
            source_offset: idx as u64,
            adapter_version: ADAPTER_VERSION.to_string(),
            parser_version: PARSER_VERSION.to_string(),
        };

        observations.push(obs);
    }

    Ok(ParseResult {
        observations,
        anomalies,
        stats,
        final_state: state,
    })
}

pub fn import_gemini_file(connection: &mut Connection, path: &Path) -> Result<GeminiImportResult> {
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
            "SELECT last_offset, file_hash, parser_version, adapter_state_json, last_event_hash FROM ingestion_checkpoints WHERE adapter = 'gemini' AND source_path = ?1",
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
            return Ok(GeminiImportResult {
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
            "gemini:{}:{}:{}:{}",
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
        ) VALUES('gemini', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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

    Ok(GeminiImportResult {
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
