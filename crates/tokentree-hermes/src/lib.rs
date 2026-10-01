// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use walkdir::WalkDir;

pub const ADAPTER_VERSION: &str = "0.2.0-rust";
pub const PARSER_VERSION: &str = "0.2.0-rust";

#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub anomalies: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HermesAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HermesParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub last_seen_timestamp: Option<f64>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<HermesAnomaly>,
    pub stats: ParseStats,
    pub final_state: HermesParserState,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct HermesImportResult {
    pub inserted: u64,
    pub duplicates: u64,
    pub anomalies: u64,
    pub start_offset: u64,
    pub end_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HermesCostClassification {
    /// Free model has authoritative zero cost
    AuthoritativeZero,
    /// Explicitly measured zero cost from provider (e.g. cache-only turn, promo tier, or zero-billed transaction)
    MeasuredZero,
    /// Provider reported positive cost, converted exactly to micros
    AuthoritativeProvider(u64),
    /// Provider cost is unavailable; TokenTree catalog pricing will compute fallback cost
    UnavailableProviderCost,
}

#[must_use]
pub fn is_free_model(model: &str) -> bool {
    let lower = model.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    lower.ends_with(":free")
        || lower.ends_with("/free")
        || lower.ends_with("-free")
        || lower.contains(":free:")
        || lower.split('/').any(|seg| seg == "free")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HermesAuxiliaryTask {
    pub api_calls: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub actual_cost_usd: Option<Value>,
    pub estimated_cost_usd: Option<Value>,
    pub cost_usd: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HermesAuxiliaryReport {
    pub api_calls: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub estimated_cost_usd: Option<Value>,
    pub total_tokens: Option<u64>,
    pub by_task: Option<HashMap<String, HermesAuxiliaryTask>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HermesUsageFile {
    pub actual_cost_usd: Option<Value>,
    pub estimated_cost_usd: Option<Value>,
    pub cost_usd: Option<Value>,
    pub total_cost: Option<Value>,
    pub cost_status: Option<String>,
    pub cost_source: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub api_calls: Option<u64>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub session_id: Option<String>,
    pub completed: Option<bool>,
    pub failed: Option<bool>,
    pub turn_exit_reason: Option<String>,
    pub auxiliary: Option<HermesAuxiliaryReport>,
}

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        let ext = root.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "db" || ext == "json" {
            return vec![root.to_path_buf()];
        }
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
            name == "state.db"
                || name.ends_with("_state.db")
                || name.ends_with("-state.db")
                || name.ends_with("usage.json")
                || name.starts_with("request_dump_")
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

/// Convert exact decimal string representation of USD dollars to integer microdollars ($10^-6 USD).
/// Completely avoids IEEE-754 floating-point drift.
pub fn decimal_dollars_to_micros(val_str: &str) -> Result<u64, String> {
    let s = val_str.trim();
    if s.is_empty() {
        return Err("empty cost string".to_string());
    }
    let (whole_str, frac_str) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole_str.is_empty() || !whole_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid whole dollars: {whole_str}"));
    }
    if !frac_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid fractional dollars: {frac_str}"));
    }
    let whole: u64 = whole_str
        .parse()
        .map_err(|e| format!("whole dollar parse error: {e}"))?;

    let frac_val: u64 = if frac_str.is_empty() {
        0
    } else if frac_str.len() <= 6 {
        let padded = format!("{frac_str:0<6}");
        padded.parse().map_err(|e| format!("{e}"))?
    } else {
        // Round half up on 7th decimal digit
        let first_6: u64 = frac_str[..6].parse().map_err(|e| format!("{e}"))?;
        let next_digit = frac_str.as_bytes()[6];
        if next_digit >= b'5' {
            first_6 + 1
        } else {
            first_6
        }
    };

    whole
        .checked_mul(1_000_000)
        .and_then(|w| w.checked_add(frac_val))
        .ok_or_else(|| "cost overflow".to_string())
}

#[must_use]
pub fn value_to_micros(val: Option<&Value>) -> Option<u64> {
    match val {
        Some(Value::Number(num)) => decimal_dollars_to_micros(&num.to_string()).ok(),
        Some(Value::String(s)) => decimal_dollars_to_micros(s).ok(),
        _ => None,
    }
}

pub fn parse_json_usage_str(
    content: &str,
    path: &Path,
    source_offset: u64,
    initial_state: HermesParserState,
) -> Result<ParseResult> {
    let mut observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let state = initial_state;

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
            anomalies.push(HermesAnomaly {
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

    let usage_file: HermesUsageFile = match serde_json::from_value(parsed_json) {
        Ok(u) => u,
        Err(e) => {
            stats.malformed += 1;
            stats.anomalies += 1;
            anomalies.push(HermesAnomaly {
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

    let session_id = usage_file.session_id.clone().unwrap_or_else(|| {
        // Derive stable session ID from file stem
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "hermes_session".to_string())
    });

    let observed_at = Utc::now().to_rfc3339();
    let is_failed = usage_file.failed.unwrap_or(false);

    // Main task observation
    stats.parsed += 1;
    let main_tokens = if is_failed {
        TokenUsage {
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
        }
    } else {
        TokenUsage {
            input_tokens: usage_file.input_tokens,
            cached_input_tokens: usage_file.cache_read_tokens,
            cache_write_tokens: usage_file.cache_write_tokens,
            output_tokens: usage_file.output_tokens,
            reasoning_tokens: usage_file.reasoning_tokens,
        }
    };

    let is_unmeasured = is_failed || !main_tokens.is_measured();

    let (source, subtype) = if is_unmeasured {
        stats.anomalies += 1;
        anomalies.push(HermesAnomaly {
            anomaly_type: "missing_provider_measurements".to_string(),
            session_id: Some(session_id.clone()),
            turn_id: Some("turn_1".to_string()),
            source_path: source_path_str.clone(),
            source_offset,
            details: serde_json::json!({
                "reason": if is_failed { "failed_request" } else { "missing_measurements" },
                "api_calls": usage_file.api_calls,
                "completed": usage_file.completed,
                "turn_exit_reason": usage_file.turn_exit_reason,
            }),
        });
        (
            MeasurementSource::Unavailable,
            if is_failed {
                "hermes_failed_run".to_string()
            } else {
                "hermes_unmeasured".to_string()
            },
        )
    } else {
        (
            MeasurementSource::ProviderFields,
            "hermes_oneshot_usage".to_string(),
        )
    };

    let model_str = usage_file.model.clone();
    let is_model_free = model_str.as_deref().is_some_and(is_free_model);

    // Cost precedence: actual cost > cost_usd > total_cost > estimated cost
    let parsed_actual = value_to_micros(
        usage_file
            .actual_cost_usd
            .as_ref()
            .or(usage_file.cost_usd.as_ref())
            .or(usage_file.total_cost.as_ref()),
    );
    let parsed_estimated = value_to_micros(usage_file.estimated_cost_usd.as_ref());
    let parsed_cost_micros = parsed_actual.or(parsed_estimated);

    let (_cost_classification, provider_cost_micros) = if is_unmeasured {
        (HermesCostClassification::UnavailableProviderCost, None)
    } else {
        match parsed_cost_micros {
            Some(0) => {
                if is_model_free {
                    (HermesCostClassification::AuthoritativeZero, Some(0))
                } else {
                    (HermesCostClassification::MeasuredZero, Some(0))
                }
            }
            Some(micros) => (
                HermesCostClassification::AuthoritativeProvider(micros),
                Some(micros),
            ),
            None => {
                if is_model_free {
                    (HermesCostClassification::AuthoritativeZero, Some(0))
                } else {
                    (HermesCostClassification::UnavailableProviderCost, None)
                }
            }
        }
    };

    let main_obs = UsageObservation {
        adapter: "hermes".to_string(),
        source,
        source_subtype: Some(subtype),
        source_event_id: Some(format!("hermes:{}:main", session_id)),
        provider_session_id: session_id.clone(),
        request_id: Some(format!("hermes:{}:main", session_id)),
        turn_id: Some("turn_1".to_string()),
        agent_id: Some(format!("hermes:{}", session_id)),
        parent_agent_id: None,
        source_timestamp: Some(observed_at.clone()),
        observed_at: observed_at.clone(),
        model: model_str,
        service_tier: None,
        region: None,
        usage: main_tokens,
        provider_reported_cost_micros: provider_cost_micros,
        source_path: source_path_str.clone(),
        source_offset,
        adapter_version: ADAPTER_VERSION.to_string(),
        parser_version: PARSER_VERSION.to_string(),
    };
    observations.push(main_obs);

    // Ingest auxiliary tasks (e.g., title generation) if present
    if let Some(aux) = usage_file.auxiliary {
        if let Some(tasks) = aux.by_task {
            for (task_name, task_data) in tasks {
                stats.parsed += 1;
                let task_tokens = TokenUsage {
                    input_tokens: task_data.input_tokens,
                    cached_input_tokens: task_data.cache_read_tokens,
                    cache_write_tokens: task_data.cache_write_tokens,
                    output_tokens: task_data.output_tokens,
                    reasoning_tokens: task_data.reasoning_tokens,
                };
                let task_is_unmeasured = !task_tokens.is_measured();
                let task_actual = value_to_micros(
                    task_data
                        .actual_cost_usd
                        .as_ref()
                        .or(task_data.cost_usd.as_ref()),
                );
                let task_estimated = value_to_micros(task_data.estimated_cost_usd.as_ref());
                let task_parsed = task_actual.or(task_estimated);
                let (task_source, task_cost_micros) = if task_is_unmeasured {
                    (MeasurementSource::Unavailable, None)
                } else {
                    let cost = match task_parsed {
                        Some(0) => Some(0),
                        Some(micros) => Some(micros),
                        None if is_model_free => Some(0),
                        None => None,
                    };
                    (MeasurementSource::ProviderFields, cost)
                };

                let aux_obs = UsageObservation {
                    adapter: "hermes".to_string(),
                    source: task_source,
                    source_subtype: Some(format!("hermes_auxiliary_{}", task_name)),
                    source_event_id: Some(format!("hermes:{}:aux:{}", session_id, task_name)),
                    provider_session_id: session_id.clone(),
                    request_id: Some(format!("hermes:{}:aux:{}", session_id, task_name)),
                    turn_id: Some(format!("task_{}", task_name)),
                    agent_id: Some(format!("hermes:{}:{}", session_id, task_name)),
                    parent_agent_id: Some(format!("hermes:{}", session_id)),
                    source_timestamp: Some(observed_at.clone()),
                    observed_at: observed_at.clone(),
                    model: usage_file.model.clone(),
                    service_tier: None,
                    region: None,
                    usage: task_tokens,
                    provider_reported_cost_micros: task_cost_micros,
                    source_path: source_path_str.clone(),
                    source_offset,
                    adapter_version: ADAPTER_VERSION.to_string(),
                    parser_version: PARSER_VERSION.to_string(),
                };
                observations.push(aux_obs);
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

pub fn parse_hermes_state_db(db_path: &Path, since_timestamp: Option<f64>) -> Result<ParseResult> {
    let mut conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")?;

    let read_tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;

    let mut observations = Vec::new();
    let anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut max_seen_ts: Option<f64> = since_timestamp;

    let source_path_str = db_path.to_string_lossy().to_string();

    // Query parent session mapping from `sessions` table if it exists
    let mut parent_map: HashMap<String, String> = HashMap::new();
    if let Ok(mut stmt) = read_tx
        .prepare("SELECT id, parent_session_id FROM sessions WHERE parent_session_id IS NOT NULL")
    {
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows.flatten() {
            parent_map.insert(r.0, r.1);
        }
    }

    // Query session_model_usage
    let query = "SELECT session_id, model, billing_provider, task, api_call_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd, cost_status, cost_source, first_seen, last_seen FROM session_model_usage";

    let mut stmt = match read_tx.prepare(query) {
        Ok(s) => s,
        Err(_) => {
            return Ok(ParseResult {
                observations,
                anomalies,
                stats,
                final_state: HermesParserState::default(),
            });
        }
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,                          // session_id
            row.get::<_, String>(1)?,                          // model
            row.get::<_, String>(2)?,                          // billing_provider
            row.get::<_, String>(3)?,                          // task
            row.get::<_, i64>(4)?,                             // api_call_count
            row.get::<_, i64>(5)?,                             // input_tokens
            row.get::<_, i64>(6)?,                             // output_tokens
            row.get::<_, i64>(7)?,                             // cache_read_tokens
            row.get::<_, i64>(8)?,                             // cache_write_tokens
            row.get::<_, i64>(9)?,                             // reasoning_tokens
            row.get::<_, Option<rusqlite::types::Value>>(10)?, // estimated_cost_usd
            row.get::<_, Option<rusqlite::types::Value>>(11)?, // actual_cost_usd
            row.get::<_, Option<String>>(12)?,                 // cost_status
            row.get::<_, Option<String>>(13)?,                 // cost_source
            row.get::<_, Option<f64>>(14)?,                    // first_seen
            row.get::<_, Option<f64>>(15)?,                    // last_seen
        ))
    })?;

    for row_res in rows {
        let (
            session_id,
            model,
            provider,
            task,
            _api_calls,
            input_tokens,
            output_tokens,
            cache_read,
            cache_write,
            reasoning,
            est_cost,
            act_cost,
            _cost_status,
            _cost_source,
            first_seen,
            last_seen,
        ) = match row_res {
            Ok(r) => r,
            Err(_) => {
                stats.malformed += 1;
                continue;
            }
        };

        let row_ts = last_seen.or(first_seen).unwrap_or(0.0);
        if let Some(cutoff) = since_timestamp {
            if row_ts <= cutoff {
                continue;
            }
        }
        if max_seen_ts.is_none_or(|m| row_ts > m) {
            max_seen_ts = Some(row_ts);
        }

        stats.parsed += 1;

        let is_model_free = is_free_model(&model);

        let est_micros = match est_cost.as_ref() {
            Some(rusqlite::types::Value::Text(s)) => decimal_dollars_to_micros(s).ok(),
            Some(rusqlite::types::Value::Integer(i)) if *i >= 0 => {
                (*i as u64).checked_mul(1_000_000)
            }
            Some(rusqlite::types::Value::Real(f)) if *f >= 0.0 => {
                let s = format!("{f:.6}");
                decimal_dollars_to_micros(&s).ok()
            }
            _ => None,
        };
        let act_micros = match act_cost.as_ref() {
            Some(rusqlite::types::Value::Text(s)) => decimal_dollars_to_micros(s).ok(),
            Some(rusqlite::types::Value::Integer(i)) if *i >= 0 => {
                (*i as u64).checked_mul(1_000_000)
            }
            Some(rusqlite::types::Value::Real(f)) if *f >= 0.0 => {
                let s = format!("{f:.6}");
                decimal_dollars_to_micros(&s).ok()
            }
            _ => None,
        };
        let cost_micros = act_micros.or(est_micros);

        let provider_cost_micros = match cost_micros {
            Some(0) => Some(0), // Free model or measured zero on paid model
            Some(micros) => Some(micros),
            None => {
                if is_model_free {
                    Some(0)
                } else {
                    None
                }
            }
        };

        let parent_session_id = parent_map.get(&session_id).cloned();
        let agent_id = Some(format!("hermes:{}", session_id));
        let parent_agent_id = parent_session_id.map(|p| format!("hermes:{}", p));

        let timestamp_str = if row_ts > 0.0 {
            DateTime::from_timestamp(row_ts as i64, ((row_ts.fract()) * 1e9) as u32)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| Utc::now().to_rfc3339())
        } else {
            Utc::now().to_rfc3339()
        };

        let turn_id = if task.is_empty() {
            "turn_main".to_string()
        } else {
            format!("task_{}", task)
        };

        let has_tokens = input_tokens > 0
            || output_tokens > 0
            || cache_read > 0
            || cache_write > 0
            || reasoning > 0;

        let usage = TokenUsage {
            input_tokens: if input_tokens > 0 {
                Some(input_tokens as u64)
            } else {
                None
            },
            cached_input_tokens: if cache_read > 0 {
                Some(cache_read as u64)
            } else {
                None
            },
            cache_write_tokens: if cache_write > 0 {
                Some(cache_write as u64)
            } else {
                None
            },
            output_tokens: if output_tokens > 0 {
                Some(output_tokens as u64)
            } else {
                None
            },
            reasoning_tokens: if reasoning > 0 {
                Some(reasoning as u64)
            } else {
                None
            },
        };

        let (source, obs_cost) = if !has_tokens {
            (MeasurementSource::Unavailable, None)
        } else {
            (MeasurementSource::ProviderFields, provider_cost_micros)
        };

        let obs = UsageObservation {
            adapter: "hermes".to_string(),
            source,
            source_subtype: Some(if provider.is_empty() {
                "hermes_session_model_usage".to_string()
            } else {
                format!("hermes_usage_{}", provider)
            }),
            source_event_id: Some(format!("hermes:{}:{}:{}", session_id, model, task)),
            provider_session_id: session_id.clone(),
            request_id: Some(format!("hermes:{}:{}:{}", session_id, model, task)),
            turn_id: Some(turn_id),
            agent_id,
            parent_agent_id,
            source_timestamp: Some(timestamp_str.clone()),
            observed_at: timestamp_str,
            model: Some(model),
            service_tier: None,
            region: None,
            usage,
            provider_reported_cost_micros: obs_cost,
            source_path: source_path_str.clone(),
            source_offset: (row_ts * 1000.0) as u64,
            adapter_version: ADAPTER_VERSION.to_string(),
            parser_version: PARSER_VERSION.to_string(),
        };

        observations.push(obs);
    }

    Ok(ParseResult {
        observations,
        anomalies,
        stats,
        final_state: HermesParserState {
            version: 1,
            active_session_id: None,
            last_seen_timestamp: max_seen_ts,
        },
    })
}

pub fn import_hermes_file(connection: &mut Connection, path: &Path) -> Result<HermesImportResult> {
    let source_path_str = path.to_string_lossy().to_string();
    let is_sqlite = path.extension().is_some_and(|ext| ext == "db");

    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let file_size = meta.len();
    let modified_at = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    let file_content = if !is_sqlite {
        Some(fs::read(path).with_context(|| format!("read {}", path.display()))?)
    } else {
        None
    };
    let file_identity_hash = file_content
        .as_ref()
        .map(|c| hex::encode(Sha256::digest(c)))
        .unwrap_or_else(|| hex::encode(Sha256::digest(source_path_str.as_bytes())));

    if !is_sqlite {
        let checkpoint: Option<(i64, Option<String>, Option<String>)> = connection
            .query_row(
                "SELECT last_offset, file_hash, parser_version FROM ingestion_checkpoints WHERE adapter = 'hermes' AND source_path = ?1",
                [&source_path_str],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        if let Some((_, Some(last_hash), Some(p_ver))) = checkpoint {
            if last_hash == file_identity_hash && p_ver == PARSER_VERSION {
                return Ok(HermesImportResult {
                    inserted: 0,
                    duplicates: 0,
                    anomalies: 0,
                    start_offset: file_size,
                    end_offset: file_size,
                });
            }
        }
    }

    let parse_res = if is_sqlite {
        // Read previous state checkpoint
        let prev_state_ts: Option<f64> = connection
            .query_row(
                "SELECT adapter_state_json FROM ingestion_checkpoints WHERE adapter = 'hermes' AND source_path = ?1",
                [&source_path_str],
                |row| {
                    let json_str: Option<String> = row.get(0)?;
                    if let Some(s) = json_str {
                        let parsed: HermesParserState = serde_json::from_str(&s).unwrap_or_default();
                        Ok(parsed.last_seen_timestamp)
                    } else {
                        Ok(None)
                    }
                },
            )
            .unwrap_or(None);

        parse_hermes_state_db(path, prev_state_ts)?
    } else {
        let content_str = String::from_utf8_lossy(file_content.as_ref().unwrap());
        parse_json_usage_str(&content_str, path, 0, HermesParserState::default())?
    };

    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

    let mut inserted = 0_u64;
    let mut duplicates = 0_u64;
    let mut last_event_hash: Option<String> = None;

    for obs in &parse_res.observations {
        let session_id = format!(
            "ses_{}",
            hex::encode(&Sha256::digest(obs.provider_session_id.as_bytes())[..8])
        );
        tx.execute(
            "INSERT OR IGNORE INTO sessions(id, adapter, provider_session_id, source_path, started_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![session_id, obs.adapter, obs.provider_session_id, obs.source_path, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
        )?;

        let turn_db_id = if let Some(t_id) = &obs.turn_id {
            let turn_id = format!(
                "turn_{}",
                hex::encode(&Sha256::digest(format!("{}:{}", session_id, t_id).as_bytes())[..8])
            );
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
                obs.source_subtype.as_deref().unwrap_or(obs.source.as_str()),
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
            "hermes:{}:{}:{}:{}",
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
        ) VALUES('hermes', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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

    Ok(HermesImportResult {
        inserted,
        duplicates,
        anomalies: parse_res.anomalies.len() as u64,
        start_offset: 0,
        end_offset: file_size,
    })
}
