// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tempfile::tempdir;
use tokentree_ledger::Ledger;
use tokentree_opencode::*;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn test_opencode_capabilities() {
    let caps = capabilities();
    assert!(!caps.subagent_tokens_already_in_parent);
}

#[test]
fn test_opencode_discover_sessions() {
    let root = repo_root().join("fixtures/parsers/opencode");
    let sessions = discover_sessions(&root);
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].ends_with("opencode.db"));
}

#[test]
fn test_opencode_parse_messages() {
    let fixture = repo_root().join("fixtures/parsers/opencode/opencode.db");
    let res = parse_session(&fixture).expect("parse opencode fixture");

    assert_eq!(res.stats.parsed, 2);
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.stats.anomalies, 0);
    assert_eq!(res.observations.len(), 2);

    let obs1 = &res.observations[0];
    assert_eq!(obs1.adapter, "opencode");
    assert_eq!(
        obs1.source_subtype.as_deref(),
        Some("opencode_message_usage")
    );
    assert_eq!(obs1.provider_session_id, "ses_0164a8819ffeAfJq7ngefZChud");
    assert_eq!(obs1.turn_id.as_deref(), Some("msg_1"));
    assert_eq!(obs1.request_id.as_deref(), Some("msg_1"));
    assert_eq!(obs1.agent_id.as_deref(), Some("build"));
    assert_eq!(obs1.model.as_deref(), Some("claude-3-7-sonnet"));
    assert_eq!(obs1.usage.input_tokens, Some(2000));
    assert_eq!(obs1.usage.output_tokens, Some(400));
    assert_eq!(obs1.usage.cached_input_tokens, Some(500));
    assert_eq!(obs1.usage.cache_write_tokens, Some(100));
    assert_eq!(obs1.usage.reasoning_tokens, Some(100));
    assert_eq!(obs1.provider_reported_cost_micros, Some(25000)); // $0.025 = 25000 micros

    let obs2 = &res.observations[1];
    assert_eq!(obs2.turn_id.as_deref(), Some("msg_2"));
    assert_eq!(obs2.request_id.as_deref(), Some("msg_2"));
    assert_eq!(obs2.usage.input_tokens, Some(3000));
    assert_eq!(obs2.usage.output_tokens, Some(600));
    assert_eq!(obs2.usage.cached_input_tokens, Some(700));
    assert_eq!(obs2.usage.cache_write_tokens, Some(200));
    assert_eq!(obs2.usage.reasoning_tokens, Some(100));
    assert_eq!(obs2.provider_reported_cost_micros, Some(35000));
}

#[test]
fn test_opencode_import_idempotent_with_checkpoints() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/opencode/opencode.db");

    // First import
    let res1 = import_opencode_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res1.inserted, 2);
    assert_eq!(res1.duplicates, 0);

    // Second import (checkpoint hit: skipped)
    let res2 = import_opencode_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);

    // Verify usage events count in SQLite
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'opencode'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // Verify checkpoint
    let cp_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE adapter = 'opencode'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cp_count, 1);
}
