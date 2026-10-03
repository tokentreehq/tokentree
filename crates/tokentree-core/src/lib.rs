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
            Self::Unavailable => source_kind::UNAVAILABLE,
            Self::ExplicitCli => source_kind::EXPLICIT_CLI,
            Self::SnapshotDelta => source_kind::SNAPSHOT_DELTA,
            Self::TranscriptRequest => source_kind::TRANSCRIPT_REQUEST,
            Self::ProviderFields => source_kind::PROVIDER_FIELDS,
            Self::OfficialTelemetry => source_kind::OFFICIAL_TELEMETRY,
        }
    }
}

/// Version of the [`UsageObservation::canonical_identity`] scheme.
/// - v1: the request branch was `{adapter}:request:{request_id}` (the request
///   id alone — not unique across sessions).
/// - v2 (current): the request branch is
///   `{adapter}:request:{provider_session_id}:{turn_id}:{request_id}`.
///
/// Bumped by the H6 fix. Ingest performs dedup lookups against *both* schemes
/// so that re-importing files ingested under v1 neither duplicates rows nor
/// drifts accounting. See [`UsageObservation::canonical_identity_v1`].
pub const IDENTITY_SCHEME_VERSION: u32 = 2;

/// Truth-ladder rank for a stored `usage_events.source_kind` value.
///
/// Mirrors [`MeasurementSource::rank`] for the six canonical kinds. Adapter
/// subtype markers are mapped to the rank of the ladder rung they sit on.
/// Values outside the known vocabulary return `u8::MAX`: a binary must never
/// supersede a row whose provenance it does not understand (forward
/// compatibility with future vocabulary).
#[must_use]
pub fn rank_of_source_kind(kind: &str) -> u8 {
    match kind {
        k if k == source_kind::UNAVAILABLE => 0,
        k if k == source_kind::EXPLICIT_CLI || k == source_kind::MANUAL_STOP => 1,
        k if k == source_kind::SNAPSHOT_DELTA
            || k == source_kind::HERMES_SNAPSHOT_DELTA
            || k == source_kind::CODEX_TURN_COUNTER
            || k == source_kind::TURN_COUNTER
            || k == source_kind::TURN_SUMMARY
            || k == source_kind::CUMULATIVE_TURN_COUNTER =>
        {
            2
        }
        k if k == source_kind::TRANSCRIPT_REQUEST
            || k == source_kind::PROTOTYPE_JSON
            || k == source_kind::FINAL_REQUEST_COUNTER
            || k == source_kind::SUBAGENT_STOP
            || k == source_kind::SUBAGENT_LIFECYCLE_COUNTER =>
        {
            3
        }
        k if k == source_kind::PROVIDER_FIELDS => 4,
        k if k == source_kind::OFFICIAL_TELEMETRY => 5,
        k if k == source_kind::GROK_TURN_FAILED
            || k == source_kind::GROK_SESSION_FAILED
            || k == source_kind::GROK_TURN_UNMEASURED
            || k == source_kind::GROK_SESSION_UNMEASURED
            || k == source_kind::HERMES_FAILED_RUN
            || k == source_kind::HERMES_UNMEASURED =>
        {
            0
        }
        _ if kind.starts_with(source_kind::HERMES_AUXILIARY_PREFIX)
            || kind.starts_with(source_kind::HERMES_USAGE_PREFIX) =>
        {
            4
        }
        _ => u8::MAX,
    }
}

