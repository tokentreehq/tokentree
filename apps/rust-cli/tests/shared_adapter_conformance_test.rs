// SPDX-License-Identifier: Apache-2.0
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use tokentree_claude::parse_session as parse_claude_session;
use tokentree_codex::import_codex_file;
use tokentree_grok::import_grok_file;
use tokentree_hermes::import_hermes_file;
use tokentree_ledger::{Ledger, audit_prompt_leakage};

#[test]
fn test_conformance_idempotence_all_adapters() {
    let dir = tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // 1. Claude Idempotence
    let claude_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parsers/claude/public-small-v2.1.80.jsonl");
    let claude_res = parse_claude_session(&claude_fixture).unwrap();
    let initial_count = claude_res.observations.len();
    assert!(initial_count > 0, "Claude fixture should have observations");

    let ing1 = ledger.ingest(claude_res.observations.clone()).unwrap();
    assert!(ing1.inserted > 0, "Claude first ingestion inserts records");
    let ing2 = ledger.ingest(claude_res.observations).unwrap();
    assert_eq!(
        ing2.inserted, 0,
        "Claude re-ingestion must insert 0 records"
    );

    // 2. Codex Idempotence
    let codex_file = dir.path().join("codex_session.jsonl");
    fs::write(
        &codex_file,
        r#"{"type":"turn/started","turn_id":"turn_1","session_id":"codex_ses_conf","timestamp":"2026-09-30T12:00:00Z"}
{"type":"thread/tokenUsage/updated","request_id":"req_cdx_conf_1","model":"o3-mini","token_usage":{"input_tokens":100,"output_tokens":20},"timestamp":"2026-09-30T12:00:02Z"}
"#,
    ).unwrap();

    let c_res1 = import_codex_file(ledger.connection_mut(), &codex_file).unwrap();
    assert_eq!(c_res1.inserted, 1);
    let c_res2 = import_codex_file(ledger.connection_mut(), &codex_file).unwrap();
    assert_eq!(c_res2.inserted, 0, "Codex re-import must insert 0 records");

    // 3. Grok Idempotence
    let grok_dir = dir.path().join("grok_conf");
    fs::create_dir_all(&grok_dir).unwrap();
    let grok_file = grok_dir.join("usage.json");
    fs::write(
        &grok_file,
        r#"{
  "sessionId": "grok_ses_conf",
  "session": { "turnCount": 1, "inputTokens": 200, "outputTokens": 30 },
  "turns": [
    { "turnNumber": 1, "inputTokens": 200, "outputTokens": 30, "modelCalls": 1 }
  ]
}"#,
    )
    .unwrap();

    let g_res1 = import_grok_file(ledger.connection_mut(), &grok_file).unwrap();
    assert_eq!(g_res1.inserted, 1);
    let g_res2 = import_grok_file(ledger.connection_mut(), &grok_file).unwrap();
    assert_eq!(g_res2.inserted, 0, "Grok re-import must insert 0 records");

    // 4. Hermes Idempotence
    let hermes_db = dir.path().join("hermes_conf.db");
    {
        let conn = rusqlite::Connection::open(&hermes_db).unwrap();
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
             INSERT INTO sessions VALUES ('hermes_ses_conf', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('hermes_ses_conf', 'liquid/lfm-2.5-2.6b:free', 'main', 300, 40, 0.0, 100.0, 100.0);",
        ).unwrap();
    }

    let h_res1 = import_hermes_file(ledger.connection_mut(), &hermes_db).unwrap();
    assert_eq!(h_res1.inserted, 1);
    let h_res2 = import_hermes_file(ledger.connection_mut(), &hermes_db).unwrap();
    assert_eq!(h_res2.inserted, 0, "Hermes re-import must insert 0 records");

    // Unified Reconciliation
    let recon = ledger.reconcile().unwrap();
    assert_eq!(
        recon.duplicate_request_ids, 0,
        "Zero duplicate request IDs after multi-adapter imports"
    );
}

