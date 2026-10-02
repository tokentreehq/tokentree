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

#[test]
fn test_turn_end_variants_clear_active_turn() {
    // C1: "turn/complete" (via method/event key) and "turn_end" (via type key)
    // are real Codex record names (see fixtures README) and must clear the
    // active turn so later events are not misattributed to a dead turn.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("turn_variants.jsonl");
    let content = concat!(
        "{\"type\":\"turn/started\",\"turn_id\":\"t1\",\"session_id\":\"s1\"}\n",
        "{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"threadId\":\"s1\",\"turnId\":\"t1\",\"tokenUsage\":{\"inputTokens\":10,\"outputTokens\":5}}}\n",
        "{\"method\":\"turn/complete\",\"params\":{\"threadId\":\"s1\",\"turnId\":\"t1\"}}\n",
        "{\"type\":\"turn/started\",\"turn_id\":\"t2\",\"session_id\":\"s1\"}\n",
        "{\"type\":\"turn_end\",\"turn_id\":\"t2\",\"session_id\":\"s1\"}\n",
        // No turn_id and no live turn: must NOT inherit t1 or t2.
        "{\"type\":\"thread/tokenUsage/updated\",\"session_id\":\"s1\",\"token_usage\":{\"input_tokens\":7,\"output_tokens\":3}}\n",
    );
    std::fs::write(&path, content).unwrap();

    let result = parse_session(&path).expect("parse must succeed");
    let orphan = result
        .observations
        .iter()
        .find(|o| o.usage.input_tokens == Some(7))
        .expect("orphan event must parse");
    assert_eq!(
        orphan.turn_id, None,
        "events after turn/complete and turn_end must not inherit the dead turn"
    );
    assert_eq!(result.stats.malformed, 0);
}

#[test]
fn test_cumulative_counters_are_scoped_to_session_and_turn() {
    // H5: counters keyed by {model}:{agent} alone let one session's cumulative
    // state contaminate another's. Keys must include session and turn.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("scoped_counters.jsonl");
    let content = concat!(
        "{\"type\":\"thread/tokenUsage/updated\",\"is_cumulative\":true,\"request_id\":\"a1\",\"session_id\":\"sA\",\"turn_id\":\"tA\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":100,\"output_tokens\":40}}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"is_cumulative\":true,\"request_id\":\"b1\",\"session_id\":\"sB\",\"turn_id\":\"tB\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":100,\"output_tokens\":40}}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"is_cumulative\":true,\"request_id\":\"a2\",\"session_id\":\"sA\",\"turn_id\":\"tA\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":150,\"output_tokens\":60}}\n",
    );
    std::fs::write(&path, content).unwrap();

    let result = parse_session(&path).expect("parse must succeed");
    let usage_of = |id: &str| {
        result
            .observations
            .iter()
            .find(|o| o.request_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("{id} must parse"))
            .usage
            .clone()
    };
    // First-seen cumulative per stream is the baseline (emitted as-is).
    assert_eq!(usage_of("a1").input_tokens, Some(100));
    // sB must NOT diff against sA's counter: with the old key this was Some(0).
    assert_eq!(usage_of("b1").input_tokens, Some(100));
    assert_eq!(usage_of("b1").output_tokens, Some(40));
    // sA's second counter diffs only against sA's baseline.
    assert_eq!(usage_of("a2").input_tokens, Some(50));
    assert_eq!(usage_of("a2").output_tokens, Some(20));
}

#[test]
fn test_invalid_utf8_line_is_skipped_without_dropping_rest_of_file() {
    // H7: one bad line must not permanently skip the remainder of the file.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bad_utf8.jsonl");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"u1\",\"session_id\":\"s1\",\"token_usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n");
    bytes.extend_from_slice(
        b"{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"bad\",\"broken\":\xff\xfe}\n",
    );
    bytes.extend_from_slice(b"{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"u2\",\"session_id\":\"s1\",\"token_usage\":{\"input_tokens\":20,\"output_tokens\":8}}\n");
    std::fs::write(&path, &bytes).unwrap();

    let result = parse_session(&path).expect("parse must succeed");
    assert_eq!(result.stats.malformed, 1, "bad line must be counted once");
    assert!(
        result
            .observations
            .iter()
            .any(|o| o.request_id.as_deref() == Some("u1")),
        "line before the bad line must parse"
    );
    let after = result
        .observations
        .iter()
        .find(|o| o.request_id.as_deref() == Some("u2"))
        .expect("line after the bad line must still parse");
    assert_eq!(after.usage.input_tokens, Some(20));
    // Offset tracking must account for the skipped bad line exactly.
    let bad_line_len =
        b"{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"bad\",\"broken\":\xff\xfe}\n"
            .len() as u64;
    let first_line_len = b"{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"u1\",\"session_id\":\"s1\",\"token_usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n".len() as u64;
    assert_eq!(after.source_offset, first_line_len + bad_line_len);
    assert!(
        result.final_state.active_session_id.as_deref() == Some("s1"),
        "parser state must survive the bad line"
    );
}
