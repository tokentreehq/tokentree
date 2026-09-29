// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation, deduplicate};
use walkdir::WalkDir;

pub const ADAPTER_VERSION: &str = "0.2.0-rust";
pub const PARSER_VERSION: &str = "0.2.0-rust";

#[must_use]
pub fn is_supported_codex_version(version: &str) -> bool {
    matches!(
        version,
        "v1" | "v1.0"
            | "1.0"
            | "2024-11-05"
            | "codex-app-server-v1"
            | "codex-rollout-v1"
            | "0.2.0-rust"
    )
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct ParseStats {
    pub parsed: u64,
    pub unknown: u64,
    pub malformed: u64,
    pub anomalies: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CodexParserState {
    pub version: u32,
    pub active_session_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub active_agent_id: Option<String>,
    pub active_parent_agent_id: Option<String>,
    pub cumulative_counters: HashMap<String, TokenUsage>,
    pub protocol_version: Option<String>,
    pub turn_detailed_tokens: HashMap<String, TokenUsage>,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<CodexAnomaly>,
    pub stats: ParseStats,
    pub final_state: CodexParserState,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CodexImportResult {
    pub inserted: u64,
    pub duplicates: u64,
    pub anomalies: u64,
    pub start_offset: u64,
    pub end_offset: u64,
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
    parse_reader(BufReader::new(file), path, 0, CodexParserState::default())
}

pub fn parse_reader<R: BufRead>(
    mut reader: R,
    path: &Path,
    start_offset: u64,
    initial_state: CodexParserState,
) -> Result<ParseResult> {
    let fallback_session = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");

    let mut raw_observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut offset = start_offset;

    let mut active_session_id: Option<String> = initial_state.active_session_id;
    let mut active_turn_id: Option<String> = initial_state.active_turn_id;
    let mut active_agent_id: Option<String> = initial_state.active_agent_id;
    let mut active_parent_agent_id: Option<String> = initial_state.active_parent_agent_id;
    let mut active_protocol_version: Option<String> = initial_state.protocol_version;

    let mut cumulative_counters: HashMap<String, TokenUsage> = initial_state.cumulative_counters;
    let mut turn_detailed_tokens: HashMap<String, TokenUsage> = initial_state.turn_detailed_tokens;

    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        let bytes_read = match reader.read_line(&mut line_buf) {
            Ok(n) => n,
            Err(_) => {
                stats.malformed += 1;
                stats.anomalies += 1;
                anomalies.push(CodexAnomaly {
                    anomaly_type: "malformed_record".to_string(),
                    session_id: active_session_id.clone(),
                    turn_id: active_turn_id.clone(),
                    source_path: path.display().to_string(),
                    source_offset: offset,
                    details: serde_json::json!({ "error": "invalid utf-8 line" }),
                });
                break;
            }
        };

        if bytes_read == 0 {
            break;
        }

        let line_offset = offset;
        offset += bytes_read as u64;

        let trimmed = line_buf.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(Value::Object(record)) => Value::Object(record),
            _ => {
                stats.malformed += 1;
                stats.anomalies += 1;
                anomalies.push(CodexAnomaly {
                    anomaly_type: "malformed_record".to_string(),
                    session_id: active_session_id.clone(),
                    turn_id: active_turn_id.clone(),
                    source_path: path.display().to_string(),
                    source_offset: line_offset,
                    details: serde_json::json!({ "error": "malformed JSON object", "raw_length": bytes_read }),
                });
                continue;
            }
        };

        // Check version if present: mark unsupported versions degraded rather than silently guessing
        if let Some(ver) = string(&record, &["version", "protocol_version", "schema_version"]) {
            if !is_supported_codex_version(&ver) {
                stats.unknown += 1;
                stats.anomalies += 1;
                anomalies.push(CodexAnomaly {
                    anomaly_type: "unsupported_version".to_string(),
                    session_id: active_session_id.clone(),
                    turn_id: active_turn_id.clone(),
                    source_path: path.display().to_string(),
                    source_offset: line_offset,
                    details: serde_json::json!({ "unsupported_version": ver }),
                });
                continue;
            } else {
                active_protocol_version = Some(ver);
            }
        }

        let record_type =
            string(&record, &["type", "event", "boundary", "method"]).unwrap_or_default();

        if is_hook_boundary_start(&record_type) {
            if let Some(s_id) = string(
                &record,
                &["session_id", "sessionId", "thread_id", "threadId"],
            )
            .or_else(|| {
                record
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
                active_session_id = Some(s_id);
            }
            if let Some(t_id) = string(&record, &["turn_id", "turnId", "id"]).or_else(|| {
                record
                    .pointer("/params/turnId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
                active_turn_id = Some(t_id);
            }
            if let Some(a_id) = string(&record, &["agent_id", "agentId"]).or_else(|| {
                record
                    .pointer("/params/agentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
                active_agent_id = Some(a_id);
            }
            if let Some(pa_id) =
                string(&record, &["parent_agent_id", "parentAgentId"]).or_else(|| {
                    record
                        .pointer("/params/parentAgentId")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            {
                active_parent_agent_id = Some(pa_id);
            }
        }

        if is_hook_boundary_end(&record_type) {
            if record_type == "turn/completed"
                || record_type == "turn_complete"
                || record_type == "turn/end"
            {
                active_turn_id = None;
            } else if record_type == "session/completed" {
                active_session_id = None;
                active_turn_id = None;
                active_agent_id = None;
                active_parent_agent_id = None;
            }
        }

        if let Some(s_id) = string(
            &record,
            &["session_id", "sessionId", "thread_id", "threadId"],
        )
        .or_else(|| {
            record
                .pointer("/params/threadId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            active_session_id = Some(s_id);
        }

        let is_cumulative = record
            .get("is_cumulative")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || record.get("cumulative_token_usage").is_some()
            || record_type == "cumulative_usage";

        let usage_value = record
            .get("token_usage")
            .or_else(|| record.get("usage"))
            .or_else(|| record.get("cumulative_token_usage"))
            .or_else(|| record.pointer("/params/tokenUsage"))
            .or_else(|| record.pointer("/params/usage"))
            .or_else(|| record.pointer("/message/usage"));

        let Some(usage_value) = usage_value else {
            if !is_hook_boundary_start(&record_type) && !is_hook_boundary_end(&record_type) {
                stats.unknown += 1;
            }
            continue;
        };

        let raw_usage = TokenUsage {
            input_tokens: count(
                usage_value,
                &["input_tokens", "inputTokens", "prompt_tokens"],
            ),
            cached_input_tokens: count(
                usage_value,
                &[
                    "cached_input_tokens",
                    "cachedInputTokens",
                    "cache_read_input_tokens",
                ],
            )
            .or_else(|| {
                usage_value
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
            }),
            cache_write_tokens: count(
                usage_value,
                &[
                    "cache_creation_input_tokens",
                    "cache_write_tokens",
                    "cacheWriteTokens",
                ],
            ),
            output_tokens: count(
                usage_value,
                &["output_tokens", "outputTokens", "completion_tokens"],
            ),
            reasoning_tokens: count(usage_value, &["reasoning_tokens", "reasoningTokens"]).or_else(
                || {
                    usage_value
                        .pointer("/completion_tokens_details/reasoning_tokens")
                        .or_else(|| usage_value.pointer("/output_tokens_details/thinking_tokens"))
                        .and_then(Value::as_u64)
                },
            ),
        };

        if !raw_usage.is_measured() {
            stats.unknown += 1;
            continue;
        }

        let session_id = string(
            &record,
            &["session_id", "sessionId", "thread_id", "threadId"],
        )
        .or_else(|| {
            record
                .pointer("/params/threadId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| active_session_id.clone())
        .unwrap_or_else(|| fallback_session.into());

        let turn_id = string(&record, &["turn_id", "turnId"])
            .or_else(|| {
                record
                    .pointer("/params/turnId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| active_turn_id.clone());

        let agent_id = string(&record, &["agent_id", "agentId"])
            .or_else(|| {
                record
                    .pointer("/params/agentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| active_agent_id.clone());

        let parent_agent_id = string(&record, &["parent_agent_id", "parentAgentId"])
            .or_else(|| {
                record
                    .pointer("/params/parentAgentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| active_parent_agent_id.clone());

        let request_id = string(&record, &["request_id", "requestId", "id"]).or_else(|| {
            record
                .pointer("/params/requestId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });

        let stream_key = format!(
            "{}:{}",
            string(&record, &["model"]).unwrap_or_default(),
            agent_id.as_deref().unwrap_or("root")
        );

        let effective_usage = if is_cumulative {
            if let Some(prev) = cumulative_counters.get(&stream_key) {
                let neg_input =
                    raw_usage.input_tokens.unwrap_or(0) < prev.input_tokens.unwrap_or(0);
                let neg_output =
                    raw_usage.output_tokens.unwrap_or(0) < prev.output_tokens.unwrap_or(0);
                let neg_cached = raw_usage.cached_input_tokens.unwrap_or(0)
                    < prev.cached_input_tokens.unwrap_or(0);
                let neg_reasoning =
                    raw_usage.reasoning_tokens.unwrap_or(0) < prev.reasoning_tokens.unwrap_or(0);

                if neg_input || neg_output || neg_cached || neg_reasoning {
                    stats.anomalies += 1;
                    anomalies.push(CodexAnomaly {
                        anomaly_type: "negative_delta".to_string(),
                        session_id: Some(session_id.clone()),
                        turn_id: turn_id.clone(),
                        source_path: path.display().to_string(),
                        source_offset: line_offset,
                        details: serde_json::json!({
                            "stream": stream_key,
                            "previous": prev,
                            "current": raw_usage,
                        }),
                    });

                    cumulative_counters.insert(stream_key, raw_usage.clone());
                    raw_usage
                } else {
                    let delta = TokenUsage {
                        input_tokens: Some(
                            raw_usage.input_tokens.unwrap_or(0) - prev.input_tokens.unwrap_or(0),
                        ),
                        cached_input_tokens: Some(
                            raw_usage.cached_input_tokens.unwrap_or(0)
                                - prev.cached_input_tokens.unwrap_or(0),
                        ),
                        cache_write_tokens: Some(
                            raw_usage.cache_write_tokens.unwrap_or(0)
                                - prev.cache_write_tokens.unwrap_or(0),
                        ),
                        output_tokens: Some(
                            raw_usage.output_tokens.unwrap_or(0) - prev.output_tokens.unwrap_or(0),
                        ),
                        reasoning_tokens: Some(
                            raw_usage.reasoning_tokens.unwrap_or(0)
                                - prev.reasoning_tokens.unwrap_or(0),
                        ),
                    };
                    cumulative_counters.insert(stream_key, raw_usage);
                    delta
                }
            } else {
                cumulative_counters.insert(stream_key, raw_usage.clone());
                raw_usage
            }
        } else {
            let entry = cumulative_counters.entry(stream_key).or_default();
            entry.input_tokens =
                Some(entry.input_tokens.unwrap_or(0) + raw_usage.input_tokens.unwrap_or(0));
            entry.cached_input_tokens = Some(
                entry.cached_input_tokens.unwrap_or(0) + raw_usage.cached_input_tokens.unwrap_or(0),
            );
            entry.cache_write_tokens = Some(
                entry.cache_write_tokens.unwrap_or(0) + raw_usage.cache_write_tokens.unwrap_or(0),
            );
            entry.output_tokens =
                Some(entry.output_tokens.unwrap_or(0) + raw_usage.output_tokens.unwrap_or(0));
            entry.reasoning_tokens =
                Some(entry.reasoning_tokens.unwrap_or(0) + raw_usage.reasoning_tokens.unwrap_or(0));
            raw_usage
        };

        // Source classification and explicit precedence:
        // 1. official app-server request event (MeasurementSource::OfficialTelemetry, rank 5)
        // 2. rollout request event (MeasurementSource::TranscriptRequest, rank 3)
        // 3. cumulative/turn counter fallback (MeasurementSource::SnapshotDelta, rank 2)
        let source_kind = string(&record, &["source_kind"]).unwrap_or_else(|| {
            if record_type == "turn_counter"
                || record_type == "turn_summary"
                || record.get("turn_counter").is_some()
            {
                "codex_turn_counter".to_string()
            } else if record_type == "thread/tokenUsage/updated"
                || record_type == "turn/tokenUsage/updated"
                || record_type == "token_usage"
            {
                "codex_app_server".to_string()
            } else {
                "codex_rollout".to_string()
            }
        });

        let source = if source_kind == "codex_turn_counter" {
            MeasurementSource::SnapshotDelta
        } else if source_kind == "codex_app_server" {
            MeasurementSource::OfficialTelemetry
        } else {
            MeasurementSource::TranscriptRequest
        };

        let observed_at = Utc::now().to_rfc3339();
        let model = string(&record, &["model"]).or_else(|| {
            record
                .pointer("/params/model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });

        let source_timestamp = string(&record, &["timestamp", "created_at"]);

        // Strict privacy: no prompt or completion text stored
        raw_observations.push(UsageObservation {
            adapter: "codex".into(),
            source,
            source_subtype: Some(source_kind),
            source_event_id: string(&record, &["event_id", "uuid"]),
            provider_session_id: session_id,
            request_id,
            turn_id,
            agent_id,
            parent_agent_id,
            source_timestamp,
            observed_at,
            model,
            service_tier: string(&record, &["service_tier"]),
            region: string(&record, &["region"]),
            usage: effective_usage,
            provider_reported_cost_micros: record.get("cost_micros").and_then(Value::as_u64),
            source_path: path.display().to_string(),
            source_offset: line_offset,
            adapter_version: ADAPTER_VERSION.into(),
            parser_version: PARSER_VERSION.into(),
        });
        stats.parsed += 1;
    }

    // Correlate turn counters by adapter + session + turn:
    // A turn counter must be used ONLY when no authoritative detailed events cover that turn.
    // Never sum a detailed request event and a covering turn counter.
    let mut turn_detailed_map: HashMap<(String, String), Vec<UsageObservation>> = HashMap::new();
    let mut turn_counter_map: HashMap<(String, String), Vec<UsageObservation>> = HashMap::new();
    let mut non_turn_observations = Vec::new();

    for obs in raw_observations {
        if let Some(turn) = &obs.turn_id {
            let key = (obs.provider_session_id.clone(), turn.clone());
            if obs.source_subtype.as_deref() == Some("codex_turn_counter") {
                turn_counter_map.entry(key).or_default().push(obs);
            } else {
                turn_detailed_map.entry(key).or_default().push(obs);
            }
        } else {
            non_turn_observations.push(obs);
        }
    }

    let mut final_observations = non_turn_observations;

    // Collect all unique (session, turn) keys
    let mut all_turns: Vec<(String, String)> = turn_detailed_map
        .keys()
        .chain(turn_counter_map.keys())
        .cloned()
        .collect();
    all_turns.sort();
    all_turns.dedup();

    for key in all_turns {
        let (session, turn) = &key;
        let detailed = turn_detailed_map.remove(&key).unwrap_or_default();
        let counters = turn_counter_map.remove(&key).unwrap_or_default();

        if !detailed.is_empty() {
            // Detailed events exist! They take precedence.
            // Deduplicate detailed events (e.g. app-server vs rollout for same request_id)
            let deduped = deduplicate(detailed);
            let detailed_events = deduped.canonical;

            // Track detailed tokens for this turn in state so subsequent restarts know this turn is covered
            let turn_key = format!("{session}:{turn}");
            let entry = turn_detailed_tokens.entry(turn_key).or_default();
            for d in &detailed_events {
                entry.input_tokens =
                    Some(entry.input_tokens.unwrap_or(0) + d.usage.input_tokens.unwrap_or(0));
                entry.cached_input_tokens = Some(
                    entry.cached_input_tokens.unwrap_or(0)
                        + d.usage.cached_input_tokens.unwrap_or(0),
                );
                entry.cache_write_tokens = Some(
                    entry.cache_write_tokens.unwrap_or(0) + d.usage.cache_write_tokens.unwrap_or(0),
                );
                entry.output_tokens =
                    Some(entry.output_tokens.unwrap_or(0) + d.usage.output_tokens.unwrap_or(0));
                entry.reasoning_tokens = Some(
                    entry.reasoning_tokens.unwrap_or(0) + d.usage.reasoning_tokens.unwrap_or(0),
                );
            }

            if !counters.is_empty() {
                // There is a covering counter!
                // Verify whether counter matches sum of detailed events
                let det_input: u64 = detailed_events
                    .iter()
                    .map(|d| d.usage.input_tokens.unwrap_or(0))
                    .sum();
                let det_cached: u64 = detailed_events
                    .iter()
                    .map(|d| d.usage.cached_input_tokens.unwrap_or(0))
                    .sum();
                let det_output: u64 = detailed_events
                    .iter()
                    .map(|d| d.usage.output_tokens.unwrap_or(0))
                    .sum();
                let det_reasoning: u64 = detailed_events
                    .iter()
                    .map(|d| d.usage.reasoning_tokens.unwrap_or(0))
                    .sum();

                let counter = &counters[0];
                let ctr_input = counter.usage.input_tokens.unwrap_or(0);
                let ctr_cached = counter.usage.cached_input_tokens.unwrap_or(0);
                let ctr_output = counter.usage.output_tokens.unwrap_or(0);
                let ctr_reasoning = counter.usage.reasoning_tokens.unwrap_or(0);

                if det_input != ctr_input
                    || det_output != ctr_output
                    || det_cached != ctr_cached
                    || det_reasoning != ctr_reasoning
                {
                    // Conflicting counter! Record anomaly.
                    stats.anomalies += 1;
                    anomalies.push(CodexAnomaly {
                        anomaly_type: "counter_conflict".to_string(),
                        session_id: Some(session.clone()),
                        turn_id: Some(turn.clone()),
                        source_path: counter.source_path.clone(),
                        source_offset: counter.source_offset,
                        details: serde_json::json!({
                            "detailed_sum": {
                                "input_tokens": det_input,
                                "cached_input_tokens": det_cached,
                                "output_tokens": det_output,
                                "reasoning_tokens": det_reasoning
                            },
                            "counter": {
                                "input_tokens": ctr_input,
                                "cached_input_tokens": ctr_cached,
                                "output_tokens": ctr_output,
                                "reasoning_tokens": ctr_reasoning
                            }
                        }),
                    });
                }
                // In both equal and conflicting cases, the covering counter is SUPPRESSED
                // and never summed with the detailed request events!
            }

            final_observations.extend(detailed_events);
        } else if !counters.is_empty() {
            let turn_key = format!("{session}:{turn}");
            if let Some(prev_detailed) = turn_detailed_tokens.get(&turn_key) {
                // Covered by detailed events from a previous batch before restart!
                let counter = &counters[0];
                let ctr_input = counter.usage.input_tokens.unwrap_or(0);
                let ctr_cached = counter.usage.cached_input_tokens.unwrap_or(0);
                let ctr_output = counter.usage.output_tokens.unwrap_or(0);
                let ctr_reasoning = counter.usage.reasoning_tokens.unwrap_or(0);

                let det_input = prev_detailed.input_tokens.unwrap_or(0);
                let det_cached = prev_detailed.cached_input_tokens.unwrap_or(0);
                let det_output = prev_detailed.output_tokens.unwrap_or(0);
                let det_reasoning = prev_detailed.reasoning_tokens.unwrap_or(0);

                if det_input != ctr_input
                    || det_output != ctr_output
                    || det_cached != ctr_cached
                    || det_reasoning != ctr_reasoning
                {
                    stats.anomalies += 1;
                    anomalies.push(CodexAnomaly {
                        anomaly_type: "counter_conflict".to_string(),
                        session_id: Some(session.clone()),
                        turn_id: Some(turn.clone()),
                        source_path: counter.source_path.clone(),
                        source_offset: counter.source_offset,
                        details: serde_json::json!({
                            "detailed_sum": {
                                "input_tokens": det_input,
                                "cached_input_tokens": det_cached,
                                "output_tokens": det_output,
                                "reasoning_tokens": det_reasoning
                            },
                            "counter": {
                                "input_tokens": ctr_input,
                                "cached_input_tokens": ctr_cached,
                                "output_tokens": ctr_output,
                                "reasoning_tokens": ctr_reasoning
                            }
                        }),
                    });
                }
                // Suppressed! Do not emit counter.
            } else {
                // No detailed events cover this turn: the turn counter is the fallback.
                // Deduplicate repeated counters so only 1 counter is added.
                let deduped = deduplicate(counters);
                final_observations.extend(deduped.canonical);
            }
        }
    }

    let final_state = CodexParserState {
        version: 1,
        active_session_id,
        active_turn_id,
        active_agent_id,
        active_parent_agent_id,
        cumulative_counters,
        protocol_version: active_protocol_version,
        turn_detailed_tokens,
    };

    Ok(ParseResult {
        observations: final_observations,
        anomalies,
        stats,
        final_state,
    })
}

/// Durable, restartable Codex session import with atomic checkpoints.
pub fn import_codex_file(connection: &mut Connection, path: &Path) -> Result<CodexImportResult> {
    let source_path_str = path.display().to_string();
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let current_size = metadata.len();
    let modified_at = chrono::DateTime::<Utc>::from(metadata.modified()?).to_rfc3339();

    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;

    struct IngestionCheckpointRow {
        last_offset: i64,
        file_hash: Option<String>,
        parser_version: Option<String>,
        adapter_state_json: Option<String>,
    }

    // Query existing checkpoint
    let checkpoint: Option<IngestionCheckpointRow> = connection
        .query_row(
            "SELECT last_offset, file_hash, parser_version, adapter_state_json
             FROM ingestion_checkpoints
             WHERE adapter = 'codex' AND source_path = ?1",
            [&source_path_str],
            |row| {
                Ok(IngestionCheckpointRow {
                    last_offset: row.get(0)?,
                    file_hash: row.get(1)?,
                    parser_version: row.get(2)?,
                    adapter_state_json: row.get(3)?,
                })
            },
        )
        .ok();

    let mut start_offset = 0_u64;
    let mut initial_state = CodexParserState::default();
    if let Some(cp) = checkpoint {
        let last_off = cp.last_offset.max(0) as u64;
        let is_truncated = current_size < last_off;
        let is_rotated = if let (Some(ph), true) = (cp.file_hash.as_deref(), last_off > 0) {
            let check_len = 4096.min(last_off as usize);
            if current_size < check_len as u64 {
                true
            } else {
                let mut prefix_buf = vec![0_u8; check_len];
                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut prefix_buf)?;
                let check_hash = hex::encode(Sha256::digest(&prefix_buf));
                check_hash != ph
            }
        } else {
            false
        };
        let is_parser_changed =
            cp.parser_version.is_some() && cp.parser_version.as_deref() != Some(PARSER_VERSION);

        if !is_truncated && !is_rotated && !is_parser_changed {
            start_offset = last_off;
            if let Some(state_json) = cp.adapter_state_json {
                if let Ok(st) = serde_json::from_str::<CodexParserState>(&state_json) {
                    initial_state = st;
                }
            }
        }
    }

    if start_offset >= current_size {
        return Ok(CodexImportResult {
            inserted: 0,
            duplicates: 0,
            anomalies: 0,
            start_offset,
            end_offset: start_offset,
        });
    }

    // Read only complete lines up to the last '\n'
    file.seek(SeekFrom::Start(start_offset))?;
    let remaining_bytes = (current_size - start_offset) as usize;
    let mut read_buf = vec![0_u8; remaining_bytes];
    file.read_exact(&mut read_buf)?;

    // Find the last newline position
    let valid_len = match read_buf.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => {
            // Partial final line without newline: do not commit, wait for completion
            return Ok(CodexImportResult {
                inserted: 0,
                duplicates: 0,
                anomalies: 0,
                start_offset,
                end_offset: start_offset,
            });
        }
    };

    let slice_to_parse = &read_buf[..valid_len];
    let end_offset = start_offset + valid_len as u64;

    let parse_res = parse_reader(
        BufReader::new(slice_to_parse),
        path,
        start_offset,
        initial_state,
    )?;

    // Commit observations, anomalies, and checkpoint in a single atomic transaction
    let tx = connection.transaction()?;

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
            "codex:{}:{}:{}:{}",
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

    // Compute prefix hash of committed bytes (up to 4096 bytes)
    let commit_prefix_len = 4096.min(end_offset as usize);
    let commit_file_hash = if commit_prefix_len > 0 {
        let mut prefix_buf = vec![0_u8; commit_prefix_len];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut prefix_buf)?;
        Some(hex::encode(Sha256::digest(&prefix_buf)))
    } else {
        None
    };

    let state_json = serde_json::to_string(&parse_res.final_state).unwrap_or_default();

    // Update ingestion checkpoint
    tx.execute(
        "INSERT INTO ingestion_checkpoints(
            adapter, source_path, file_size, modified_at, last_offset, last_event_hash, file_hash, parser_version, adapter_state_json
         ) VALUES ('codex', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
            current_size as i64,
            modified_at,
            end_offset as i64,
            last_event_hash,
            commit_file_hash,
            PARSER_VERSION,
            state_json,
        ],
    )?;

    tx.commit()?;

    Ok(CodexImportResult {
        inserted,
        duplicates,
        anomalies: parse_res.anomalies.len() as u64,
        start_offset,
        end_offset,
    })
}

fn is_hook_boundary_start(record_type: &str) -> bool {
    matches!(
        record_type,
        "turn/started"
            | "turn_start"
            | "turn/start"
            | "hook_boundary"
            | "turn_boundary"
            | "session/started"
            | "session_meta"
    )
}

fn is_hook_boundary_end(record_type: &str) -> bool {
    matches!(
        record_type,
        "turn/completed" | "turn_complete" | "turn/end" | "session/completed"
    )
}

fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
}

fn count(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}
