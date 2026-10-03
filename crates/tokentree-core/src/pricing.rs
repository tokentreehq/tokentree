// SPDX-License-Identifier: Apache-2.0
use crate::{ExactRates, sha256_hex};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceRate {
    pub provider: String,
    pub model_pattern: String,
    pub effective_from: String,
    pub input_per_million: String,
    pub cached_input_per_million: Option<String>,
    pub cache_write_per_million: Option<String>,
    pub output_per_million: String,
    // L5: skip serializing when absent so pre-existing checksummed
    // snapshots (without this field) keep their hashes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_per_million: Option<String>,
    pub source: String,
}

impl PriceRate {
    #[must_use]
    pub fn exact_rates(&self) -> ExactRates<'_> {
        ExactRates {
            input_per_million: &self.input_per_million,
            cached_input_per_million: self.cached_input_per_million.as_deref(),
            cache_write_per_million: self.cache_write_per_million.as_deref(),
            output_per_million: &self.output_per_million,
            reasoning_per_million: self.reasoning_per_million.as_deref(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub version: u32,
    pub updated: String,
    pub currency: String,
    pub models: Vec<PriceRate>,
    pub sha256: String,
}

#[derive(Serialize)]
struct CanonicalSnapshotPayload<'a> {
    version: u32,
    updated: &'a str,
    currency: &'a str,
    models: &'a [PriceRate],
}

impl PriceSnapshot {
    pub fn load_from_str(content: &str) -> Result<Self> {
        let snapshot: Self = serde_json::from_str(content).context("parse price snapshot JSON")?;
        snapshot.verify_hash()?;
        Ok(snapshot)
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("read price snapshot at {}", path.display()))?;
        Self::load_from_str(&content)
    }

    pub fn verify_hash(&self) -> Result<()> {
        let payload = CanonicalSnapshotPayload {
            version: self.version,
            updated: &self.updated,
            currency: &self.currency,
            models: &self.models,
        };
        let canonical_json = serde_json::to_string(&payload)
            .context("serialize canonical price snapshot payload")?;
        let computed = sha256_hex(canonical_json.as_bytes());
        if computed != self.sha256 {
            bail!(
                "price snapshot hash mismatch: expected {}, got {}",
                self.sha256,
                computed
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn resolve_rate(&self, model: &str, timestamp: Option<&str>) -> Option<&PriceRate> {
        let mut candidates: Vec<(&PriceRate, bool, usize)> = self
            .models
            .iter()
            .filter_map(|rate| {
                // Track match specificity: exact matches beat wildcards, and
                // longer patterns beat shorter ones at the same tier.
                let (exact, specificity) =
                    if let Some(prefix) = rate.model_pattern.strip_suffix('*') {
                        if !model.starts_with(prefix) {
                            return None;
                        }
                        (false, prefix.len())
                    } else {
                        if rate.model_pattern != model {
                            return None;
                        }
                        (true, rate.model_pattern.len())
                    };
                if let Some(ts) = timestamp {
                    let ts_date = ts.split('T').next().unwrap_or(ts);
                    if !effective_on_or_before(&rate.effective_from, ts_date) {
                        return None;
                    }
                }
                Some((rate, exact, specificity))
            })
            .collect();

        // L3: the old tie-break sorted by effective_from alone, so an exact
        // pattern and a wildcard with the same date resolved arbitrarily
        // (stable sort = snapshot order). Prefer exact > longer wildcard >
        // latest effective date.
        candidates.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| compare_iso_dates(&b.0.effective_from, &a.0.effective_from))
        });
        candidates.into_iter().next().map(|(rate, _, _)| rate)
    }
}

