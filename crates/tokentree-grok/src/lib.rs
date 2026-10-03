// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use tokentree_core::{
    MeasurementSource, TokenUsage, UsageObservation, canonical_source_kind, source_kind,
};
use walkdir::WalkDir;

pub const ADAPTER_VERSION: &str = "0.2.0-rust";
pub const PARSER_VERSION: &str = "0.2.0-rust";

#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub unsupported: u64,
    pub anomalies: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GrokParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub last_turn_number: Option<u64>,
    pub cumulative_tokens: TokenUsage,
    pub cumulative_cost_ticks: Option<u64>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<GrokAnomaly>,
    pub stats: ParseStats,
    pub final_state: GrokParserState,
}

pub type GrokImportResult = tokentree_core::AdapterImportResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub cost_usd_ticks: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokSessionMetrics {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub turn_count: Option<u64>,
    pub cost_usd_ticks: Option<u64>,
    pub primary_model_id: Option<String>,
    pub model_usage: Option<HashMap<String, GrokModelUsage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokTurnMetrics {
    pub turn_number: u64,
    pub ended_at: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub model_calls: Option<u64>,
    pub turn_count: Option<u64>,
    pub cost_usd_ticks: Option<u64>,
    pub primary_model_id: Option<String>,
    pub model_usage: Option<HashMap<String, GrokModelUsage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokUsageFile {
    pub session_id: String,
    pub updated_at: Option<String>,
    pub session: Option<GrokSessionMetrics>,
    pub turns: Option<Vec<GrokTurnMetrics>>,
}

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    let mut sessions: Vec<_> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name == "usage.json")
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

/// Convert Grok cost ticks ($10^-9 USD) to integer microdollars ($10^-6 USD).
#[must_use]
pub const fn ticks_to_micros(ticks: u64) -> u64 {
    // 1 tick = 10^-9 USD. 1 micro = 10^-6 USD.
    // 1 micro = 1000 ticks.
    ticks / 1000
}

pub fn parse_session(path: &Path) -> Result<ParseResult> {
    // L9: refuse to buffer a degenerate file into memory.
    tokentree_core::check_file_size(path).map_err(|e| anyhow::anyhow!(e))?;
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    parse_str(&contents, path, 0, GrokParserState::default())
}

pub fn parse_str(
    content: &str,
    path: &Path,
    source_offset: u64,
    initial_state: GrokParserState,
) -> Result<ParseResult> {
    let mut observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut state = initial_state;

    let source_path_str = path.to_string_lossy().to_string();

    if content.trim().is_empty() {
        return Ok(ParseResult {
            observations,
            anomalies,
            stats,
            final_state: state,
        });
    }

    let parsed_json: Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            stats.malformed += 1;
            stats.anomalies += 1;
            anomalies.push(GrokAnomaly {
                anomaly_type: "malformed_record".to_string(),
                session_id: state.active_session_id.clone(),
                turn_id: None,
                source_path: source_path_str.clone(),
                source_offset,
                details: serde_json::json!({
                    "error": e.to_string(),
                    "byte_length": content.len()
                }),
            });
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: state,
            });
        }
    };

    if let Some(v) = parsed_json.get("version").and_then(Value::as_u64) {
        if v >= 99 {
            stats.unsupported += 1;
            stats.anomalies += 1;
            anomalies.push(GrokAnomaly {
                anomaly_type: "unsupported_version".to_string(),
                session_id: state.active_session_id.clone(),
                turn_id: None,
                source_path: source_path_str.clone(),
                source_offset,
                details: serde_json::json!({
                    "version": v
                }),
            });
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: state,
            });
        }
    }
    if let Some(v) = parsed_json.get("schema_version").and_then(Value::as_str) {
        if v.starts_with("99") || (v.starts_with('9') && v.len() > 2) {
            stats.unsupported += 1;
            stats.anomalies += 1;
            anomalies.push(GrokAnomaly {
                anomaly_type: "unsupported_version".to_string(),
                session_id: state.active_session_id.clone(),
                turn_id: None,
                source_path: source_path_str.clone(),
                source_offset,
                details: serde_json::json!({
                    "schema_version": v
                }),
            });
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: state,
            });
        }
    }
    if parsed_json.get("unsupported_version") == Some(&Value::Bool(true)) {
        stats.unsupported += 1;
        stats.anomalies += 1;
        anomalies.push(GrokAnomaly {
            anomaly_type: "unsupported_version".to_string(),
            session_id: state.active_session_id.clone(),
            turn_id: None,
            source_path: source_path_str.clone(),
            source_offset,
            details: serde_json::json!({
                "unsupported_version": true
            }),
        });
        return Ok(ParseResult {
            observations,
            anomalies,
            stats,
            final_state: state,
        });
    }

    let usage_file: GrokUsageFile = match serde_json::from_value(parsed_json) {
        Ok(u) => u,
        Err(e) => {
            stats.malformed += 1;
            stats.anomalies += 1;
            anomalies.push(GrokAnomaly {
                anomaly_type: "schema_mismatch".to_string(),
                session_id: state.active_session_id.clone(),
                turn_id: None,
                source_path: source_path_str.clone(),
                source_offset,
                details: serde_json::json!({
                    "error": e.to_string()
                }),
            });
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: state,
            });
        }
    };

    let session_id = usage_file.session_id.clone();
    state.active_session_id = Some(session_id.clone());

    let default_timestamp = usage_file
        .updated_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    let default_model = usage_file
        .session
        .as_ref()
        .and_then(|s| s.primary_model_id.clone())
        .or_else(|| {
            usage_file
                .turns
                .as_ref()
                .and_then(|turns| turns.iter().find_map(|t| t.primary_model_id.clone()))
        });

    let mut turn_obs_count = 0_usize;

    if let Some(turns) = usage_file.turns {
        for turn in turns {
            stats.parsed += 1;
            turn_obs_count += 1;
            state.last_turn_number = Some(turn.turn_number);

            let turn_id_str = format!("turn_{}", turn.turn_number);
            let timestamp = turn
                .ended_at
                .clone()
                .unwrap_or_else(|| default_timestamp.clone());
            let model = turn
                .primary_model_id
                .clone()
                .or_else(|| default_model.clone());

            let is_failed_or_zero_calls = turn.model_calls == Some(0);
            let has_missing_measurements =
                turn.input_tokens.is_none() && turn.output_tokens.is_none();
            let is_unmeasured = is_failed_or_zero_calls || has_missing_measurements;

            let token_usage = if is_unmeasured {
                TokenUsage {
                    input_tokens: None,
                    cached_input_tokens: None,
                    cache_write_tokens: None,
                    output_tokens: None,
                    reasoning_tokens: None,
                }
            } else {
                TokenUsage {
                    input_tokens: turn.input_tokens,
                    cached_input_tokens: turn.cached_read_tokens,
                    cache_write_tokens: turn.cache_creation_tokens,
                    output_tokens: turn.output_tokens,
                    reasoning_tokens: turn.reasoning_tokens,
                }
            };

            // Check for category total sum discrepancy if totalTokens provided and measured
            if !is_unmeasured {
                if let Some(total) = turn.total_tokens {
                    let sum = turn.input_tokens.unwrap_or(0) + turn.output_tokens.unwrap_or(0);
                    if total != sum && turn.total_tokens != Some(0) {
                        stats.anomalies += 1;
                        anomalies.push(GrokAnomaly {
                            anomaly_type: "token_sum_mismatch".to_string(),
                            session_id: Some(session_id.clone()),
                            turn_id: Some(turn_id_str.clone()),
                            source_path: source_path_str.clone(),
                            source_offset,
                            details: serde_json::json!({
                                "reported_total": total,
                                "input_tokens": turn.input_tokens,
                                "output_tokens": turn.output_tokens,
                                "cached_read_tokens": turn.cached_read_tokens,
                                "reasoning_tokens": turn.reasoning_tokens,
                            }),
                        });
                    }
                }
            }

            let (source, subtype) = if is_unmeasured {
                stats.anomalies += 1;
                anomalies.push(GrokAnomaly {
                    anomaly_type: "missing_provider_measurements".to_string(),
                    session_id: Some(session_id.clone()),
                    turn_id: Some(turn_id_str.clone()),
                    source_path: source_path_str.clone(),
                    source_offset,
                    details: serde_json::json!({
                        "reason": if is_failed_or_zero_calls { "zero_model_calls" } else { "missing_measurements" },
                        "model_calls": turn.model_calls,
                        "turn_number": turn.turn_number,
                    }),
                });
                (
                    MeasurementSource::Unavailable,
                    if is_failed_or_zero_calls {
                        source_kind::GROK_TURN_FAILED.to_string()
                    } else {
                        source_kind::GROK_TURN_UNMEASURED.to_string()
                    },
                )
            } else {
                (
                    MeasurementSource::ProviderFields,
                    source_kind::GROK_TURN_USAGE.to_string(),
                )
            };

            let provider_cost_micros = if is_unmeasured {
                None
            } else {
                turn.cost_usd_ticks.map(ticks_to_micros)
            };

            let obs = UsageObservation {
                adapter: "grok".to_string(),
                source,
                source_subtype: Some(subtype),
                source_event_id: Some(format!("grok:{}:turn_{}", session_id, turn.turn_number)),
                provider_session_id: session_id.clone(),
                request_id: Some(format!("grok:{}:turn_{}", session_id, turn.turn_number)),
                turn_id: Some(turn_id_str),
                agent_id: None,
                parent_agent_id: None,
                source_timestamp: Some(timestamp.clone()),
                observed_at: timestamp,
                model,
                service_tier: None,
                region: None,
                usage: token_usage,
                provider_reported_cost_micros: provider_cost_micros,
                source_path: source_path_str.clone(),
                source_offset,
                adapter_version: ADAPTER_VERSION.to_string(),
                parser_version: PARSER_VERSION.to_string(),
            };

            observations.push(obs);
        }
    }

    // Truth ladder fallback:
    // If NO turns were provided, but session metrics exist, ingest session counter
    if turn_obs_count == 0 {
        if let Some(session_metrics) = usage_file.session {
            stats.parsed += 1;
            let timestamp = default_timestamp.clone();
            let model = session_metrics
                .primary_model_id
                .clone()
                .or_else(|| default_model.clone());

            let is_failed_or_zero_calls = session_metrics.model_calls == Some(0);
            let has_missing_measurements =
                session_metrics.input_tokens.is_none() && session_metrics.output_tokens.is_none();
            let is_unmeasured = is_failed_or_zero_calls || has_missing_measurements;

            let token_usage = if is_unmeasured {
                TokenUsage {
                    input_tokens: None,
                    cached_input_tokens: None,
                    cache_write_tokens: None,
                    output_tokens: None,
                    reasoning_tokens: None,
                }
            } else {
                TokenUsage {
                    input_tokens: session_metrics.input_tokens,
                    cached_input_tokens: session_metrics.cached_read_tokens,
                    cache_write_tokens: session_metrics.cache_creation_tokens,
                    output_tokens: session_metrics.output_tokens,
                    reasoning_tokens: session_metrics.reasoning_tokens,
                }
            };

            let (source, subtype) = if is_unmeasured {
                stats.anomalies += 1;
                anomalies.push(GrokAnomaly {
                    anomaly_type: "missing_provider_measurements".to_string(),
                    session_id: Some(session_id.clone()),
                    turn_id: None,
                    source_path: source_path_str.clone(),
                    source_offset,
                    details: serde_json::json!({
                        "reason": if is_failed_or_zero_calls { "zero_model_calls" } else { "missing_measurements" },
                        "model_calls": session_metrics.model_calls,
                    }),
                });
                (
                    MeasurementSource::Unavailable,
                    if is_failed_or_zero_calls {
                        source_kind::GROK_SESSION_FAILED.to_string()
                    } else {
                        source_kind::GROK_SESSION_UNMEASURED.to_string()
                    },
                )
            } else {
                (
                    MeasurementSource::ProviderFields,
                    source_kind::GROK_SESSION_USAGE.to_string(),
                )
            };

            let provider_cost_micros = if is_unmeasured {
                None
            } else {
                session_metrics.cost_usd_ticks.map(ticks_to_micros)
            };

            let obs = UsageObservation {
                adapter: "grok".to_string(),
                source,
                source_subtype: Some(subtype),
                source_event_id: Some(format!("grok:{}:session_summary", session_id)),
                provider_session_id: session_id.clone(),
                request_id: Some(format!("grok:{}:session_summary", session_id)),
                turn_id: None,
                agent_id: None,
                parent_agent_id: None,
                source_timestamp: Some(timestamp.clone()),
                observed_at: timestamp,
                model,
                service_tier: None,
                region: None,
                usage: token_usage,
                provider_reported_cost_micros: provider_cost_micros,
                source_path: source_path_str.clone(),
                source_offset,
                adapter_version: ADAPTER_VERSION.to_string(),
                parser_version: PARSER_VERSION.to_string(),
            };

            observations.push(obs);
        }
    }

    Ok(ParseResult {
        observations,
        anomalies,
        stats,
        final_state: state,
    })
}