#[test]
fn test_conformance_append_and_growth() {
    let dir = tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // Codex Append Turn
    let codex_file = dir.path().join("codex_append.jsonl");
    fs::write(
        &codex_file,
        r#"{"type":"turn/started","turn_id":"turn_1","session_id":"ses_cdx_app","timestamp":"2026-09-30T12:00:00Z"}
{"type":"thread/tokenUsage/updated","request_id":"req_cdx_app_1","model":"o3-mini","token_usage":{"input_tokens":100,"output_tokens":20},"timestamp":"2026-09-30T12:00:02Z"}
"#,
    ).unwrap();

    let c_res1 = import_codex_file(ledger.connection_mut(), &codex_file).unwrap();
    assert_eq!(c_res1.inserted, 1);

    // Append turn 2
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&codex_file)
        .unwrap();
    use std::io::Write;
    writeln!(file, r#"{{"type":"turn/started","turn_id":"turn_2","session_id":"ses_cdx_app","timestamp":"2026-09-30T12:01:00Z"}}"#).unwrap();
    writeln!(file, r#"{{"type":"thread/tokenUsage/updated","request_id":"req_cdx_app_2","model":"o3-mini","token_usage":{{"input_tokens":150,"output_tokens":30}},"timestamp":"2026-09-30T12:01:02Z"}}"#).unwrap();
    drop(file);

    let c_res2 = import_codex_file(ledger.connection_mut(), &codex_file).unwrap();
    assert_eq!(
        c_res2.inserted, 1,
        "Appended turn in Codex must insert exactly 1 new event"
    );

    // Grok Append Turn
    let grok_dir = dir.path().join("grok_app");
    fs::create_dir_all(&grok_dir).unwrap();
    let grok_file = grok_dir.join("usage.json");
    fs::write(
        &grok_file,
        r#"{
  "sessionId": "ses_grk_app",
  "session": { "turnCount": 1, "inputTokens": 100, "outputTokens": 10 },
  "turns": [
    { "turnNumber": 1, "inputTokens": 100, "outputTokens": 10, "modelCalls": 1 }
  ]
}"#,
    )
    .unwrap();

    let g_res1 = import_grok_file(ledger.connection_mut(), &grok_file).unwrap();
    assert_eq!(g_res1.inserted, 1);

    // Update Grok usage with turn 2
    fs::write(
        &grok_file,
        r#"{
  "sessionId": "ses_grk_app",
  "session": { "turnCount": 2, "inputTokens": 250, "outputTokens": 25 },
  "turns": [
    { "turnNumber": 1, "inputTokens": 100, "outputTokens": 10, "modelCalls": 1 },
    { "turnNumber": 2, "inputTokens": 150, "outputTokens": 15, "modelCalls": 1 }
  ]
}"#,
    )
    .unwrap();

    let g_res2 = import_grok_file(ledger.connection_mut(), &grok_file).unwrap();
    assert_eq!(
        g_res2.inserted, 1,
        "Appended turn in Grok must insert exactly 1 new event"
    );

    // Hermes Row Delta Growth
    let hermes_db = dir.path().join("hermes_growth.db");
    {
        let conn = rusqlite::Connection::open(&hermes_db).unwrap();
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
             INSERT INTO sessions VALUES ('ses_hrm_app', NULL);
             INSERT INTO session_model_usage (session_id, model, task, input_tokens, output_tokens, estimated_cost_usd, first_seen, last_seen)
             VALUES ('ses_hrm_app', 'liquid/lfm-2.5-2.6b:free', 'main', 500, 50, 0.0, 100.0, 100.0);",
        ).unwrap();
    }

    let h_res1 = import_hermes_file(ledger.connection_mut(), &hermes_db).unwrap();
    assert_eq!(h_res1.inserted, 1);

    // Row tokens grow in Hermes
    {
        let conn = rusqlite::Connection::open(&hermes_db).unwrap();
        conn.execute(
            "UPDATE session_model_usage SET input_tokens = 800, output_tokens = 90 WHERE session_id = 'ses_hrm_app'",
            [],
        ).unwrap();
    }

    let h_res2 = import_hermes_file(ledger.connection_mut(), &hermes_db).unwrap();
    assert_eq!(
        h_res2.inserted, 1,
        "Hermes token growth must emit 1 delta event"
    );

    // Verify token totals for Hermes: 500 + 50 + 300 + 40 = 890
    let total_hrm_tokens: i64 = ledger
        .connection()
        .query_row(
            "SELECT sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) FROM usage_events WHERE adapter = 'hermes'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total_hrm_tokens, 890);

    let recon = ledger.reconcile().unwrap();
    assert_eq!(recon.duplicate_request_ids, 0);
}

