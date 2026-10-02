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
    assert_eq!(res.stats.anomalies, 1);
    assert_eq!(
        res.anomalies[0].anomaly_type,
        "missing_provider_measurements"
    );
    assert_eq!(res.observations.len(), 1);

    let obs = &res.observations[0];
    assert_eq!(obs.source, tokentree_core::MeasurementSource::Unavailable);
    assert_eq!(obs.source_subtype.as_deref(), Some("hermes_failed_run"));
    assert_eq!(obs.usage.input_tokens, None);
    assert_eq!(obs.usage.output_tokens, None);
    assert_eq!(obs.usage.cached_input_tokens, None);
    assert_eq!(obs.provider_reported_cost_micros, None);
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

#[test]
fn test_exact_decimal_dollars_to_micros() {
    assert_eq!(decimal_dollars_to_micros("0").unwrap(), 0);
    assert_eq!(decimal_dollars_to_micros("0.0").unwrap(), 0);
    assert_eq!(decimal_dollars_to_micros("0.000001").unwrap(), 1);
    assert_eq!(decimal_dollars_to_micros("0.000029").unwrap(), 29);
    assert_eq!(decimal_dollars_to_micros("1.5").unwrap(), 1_500_000);
    assert_eq!(decimal_dollars_to_micros("12.345678").unwrap(), 12_345_678);
    // Rounding on 7th digit:
    assert_eq!(decimal_dollars_to_micros("0.0000294").unwrap(), 29);
    assert_eq!(decimal_dollars_to_micros("0.0000295").unwrap(), 30);
}

#[test]
fn test_hermes_state_db_read_only_snapshot_safe_and_idempotent_growth() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_state.db");

    // Initialize mock hermes state.db with schema
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT
            );
            CREATE TABLE session_model_usage (
                session_id TEXT NOT NULL,
                model TEXT NOT NULL,
                billing_provider TEXT DEFAULT '',
                task TEXT DEFAULT '',
                api_call_count INTEGER DEFAULT 1,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                estimated_cost_usd REAL,
                actual_cost_usd REAL,
                cost_status TEXT,
                cost_source TEXT,
                first_seen REAL,
                last_seen REAL,
                PRIMARY KEY (session_id, model, task)
            );
            INSERT INTO sessions VALUES ('ses_1', NULL);
            INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
            VALUES ('ses_1', 'liquid/lfm-2.5-2.6b:free', '', 1000, 50, 0.0, 100.0, 100.0);
            INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
            VALUES ('ses_1', 'liquid/lfm-2.5-2.6b:free', 'title_generation', 200, 30, 0.0, 101.0, 101.0);",
        )
        .unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // 1. Initial import: 2 rows inserted
    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 2);
    assert_eq!(res1.duplicates, 0);

    // 2. Immediate re-import without DB changes: 0 inserted, 2 rows recognized as duplicates via row checkpoints
    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 2);

    // 3. Hermes appends 2 new rows while running (database grows)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions VALUES ('ses_2', 'ses_1');
            INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
            VALUES ('ses_2', 'openai/gpt-4o', 'subtask', 500, 100, 0.005, 105.0, 105.0);
            INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
            VALUES ('ses_2', 'openai/gpt-4o', '', 1500, 200, 0.015, 106.0, 106.0);",
        )
        .unwrap();
    }

    // 4. Third import: exactly the 2 new rows are inserted, 2 existing rows recorded as duplicates
    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 2);
    assert_eq!(res3.duplicates, 2);

    // 5. Fourth import without changes: 0 inserted, all 4 rows recognized as duplicates
    let res4 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res4.inserted, 0);
    assert_eq!(res4.duplicates, 4);

    // 6. Verify ledger total usage events is exactly 4
    let total_evts: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total_evts, 4);

    // 7. Verify reconcile reports 0 duplicates and 0 unresolved anomalies
    let recon = ledger.reconcile().unwrap();
    assert_eq!(recon.duplicate_request_ids, 0);
    assert_eq!(recon.unresolved_anomalies, 0);
}

