// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::json;
use tokentree_core::{PriceSnapshot, TokenUsage, calculate_cost_micros};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct PricingSummary {
    pub priced: u64,
    pub unavailable: u64,
    pub already_calculated: u64,
}

pub fn apply_price_snapshot(
    connection: &mut Connection,
    snapshot: &PriceSnapshot,
) -> Result<PricingSummary> {
    let mut summary = PricingSummary::default();
    let transaction = connection.transaction()?;

    struct EventRow {
        id: String,
        model: Option<String>,
        timestamp: Option<String>,
        input_tokens: Option<i64>,
        cached_input_tokens: Option<i64>,
        cache_write_tokens: Option<i64>,
        output_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
    }

    let mut stmt = transaction.prepare(
        "SELECT id, model, coalesce(source_timestamp, observed_at),
                input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens
         FROM usage_events
         WHERE superseded_by IS NULL",
    )?;

    let event_rows = stmt
        .query_map([], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                model: row.get(1)?,
                timestamp: row.get(2)?,
                input_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                cache_write_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                reasoning_tokens: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    for event in event_rows {
        let Some(model) = &event.model else {
            summary.unavailable += 1;
            continue;
        };

        let rate = snapshot.resolve_rate(model, event.timestamp.as_deref());
        let Some(rate) = rate else {
            summary.unavailable += 1;
            continue;
        };

        let price_id = stable_id(
            "price",
            &format!(
                "{}:{}:{}:{}",
                rate.provider, rate.model_pattern, rate.effective_from, snapshot.sha256
            ),
        );

        let rate_json = serde_json::to_string(rate).context("serialize rate JSON")?;
        transaction.execute(
            "INSERT OR IGNORE INTO pricing_versions(
                id, provider, model_pattern, effective_from, rates_json, source, retrieved_at, signature_or_hash
            ) VALUES(?,?,?,?,?,?,?,?)",
            params![
                price_id,
                rate.provider,
                rate.model_pattern,
                rate.effective_from,
                rate_json,
                rate.source,
                snapshot.updated,
                snapshot.sha256,
            ],
        )?;

        let usage = TokenUsage {
            input_tokens: event.input_tokens.map(|v| v as u64),
            cached_input_tokens: event.cached_input_tokens.map(|v| v as u64),
            cache_write_tokens: event.cache_write_tokens.map(|v| v as u64),
            output_tokens: event.output_tokens.map(|v| v as u64),
            reasoning_tokens: event.reasoning_tokens.map(|v| v as u64),
        };

        let cost_res = calculate_cost_micros(&usage, &rate.exact_rates())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let Some(amount_micros) = cost_res else {
            summary.unavailable += 1;
            continue;
        };

        let now = Utc::now().to_rfc3339();
        let coverage = json!({
            "model": true,
            "cache": true,
            "source": snapshot.sha256,
        })
        .to_string();

        let changed = transaction.execute(
            "INSERT OR IGNORE INTO cost_calculations(
                usage_event_id, pricing_version_id, amount_micros, currency, cost_type, attribution_policy, coverage_json, calculated_at
            ) VALUES(?,?,?,'USD','api_equivalent_estimate','causal-request',?,?)",
            params![
                event.id,
                price_id,
                i64::try_from(amount_micros)
                    .context("cost amount exceeds SQLite integer range")?,
                coverage,
                now,
            ],
        )?;

        if changed == 1 {
            summary.priced += 1;
        } else {
            summary.already_calculated += 1;
        }
    }

    transaction.commit()?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    use tokentree_core::{MeasurementSource, UsageObservation};

    #[test]
    fn prices_ledger_events() {
        let mut ledger = Ledger::open_memory().unwrap();
        ledger
            .ingest(vec![UsageObservation {
                adapter: "claude".into(),
                source: MeasurementSource::OfficialTelemetry,
                source_subtype: None,
                source_event_id: None,
                provider_session_id: "s1".into(),
                request_id: Some("req1".into()),
                turn_id: None,
                agent_id: None,
                parent_agent_id: None,
                source_timestamp: Some("2026-09-29T12:00:00Z".into()),
                observed_at: "2026-09-29T12:00:00Z".into(),
                model: Some("claude-sonnet-4-6".into()),
                service_tier: None,
                region: None,
                usage: TokenUsage {
                    input_tokens: Some(1_000_000),
                    cached_input_tokens: Some(1_000_000),
                    cache_write_tokens: Some(1_000_000),
                    output_tokens: Some(1_000_000),
                    reasoning_tokens: None,
                },
                provider_reported_cost_micros: None,
                source_path: "test".into(),
                source_offset: 0,
                adapter_version: "test".into(),
                parser_version: "test".into(),
            }])
            .unwrap();

        const PRICES: &str = r#"{
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

        let snapshot = PriceSnapshot::load_from_str(PRICES).unwrap();
        let summary = ledger.apply_price_snapshot(&snapshot).unwrap();
        assert_eq!(summary.priced, 1);
        assert_eq!(summary.unavailable, 0);

        let amount: i64 = ledger
            .connection()
            .query_row(
                "SELECT amount_micros FROM cost_calculations LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(amount, 22_050_000);
    }
}
