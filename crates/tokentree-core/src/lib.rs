// SPDX-License-Identifier: Apache-2.0
pub mod classifier;
pub mod pricing;
pub mod project;

pub use classifier::{
    BoundaryInput, BoundaryOutcome, BoundaryResult, classify_boundary, redacted_label,
};
pub use pricing::{PriceRate, PriceSnapshot};
pub use project::{
    ProjectCandidate, ProjectDetectionMethod, ResolveProjectInput, format_title, resolve_project,
    slug_key,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementSource {
    Unavailable,
    ExplicitCli,
    SnapshotDelta,
    TranscriptRequest,
    ProviderFields,
    OfficialTelemetry,
}

impl MeasurementSource {
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Unavailable => 0,
            Self::ExplicitCli => 1,
            Self::SnapshotDelta => 2,
            Self::TranscriptRequest => 3,
            Self::ProviderFields => 4,
            Self::OfficialTelemetry => 5,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::ExplicitCli => "explicit_cli",
            Self::SnapshotDelta => "snapshot_delta",
            Self::TranscriptRequest => "transcript_request",
            Self::ProviderFields => "provider_fields",
            Self::OfficialTelemetry => "official_telemetry",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

impl TokenUsage {
    #[must_use]
    pub fn is_measured(&self) -> bool {
        self.input_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.output_tokens.is_some()
            || self.reasoning_tokens.is_some()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdapterImportResult {
    pub inserted: u64,
    pub duplicates: u64,
    pub malformed: u64,
    pub unsupported: u64,
    pub anomalies: u64,
    pub anomaly_types: Vec<String>,
    pub latest_event_identity: Option<String>,
    pub latest_authoritative_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub start_offset: u64,
    pub end_offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageObservation {
    pub adapter: String,
    pub source: MeasurementSource,
    #[serde(default)]
    pub source_subtype: Option<String>,
    pub source_event_id: Option<String>,
    pub provider_session_id: String,
    pub request_id: Option<String>,
    pub turn_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub parent_agent_id: Option<String>,
    pub source_timestamp: Option<String>,
    pub observed_at: String,
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub region: Option<String>,
    pub usage: TokenUsage,
    pub provider_reported_cost_micros: Option<u64>,
    pub source_path: String,
    pub source_offset: u64,
    pub adapter_version: String,
    pub parser_version: String,
}

impl UsageObservation {
    #[must_use]
    pub fn canonical_identity(&self) -> String {
        if self.source == MeasurementSource::SnapshotDelta {
            if let Some(event) = &self.source_event_id {
                return format!("{}:delta:{event}", self.adapter);
            }
        }
        if let Some(request) = &self.request_id {
            return format!("{}:request:{request}", self.adapter);
        }
        if let Some(event) = &self.source_event_id {
            return format!("{}:event:{event}", self.adapter);
        }
        if (self.source_subtype.as_deref() == Some("codex_turn_counter")
            || self.source == MeasurementSource::SnapshotDelta)
            && self.turn_id.is_some()
        {
            return format!(
                "{}:counter:{}:{}",
                self.adapter,
                self.provider_session_id,
                self.turn_id.as_deref().unwrap_or_default()
            );
        }
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            self.adapter,
            self.provider_session_id,
            self.turn_id.as_deref().unwrap_or_default(),
            self.model.as_deref().unwrap_or_default(),
            self.source_timestamp.as_deref().unwrap_or_default(),
            self.usage
                .input_tokens
                .map_or(String::new(), |v| v.to_string()),
            self.usage
                .output_tokens
                .map_or(String::new(), |v| v.to_string()),
            self.source_offset,
        )
    }

    #[must_use]
    pub fn event_hash(&self) -> String {
        sha256_hex(self.canonical_identity().as_bytes())
    }
}

#[derive(Debug)]
pub struct DedupeResult {
    pub canonical: Vec<UsageObservation>,
    pub conflicts: usize,
}

#[must_use]
pub fn deduplicate(observations: Vec<UsageObservation>) -> DedupeResult {
    let mut by_identity: HashMap<String, UsageObservation> = HashMap::new();
    let mut conflicts = 0;
    for observation in observations {
        let identity = observation.canonical_identity();
        if let Some(current) = by_identity.get(&identity) {
            if current.usage != observation.usage {
                conflicts += 1;
            }
            let replace = observation.source.rank() > current.source.rank()
                || (observation.source.rank() == current.source.rank()
                    && observation.usage.is_measured()
                    && !current.usage.is_measured());
            if replace {
                by_identity.insert(identity, observation);
            }
        } else {
            by_identity.insert(identity, observation);
        }
    }
    DedupeResult {
        canonical: by_identity.into_values().collect(),
        conflicts,
    }
}

#[must_use]
pub fn token_completeness(measured: u64, unavailable: u64, anomalous: u64) -> Option<f64> {
    let denominator = measured + unavailable + anomalous;
    (denominator != 0).then(|| 100.0 * measured as f64 / denominator as f64)
}

#[derive(Clone, Debug)]
pub struct ExactRates<'a> {
    pub input_per_million: &'a str,
    pub cached_input_per_million: Option<&'a str>,
    pub cache_write_per_million: Option<&'a str>,
    pub output_per_million: &'a str,
    pub reasoning_per_million: Option<&'a str>,
}

pub fn calculate_cost_micros(
    usage: &TokenUsage,
    rates: &ExactRates<'_>,
) -> Result<Option<u64>, String> {
    let categories = [
        (usage.input_tokens, Some(rates.input_per_million)),
        (usage.cached_input_tokens, rates.cached_input_per_million),
        (usage.cache_write_tokens, rates.cache_write_per_million),
        (usage.output_tokens, Some(rates.output_per_million)),
        (usage.reasoning_tokens, rates.reasoning_per_million),
    ];
    let mut total = 0_u128;
    for (tokens, rate) in categories {
        let Some(tokens) = tokens else { continue };
        if tokens == 0 {
            continue;
        }
        let Some(rate) = rate else { return Ok(None) };
        let rate_micros = decimal_dollars_to_micros(rate)?;
        total = total
            .checked_add((u128::from(tokens) * rate_micros + 500_000) / 1_000_000)
            .ok_or_else(|| "cost overflow".to_owned())?;
    }
    u64::try_from(total)
        .map(Some)
        .map_err(|_| "cost overflow".to_owned())
}

fn decimal_dollars_to_micros(value: &str) -> Result<u128, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("invalid USD rate: {value}"));
    }
    let whole: u128 = whole
        .parse()
        .map_err(|_| format!("invalid USD rate: {value}"))?;
    let padded = format!("{fraction:0<6}");
    let fractional: u128 = padded
        .parse()
        .map_err(|_| format!("invalid USD rate: {value}"))?;
    Ok(whole * 1_000_000 + fractional)
}