#[test]
fn test_free_model_detection() {
    assert!(is_free_model("liquid/lfm-2.5-2.6b:free"));
    assert!(is_free_model("meta-llama/llama-3-8b-instruct:free"));
    assert!(is_free_model("openrouter/free/test-model"));
    assert!(is_free_model("provider/model-free"));
    assert!(is_free_model("provider/model/free"));

    // Non-free models with "free" substring in words must NOT be detected as free
    assert!(!is_free_model("freedom-ai/model"));
    assert!(!is_free_model("freeze-v1"));
    assert!(!is_free_model("freebsd-tools"));
    assert!(!is_free_model("anthropic/claude-sonnet-4.6"));
}

#[test]
fn test_cost_precedence_actual_over_estimated() {
    let json = serde_json::json!({
        "actual_cost_usd": 0.005,
        "estimated_cost_usd": 0.010,
        "input_tokens": 1000,
        "output_tokens": 500,
        "model": "anthropic/claude-sonnet-4.6",
        "session_id": "ses_precedence_test",
        "completed": true
    })
    .to_string();

    let res = parse_json_usage_str(
        &json,
        Path::new("test.json"),
        0,
        HermesParserState::default(),
    )
    .unwrap();
    assert_eq!(res.observations.len(), 1);
    // actual_cost_usd ($0.005 = 5000 micros) must win over estimated_cost_usd ($0.010 = 10000 micros)
    assert_eq!(
        res.observations[0].provider_reported_cost_micros,
        Some(5000)
    );
}

#[test]
fn test_measured_zero_cost_on_paid_model() {
    let json = serde_json::json!({
        "actual_cost_usd": 0.0,
        "input_tokens": 1000,
        "output_tokens": 500,
        "model": "anthropic/claude-sonnet-4.6",
        "session_id": "ses_measured_zero_test",
        "completed": true
    })
    .to_string();

    let res = parse_json_usage_str(
        &json,
        Path::new("test.json"),
        0,
        HermesParserState::default(),
    )
    .unwrap();
    assert_eq!(res.observations.len(), 1);
    // Paid model with explicitly measured 0 cost has Some(0) (MeasuredZero)
    assert_eq!(res.observations[0].provider_reported_cost_micros, Some(0));
}

#[test]
fn test_authoritative_some_zero_tokens_on_measured_row() {
    let dir = tempfile::tempdir().unwrap();
    let state_db_path = dir.path().join("state.db");
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT
            );
            CREATE TABLE session_model_usage (
                session_id TEXT NOT NULL,
                model TEXT NOT NULL,
                billing_provider TEXT,
                task TEXT,
                api_call_count INTEGER DEFAULT 1,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                estimated_cost_usd REAL,
                actual_cost_usd REAL,
                cost_status TEXT,
                cost_source TEXT,
                first_seen REAL,
                last_seen REAL,
                PRIMARY KEY (session_id, model, task)
            );
            INSERT INTO sessions VALUES ('ses_zero_tokens', NULL);
            INSERT INTO session_model_usage (session_id, model, task, api_call_count, input_tokens, output_tokens, cache_read_tokens, reasoning_tokens, estimated_cost_usd, first_seen, last_seen)
            VALUES ('ses_zero_tokens', 'openai/gpt-4o', '', 1, 500, 0, 0, 0, 0.001, 100.0, 100.0);",
        )
        .unwrap();
    }

    let res = parse_hermes_state_db(&state_db_path, None).unwrap();
    assert_eq!(res.observations.len(), 1);
    let obs = &res.observations[0];
    assert_eq!(obs.usage.input_tokens, Some(500));
    // Authoritative Some(0) must be preserved on measured rows
    assert_eq!(obs.usage.output_tokens, Some(0));
    assert_eq!(obs.usage.cached_input_tokens, Some(0));
    assert_eq!(obs.usage.reasoning_tokens, Some(0));
}

