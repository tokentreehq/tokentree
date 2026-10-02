// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tokentree_codex::{import_codex_file, parse_session};
use tokentree_core::token_completeness;
use tokentree_ledger::Ledger;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/parsers/codex")
}

fn open_test_ledger() -> Ledger {
    let tmp = tempfile::tempdir().unwrap();
    Ledger::open(tmp.path().join("ledger.db")).unwrap()
}

#[test]
fn test_case_a_detailed_events_only() {
    let fixture = fixtures_dir().join("adversarial/case-a-detailed-only.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(parsed.anomalies.len(), 0);

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    assert_eq!(agg.input, 250);
    assert_eq!(agg.cache_read, 50);
    assert_eq!(agg.output, 120);
    assert_eq!(agg.reasoning, 25);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert_eq!(comp, Some(100.0));
}

#[test]
fn test_case_b_counter_only() {
    let fixture = fixtures_dir().join("adversarial/case-b-counter-only.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    assert_eq!(parsed.observations.len(), 1);
    assert_eq!(parsed.anomalies.len(), 0);

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 1);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    assert_eq!(agg.input, 250);
    assert_eq!(agg.cache_read, 50);
    assert_eq!(agg.output, 120);
    assert_eq!(agg.reasoning, 25);
    assert_eq!(agg.requests, 1);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert_eq!(comp, Some(100.0));
}

#[test]
fn test_case_c_detailed_plus_equal_counter() {
    let fixture = fixtures_dir().join("adversarial/case-c-detailed-equal-counter.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    // Counter is recognized as covering equal counter and suppressed
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(parsed.anomalies.len(), 0);

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    // Assert exact totals: NEVER sum detailed event and covering counter
    assert_eq!(agg.input, 250);
    assert_eq!(agg.cache_read, 50);
    assert_eq!(agg.output, 120);
    assert_eq!(agg.reasoning, 25);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert_eq!(comp, Some(100.0));
}

#[test]
fn test_case_d_detailed_plus_conflicting_counter() {
    let fixture = fixtures_dir().join("adversarial/case-d-detailed-conflicting-counter.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    // Authoritative detailed events win; conflicting counter creates anomaly
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(parsed.anomalies.len(), 1);
    assert_eq!(parsed.anomalies[0].anomaly_type, "counter_conflict");

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 1);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    assert_eq!(agg.input, 250);
    assert_eq!(agg.cache_read, 50);
    assert_eq!(agg.output, 120);
    assert_eq!(agg.reasoning, 25);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 1);
    // Completeness is penalized by anomaly: 2 / (2 + 0 + 1) = 66.6666...%
    let expected_comp = 100.0 * 2.0 / 3.0;
    assert!((comp.unwrap() - expected_comp).abs() < 1e-6);
}

#[test]
fn test_case_e_repeated_counters() {
    let fixture = fixtures_dir().join("adversarial/case-e-repeated-counters.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    // Repeated identical turn counters deduplicate to 1
    assert_eq!(parsed.observations.len(), 1);
    assert_eq!(parsed.anomalies.len(), 0);

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 1);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    assert_eq!(agg.input, 250);
    assert_eq!(agg.cache_read, 50);
    assert_eq!(agg.output, 120);
    assert_eq!(agg.reasoning, 25);
    assert_eq!(agg.requests, 1);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert_eq!(comp, Some(100.0));
}

#[test]
fn test_case_f_cumulative_scoped_per_turn_no_spurious_reset() {
    // H5: cumulative counters are keyed per (session, turn, model, agent), so a
    // counter that restarts in a new turn is a fresh stream, NOT a negative
    // delta. The old {model}:{agent} key spuriously flagged this as an anomaly.
    let fixture = fixtures_dir().join("adversarial/case-f-cumulative-resets.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(
        parsed.anomalies.len(),
        0,
        "cross-turn counter restart must not raise negative_delta"
    );

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    // Turn 1 baseline (500, 100) + turn 2 baseline (200, 50) = 700, 150
    assert_eq!(agg.input, 700);
    assert_eq!(agg.cache_read, 0);
    assert_eq!(agg.output, 150);
    assert_eq!(agg.reasoning, 0);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert!((comp.unwrap() - 100.0).abs() < 1e-6);
}

#[test]
fn test_case_h_cumulative_reset_within_turn_still_detected() {
    // A counter that moves backwards WITHIN the same turn is a genuine reset:
    // it must still raise negative_delta and re-baseline.
    let fixture = fixtures_dir().join("adversarial/case-h-cumulative-reset-within-turn.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(parsed.anomalies.len(), 1);
    assert_eq!(parsed.anomalies[0].anomaly_type, "negative_delta");

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 1);

    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    // Baseline (500, 100) + reset value (200, 50) = 700, 150
    assert_eq!(agg.input, 700);
    assert_eq!(agg.cache_read, 0);
    assert_eq!(agg.output, 150);
    assert_eq!(agg.reasoning, 0);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 1);
    let expected_comp = 100.0 * 2.0 / 3.0;
    assert!((comp.unwrap() - expected_comp).abs() < 1e-6);
}

#[test]
fn test_case_g_subagent_covering_counter() {
    let fixture = fixtures_dir().join("adversarial/case-g-subagent-covering-counter.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    // Covering counter suppressed, detailed parent & child preserved
    assert_eq!(parsed.observations.len(), 2);
    assert_eq!(parsed.anomalies.len(), 0);

    let mut ledger = open_test_ledger();
    let import = import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(import.inserted, 2);
    assert_eq!(import.duplicates, 0);
    assert_eq!(import.anomalies, 0);
    // Register capability: subagents are independent (subagent_tokens_already_in_parent = false)
    ledger
        .connection()
        .execute(
            "INSERT INTO adapter_capabilities (adapter, adapter_version, host_version, capability, state, detail, checked_at)
             VALUES ('codex', '0.2.0', '1.0.0', 'subagent_tokens_already_in_parent', 'available', 'false', '2026-03-30T10:00:00Z')",
            [],
        )
        .unwrap();

    // Under Independent policy: child and parent both roll up exactly once
    let agg = ledger.aggregate_usage().unwrap();
    let comp = token_completeness(agg.measured, agg.unavailable, agg.anomalous);

    assert_eq!(agg.input, 150); // 100 parent + 50 child
    assert_eq!(agg.cache_read, 0);
    assert_eq!(agg.output, 30); // 20 parent + 10 child
    assert_eq!(agg.reasoning, 0);
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.unavailable, 0);
    assert_eq!(agg.anomalous, 0);
    assert_eq!(comp, Some(100.0));

    // Under AlreadyInParent policy: child is excluded from aggregate
    ledger
        .connection()
        .execute(
            "UPDATE adapter_capabilities SET detail = 'true' WHERE capability = 'subagent_tokens_already_in_parent'",
            [],
        )
        .unwrap();
    let agg_parent = ledger.aggregate_usage().unwrap();
    assert_eq!(agg_parent.input, 100);
    assert_eq!(agg_parent.output, 20);
}

#[test]
fn test_unknown_version_is_degraded() {
    let fixture = fixtures_dir().join("adversarial/case-unknown-version.jsonl");
    let parsed = parse_session(&fixture).unwrap();
    assert_eq!(parsed.stats.unknown, 2);
    assert_eq!(parsed.anomalies.len(), 2);
    assert_eq!(parsed.anomalies[0].anomaly_type, "unsupported_version");
    assert_eq!(parsed.anomalies[1].anomaly_type, "unsupported_version");
}
