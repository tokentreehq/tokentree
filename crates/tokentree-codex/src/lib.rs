// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
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
pub struct CodexAnomaly {
    pub anomaly_type: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub source_path: String,
    pub source_offset: u64,
    pub details: Value,
}

#[derive(Debug)]
pub struct ParseResult {
    pub observations: Vec<UsageObservation>,
    pub anomalies: Vec<CodexAnomaly>,
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
    // Read-only source file opening
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let fallback_session = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");

    let mut observations = Vec::new();
    let mut anomalies = Vec::new();
    let mut stats = ParseStats::default();
    let mut offset = 0_u64;

    // Stream correlation state
    let mut active_session_id: Option<String> = None;
    let mut active_turn_id: Option<String> = None;
    let mut active_agent_id: Option<String> = None;
    let mut active_parent_agent_id: Option<String> = None;

    // Cumulative token usage tracking for counter-reset / negative-delta detection
    let mut cumulative_counters: HashMap<String, TokenUsage> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
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
                stats.anomalies += 1;
                anomalies.push(CodexAnomaly {
                    anomaly_type: "malformed_record".to_string(),
                    session_id: active_session_id.clone(),
                    turn_id: active_turn_id.clone(),
                    source_path: path.display().to_string(),
                    source_offset: line_offset,
                    details: serde_json::json!({ "error": "malformed JSON object", "raw_length": line.len() }),
                });
                continue;
            }
        };

        // 1. Hook boundary correlation:
        // Identify turn boundaries, session/thread IDs, and agent identifiers
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

        // Update session_id if explicitly present on any record
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

        // 2. Token usage event extraction:
        // Support app-server events (thread/tokenUsage/updated), rollout events, and turn summaries
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
            if is_hook_boundary_start(&record_type) || is_hook_boundary_end(&record_type) {
                // Control hook event without tokens
                if is_hook_boundary_end(&record_type) {
                    // Turn completion marker
                }
            } else {
                stats.unknown += 1;
            }
            continue;
        };

        // Extract raw token values
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

        // 3. Correlate identifiers with hook boundaries
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

        // 4. Handle cumulative counters, negative deltas, and counter resets
        let effective_usage = if is_cumulative {
            if let Some(prev) = cumulative_counters.get(&stream_key) {
                // Check for negative deltas
                let neg_input =
                    raw_usage.input_tokens.unwrap_or(0) < prev.input_tokens.unwrap_or(0);
                let neg_output =
                    raw_usage.output_tokens.unwrap_or(0) < prev.output_tokens.unwrap_or(0);
                let neg_cached = raw_usage.cached_input_tokens.unwrap_or(0)
                    < prev.cached_input_tokens.unwrap_or(0);
                let neg_reasoning =
                    raw_usage.reasoning_tokens.unwrap_or(0) < prev.reasoning_tokens.unwrap_or(0);

                if neg_input || neg_output || neg_cached || neg_reasoning {
                    // Counter reset or negative delta detected!
                    stats.anomalies += 1;
                    anomalies.push(CodexAnomaly {
                        anomaly_type: "negative_delta".to_string(),
                        session_id: Some(session_id.clone()),
                        turn_id: turn_id.clone(),
                        source_path: path.display().to_string(),
                        source_offset: line_offset,
                        details: serde_json::json!({
                            "stream": stream_key,
                            "previous": {
                                "input_tokens": prev.input_tokens,
                                "output_tokens": prev.output_tokens,
                                "cached_input_tokens": prev.cached_input_tokens,
                                "reasoning_tokens": prev.reasoning_tokens,
                            },
                            "current": {
                                "input_tokens": raw_usage.input_tokens,
                                "output_tokens": raw_usage.output_tokens,
                                "cached_input_tokens": raw_usage.cached_input_tokens,
                                "reasoning_tokens": raw_usage.reasoning_tokens,
                            }
                        }),
                    });

                    // On counter reset, update tracker and count full reset values
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

        // 5. Source ranking and counter deduplication
        let source_kind = string(&record, &["source_kind"]).unwrap_or_else(|| {
            if record_type == "turn_counter" || record_type == "turn_summary" {
                "codex_turn_counter".to_string()
            } else if record_type == "thread/tokenUsage/updated" || record_type == "token_usage" {
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

        // 6. Privacy: STRICT ENFORCEMENT - NO PROMPT / COMPLETION TEXT EXTRACTED
        observations.push(UsageObservation {
            adapter: "codex".into(),
            source,
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

    Ok(ParseResult {
        observations,
        anomalies,
        stats,
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