pub fn import_grok_file(connection: &mut Connection, path: &Path) -> Result<GrokImportResult> {
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
            "SELECT last_offset, file_hash, parser_version, adapter_state_json, last_event_hash FROM ingestion_checkpoints WHERE adapter = 'grok' AND source_path = ?1",
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
            // Already imported at this hash and parser version
            return Ok(GrokImportResult {
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

    let initial_state: GrokParserState = match &checkpoint {
        Some(cp) => {
            if let Some(state_json) = &cp.adapter_state_json {
                serde_json::from_str(state_json).unwrap_or_default()
            } else {
                GrokParserState::default()
            }
        }
        None => GrokParserState::default(),
    };

    let content_str = String::from_utf8_lossy(&file_content);
    let parse_res = parse_str(&content_str, path, 0, initial_state)?;

    let tx = connection.transaction()?;

    let mut inserted = 0_u64;
    let mut duplicates = 0_u64;
    let mut last_event_hash: Option<String> = None;

    for obs in &parse_res.observations {
        // Canonical session-ID derivation shared with the ledger
        // (tokentree_core::session_stable_id): default attribution and
        // every lookup must resolve the same ID ingest wrote.
        let session_id = tokentree_core::session_stable_id(&obs.adapter, &obs.provider_session_id);
        tx.execute(
            "INSERT OR IGNORE INTO sessions(id, adapter, provider_session_id, source_path, started_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![session_id, obs.adapter, obs.provider_session_id, obs.source_path, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
        )?;

        let turn_db_id = if let Some(t_id) = &obs.turn_id {
            // Same derivation as the ledger's ingest path: turn IDs are
            // namespaced under the canonical session ID.
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
            "grok:{}:{}:{}:{}",
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
        ) VALUES('grok', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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

    let mut anomaly_types = std::collections::BTreeSet::new();
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

    Ok(GrokImportResult {
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