#[must_use]
pub fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(source: MeasurementSource, input: u64) -> UsageObservation {
        UsageObservation {
            adapter: "claude".into(),
            source,
            source_subtype: None,
            source_event_id: None,
            provider_session_id: "s".into(),
            request_id: Some("r".into()),
            turn_id: None,
            agent_id: None,
            parent_agent_id: None,
            source_timestamp: None,
            observed_at: "2026-01-01T00:00:00Z".into(),
            model: Some("claude-sonnet-4-6".into()),
            service_tier: None,
            region: None,
            usage: TokenUsage {
                input_tokens: Some(input),
                output_tokens: Some(1),
                ..TokenUsage::default()
            },
            provider_reported_cost_micros: None,
            source_path: "fixture".into(),
            source_offset: 0,
            adapter_version: "test".into(),
            parser_version: "test".into(),
        }
    }

    #[test]
    fn official_telemetry_wins_deduplication() {
        let result = deduplicate(vec![
            observation(MeasurementSource::TranscriptRequest, 10),
            observation(MeasurementSource::OfficialTelemetry, 11),
        ]);
        assert_eq!(result.canonical.len(), 1);
        assert_eq!(result.canonical[0].usage.input_tokens, Some(11));
        assert_eq!(result.conflicts, 1);
    }

    #[test]
    fn completeness_is_null_without_requests() {
        assert_eq!(token_completeness(0, 0, 0), None);
        assert_eq!(token_completeness(8, 1, 1), Some(80.0));
    }

    #[test]
    fn cache_uses_its_own_exact_rate() {
        let usage = TokenUsage {
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(1_000_000),
            cache_write_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            reasoning_tokens: None,
        };
        let rates = ExactRates {
            input_per_million: "3.00",
            cached_input_per_million: Some("0.30"),
            cache_write_per_million: Some("3.75"),
            output_per_million: "15.00",
            reasoning_per_million: None,
        };
        assert_eq!(
            calculate_cost_micros(&usage, &rates).unwrap(),
            Some(22_050_000)
        );
    }
}