/// Canonical vocabulary for the `source_kind` column of `usage_events`.
///
/// CROSS-LANGUAGE CONTRACT: these exact strings are the only values the Rust
/// engine writes to `usage_events.source_kind`, and the TypeScript side
/// mirrors this vocabulary verbatim. A database touched by both binaries must
/// aggregate identically, so parsers must never write raw provider-internal
/// record types (e.g. Claude transcript `"type"` values like `"assistant"`)
/// into this column — use the canonical [`MeasurementSource`] string instead.
///
/// The vocabulary has two tiers:
/// * Canonical measurement sources (the [`MeasurementSource::as_str`] set).
/// * Derived/lifecycle kinds produced by adapters and the ledger itself
///   (turn counters, lifecycle counters, per-adapter failure/unmeasured
///   markers). Aggregation SQL matches on these, so they are closed too.
///
/// Provider-detail subtypes that carry an open-ended suffix
/// (`hermes_auxiliary_{task}`, `hermes_usage_{provider}`) are recognized by
/// their prefixes below; everything else not listed here is rejected by
/// [`canonical_source_kind`] and falls back to the canonical source string.
pub mod source_kind {
    // Canonical measurement sources (see MeasurementSource::as_str).
    pub const UNAVAILABLE: &str = "unavailable";
    pub const EXPLICIT_CLI: &str = "explicit_cli";
    pub const SNAPSHOT_DELTA: &str = "snapshot_delta";
    pub const TRANSCRIPT_REQUEST: &str = "transcript_request";
    pub const PROVIDER_FIELDS: &str = "provider_fields";
    pub const OFFICIAL_TELEMETRY: &str = "official_telemetry";

    // Lifecycle-counter kinds: rollup/counter rows, never standalone requests.
    pub const FINAL_REQUEST_COUNTER: &str = "final_request_counter";
    pub const SUBAGENT_STOP: &str = "subagent_stop";
    pub const SUBAGENT_LIFECYCLE_COUNTER: &str = "subagent_lifecycle_counter";

    // Turn/cumulative counter fallbacks (covered-counter suppression matches these).
    pub const CODEX_TURN_COUNTER: &str = "codex_turn_counter";
    pub const TURN_COUNTER: &str = "turn_counter";
    pub const TURN_SUMMARY: &str = "turn_summary";
    pub const CUMULATIVE_TURN_COUNTER: &str = "cumulative_turn_counter";

    // Codex app-server / rollout detail kinds.
    pub const CODEX_APP_SERVER: &str = "codex_app_server";
    pub const CODEX_ROLLOUT: &str = "codex_rollout";

    // Hermes snapshot deltas (distinct from generic snapshot_delta for adapter scoping).
    pub const HERMES_SNAPSHOT_DELTA: &str = "hermes_snapshot_delta";

    // Per-adapter failure / unmeasured markers.
    pub const GROK_TURN_FAILED: &str = "grok_turn_failed";
    pub const GROK_SESSION_FAILED: &str = "grok_session_failed";
    pub const GROK_TURN_UNMEASURED: &str = "grok_turn_unmeasured";
    pub const GROK_SESSION_UNMEASURED: &str = "grok_session_unmeasured";
    pub const GROK_TURN_USAGE: &str = "grok_turn_usage";
    pub const GROK_SESSION_USAGE: &str = "grok_session_usage";
    pub const HERMES_FAILED_RUN: &str = "hermes_failed_run";
    pub const HERMES_UNMEASURED: &str = "hermes_unmeasured";
    pub const HERMES_ONESHOT_USAGE: &str = "hermes_oneshot_usage";
    pub const HERMES_SESSION_MODEL_USAGE: &str = "hermes_session_model_usage";

    // Ledger-originated kinds.
    pub const MANUAL_STOP: &str = "manual_stop";
    pub const PROTOTYPE_JSON: &str = "prototype_json";
    pub const OTEL_API_REQUEST: &str = "otel_api_request";

    /// Prefix for open-ended Hermes auxiliary-task subtypes (`hermes_auxiliary_{task}`).
    pub const HERMES_AUXILIARY_PREFIX: &str = "hermes_auxiliary_";
    /// Prefix for open-ended Hermes per-provider usage subtypes (`hermes_usage_{provider}`).
    pub const HERMES_USAGE_PREFIX: &str = "hermes_usage_";

