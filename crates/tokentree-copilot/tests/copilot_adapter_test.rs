// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tempfile::tempdir;
use tokentree_copilot::*;
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
fn test_copilot_capabilities() {
    let caps = capabilities();
    assert!(!caps.subagent_tokens_already_in_parent);
}

#[test]
fn test_copilot_discover_sessions() {
    let root = repo_root().join("fixtures/parsers/copilot");
    let sessions = discover_sessions(&root);
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].ends_with("session-store.db"));
}

#[test]
fn test_copilot_parse_events() {
    let fixture = repo_root().join("fixtures/parsers/copilot/session-store.db");
    let res = parse_session(&fixture).expect("parse copilot fixture");

    assert_eq!(res.stats.parsed, 2);
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.stats.anomalies, 0);
    assert_eq!(res.observations.len(), 2);

    let obs1 = &res.observations[0];
    assert_eq!(obs1.adapter, "copilot");
    assert_eq!(obs1.source_subtype.as_deref(), Some("copilot_usage_event"));
    assert_eq!(
        obs1.provider_session_id,
        "e081beca-d97d-4520-8987-41f9c8237821"
    );
    assert_eq!(obs1.turn_id.as_deref(), Some("turn_1"));
    assert_eq!(obs1.request_id.as_deref(), Some("copilot_evt_1"));
    assert_eq!(obs1.agent_id.as_deref(), Some("agent_1"));
    assert_eq!(obs1.parent_agent_id, None);
    assert_eq!(obs1.model.as_deref(), Some("gpt-4o"));
    assert_eq!(obs1.usage.input_tokens, Some(1200));
    assert_eq!(obs1.usage.output_tokens, Some(350));
    assert_eq!(obs1.usage.cached_input_tokens, Some(400));
    assert_eq!(obs1.usage.cache_write_tokens, Some(100));
    assert_eq!(obs1.usage.reasoning_tokens, Some(50));
    assert_eq!(obs1.provider_reported_cost_micros, Some(15000)); // 15,000,000 nano / 1000 = 15,000 micros

    let obs2 = &res.observations[1];
    assert_eq!(obs2.turn_id.as_deref(), Some("turn_2"));
    assert_eq!(obs2.request_id.as_deref(), Some("copilot_evt_2"));
    assert_eq!(obs2.agent_id.as_deref(), Some("agent_1"));
    assert_eq!(obs2.parent_agent_id.as_deref(), Some("tool_1"));
    assert_eq!(obs2.usage.input_tokens, Some(1800));
    assert_eq!(obs2.usage.output_tokens, Some(450));
    assert_eq!(obs2.usage.cached_input_tokens, Some(600));
    assert_eq!(obs2.usage.cache_write_tokens, Some(150));
    assert_eq!(obs2.usage.reasoning_tokens, Some(80));
    assert_eq!(obs2.provider_reported_cost_micros, Some(22000));
}

#[test]
fn test_copilot_import_idempotent_with_checkpoints() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/copilot/session-store.db");

    // First import
    let res1 = import_copilot_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res1.inserted, 2);
    assert_eq!(res1.duplicates, 0);

    // Second import (checkpoint hit: skipped)
    let res2 = import_copilot_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);

    // Verify usage events count in SQLite
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'copilot'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // Verify checkpoint
    let cp_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE adapter = 'copilot'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cp_count, 1);
}
