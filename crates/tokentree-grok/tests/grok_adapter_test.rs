// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tempfile::tempdir;
use tokentree_grok::*;
use tokentree_ledger::Ledger;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn test_parse_single_turn() {
    let fixture = repo_root().join("fixtures/parsers/grok/single-turn.json");
    let res = parse_session(&fixture).expect("parse single turn fixture");

    assert_eq!(res.stats.parsed, 1);
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.stats.anomalies, 0);
    assert_eq!(res.observations.len(), 1);

    let obs = &res.observations[0];
    assert_eq!(obs.adapter, "grok");
    assert_eq!(obs.source_subtype.as_deref(), Some("grok_turn_usage"));
    assert_eq!(
        obs.provider_session_id,
        "01a0c4f6-31c3-7740-99e6-018d9e1485cd"
    );
    assert_eq!(obs.turn_id.as_deref(), Some("turn_1"));
    assert_eq!(obs.model.as_deref(), Some("grok-4.7-build"));
    assert_eq!(obs.usage.input_tokens, Some(2471317));
    assert_eq!(obs.usage.output_tokens, Some(36242));
    assert_eq!(obs.usage.cached_input_tokens, Some(2166784));
    assert_eq!(obs.usage.reasoning_tokens, Some(27450));
    assert_eq!(obs.provider_reported_cost_micros, Some(19099100)); // 19099100000 ticks / 1000 = 19099100 micros ($19.0991)
}

#[test]
fn test_parse_multi_turn_suppresses_session_counter() {
    let fixture = repo_root().join("fixtures/parsers/grok/multi-turn.json");
    let res = parse_session(&fixture).expect("parse multi-turn fixture");

    assert_eq!(res.stats.parsed, 2);
    assert_eq!(res.observations.len(), 2);

    // Assert both turns are parsed and session summary is not duplicated
    assert_eq!(res.observations[0].turn_id.as_deref(), Some("turn_1"));
    assert_eq!(res.observations[0].usage.input_tokens, Some(2471317));
    assert_eq!(
        res.observations[0].provider_reported_cost_micros,
        Some(19099100)
    );

    assert_eq!(res.observations[1].turn_id.as_deref(), Some("turn_2"));
    assert_eq!(res.observations[1].usage.input_tokens, Some(3866652));
    assert_eq!(
        res.observations[1].provider_reported_cost_micros,
        Some(22909860)
    );
}

#[test]
fn test_parse_zero_tokens_failed_run() {
    let fixture = repo_root().join("fixtures/parsers/grok/zero-tokens-failed.json");
    let res = parse_session(&fixture).expect("parse zero tokens fixture");

    assert_eq!(res.stats.parsed, 1);
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.observations.len(), 1);

    let obs = &res.observations[0];
    assert_eq!(obs.usage.input_tokens, Some(0));
    assert_eq!(obs.usage.output_tokens, Some(0));
    assert_eq!(obs.usage.cached_input_tokens, Some(0));
}

#[test]
fn test_adversarial_corrupted_json() {
    let fixture = repo_root().join("fixtures/parsers/grok/adversarial/corrupted.json");
    let res = parse_session(&fixture).expect("handle corrupted json");

    assert_eq!(res.stats.malformed, 1);
    assert_eq!(res.stats.anomalies, 1);
    assert!(res.observations.is_empty());
    assert_eq!(res.anomalies[0].anomaly_type, "malformed_record");
}

#[test]
fn test_adversarial_sum_mismatch_detected() {
    let fixture = repo_root().join("fixtures/parsers/grok/adversarial/sum-mismatch.json");
    let res = parse_session(&fixture).expect("parse sum mismatch");

    assert_eq!(res.stats.anomalies, 1);
    assert_eq!(res.anomalies[0].anomaly_type, "token_sum_mismatch");
}

#[test]
fn test_grok_import_idempotent_with_checkpoints() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/grok/multi-turn.json");

    // First import
    let res1 = import_grok_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res1.inserted, 2);
    assert_eq!(res1.duplicates, 0);

    // Second import (checkpoint hit: skipped)
    let res2 = import_grok_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);

    // Verify usage events count in SQLite
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'grok'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // Verify checkpoint
    let cp_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE adapter = 'grok'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cp_count, 1);
}