#[test]
fn test_hermes_decimal_parsing_rejects_malformed_and_exponents() {
    // Malformed and exponent strings must be rejected
    assert!(decimal_dollars_to_micros("1e-5").is_err());
    assert!(decimal_dollars_to_micros("-0.05").is_err());
    assert!(decimal_dollars_to_micros("1.2.3").is_err());
    assert!(decimal_dollars_to_micros("").is_err());
    assert!(decimal_dollars_to_micros("NaN").is_err());
    assert!(decimal_dollars_to_micros("inf").is_err());
    assert!(decimal_dollars_to_micros("abc").is_err());
    assert!(decimal_dollars_to_micros("999999999999999999999999").is_err());

    // value_to_micros must reject negative, infinite, and out-of-range values
    assert_eq!(value_to_micros(Some(&serde_json::json!(-0.01))), None);
    assert_eq!(value_to_micros(Some(&serde_json::json!("1e-5"))), None);
    assert_eq!(value_to_micros(Some(&serde_json::json!("-5.0"))), None);
    assert_eq!(value_to_micros(Some(&serde_json::json!(2_000_000.0))), None);
}

#[test]
fn test_hermes_state_db_source_database_byte_for_byte_unchanged() {
    use sha2::{Digest, Sha256};
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_state.db");

    // Initialize mock database
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_test', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_test', 'liquid/lfm-2.5-2.6b:free', '', 500, 50, 0.0, 100.0, 100.0);",
        ).unwrap();
    }

    let initial_bytes = std::fs::read(&state_db_path).unwrap();
    let initial_hash = hex::encode(Sha256::digest(&initial_bytes));

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // Ingest into ledger
    let res = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res.inserted, 1);

    // Verify byte-for-byte identity of source database
    let post_bytes = std::fs::read(&state_db_path).unwrap();
    let post_hash = hex::encode(Sha256::digest(&post_bytes));

    assert_eq!(
        initial_hash, post_hash,
        "TokenTree must never modify source Hermes database"
    );
    assert_eq!(initial_bytes, post_bytes, "Source bytes must be identical");
}

#[test]
fn test_hermes_state_db_timestamp_collisions_and_updated_rows() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_state.db");

    // 1. Initialize DB with 2 records sharing the EXACT same timestamp (collision)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_1', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_1', 'liquid/lfm-2.5-2.6b:free', 'main', 500, 50, 0.0, 100.0, 100.0);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_1', 'liquid/lfm-2.5-2.6b:free', 'aux', 200, 20, 0.0, 100.0, 100.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res1.inserted, 2,
        "both records sharing timestamp 100.0 must be inserted"
    );

    // 2. Hermes updates the 'main' task row in place with more tokens as session progresses
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 1200, output_tokens = 150 WHERE session_id = 'ses_1' AND task = 'main'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 1,
        "updated row must emit append-only delta event"
    );

    // Total events in ledger is 3 (2 initial + 1 append-only delta)
    let total_evts: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total_evts, 3);

    // Total tokens must reflect updated counts: 1200 + 150 + 200 + 20 = 1570
    let total_tokens: i64 = ledger
        .connection()
        .query_row("SELECT sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) FROM usage_events WHERE adapter = 'hermes'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_tokens, 1570);

    let recon = ledger.reconcile().unwrap();
    assert_eq!(recon.duplicate_request_ids, 0);
}

#[test]
fn test_hermes_state_db_replacement_and_truncation() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_state.db");

    // 1. Initial database with session ses_old
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_old', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_old', 'liquid/lfm-2.5-2.6b:free', '', 500, 50, 0.0, 200.0, 200.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // 2. Database is replaced with a new truncated DB that has an older timestamp (e.g. 50.0)
    std::fs::remove_file(&state_db_path).unwrap();
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_new', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_new', 'openai/gpt-4o', '', 800, 80, 0.005, 50.0, 50.0);",
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 1,
        "records from replacement database must be ingested"
    );

    let total_evts: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total_evts, 2);
}