#[test]
fn test_conformance_privacy_and_prompt_leakage_audit() {
    let dir = tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // 1. Ingest telemetry from all adapters
    let claude_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parsers/claude/public-small-v2.1.80.jsonl");
    let claude_res = parse_claude_session(&claude_fixture).unwrap();
    ledger.ingest(claude_res.observations).unwrap();

    let codex_file = dir.path().join("codex_priv.jsonl");
    fs::write(
        &codex_file,
        r#"{"type":"turn/started","turn_id":"turn_1","session_id":"ses_priv","timestamp":"2026-09-30T12:00:00Z"}
{"type":"thread/tokenUsage/updated","request_id":"req_cdx_priv_1","model":"o3-mini","token_usage":{"input_tokens":100,"output_tokens":20},"timestamp":"2026-09-30T12:00:02Z"}
"#,
    ).unwrap();
    import_codex_file(ledger.connection_mut(), &codex_file).unwrap();

    let grok_dir = dir.path().join("grok_priv");
    fs::create_dir_all(&grok_dir).unwrap();
    let grok_file = grok_dir.join("usage.json");
    fs::write(
        &grok_file,
        r#"{
  "sessionId": "ses_grk_priv",
  "session": { "turnCount": 1, "inputTokens": 100, "outputTokens": 10 },
  "turns": [
    { "turnNumber": 1, "inputTokens": 100, "outputTokens": 10 }
  ]
}"#,
    )
    .unwrap();
    import_grok_file(ledger.connection_mut(), &grok_file).unwrap();

    // 2. Audit prompt leakage
    let audit = audit_prompt_leakage(ledger.connection(), None).unwrap();
    assert_eq!(
        audit.leaks_detected, 0,
        "No raw prompt leaks allowed in turns table"
    );
    assert!(
        audit.leak_details.is_empty(),
        "Ledger prompt leakage audit must pass cleanly"
    );

    // 3. Verify turns table prompt_storage_mode is strictly fingerprint_only
    let mut stmt = ledger
        .connection()
        .prepare("SELECT prompt_storage_mode FROM turns")
        .unwrap();
    let modes: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    for mode in modes {
        assert_eq!(mode, "fingerprint_only");
    }
}

#[test]
fn test_conformance_corrupted_and_truncated_records() {
    let dir = tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.db");
    let mut ledger = Ledger::open(&ledger_path).unwrap();

    // 1. Codex with trailing truncated line
    let codex_file = dir.path().join("codex_trunc.jsonl");
    fs::write(
        &codex_file,
        r#"{"type":"turn/started","turn_id":"turn_1","session_id":"ses_cdx_trunc","timestamp":"2026-09-30T12:00:00Z"}
{"type":"thread/tokenUsage/updated","request_id":"req_cdx_valid","model":"o3-mini","token_usage":{"input_tokens":100,"output_tokens":20},"timestamp":"2026-09-30T12:00:02Z"}
{"type":"thread/tokenUsage/updated","request_id":"req_cdx_tru
"#,
    ).unwrap();

    let c_res = import_codex_file(ledger.connection_mut(), &codex_file).unwrap();
    assert_eq!(
        c_res.inserted, 1,
        "Valid records in truncated file must be imported"
    );
    assert!(
        c_res.anomalies >= 1,
        "Truncated line must register an anomaly"
    );

    // 2. Grok with corrupt JSON records anomaly gracefully and inserts 0 events
    let grok_dir = dir.path().join("grok_corrupt");
    fs::create_dir_all(&grok_dir).unwrap();
    let grok_file = grok_dir.join("usage.json");
    fs::write(
        &grok_file,
        r#"{"sessionId": "corrupt_session", "session": { invalid_json_here "#,
    )
    .unwrap();

    let g_res = import_grok_file(ledger.connection_mut(), &grok_file).unwrap();
    assert_eq!(
        g_res.inserted, 0,
        "Corrupted Grok file must insert 0 events"
    );
    assert!(
        g_res.anomalies >= 1,
        "Corrupted Grok file must register an anomaly"
    );

    // 3. Hermes with non-sqlite garbage safely inserts 0 records
    let hermes_corrupt = dir.path().join("hermes_corrupt.db");
    fs::write(
        &hermes_corrupt,
        b"NOT_A_SQLITE_DATABASE_HEADER_JUST_GARBAGE_BYTES",
    )
    .unwrap();

    let h_res = import_hermes_file(ledger.connection_mut(), &hermes_corrupt).unwrap();
    assert_eq!(
        h_res.inserted, 0,
        "Corrupted Hermes DB must insert 0 records"
    );

    // 4. Ledger integrity check and foreign key invariant check
    let integrity = ledger.integrity_check().unwrap();
    assert_eq!(
        integrity, "ok",
        "Ledger integrity must remain intact after malformed encounters"
    );

    let fk_count: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check()", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk_count, 0, "No foreign key violations allowed in ledger");
}
