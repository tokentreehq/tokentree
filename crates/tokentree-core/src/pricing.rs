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
            reasoning_per_million: None,
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
        let mut candidates: Vec<&PriceRate> = self
            .models
            .iter()
            .filter(|rate| {
                let matches = if let Some(prefix) = rate.model_pattern.strip_suffix('*') {
                    model.starts_with(prefix)
                } else {
                    rate.model_pattern == model
                };
                if !matches {
                    return false;
                }
                if let Some(ts) = timestamp {
                    let ts_date = ts.split('T').next().unwrap_or(ts);
                    rate.effective_from.as_str() <= ts_date
                } else {
                    true
                }
            })
            .collect();

        candidates.sort_by(|a, b| b.effective_from.cmp(&a.effective_from));
        candidates.first().copied()
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
}