#[test]
fn test_hermes_wal_mode_commits_ingested_when_db_file_unchanged_and_zero_mutation() {
    use sha2::{Digest, Sha256};
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_wal.db");

    // Initialize in WAL mode
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_wal_1', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_wal_1', 'liquid/lfm-2.5-2.6b:free', '', 100, 10, 0.0, 10.0, 10.0);
             PRAGMA wal_checkpoint(TRUNCATE);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // Open writer connection with wal_autocheckpoint = 0 and keep it alive,
    // exactly simulating an active running Hermes agent process committing to WAL
    let writer_conn = rusqlite::Connection::open(&state_db_path).unwrap();
    writer_conn
        .execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .unwrap();

    let db_bytes_before = std::fs::read(&state_db_path).unwrap();
    let db_hash_before = hex::encode(Sha256::digest(&db_bytes_before));

    // Commit a new row into SQLite that goes into the WAL file
    writer_conn.execute_batch(
        "INSERT INTO sessions VALUES ('ses_wal_2', NULL);
         INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
         VALUES ('ses_wal_2', 'openai/gpt-4o', '', 250, 25, 0.002, 20.0, 20.0);",
    ).unwrap();

    // Verify main DB file bytes remain unchanged (commit resides in WAL)
    let db_bytes_during = std::fs::read(&state_db_path).unwrap();
    let db_hash_during = hex::encode(Sha256::digest(&db_bytes_during));
    assert_eq!(
        db_hash_before, db_hash_during,
        "state.db file hash must be identical because commit is in WAL"
    );

    let wal_path = dir.path().join("hermes_wal.db-wal");
    assert!(wal_path.exists(), "WAL file must exist");
    let wal_bytes_before = std::fs::read(&wal_path).unwrap();
    let wal_hash_before = hex::encode(Sha256::digest(&wal_bytes_before));

    // Ingest with TokenTree
    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 1,
        "TokenTree must ingest WAL commit even though state.db main file was unchanged"
    );
    assert_eq!(
        res2.duplicates, 1,
        "Initial row must be recognized as duplicate"
    );

    // Verify neither state.db nor state.db-wal was mutated by TokenTree
    let db_bytes_after = std::fs::read(&state_db_path).unwrap();
    let db_hash_after = hex::encode(Sha256::digest(&db_bytes_after));
    assert_eq!(
        db_hash_during, db_hash_after,
        "state.db must not be mutated"
    );

    let wal_bytes_after = std::fs::read(&wal_path).unwrap();
    let wal_hash_after = hex::encode(Sha256::digest(&wal_bytes_after));
    assert_eq!(
        wal_hash_before, wal_hash_after,
        "state.db-wal must not be mutated"
    );
}

#[test]
fn test_hermes_snapshot_delta_category_redistribution_and_token_regression_anomaly() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_regression.db");

    // 1. Initial state: 80 input tokens, 20 output tokens (total = 100)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_redist', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_redist', 'liquid/lfm-2.5-2.6b:free', '', 80, 20, 0.0, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);
    assert_eq!(res1.anomalies, 0);

    // 2. Category redistribution: total remains 100, but input regresses 80 -> 70 while output increases 20 -> 30
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 70, output_tokens = 30 WHERE session_id = 'ses_redist'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 0,
        "must not emit delta when category regresses"
    );
    assert_eq!(
        res2.anomalies, 1,
        "must record token_category_regression anomaly"
    );

    let anom_type: String = ledger
        .connection()
        .query_row(
            "SELECT type FROM measurement_anomalies WHERE type = 'token_category_regression' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(anom_type, "token_category_regression");

    // 3. Token reset regression: input drops from 80 to 20
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 20, output_tokens = 10 WHERE session_id = 'ses_redist'",
            [],
        ).unwrap();
    }

    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 0, "reset must not emit delta or clamp");
    assert_eq!(res3.anomalies, 1);
}

