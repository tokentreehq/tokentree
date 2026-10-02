// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use sha2::Digest;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use walkdir::WalkDir;

pub const ADAPTER_VERSION: &str = "0.2.0-rust";
pub const PARSER_VERSION: &str = "0.2.0-rust";

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub unsupported: u64,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub stats: ParseStats,
}

#[must_use]
pub fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    let mut sessions: Vec<_> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    sessions.sort();
    sessions
}

pub fn parse_session(path: &Path) -> Result<ParseResult> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let fallback_session = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");
    let mut observations = Vec::new();
    let mut stats = ParseStats::default();
    let mut offset = 0_u64;

    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                stats.malformed += 1;
                continue;
            }
        };
        let line_offset = offset;
        offset += line.len() as u64 + 1;
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = match serde_json::from_str(&line) {
            Ok(Value::Object(record)) => Value::Object(record),
            _ => {
                stats.malformed += 1;
                continue;
            }
        };

        if let Some(v) = record.get("version").and_then(Value::as_u64) {
            if v > 2 {
                stats.unsupported += 1;
                continue;
            }
        }
        if let Some(v) = record.get("schema_version").and_then(Value::as_str) {
            if v.starts_with("99") || (v.starts_with('9') && v.len() > 2) {
                stats.unsupported += 1;
                continue;
            }
        }
        if record.get("unsupported_version") == Some(&Value::Bool(true)) {
            stats.unsupported += 1;
            continue;
        }

        let Some(usage_value) = record
            .get("usage")
            .or_else(|| record.pointer("/message/usage"))
        else {
            stats.unknown += 1;
            continue;
        };
        let usage = TokenUsage {
            input_tokens: count(usage_value, &["input_tokens", "inputTokens"]),
            cached_input_tokens: count(
                usage_value,
                &[
                    "cache_read_input_tokens",
                    "cached_input_tokens",
                    "cachedInputTokens",
                ],
            ),
            cache_write_tokens: count(
                usage_value,
                &[
                    "cache_creation_input_tokens",
                    "cache_write_tokens",
                    "cacheWriteTokens",
                ],
            ),
            output_tokens: count(usage_value, &["output_tokens", "outputTokens"]),
            reasoning_tokens: count(usage_value, &["reasoning_tokens", "reasoningTokens"]).or_else(
                || {
                    usage_value
                        .pointer("/output_tokens_details/thinking_tokens")
                        .and_then(Value::as_u64)
                },
            ),
        };
        if !usage.is_measured() {
            stats.unknown += 1;
            continue;
        }
        let record_type = string(&record, &["type"]).unwrap_or_default();
        let source = match record_type.as_str() {
            "otel_api_request" => MeasurementSource::OfficialTelemetry,
            "provider_usage" => MeasurementSource::ProviderFields,
            _ => MeasurementSource::TranscriptRequest,
        };
        let observed_at = Utc::now().to_rfc3339();
        observations.push(UsageObservation {
            adapter: "claude".into(),
            source,
            source_subtype: if record_type.is_empty() {
                None
            } else {
                Some(record_type)
            },
            source_event_id: string(&record, &["event_id", "uuid"]),
            provider_session_id: string(&record, &["session_id", "sessionId"])
                .unwrap_or_else(|| fallback_session.into()),
            request_id: string(&record, &["request_id", "requestId"]).or_else(|| {
                record
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            turn_id: string(&record, &["turn_id", "prompt_id", "promptId"]),
            agent_id: string(&record, &["agent_id", "agentId"]),
            parent_agent_id: string(&record, &["parent_agent_id", "parentAgentId"]),
            source_timestamp: string(&record, &["timestamp"]),
            observed_at,
            model: string(&record, &["model"]).or_else(|| {
                record
                    .pointer("/message/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            service_tier: string(&record, &["service_tier"]).or_else(|| {
                usage_value
                    .get("service_tier")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            region: string(&record, &["region"]).or_else(|| {
                usage_value
                    .get("inference_geo")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            usage,
            provider_reported_cost_micros: record.get("cost_micros").and_then(Value::as_u64),
            source_path: path.display().to_string(),
            source_offset: line_offset,
            adapter_version: ADAPTER_VERSION.into(),
            parser_version: PARSER_VERSION.into(),
        });
        stats.parsed += 1;
    }
    Ok(ParseResult {
        observations,
        stats,
    })
}

fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
}
fn count(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

pub fn import_claude_file(
    connection: &mut rusqlite::Connection,
    path: &Path,
) -> Result<tokentree_core::AdapterImportResult> {
    let file_content = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let file_size = file_content.len() as u64;
    let file_identity_hash = hex::encode(sha2::Sha256::digest(&file_content));
    let source_path_str = path.to_string_lossy().to_string();

    struct CheckpointRow {
        file_hash: Option<String>,
        parser_version: Option<String>,
    }

    let checkpoint: Option<CheckpointRow> = connection
        .query_row(
            "SELECT file_hash, parser_version FROM ingestion_checkpoints WHERE adapter = 'claude' AND source_path = ?1",
            [&source_path_str],
            |row| Ok(CheckpointRow { file_hash: row.get(0)?, parser_version: row.get(1)? }),
        )
        .ok();

    if let Some(cp) = &checkpoint {
        if cp.file_hash.as_deref() == Some(&file_identity_hash)
            && cp.parser_version.as_deref() == Some(PARSER_VERSION)
        {
            return Ok(tokentree_core::AdapterImportResult {
                inserted: 0,
                duplicates: 0,
                malformed: 0,
                unsupported: 0,
                anomalies: 0,
                anomaly_types: Vec::new(),
                latest_event_identity: None,
                latest_authoritative_timestamp: None,
                start_offset: file_size,
                end_offset: file_size,
            });
        }
    }

    let parse_res = parse_session(path)?;
    let mut inserted = 0u64;
    let mut duplicates = 0u64;
    let mut latest_event_identity: Option<String> = None;
    let mut latest_authoritative_timestamp: Option<chrono::DateTime<chrono::Utc>> = None;

    let tx = connection.transaction()?;

    for obs in &parse_res.observations {
        let session_id = format!(
            "ses_{}",
            hex::encode(&sha2::Sha256::digest(obs.provider_session_id.as_bytes())[..8])
        );
        tx.execute(
            "INSERT OR IGNORE INTO sessions(id, adapter, provider_session_id, source_path, started_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, obs.adapter, obs.provider_session_id, obs.source_path, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
        )?;

        let turn_db_id = if let Some(t_id) = &obs.turn_id {
            let turn_id = format!(
                "turn_{}",
                hex::encode(
                    &sha2::Sha256::digest(format!("{}:{}", session_id, t_id).as_bytes())[..8]
                )
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
                rusqlite::params![turn_id, session_id, seq, obs.source_timestamp.as_deref().unwrap_or(&obs.observed_at)],
            )?;
            Some(turn_id)
        } else {
            None
        };

        let evt_hash = obs.event_hash();
        latest_event_identity = Some(obs.canonical_identity());
        if let Some(ts) = obs
            .source_timestamp
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        {
            let utc_ts = ts.with_timezone(&chrono::Utc);
            latest_authoritative_timestamp = match latest_authoritative_timestamp {
                None => Some(utc_ts),
                Some(prev) if utc_ts > prev => Some(utc_ts),
                Some(prev) => Some(prev),
            };
        }

        let changed = tx.execute(
            "INSERT OR IGNORE INTO usage_events(
              id, adapter, source_kind, source_event_id, session_id, turn_id, request_id, agent_id, parent_agent_id, source_timestamp,
              observed_at, ingested_at, model, service_tier, region, input_tokens, cached_input_tokens,
              cache_write_tokens, output_tokens, reasoning_tokens, provider_reported_cost_micros,
              source_path, source_offset, event_hash, adapter_version, parser_version
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            rusqlite::params![
                format!("evt_{}", &evt_hash[..16]),
                obs.adapter,
                obs.source_subtype.as_deref().unwrap_or("claude_transcript"),
                obs.source_event_id,
                session_id,
                turn_db_id,
                obs.request_id,
                obs.agent_id,
                obs.parent_agent_id,
                obs.source_timestamp,
                obs.observed_at,
                chrono::Utc::now().to_rfc3339(),
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

        if changed > 0 {
            inserted += 1;
        } else {
            duplicates += 1;
        }
    }

    let modified_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    tx.execute(
        "INSERT INTO ingestion_checkpoints (
          adapter, source_path, file_size, modified_at, last_offset, last_event_hash, file_hash, parser_version
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(adapter, source_path) DO UPDATE SET
          file_size = excluded.file_size,
          modified_at = excluded.modified_at,
          last_offset = excluded.last_offset,
          last_event_hash = excluded.last_event_hash,
          file_hash = excluded.file_hash,
          parser_version = excluded.parser_version",
        rusqlite::params![
            "claude",
            source_path_str,
            file_size as i64,
            modified_at,
            file_size as i64,
            latest_event_identity,
            file_identity_hash,
            PARSER_VERSION,
        ],
    )?;

    tx.commit()?;

    let mut anomaly_types = Vec::new();
    if parse_res.stats.unsupported > 0 {
        anomaly_types.push("unsupported_version".to_string());
    }
    if parse_res.stats.malformed > 0 {
        anomaly_types.push("malformed_record".to_string());
    }

    Ok(tokentree_core::AdapterImportResult {
        inserted,
        duplicates,
        malformed: parse_res.stats.malformed,
        unsupported: parse_res.stats.unsupported,
        anomalies: anomaly_types.len() as u64,
        anomaly_types,
        latest_event_identity,
        latest_authoritative_timestamp,
        start_offset: 0,
        end_offset: file_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_sanitized_v2_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/parsers/claude/public-small-v2.1.80.jsonl");
        let result = parse_session(&path).unwrap();
        let row = result
            .observations
            .iter()
            .find(|item| item.request_id.as_deref() == Some("req_stage0_nested"))
            .unwrap();
        assert_eq!(row.provider_session_id, "ses_stage0_small");
        assert_eq!(row.usage.input_tokens, Some(10));
        assert_eq!(row.usage.output_tokens, Some(5));
        assert!(result.stats.unknown > 0);
    }
}
