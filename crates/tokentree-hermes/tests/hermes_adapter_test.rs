// SPDX-License-Identifier: Apache-2.0
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokentree_hermes::*;
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
fn test_parse_oneshot_usage() {
    let fixture = repo_root().join("fixtures/parsers/hermes/oneshot-usage.json");
    let res = parse_session_file(&fixture).expect("parse oneshot fixture");

    assert_eq!(res.stats.parsed, 2); // main task + title_generation auxiliary task
    assert_eq!(res.stats.malformed, 0);
    assert_eq!(res.stats.anomalies, 0);
    assert_eq!(res.observations.len(), 2);

    let main_obs = &res.observations[0];
    assert_eq!(main_obs.adapter, "hermes");
    assert_eq!(
        main_obs.source_subtype.as_deref(),
        Some("hermes_oneshot_usage")
    );
    assert_eq!(main_obs.provider_session_id, "20260930_194647_5766b6");
    assert_eq!(main_obs.model.as_deref(), Some("liquid/lfm-2.5-2.6b:free"));
    assert_eq!(main_obs.usage.input_tokens, Some(13835));
    assert_eq!(main_obs.usage.output_tokens, Some(40));
    assert_eq!(main_obs.usage.cached_input_tokens, Some(576));
    assert_eq!(main_obs.usage.reasoning_tokens, Some(29));
    assert_eq!(main_obs.provider_reported_cost_micros, Some(0)); // free model

    let aux_obs = &res.observations[1];
    assert_eq!(
        aux_obs.source_subtype.as_deref(),
        Some("hermes_auxiliary_title_generation")
    );
    assert_eq!(aux_obs.turn_id.as_deref(), Some("task_title_generation"));
    assert_eq!(aux_obs.usage.input_tokens, Some(249));
    assert_eq!(aux_obs.usage.output_tokens, Some(464));
    assert_eq!(aux_obs.usage.reasoning_tokens, Some(453));
    assert_eq!(
        aux_obs.parent_agent_id.as_deref(),
        Some("hermes:20260930_194647_5766b6")
    );
}

#[test]
fn test_parse_oneshot_failed() {
    let fixture = repo_root().join("fixtures/parsers/hermes/oneshot-failed.json");
    let res = parse_session_file(&fixture).expect("parse failed oneshot fixture");

    assert_eq!(res.stats.parsed, 1);
    assert_eq!(res.observations.len(), 1);

    let obs = &res.observations[0];
    assert_eq!(obs.source_subtype.as_deref(), Some("hermes_failed_run"));
    assert_eq!(obs.usage.input_tokens, Some(0));
    assert_eq!(obs.usage.output_tokens, Some(0));
}

#[test]
fn test_adversarial_corrupted_json() {
    let fixture = repo_root().join("fixtures/parsers/hermes/adversarial/corrupted.json");
    let res = parse_session_file(&fixture).expect("handle corrupted json");

    assert_eq!(res.stats.malformed, 1);
    assert_eq!(res.stats.anomalies, 1);
    assert!(res.observations.is_empty());
    assert_eq!(res.anomalies[0].anomaly_type, "malformed_record");
}

#[test]
fn test_adversarial_missing_cost_on_paid_model() {
    let fixture = repo_root().join("fixtures/parsers/hermes/adversarial/missing-cost.json");
    let res = parse_session_file(&fixture).expect("handle missing cost");

    assert_eq!(res.observations.len(), 1);
    let obs = &res.observations[0];
    assert_eq!(obs.model.as_deref(), Some("anthropic/claude-sonnet-4.6"));
    // On paid model with missing provider cost, provider_reported_cost_micros is None (not 0!)
    assert_eq!(obs.provider_reported_cost_micros, None);
}

#[test]
fn test_hermes_import_idempotent_with_checkpoints() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let fixture = repo_root().join("fixtures/parsers/hermes/oneshot-usage.json");

    // First import
    let res1 = import_hermes_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res1.inserted, 2);
    assert_eq!(res1.duplicates, 0);

    // Second import (checkpoint hit: skipped)
    let res2 = import_hermes_file(ledger.connection_mut(), &fixture).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);

    // Verify usage events count in SQLite
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // Verify checkpoint
    let cp_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM ingestion_checkpoints WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cp_count, 1);
}

#[test]
fn test_parse_real_hermes_state_db_if_present() {
    let local_state_db = PathBuf::from(r"C:\Users\Shrikar\AppData\Local\hermes\state.db");
    if local_state_db.exists() {
        let res =
            parse_hermes_state_db(&local_state_db, None).expect("parse real local hermes state.db");
        assert!(res.stats.parsed > 0);
        assert!(!res.observations.is_empty());
        for obs in &res.observations {
            assert_eq!(obs.adapter, "hermes");
            assert!(obs.usage.input_tokens.is_some());
        }
    }
}

fn parse_session_file(path: &Path) -> anyhow::Result<ParseResult> {
    let mut file = std::fs::File::open(path)?;
    let mut content = String::new();
    std::io::Read::read_to_string(&mut file, &mut content)?;
    parse_json_usage_str(&content, path, 0, HermesParserState::default())
}