#[test]
fn test_hermes_snapshot_delta_cost_growth_and_cost_regression_anomaly() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_cost.db");

    // 1. Initial row with cost $0.010 = 10,000 micros
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_cost', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, actual_cost_usd, first_seen, last_seen)
             VALUES ('ses_cost', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // 2. Growth: tokens grow by 500 in, 50 out; cost increases to $0.016 (16,000 micros)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 1500, output_tokens = 150, actual_cost_usd = 0.016 WHERE session_id = 'ses_cost'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res2.inserted, 1, "delta must be emitted");

    // Verify delta cost is 6,000 micros (delta only), NOT 16,000 micros
    let delta_cost: Option<i64> = ledger
        .connection()
        .query_row(
            "SELECT provider_reported_cost_micros FROM usage_events WHERE source_kind = 'hermes_snapshot_delta'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        delta_cost,
        Some(6000),
        "delta cost must be exactly the difference (6,000 micros), never cloned"
    );

    // 3. Cost regression: cost unexpectedly drops to $0.005 (5,000 micros < 16,000 micros)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = 0.005 WHERE session_id = 'ses_cost'",
            [],
        )
        .unwrap();
    }

    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 0, "cost regression must not emit delta");
    assert_eq!(
        res3.anomalies, 1,
        "must record provider_cost_regression anomaly"
    );

    let anom_type: String = ledger
        .connection()
        .query_row(
            "SELECT type FROM measurement_anomalies WHERE type = 'provider_cost_regression' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(anom_type, "provider_cost_regression");
}

#[test]
fn test_hermes_restart_preserves_row_checkpoints_and_reconciliation() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_restart.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_restart', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_restart', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    {
        let mut ledger = Ledger::open(&ledger_path).unwrap();
        let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res1.inserted, 1);
    } // Ledger closes

    // Source row updates while ledger was offline
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 1400, output_tokens = 140 WHERE session_id = 'ses_restart'",
            [],
        ).unwrap();
    }

    // Ledger reopens (simulating process restart)
    {
        let mut ledger = Ledger::open(&ledger_path).unwrap();
        let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(
            res2.inserted, 1,
            "restarted ledger must use persisted row checkpoints to emit delta"
        );

        // Verify total tokens: 1000 + 100 + 400 + 40 = 1540
        let total_tokens: i64 = ledger
            .connection()
            .query_row(
                "SELECT sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) FROM usage_events WHERE adapter = 'hermes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total_tokens, 1540);

        // Verify reconciliation: requests count as 1, 0 duplicate requests
        let recon = ledger.reconcile().unwrap();
        assert_eq!(recon.duplicate_request_ids, 0);
        assert_eq!(recon.duplicate_subagent_counters, 0);
    }
}

#[test]
fn test_hermes_epoch_reset_to_zero_then_growth() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_reset_zero.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_rz', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_rz', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // 1. Initial ingestion
    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);
    assert_eq!(res1.anomalies, 0);

    // 2. Unexpected reset to zero
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 0, output_tokens = 0, last_seen = 20.0 WHERE session_id = 'ses_rz'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 0,
        "reset-to-zero snapshot must not emit delta usage"
    );
    assert_eq!(
        res2.anomalies, 1,
        "must record token_category_regression anomaly"
    );

    // 3. Future growth from new baseline (300 in, 30 out)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 300, output_tokens = 30, last_seen = 30.0 WHERE session_id = 'ses_rz'",
            [],
        ).unwrap();
    }

    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 1, "growth from new baseline must emit delta");
    assert_eq!(res3.anomalies, 0);

    // Verify supplemental event ID contains epoch 1 and sequence 1
    let delta_event_id: String = ledger.connection().query_row(
        "SELECT source_event_id FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(
        delta_event_id.contains(":te1ts1:"),
        "event ID must reflect token epoch 1 sequence 1, got {delta_event_id}"
    );

    // Total tokens: (1000 + 100) + (300 + 30) = 1430
    let total_tokens: i64 = ledger.connection().query_row(
        "SELECT sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) FROM usage_events WHERE adapter = 'hermes'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(total_tokens, 1430);
}