    /// SQL `IN (...)` list fragment for the lifecycle-counter exclusion.
    /// Mirrors [`FINAL_REQUEST_COUNTER`], [`SUBAGENT_STOP`], [`SUBAGENT_LIFECYCLE_COUNTER`];
    /// keep in sync if those change.
    pub const LIFECYCLE_COUNTER_SQL_LIST: &str =
        "'final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter'";

    /// SQL `IN (...)` list fragment for turn-counter kinds.
    pub const TURN_COUNTER_SQL_LIST: &str =
        "'codex_turn_counter', 'turn_counter', 'turn_summary', 'cumulative_turn_counter'";

    /// SQL `IN (...)` list fragment for snapshot-delta kinds excluded from
    /// request counts (deltas are not standalone requests).
    pub const SNAPSHOT_DELTA_SQL_LIST: &str = "'hermes_snapshot_delta', 'snapshot_delta'";

    /// Returns true when `value` is a recognized `source_kind` vocabulary value,
    /// including the open-ended Hermes provider-detail prefixes.
    #[must_use]
    pub fn is_recognized(value: &str) -> bool {
        matches!(
            value,
            UNAVAILABLE
                | EXPLICIT_CLI
                | SNAPSHOT_DELTA
                | TRANSCRIPT_REQUEST
                | PROVIDER_FIELDS
                | OFFICIAL_TELEMETRY
                | FINAL_REQUEST_COUNTER
                | SUBAGENT_STOP
                | SUBAGENT_LIFECYCLE_COUNTER
                | CODEX_TURN_COUNTER
                | TURN_COUNTER
                | TURN_SUMMARY
                | CUMULATIVE_TURN_COUNTER
                | CODEX_APP_SERVER
                | CODEX_ROLLOUT
                | HERMES_SNAPSHOT_DELTA
                | GROK_TURN_FAILED
                | GROK_SESSION_FAILED
                | GROK_TURN_UNMEASURED
                | GROK_SESSION_UNMEASURED
                | GROK_TURN_USAGE
                | GROK_SESSION_USAGE
                | HERMES_FAILED_RUN
                | HERMES_UNMEASURED
                | HERMES_ONESHOT_USAGE
                | HERMES_SESSION_MODEL_USAGE
                | MANUAL_STOP
                | PROTOTYPE_JSON
                | OTEL_API_REQUEST
        ) || value.starts_with(HERMES_AUXILIARY_PREFIX)
            || value.starts_with(HERMES_USAGE_PREFIX)
    }
}

