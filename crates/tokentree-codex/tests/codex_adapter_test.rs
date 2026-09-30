// SPDX-License-Identifier: Apache-2.0
use std::path::Path;
use tokentree_codex::{discover_sessions, parse_session};
use tokentree_core::token_completeness;
use tokentree_ledger::Ledger;

#[test]
fn test_codex_parser_and_correlations() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_path =
        Path::new(manifest_dir).join("../../fixtures/parsers/codex/public-small-codex.jsonl");

    assert!(
        fixture_path.exists(),
        "fixture must exist at {}",
        fixture_path.display()
    );

    let result = parse_session(&fixture_path).expect("parse_session must succeed");

    // 1. Check parse stats
    assert!(result.stats.parsed >= 4, "must parse valid token events");
    assert_eq!(result.stats.malformed, 1, "malformed line must be counted");
    assert!(
        result.stats.anomalies >= 2,
        "must capture negative delta and malformed anomalies"
    );

    // 2. Correlation with hook boundaries:
    // req_cdx_001 had no explicit turn_id, so it should inherit active turn_cdx_001
    let req1 = result
        .observations
        .iter()
        .find(|o| o.request_id.as_deref() == Some("req_cdx_001"))
        .expect("req1 found");
    assert_eq!(req1.provider_session_id, "ses_cdx_test");
    assert_eq!(
        req1.turn_id.as_deref(),
        Some("turn_cdx_001"),
        "must correlate with active hook boundary turn_id"
    );
    assert_eq!(req1.usage.input_tokens, Some(150));
    assert_eq!(req1.usage.output_tokens, Some(50));
    assert_eq!(req1.usage.cached_input_tokens, Some(30));
    assert_eq!(req1.usage.reasoning_tokens, Some(20));

    // 3. Subagent activity preserved
    let sub = result
        .observations
        .iter()
        .find(|o| o.request_id.as_deref() == Some("req_cdx_sub_001"))
        .expect("subagent found");
    assert_eq!(sub.agent_id.as_deref(), Some("codex_child_agent"));
    assert_eq!(sub.parent_agent_id.as_deref(), Some("codex_main_agent"));
    assert_eq!(sub.turn_id.as_deref(), Some("turn_cdx_001"));

    // 4. Cumulative counter delta calculation
    let cum1 = result
        .observations
        .iter()
        .find(|o| o.request_id.as_deref() == Some("req_cdx_cum_001"))
        .expect("cum1 found");
    // prev cumulative for o3-mini was 150 input, 30 cached, 50 output, 20 reasoning
    // current cumulative was 250 input, 30 cached, 80 output, 30 reasoning
    // delta should be 100 input, 0 cached, 30 output, 10 reasoning
    assert_eq!(cum1.usage.input_tokens, Some(100));
    assert_eq!(cum1.usage.output_tokens, Some(30));
    assert_eq!(cum1.usage.reasoning_tokens, Some(10));

    // 5. Negative delta / counter reset detection
    let neg_anom = result
        .anomalies
        .iter()
        .find(|a| a.anomaly_type == "negative_delta")
        .expect("negative delta anomaly recorded");
    assert_eq!(neg_anom.session_id.as_deref(), Some("ses_cdx_test"));
    assert_eq!(neg_anom.turn_id.as_deref(), Some("turn_cdx_001"));

    // 6. Privacy verification:
    // Ensure no sensitive prompts or completions leaked into observations
    let priv_req = result
        .observations
        .iter()
        .find(|o| o.request_id.as_deref() == Some("req_cdx_priv_001"))
        .expect("privacy req found");
    assert_eq!(priv_req.usage.input_tokens, Some(25));
    // Serialize all observations to JSON and assert "SUPER_SECRET" never appears
    let json_dump = serde_json::to_string(&result.observations).unwrap();
    assert!(
        !json_dump.contains("SUPER_SECRET"),
        "raw prompt must NEVER appear in observations"
    );
    assert!(
        !json_dump.contains("SECRET_PASSWORD"),
        "raw completion must NEVER appear in observations"
    );
}

#[test]
fn test_codex_ingest_dedup_and_anomalies_in_ledger() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_path =
        Path::new(manifest_dir).join("../../fixtures/parsers/codex/public-small-codex.jsonl");

    let parsed = parse_session(&fixture_path).unwrap();
    let mut ledger = Ledger::open_memory().unwrap();

    // Record anomalies into ledger
    for anom in &parsed.anomalies {
        ledger
            .record_anomaly(
                anom.session_id.as_deref(),
                anom.turn_id.as_deref(),
                &anom.anomaly_type,
                &anom.details.to_string(),
            )
            .unwrap();
    }

    // Ingest observations
    let summary = ledger.ingest(parsed.observations).unwrap();
    assert!(summary.inserted > 0);

    // Verify deduplication against counters:
    // The turn counter event was included in parsed observations, but request-level events rank higher.
    let reconcile = ledger.reconcile().unwrap();
    assert_eq!(
        reconcile.duplicate_request_ids, 0,
        "canonical identities deduplicate cleanly"
    );
    assert!(
        reconcile.unresolved_anomalies >= 2,
        "unresolved anomalies must be reported in reconcile"
    );

    // Verify aggregate usage and completeness reduction by anomalies
    let usage = ledger.aggregate_usage().unwrap();
    assert!(
        usage.anomalous >= 2,
        "anomalous count must reflect unresolved anomalies"
    );

    let completeness = token_completeness(usage.measured, usage.unavailable, usage.anomalous);
    assert!(completeness.is_some());
    let comp_val = completeness.unwrap();
    // Because usage.anomalous > 0, completeness must be strictly less than 100%
    assert!(
        comp_val < 100.0,
        "anomalies must reduce completeness below 100%"
    );
}

#[test]
fn test_codex_discover_sessions() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_dir = Path::new(manifest_dir).join("../../fixtures/parsers/codex");
    let discovered = discover_sessions(&fixture_dir);
    assert!(
        !discovered.is_empty(),
        "must find jsonl files in codex directory"
    );
}