#[test]
fn test_hermes_epoch_partial_reset_then_growth() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_partial_reset.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_pr', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_pr', 'openai/gpt-4o', '', 1000, 200, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // Partial reset: input tokens regressed from 1000 down to 600, output grew to 250
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 600, output_tokens = 250, last_seen = 20.0 WHERE session_id = 'ses_pr'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.inserted, 0,
        "partial regression must not emit usage delta"
    );
    assert_eq!(res2.anomalies, 1, "must record token_category_regression");

    // Growth from new baseline: input 800 (delta +200), output 300 (delta +50)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 800, output_tokens = 300, last_seen = 30.0 WHERE session_id = 'ses_pr'",
            [],
        ).unwrap();
    }

    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 1, "growth from new baseline must emit delta");

    // Ingested delta should be exactly 200 in, 50 out
    let (delta_in, delta_out): (i64, i64) = ledger.connection().query_row(
        "SELECT input_tokens, output_tokens FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(delta_in, 200);
    assert_eq!(delta_out, 50);
}

#[test]
fn test_hermes_epoch_cost_disappearance_and_return() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_cost_disappear.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_cd', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, actual_cost_usd, first_seen, last_seen)
             VALUES ('ses_cd', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // Cost disappears: actual_cost_usd and estimated_cost_usd become NULL
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = NULL, estimated_cost_usd = NULL, last_seen = 20.0 WHERE session_id = 'ses_cd'",
            [],
        ).unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2.anomalies, 1,
        "cost disappearance must emit missing-cost anomaly"
    );
    assert_eq!(res2.inserted, 0, "disappeared cost must not emit delta");
    assert!(
        res2.anomaly_types
            .contains(&"provider_cost_missing".to_string())
    );

    // Repeated missing snapshot must be idempotent: 0 inserted, 0 new anomalies
    let res2_repeat = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res2_repeat.anomalies, 0,
        "repeated missing snapshot must not re-emit anomaly"
    );
    assert_eq!(res2_repeat.inserted, 0);

    let db_anoms: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM measurement_anomalies", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        db_anoms, 1,
        "only 1 anomaly recorded across repeated missing snapshots"
    );

    // Cost returns and grows to $0.018 (18,000 micros)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = 0.018, last_seen = 30.0 WHERE session_id = 'ses_cd'",
            [],
        ).unwrap();
    }

    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 1, "cost return/growth must emit delta");
    let delta_cost: i64 = ledger.connection().query_row(
        "SELECT provider_reported_cost_micros FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
        [],
        |r| r.get(0),
    ).unwrap();
    // When cost returns above last known value (10,000 micros), emit ONLY the difference: 18,000 - 10,000 = 8,000 micros!
    assert_eq!(
        delta_cost, 8000,
        "delta must be exactly difference above last known cumulative cost (8,000 micros)"
    );

    let total_cost: i64 = ledger
        .connection()
        .query_row(
            "SELECT sum(provider_reported_cost_micros) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        total_cost, 18000,
        "total cost in ledger must accurately sum to 18,000 micros"
    );
}

#[test]
fn test_hermes_epoch_cost_return_below_last_known_is_numeric_reset() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_cost_reset_below.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_crb', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, actual_cost_usd, first_seen, last_seen)
             VALUES ('ses_crb', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // Cost disappears (NULL)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = NULL, last_seen = 20.0 WHERE session_id = 'ses_crb'",
            [],
        ).unwrap();
    }
    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res2.anomalies, 1);
    assert_eq!(res2.inserted, 0);

    // Cost returns below last known value: $0.006 (6,000 < 10,000)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = 0.006, last_seen = 30.0 WHERE session_id = 'ses_crb'",
            [],
        ).unwrap();
    }
    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(
        res3.inserted, 0,
        "return below last known value must not emit reset delta"
    );
    assert_eq!(
        res3.anomalies, 1,
        "return below last known value must emit provider_cost_regression"
    );
    assert!(
        res3.anomaly_types
            .contains(&"provider_cost_regression".to_string())
    );

    // Growth from new baseline: $0.009 (9,000 micros -> delta +3,000)
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = 0.009, last_seen = 40.0 WHERE session_id = 'ses_crb'",
            [],
        ).unwrap();
    }
    let res4 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res4.inserted, 1, "growth from new baseline must emit delta");
    let delta_cost: i64 = ledger.connection().query_row(
        "SELECT provider_reported_cost_micros FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(delta_cost, 3000);

    let delta_event_id: String = ledger.connection().query_row(
        "SELECT source_event_id FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(
        delta_event_id.contains(":ce1cs1"),
        "event identity must reflect cost epoch 1 sequence 1, got {delta_event_id}"
    );
}

