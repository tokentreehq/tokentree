// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
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