/// Parse an ISO date (allowing non-padded components like `2026-2-1`) into
/// a numerically comparable tuple.
fn parse_iso_date(s: &str) -> Option<(i32, u32, u32)> {
    let date_part = s.split('T').next().unwrap_or(s);
    let mut parts = date_part.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// L3: plain string comparison breaks on non-padded dates
/// (`"2026-2-1" > "2026-02-01"` lexicographically), so compare parsed
/// components; fall back to string order only for unparseable input.
fn effective_on_or_before(effective_from: &str, ts_date: &str) -> bool {
    match (parse_iso_date(effective_from), parse_iso_date(ts_date)) {
        (Some(a), Some(b)) => a <= b,
        _ => effective_from <= ts_date,
    }
}

fn compare_iso_dates(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_iso_date(a), parse_iso_date(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_JSON: &str = r#"{
  "version": 1,
  "updated": "2026-09-29",
  "currency": "USD",
  "models": [
    {
      "provider": "anthropic",
      "modelPattern": "claude-sonnet-4-6",
      "effectiveFrom": "2026-02-01",
      "inputPerMillion": "3.00",
      "cachedInputPerMillion": "0.30",
      "cacheWritePerMillion": "3.75",
      "outputPerMillion": "15.00",
      "source": "https://platform.claude.com/docs/en/models/sonnet-4-6/overview"
    }
  ],
  "sha256": "285fcc5fa2ddebe65ac8ca43bb4070ee73e88a2e481e982bbd73989f6d4bb777"
}"#;

    #[test]
    fn loads_and_verifies_valid_snapshot() {
        let snapshot = PriceSnapshot::load_from_str(FIXTURE_JSON).unwrap();
        assert_eq!(snapshot.models.len(), 1);
        let rate = snapshot
            .resolve_rate("claude-sonnet-4-6", Some("2026-09-29T12:00:00Z"))
            .unwrap();
        assert_eq!(rate.input_per_million, "3.00");
        assert_eq!(rate.cached_input_per_million.as_deref(), Some("0.30"));
        assert_eq!(rate.cache_write_per_million.as_deref(), Some("3.75"));
        assert_eq!(rate.output_per_million, "15.00");
    }

    #[test]
    fn rejects_tampered_snapshot() {
        let tampered = FIXTURE_JSON.replace("2026-09-29", "2026-09-30");
        let err = PriceSnapshot::load_from_str(&tampered).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"));
    }

    #[test]
    fn unknown_model_is_unresolved() {
        let snapshot = PriceSnapshot::load_from_str(FIXTURE_JSON).unwrap();
        assert!(snapshot.resolve_rate("unknown-model", None).is_none());
    }

    fn rate(pattern: &str, effective_from: &str, input: &str) -> PriceRate {
        PriceRate {
            provider: "test".to_string(),
            model_pattern: pattern.to_string(),
            effective_from: effective_from.to_string(),
            input_per_million: input.to_string(),
            cached_input_per_million: None,
            cache_write_per_million: None,
            output_per_million: "1.00".to_string(),
            reasoning_per_million: None,
            source: "test".to_string(),
        }
    }

    fn snapshot_with(rates: Vec<PriceRate>) -> PriceSnapshot {
        PriceSnapshot {
            version: 1,
            updated: "2026-09-29".to_string(),
            currency: "USD".to_string(),
            models: rates,
            sha256: String::new(),
        }
    }

    /// L3: non-padded `effective_from` dates must compare numerically, not
    /// lexicographically (`"2026-2-1"` sorts after `"2026-02-01"` as a
    /// string, which would wrongly exclude the rate).
    #[test]
    fn non_padded_effective_from_matches() {
        let snapshot = snapshot_with(vec![rate("model-x", "2026-2-1", "3.00")]);
        let found = snapshot
            .resolve_rate("model-x", Some("2026-02-15T00:00:00Z"))
            .unwrap();
        assert_eq!(found.input_per_million, "3.00");
        // And a timestamp before the effective date still excludes it.
        assert!(
            snapshot
                .resolve_rate("model-x", Some("2026-01-15T00:00:00Z"))
                .is_none()
        );
    }

    /// L3: an exact pattern beats a wildcard with the same effective date.
    #[test]
    fn exact_pattern_beats_wildcard_on_date_tie() {
        let snapshot = snapshot_with(vec![
            rate("model-*", "2026-01-01", "9.99"),
            rate("model-x", "2026-01-01", "3.00"),
        ]);
        let found = snapshot.resolve_rate("model-x", None).unwrap();
        assert_eq!(found.input_per_million, "3.00");
    }

    /// L3: a longer (more specific) wildcard beats a shorter one.
    #[test]
    fn longer_wildcard_beats_shorter_on_tie() {
        let snapshot = snapshot_with(vec![
            rate("model-*", "2026-01-01", "9.99"),
            rate("model-x*", "2026-01-01", "3.00"),
        ]);
        let found = snapshot.resolve_rate("model-xyz", None).unwrap();
        assert_eq!(found.input_per_million, "3.00");
    }

    /// L3: latest effective date still wins among equally specific patterns.
    #[test]
    fn latest_effective_date_wins_among_equals() {
        let snapshot = snapshot_with(vec![
            rate("model-x", "2026-01-01", "9.99"),
            rate("model-x", "2026-06-01", "3.00"),
        ]);
        let found = snapshot
            .resolve_rate("model-x", Some("2026-09-01T00:00:00Z"))
            .unwrap();
        assert_eq!(found.input_per_million, "3.00");
    }
}