/// Resolve the value to store in `usage_events.source_kind` for an observation.
///
/// Prefers the parser-provided `source_subtype` when it is a recognized
/// vocabulary value; otherwise falls back to the canonical
/// [`MeasurementSource`] string. This is the choke point that keeps Rust and
/// TypeScript writes byte-identical for the same observation: raw
/// provider-internal record types (e.g. an old Claude parser writing the
/// transcript `"type"` value `"assistant"`) can never reach the column.
#[must_use]
pub fn canonical_source_kind(observation: &UsageObservation) -> &str {
    match observation.source_subtype.as_deref() {
        Some(subtype) if source_kind::is_recognized(subtype) => subtype,
        _ => observation.source.as_str(),
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
    /// Stable identity used for cross-source deduplication and as the basis of
    /// [`UsageObservation::event_hash`]. Format by branch (fields joined by `:`):
    /// - snapshot delta with event id: `{adapter}:delta:{source_event_id}`
    /// - request: `{adapter}:request:{provider_session_id}:{turn_id}:{request_id}`
    /// - bare event id: `{adapter}:event:{source_event_id}`
    /// - turn counter: `{adapter}:counter:{provider_session_id}:{turn_id}`
    /// - fallback: `{adapter}:{provider_session_id}:{turn_id}:{model}:{source_timestamp}:{input_tokens}:{output_tokens}:{source_offset}`
    ///
    /// The request branch deliberately includes the session (and turn): provider
    /// request ids are only unique within a session, so omitting it silently
    /// drops usage when ids are reused across sessions.
    ///
    /// This is identity scheme v2; see [`IDENTITY_SCHEME_VERSION`] and
    /// [`UsageObservation::canonical_identity_v1`] for the v1 format still
    /// honored by ingest dedup.
    #[must_use]
    pub fn canonical_identity(&self) -> String {
        self.canonical_identity_inner(false)
    }

    /// The pre-H6 (scheme v1) identity, for backward-compatible dedup lookups.
    /// Only the request branch differs from [`UsageObservation::canonical_identity`];
    /// every other branch is byte-identical across schemes. See
    /// [`IDENTITY_SCHEME_VERSION`].
    #[must_use]
    pub fn canonical_identity_v1(&self) -> String {
        self.canonical_identity_inner(true)
    }

    fn canonical_identity_inner(&self, legacy_request_branch: bool) -> String {
        if self.source == MeasurementSource::SnapshotDelta {
            if let Some(event) = &self.source_event_id {
                return format!("{}:delta:{event}", self.adapter);
            }
        }
        if let Some(request) = &self.request_id {
            if legacy_request_branch {
                return format!("{}:request:{request}", self.adapter);
            }
            return format!(
                "{}:request:{}:{}:{request}",
                self.adapter,
                self.provider_session_id,
                self.turn_id.as_deref().unwrap_or_default(),
            );
        }
        if let Some(event) = &self.source_event_id {
            return format!("{}:event:{event}", self.adapter);
        }
        if (self.source_subtype.as_deref() == Some(source_kind::CODEX_TURN_COUNTER)
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

/// Formats an exact micro-dollar amount as a standard USD string with the specified
/// number of decimal places (e.g. 2 for cents `1.23`, 4 for sub-cents `1.2345`),
/// using pure integer division and rounding (no floating-point cast).
#[must_use]
pub fn format_micros_to_dollars(micros: u64, decimals: usize) -> String {
    if decimals == 0 {
        let rounded = (micros + 500_000) / 1_000_000;
        return format!("{rounded}");
    }
    let decimals = decimals.min(6);
    let divisor = 10u64.pow(6 - decimals as u32);
    let half = divisor / 2;
    let rounded = (micros + half) / divisor;
    let scale = 10u64.pow(decimals as u32);
    let whole = rounded / scale;
    let frac = rounded % scale;
    format!("{whole}.{frac:0width$}", width = decimals)
}

#[must_use]
pub fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

/// Canonical stable-ID derivation: `{prefix}_{sha256(value)[..24 hex]}`.
/// Lives in core (not the ledger) so parser crates — which must not depend
/// on the SQLite owner — can derive the exact IDs ingest will write.
#[must_use]
pub fn stable_id(prefix: &str, value: &str) -> String {
    format!("{prefix}_{}", &sha256_hex(value.as_bytes())[..24])
}

/// Canonical session-ID derivation shared by ingest and the CLI import paths.
/// Keep in exactly one place so attribution lookups can never drift from ingest.
#[must_use]
pub fn session_stable_id(adapter: &str, provider_session_id: &str) -> String {
    stable_id("ses", &format!("{adapter}:{provider_session_id}"))
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
    fn request_identity_includes_session_and_turn() {
        // H6: request ids are only unique within a session; the identity must
        // not collide across sessions.
        let mut first = observation(MeasurementSource::TranscriptRequest, 10);
        first.provider_session_id = "s1".into();
        first.turn_id = Some("t1".into());
        let mut second = observation(MeasurementSource::TranscriptRequest, 10);
        second.provider_session_id = "s2".into();
        second.turn_id = Some("t1".into());
        assert_eq!(first.canonical_identity(), "claude:request:s1:t1:r");
        assert_ne!(first.canonical_identity(), second.canonical_identity());

        // Same session + turn + request still deduplicates across sources,
        // with the higher-ranked source winning.
        let mut authoritative = observation(MeasurementSource::OfficialTelemetry, 11);
        authoritative.provider_session_id = "s1".into();
        authoritative.turn_id = Some("t1".into());
        let result = deduplicate(vec![first, authoritative]);
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

    #[test]
    fn source_kind_vocabulary_matches_measurement_source_strings() {
        // The canonical vocabulary is the cross-language contract: as_str()
        // must stay byte-identical to these constants.
        assert_eq!(
            MeasurementSource::TranscriptRequest.as_str(),
            source_kind::TRANSCRIPT_REQUEST
        );
        assert_eq!(
            MeasurementSource::SnapshotDelta.as_str(),
            source_kind::SNAPSHOT_DELTA
        );
        assert_eq!(
            MeasurementSource::OfficialTelemetry.as_str(),
            source_kind::OFFICIAL_TELEMETRY
        );
        assert_eq!(
            MeasurementSource::ProviderFields.as_str(),
            source_kind::PROVIDER_FIELDS
        );
        assert_eq!(
            MeasurementSource::ExplicitCli.as_str(),
            source_kind::EXPLICIT_CLI
        );
        assert_eq!(
            MeasurementSource::Unavailable.as_str(),
            source_kind::UNAVAILABLE
        );
    }

    #[test]
    fn canonical_source_kind_prefers_recognized_subtype() {
        let mut obs = observation(MeasurementSource::TranscriptRequest, 10);
        obs.source_subtype = Some(source_kind::CODEX_TURN_COUNTER.to_string());
        assert_eq!(canonical_source_kind(&obs), source_kind::CODEX_TURN_COUNTER);

        // Open-ended Hermes provider-detail subtypes pass through.
        obs.source_subtype = Some("hermes_auxiliary_openrouter".to_string());
        assert_eq!(canonical_source_kind(&obs), "hermes_auxiliary_openrouter");
    }

    #[test]
    fn canonical_source_kind_rejects_raw_record_types() {
        // Old Rust Claude parsers wrote the raw transcript record "type"
        // (e.g. "assistant") into source_kind; the choke point must map those
        // to the canonical source string so Rust and TS writes agree.
        let mut obs = observation(MeasurementSource::TranscriptRequest, 10);
        obs.source_subtype = Some("assistant".to_string());
        assert_eq!(canonical_source_kind(&obs), source_kind::TRANSCRIPT_REQUEST);

        obs.source = MeasurementSource::OfficialTelemetry;
        obs.source_subtype = Some("otel_api_request".to_string());
        // otel_api_request IS a recognized ledger-originated kind, kept as-is.
        assert_eq!(canonical_source_kind(&obs), source_kind::OTEL_API_REQUEST);

        obs.source_subtype = None;
        assert_eq!(canonical_source_kind(&obs), source_kind::OFFICIAL_TELEMETRY);
    }

    #[test]
    fn source_kind_sql_lists_cover_their_constants() {
        for kind in [
            source_kind::FINAL_REQUEST_COUNTER,
            source_kind::SUBAGENT_STOP,
            source_kind::SUBAGENT_LIFECYCLE_COUNTER,
        ] {
            assert!(
                source_kind::LIFECYCLE_COUNTER_SQL_LIST.contains(kind),
                "LIFECYCLE_COUNTER_SQL_LIST missing {kind}"
            );
        }
        for kind in [
            source_kind::CODEX_TURN_COUNTER,
            source_kind::TURN_COUNTER,
            source_kind::TURN_SUMMARY,
            source_kind::CUMULATIVE_TURN_COUNTER,
        ] {
            assert!(
                source_kind::TURN_COUNTER_SQL_LIST.contains(kind),
                "TURN_COUNTER_SQL_LIST missing {kind}"
            );
        }
    }

    #[test]
    fn identity_scheme_v1_matches_legacy_format() {
        assert_eq!(IDENTITY_SCHEME_VERSION, 2);
        let mut obs = observation(MeasurementSource::TranscriptRequest, 10);
        obs.provider_session_id = "s1".into();
        obs.turn_id = Some("t1".into());
        // v1: request id alone, no session/turn scoping.
        assert_eq!(obs.canonical_identity_v1(), "claude:request:r");
        // v2: session- and turn-scoped.
        assert_eq!(obs.canonical_identity(), "claude:request:s1:t1:r");
        assert_ne!(obs.canonical_identity(), obs.canonical_identity_v1());
    }

    #[test]
    fn identity_v1_non_request_branches_are_scheme_independent() {
        // Only the request branch changed between schemes; every other branch
        // must be byte-identical so dual-scheme lookups stay sound.
        let mut delta = observation(MeasurementSource::SnapshotDelta, 10);
        delta.source_event_id = Some("evt-1".into());
        assert_eq!(delta.canonical_identity_v1(), delta.canonical_identity());

        let mut bare = observation(MeasurementSource::TranscriptRequest, 10);
        bare.request_id = None;
        bare.source_event_id = Some("evt-2".into());
        assert_eq!(bare.canonical_identity_v1(), bare.canonical_identity());
    }

    #[test]
    fn rank_of_source_kind_maps_the_ladder() {
        assert_eq!(rank_of_source_kind(source_kind::UNAVAILABLE), 0);
        assert_eq!(rank_of_source_kind(source_kind::EXPLICIT_CLI), 1);
        assert_eq!(rank_of_source_kind(source_kind::SNAPSHOT_DELTA), 2);
        assert_eq!(rank_of_source_kind(source_kind::TRANSCRIPT_REQUEST), 3);
        assert_eq!(rank_of_source_kind(source_kind::PROVIDER_FIELDS), 4);
        assert_eq!(rank_of_source_kind(source_kind::OFFICIAL_TELEMETRY), 5);
        // Cross-check against the enum ranks.
        for source in [
            MeasurementSource::Unavailable,
            MeasurementSource::ExplicitCli,
            MeasurementSource::SnapshotDelta,
            MeasurementSource::TranscriptRequest,
            MeasurementSource::ProviderFields,
            MeasurementSource::OfficialTelemetry,
        ] {
            assert_eq!(rank_of_source_kind(source.as_str()), source.rank());
        }
    }

    #[test]
    fn rank_of_source_kind_is_conservative_on_unknowns() {
        // Unknown / future vocabulary must never be superseded by a binary
        // that does not understand it.
        assert_eq!(rank_of_source_kind("banana"), u8::MAX);
        assert_eq!(rank_of_source_kind(""), u8::MAX);
        assert_eq!(rank_of_source_kind("transcript_request_v2"), u8::MAX);
        // Failure markers sit at the bottom of the ladder.
        assert_eq!(rank_of_source_kind(source_kind::GROK_TURN_FAILED), 0);
        assert_eq!(rank_of_source_kind(source_kind::HERMES_UNMEASURED), 0);
    }

    #[test]
    fn format_micros_to_dollars_exact_integer_arithmetic() {
        assert_eq!(format_micros_to_dollars(0, 2), "0.00");
        assert_eq!(format_micros_to_dollars(1_000_000, 2), "1.00");
        assert_eq!(format_micros_to_dollars(22_050_000, 2), "22.05");
        assert_eq!(format_micros_to_dollars(1_234_567, 2), "1.23");
        assert_eq!(format_micros_to_dollars(1_235_000, 2), "1.24");
        assert_eq!(format_micros_to_dollars(1_234_567, 4), "1.2346");
        assert_eq!(format_micros_to_dollars(12_345, 4), "0.0123");
        assert_eq!(format_micros_to_dollars(99_999, 2), "0.10");
        assert_eq!(format_micros_to_dollars(1_000_000, 0), "1");
    }
}