#[test]
fn test_hermes_cost_disappearance_restart_and_replay() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_cost_restart.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_rst', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, actual_cost_usd, first_seen, last_seen)
             VALUES ('ses_rst', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    {
        let mut ledger = Ledger::open(&ledger_path).unwrap();
        let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res1.inserted, 1);

        // Make cost disappear
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = NULL, last_seen = 20.0 WHERE session_id = 'ses_rst'",
            [],
        ).unwrap();

        let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res2.anomalies, 1);
        assert_eq!(res2.inserted, 0);

        // Replay on same ledger instance: strictly idempotent
        let res2_replay = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res2_replay.anomalies, 0);
        assert_eq!(res2_replay.inserted, 0);
    } // Ledger closes here

    // Simulate process restart: re-open ledger from disk
    {
        let mut ledger = Ledger::open(&ledger_path).unwrap();

        // Replay again after restart: state was persisted in checkpoint table
        let res2_after_restart =
            import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res2_after_restart.anomalies, 0);
        assert_eq!(res2_after_restart.inserted, 0);

        // Cost returns as 18,000 micros ($0.018)
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET actual_cost_usd = 0.018, last_seen = 30.0 WHERE session_id = 'ses_rst'",
            [],
        ).unwrap();

        let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res3.inserted, 1);

        let delta_cost: i64 = ledger.connection().query_row(
            "SELECT provider_reported_cost_micros FROM usage_events WHERE source_kind = 'hermes_snapshot_delta' ORDER BY observed_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(
            delta_cost, 8000,
            "delta cost after restart must be exactly 8,000 micros"
        );

        // Replay of cost return must be idempotent
        let res3_replay = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
        assert_eq!(res3_replay.inserted, 0);
        assert_eq!(res3_replay.anomalies, 0);

        let total_cost: i64 = ledger.connection().query_row(
            "SELECT sum(provider_reported_cost_micros) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(total_cost, 18000);
    }
}

#[test]
fn test_hermes_epoch_replay_and_anomaly_idempotence() {
    let dir = tempdir().unwrap();
    let state_db_path = dir.path().join("hermes_idempotence.db");

    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT);
             CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL, model TEXT NOT NULL, billing_provider TEXT DEFAULT '',
                 task TEXT DEFAULT '', api_call_count INTEGER DEFAULT 1, input_tokens INTEGER DEFAULT 0,
                 output_tokens INTEGER DEFAULT 0, cache_read_tokens INTEGER DEFAULT 0,
                 cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
                 estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT,
                 first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, task)
             );
             INSERT INTO sessions VALUES ('ses_idemp', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_idemp', 'openai/gpt-4o', '', 1000, 100, 0.010, 10.0, 10.0);",
        ).unwrap();
    }

    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    let res1 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res1.inserted, 1);

    // Induce regression
    {
        let conn = rusqlite::Connection::open(&state_db_path).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 500 WHERE session_id = 'ses_idemp'",
            [],
        )
        .unwrap();
    }

    let res2 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res2.anomalies, 1);

    let count_before: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM measurement_anomalies", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count_before, 1);

    // Replay exact same state
    let res3 = import_hermes_file(ledger.connection_mut(), &state_db_path).unwrap();
    assert_eq!(res3.inserted, 0);

    let count_after: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM measurement_anomalies", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        count_after, 1,
        "anomaly insertion must remain strictly idempotent on replay"
    );
}

fn parse_session_file(path: &Path) -> anyhow::Result<ParseResult> {
    let mut file = std::fs::File::open(path)?;
    let mut content = String::new();
    std::io::Read::read_to_string(&mut file, &mut content)?;
    parse_json_usage_str(&content, path, 0, HermesParserState::default())
}
