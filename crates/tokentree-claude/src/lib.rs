// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use sha2::Digest;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation, canonical_source_kind};
use walkdir::WalkDir;

pub const ADAPTER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "-rust");
/// Bumped for the C4 fix: `usage_snapshot` records are now diffed per session
/// into deltas instead of being stored as full cumulatives. The repair
/// command (`tokentree repair snapshot-overcount`) targets rows written by
/// older parser versions.
pub const PARSER_VERSION: &str = "0.2.1-rust";

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub unsupported: u64,
    pub negative_deltas: u64,
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
    // Cumulative baselines keyed per session: usage_snapshot records carry
    // cumulative counters, and only the delta since the previous snapshot for
    // the same session is a real measurement.
    let mut prior_snapshots: std::collections::HashMap<String, TokenUsage> =
        std::collections::HashMap::new();

    // L9: bounded line buffering — a degenerate multi-hundred-MB line is
    // skipped and counted instead of being fully buffered (OOM).
    let mut reader = BufReader::new(file);
    let mut raw_buf = Vec::new();
    loop {
        let (consumed, truncated) =
            match tokentree_core::read_capped_line(&mut reader, &mut raw_buf) {
                Ok(Some(v)) => v,
                Ok(None) => break,
                Err(_) => {
                    stats.malformed += 1;
                    continue;
                }
            };
        let line_offset = offset;
        offset += consumed;
        if truncated {
            stats.malformed += 1;
            continue;
        }
        // Strip the trailing newline; read_capped_line keeps it in the buffer.
        if raw_buf.ends_with(b"\n") {
            raw_buf.pop();
        }
        let line = match std::str::from_utf8(&raw_buf) {
            Ok(line) => line,
            Err(_) => {
                stats.malformed += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = match serde_json::from_str(line) {
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
            )
            .or_else(|| nested_cache_creation_tokens(usage_value)),
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
        let session_id = string(&record, &["session_id", "sessionId"])
            .unwrap_or_else(|| fallback_session.into());

        // usage_snapshot records are cumulative counters, not deltas: emit only
        // the delta against the previous snapshot for the same session.
        if record_type == "usage_snapshot" {
            if let Some(prior) = prior_snapshots.get(&session_id) {
                match snapshot_delta(prior, &usage) {
                    Some(delta) => {
                        observations.push(build_observation(
                            &record,
                            &record_type,
                            session_id.clone(),
                            MeasurementSource::SnapshotDelta,
                            delta,
                            path,
                            line_offset,
                        ));
                        stats.parsed += 1;
                    }
                    None => {
                        // Counter reset or reorder: baseline moved backwards.
                        // Do not emit; the new baseline becomes the reference.
                        stats.negative_deltas += 1;
                    }
                }
                prior_snapshots.insert(session_id, usage);
            } else {
                // First snapshot for this session: establishes the baseline,
                // emits nothing (there is no earlier counter to diff against).
                prior_snapshots.insert(session_id, usage);
            }
            continue;
        }

        let source = match record_type.as_str() {
            "otel_api_request" => MeasurementSource::OfficialTelemetry,
            "provider_usage" => MeasurementSource::ProviderFields,
            _ => MeasurementSource::TranscriptRequest,
        };
        observations.push(build_observation(
            &record,
            &record_type,
            session_id,
            source,
            usage,
            path,
            line_offset,
        ));
        stats.parsed += 1;
    }
    Ok(ParseResult {
        observations,
        stats,
    })
}

/// Diff a cumulative usage snapshot against the previous cumulative for the
/// same session. Returns `None` when any counter moved backwards (reset or
/// reorder); a field that is missing on either side stays missing.
fn snapshot_delta(before: &TokenUsage, after: &TokenUsage) -> Option<TokenUsage> {
    let diff = |b: Option<u64>, a: Option<u64>| -> Option<Option<u64>> {
        match (b, a) {
            (Some(b), Some(a)) => a.checked_sub(b).map(Some),
            _ => Some(None),
        }
    };
    Some(TokenUsage {
        input_tokens: diff(before.input_tokens, after.input_tokens)?,
        cached_input_tokens: diff(before.cached_input_tokens, after.cached_input_tokens)?,
        cache_write_tokens: diff(before.cache_write_tokens, after.cache_write_tokens)?,
        output_tokens: diff(before.output_tokens, after.output_tokens)?,
        reasoning_tokens: diff(before.reasoning_tokens, after.reasoning_tokens)?,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_observation(
    record: &Value,
    _record_type: &str,
    session_id: String,
    source: MeasurementSource,
    usage: TokenUsage,
    path: &Path,
    line_offset: u64,
) -> UsageObservation {
    let usage_value = record
        .get("usage")
        .or_else(|| record.pointer("/message/usage"));
    let observed_at = Utc::now().to_rfc3339();
    UsageObservation {
        adapter: "claude".into(),
        source,
        // source_subtype is intentionally unset: the raw transcript record
        // "type" (e.g. "assistant", "user") is provider-internal and NOT part
        // of the cross-language source_kind vocabulary. Writing it would make
        // Rust-ingested rows disagree with TypeScript-ingested rows for the
        // same observation (see tokentree_core::source_kind).
        source_subtype: None,
        source_event_id: string(record, &["event_id", "uuid"]),
        provider_session_id: session_id,
        request_id: string(record, &["request_id", "requestId"]).or_else(|| {
            record
                .pointer("/message/id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        turn_id: string(record, &["turn_id", "prompt_id", "promptId"]),
        agent_id: string(record, &["agent_id", "agentId"]),
        parent_agent_id: string(record, &["parent_agent_id", "parentAgentId"]),
        source_timestamp: string(record, &["timestamp"]),
        observed_at,
        model: string(record, &["model"]).or_else(|| {
            record
                .pointer("/message/model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        service_tier: string(record, &["service_tier"]).or_else(|| {
            usage_value
                .and_then(|v| v.get("service_tier"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        region: string(record, &["region"]).or_else(|| {
            usage_value
                .and_then(|v| v.get("inference_geo"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        usage,
        provider_reported_cost_micros: record.get("cost_micros").and_then(Value::as_u64),
        source_path: path.display().to_string(),
        source_offset: line_offset,
        adapter_version: ADAPTER_VERSION.into(),
        parser_version: PARSER_VERSION.into(),
    }
}

fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
}
fn count(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

/// Real Claude transcripts nest cache-creation counters under
/// `usage.cache_creation` (e.g. `ephemeral_1h_input_tokens`,
/// `ephemeral_5m_input_tokens`). Sum the present fields; report a measurement
/// only when at least one nested field exists.
fn nested_cache_creation_tokens(usage_value: &Value) -> Option<u64> {
    let creation = usage_value.get("cache_creation")?;
    let mut total = 0_u64;
    let mut present = false;
    for key in ["ephemeral_1h_input_tokens", "ephemeral_5m_input_tokens"] {
        if let Some(v) = creation.get(key).and_then(Value::as_u64) {
            present = true;
            total = total.saturating_add(v);
        }
    }
    present.then_some(total)
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
                canonical_source_kind(obs),
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

    if parse_res.stats.negative_deltas > 0 {
        let stable_file_id = hex::encode(&sha2::Sha256::digest(source_path_str.as_bytes())[..8]);
        let raw_key = format!(
            "claude:{}:{}:{}",
            stable_file_id, "negative_delta", PARSER_VERSION
        );
        let anom_id = format!(
            "anom_{}",
            &hex::encode(sha2::Sha256::digest(raw_key.as_bytes()))[..16]
        );
        tx.execute(
            "INSERT OR IGNORE INTO measurement_anomalies (id, session_id, turn_id, type, source_values_json, created_at) VALUES (?1, NULL, NULL, 'negative_delta', ?2, ?3)",
            rusqlite::params![
                anom_id,
                serde_json::json!({
                    "count": parse_res.stats.negative_deltas,
                    "source_path": source_path_str,
                }).to_string(),
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
    }

    tx.commit()?;

    let mut anomaly_types = Vec::new();
    if parse_res.stats.unsupported > 0 {
        anomaly_types.push("unsupported_version".to_string());
    }
    if parse_res.stats.malformed > 0 {
        anomaly_types.push("malformed_record".to_string());
    }
    if parse_res.stats.negative_deltas > 0 {
        anomaly_types.push("negative_delta".to_string());
    }

    Ok(tokentree_core::AdapterImportResult {
        inserted,
        duplicates,
        malformed: parse_res.stats.malformed,
        unsupported: parse_res.stats.unsupported,
        anomalies: parse_res.stats.malformed
            + parse_res.stats.unsupported
            + parse_res.stats.negative_deltas,
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

    #[test]
    fn usage_snapshots_become_per_session_deltas() {
        // C4: cumulative usage_snapshot records must be diffed per session,
        // not summed as if they were deltas.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("snapshots.jsonl");
        let content = concat!(
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":100,\"output_tokens\":40}}\n",
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s2\",\"usage\":{\"input_tokens\":50,\"output_tokens\":20}}\n",
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":150,\"output_tokens\":60}}\n",
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s2\",\"usage\":{\"input_tokens\":90,\"output_tokens\":35}}\n",
        );
        std::fs::write(&path, content).unwrap();

        let result = parse_session(&path).unwrap();
        // Two baselines emit nothing; two deltas are emitted.
        assert_eq!(result.stats.parsed, 2);
        assert_eq!(result.stats.negative_deltas, 0);

        let delta_s1 = result
            .observations
            .iter()
            .find(|o| o.provider_session_id == "s1")
            .expect("s1 delta emitted");
        assert_eq!(delta_s1.usage.input_tokens, Some(50));
        assert_eq!(delta_s1.usage.output_tokens, Some(20));
        assert_eq!(delta_s1.source, MeasurementSource::SnapshotDelta);

        let delta_s2 = result
            .observations
            .iter()
            .find(|o| o.provider_session_id == "s2")
            .expect("s2 delta emitted");
        // s2 must diff against s2's own baseline, not s1's.
        assert_eq!(delta_s2.usage.input_tokens, Some(40));
        assert_eq!(delta_s2.usage.output_tokens, Some(15));
    }

    #[test]
    fn negative_snapshot_delta_is_anomaly_not_observation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("reset.jsonl");
        let content = concat!(
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":100,\"output_tokens\":40}}\n",
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":30,\"output_tokens\":10}}\n",
            "{\"type\":\"usage_snapshot\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":60,\"output_tokens\":25}}\n",
        );
        std::fs::write(&path, content).unwrap();

        let result = parse_session(&path).unwrap();
        assert_eq!(result.stats.negative_deltas, 1);
        // Only the post-reset delta is emitted, diffed against the reset baseline.
        assert_eq!(result.stats.parsed, 1);
        let delta = &result.observations[0];
        assert_eq!(delta.usage.input_tokens, Some(30));
        assert_eq!(delta.usage.output_tokens, Some(15));
    }

    #[test]
    fn nested_cache_creation_fields_are_parsed() {
        // H9: real transcripts nest cache creation under usage.cache_creation.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cache.jsonl");
        let content = "{\"type\":\"assistant\",\"session_id\":\"s1\",\"message\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cache_creation\":{\"ephemeral_1h_input_tokens\":500,\"ephemeral_5m_input_tokens\":250}}}}\n";
        std::fs::write(&path, content).unwrap();

        let result = parse_session(&path).unwrap();
        let obs = result.observations.first().expect("one observation");
        assert_eq!(obs.usage.cache_write_tokens, Some(750));
    }

    #[test]
    fn flat_cache_creation_key_takes_precedence_over_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cache_flat.jsonl");
        let content = "{\"type\":\"assistant\",\"session_id\":\"s1\",\"message\":{\"usage\":{\"input_tokens\":100,\"cache_creation_input_tokens\":42,\"cache_creation\":{\"ephemeral_1h_input_tokens\":500}}}}\n";
        std::fs::write(&path, content).unwrap();

        let result = parse_session(&path).unwrap();
        let obs = result.observations.first().expect("one observation");
        assert_eq!(obs.usage.cache_write_tokens, Some(42));
    }
}
