// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tempfile::tempdir;
use tokentree_gemini::*;
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
fn test_gemini_capabilities() {
    let caps = capabilities();
    assert!(!caps.subagent_tokens_already_in_parent);
}

#[test]
fn test_gemini_discover_sessions() {
    let root = repo_root().join("fixtures/parsers/gemini");
    let sessions = discover_sessions(&root);
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].ends_with("gemini-multi-turn.db"));
}

#[test]
fn test_gemini_parse_multi_turn() {
    let fixture = repo_root().join("fixtures/parsers/gemini/gemini-multi-turn.db");
    let res = parse_session(&fixture).expect("parse gemini multi-turn fixture");

    assert_eq!(res.stats.parsed, 3); // 2 model turns + 1 failed turn
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.stats.anomalies, 1); // 1 anomaly for failed turn
    assert_eq!(res.observations.len(), 3);

    // Turn 1
    let obs1 = &res.observations[0];
    assert_eq!(obs1.adapter, "gemini");
    assert_eq!(obs1.source_subtype.as_deref(), Some("gemini_turn_usage"));
    assert_eq!(
        obs1.provider_session_id,
        "01a0c4f6-31c3-7740-99e6-018d9e1485cd"
    );
    assert_eq!(obs1.turn_id.as_deref(), Some("step_1"));
    assert_eq!(obs1.request_id.as_deref(), Some("req_gemini_1"));
    assert_eq!(obs1.agent_id.as_deref(), Some("bot-alpha"));
    assert_eq!(obs1.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(obs1.usage.input_tokens, Some(1000));
    assert_eq!(obs1.usage.output_tokens, Some(200));
    assert_eq!(obs1.usage.cached_input_tokens, Some(500));
    assert_eq!(obs1.usage.reasoning_tokens, Some(50));

    // Turn 2
    let obs2 = &res.observations[1];
    assert_eq!(obs2.adapter, "gemini");
    assert_eq!(obs2.source_subtype.as_deref(), Some("gemini_turn_usage"));
    assert_eq!(obs2.turn_id.as_deref(), Some("step_4"));
    assert_eq!(obs2.request_id.as_deref(), Some("req_gemini_2"));
    assert_eq!(obs2.agent_id.as_deref(), Some("bot-alpha"));
    assert_eq!(obs2.usage.input_tokens, Some(1500));
    assert_eq!(obs2.usage.output_tokens, Some(300));
    assert_eq!(obs2.usage.cached_input_tokens, Some(800));
    assert_eq!(obs2.usage.reasoning_tokens, Some(100));

    // Turn 3: Failed turn
    let obs3 = &res.observations[2];
    assert_eq!(obs3.source, tokentree_core::MeasurementSource::Unavailable);
    assert_eq!(obs3.source_subtype.as_deref(), Some("gemini_turn_failed"));
    assert_eq!(obs3.turn_id.as_deref(), Some("step_5"));
    assert_eq!(obs3.usage.input_tokens, None);
    assert_eq!(obs3.usage.output_tokens, None);
}

#[test]
fn test_gemini_import_idempotent_with_checkpoints() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/gemini/gemini-multi-turn.db");

    // First import
    let res1 = import_gemini_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res1.inserted, 3);
    assert_eq!(res1.duplicates, 0);

    // Second import (checkpoint hit: skipped)
    let res2 = import_gemini_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);

    // Verify usage events count in SQLite
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'gemini'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);

    // Verify checkpoint
    let cp_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE adapter = 'gemini'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cp_count, 1);
}
