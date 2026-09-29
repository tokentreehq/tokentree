// SPDX-License-Identifier: Apache-2.0
use std::fs::{self, OpenOptions};
use std::io::Write;
use tempfile::tempdir;
use tokentree_codex::import_codex_file;
use tokentree_ledger::Ledger;

#[test]
fn test_durable_checkpoint_append_resume_no_duplicates() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session.jsonl");

    // 1. Initial write: 2 turns
    let line1 = "{\"type\":\"turn/started\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n";
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r1\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":100,\"output_tokens\":20},\"timestamp\":\"2026-03-30T10:00:01Z\"}\n";
    fs::write(&session_file, format!("{line1}{line2}")).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 1);
    assert_eq!(res1.duplicates, 0);
    assert_eq!(res1.start_offset, 0);
    let offset_after_1 = res1.end_offset;
    assert!(offset_after_1 > 0);

    // Assert initial usage
    let agg1 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg1.input, 100);
    assert_eq!(agg1.output, 20);
    assert_eq!(agg1.requests, 1);

    // 2. Re-import without changes: should resume from offset and insert 0 duplicates
    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 0);
    assert_eq!(res2.start_offset, offset_after_1);
    assert_eq!(res2.end_offset, offset_after_1);

    // 3. Append new line: should parse ONLY new line, no duplicates
    let line3 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r2\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":150,\"output_tokens\":30},\"timestamp\":\"2026-03-30T10:00:02Z\"}\n";
    let mut file = OpenOptions::new().append(true).open(&session_file).unwrap();
    file.write_all(line3.as_bytes()).unwrap();
    drop(file);

    let res3 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res3.inserted, 1);
    assert_eq!(res3.duplicates, 0);
    assert_eq!(res3.start_offset, offset_after_1);
    assert!(res3.end_offset > offset_after_1);

    // Assert exact cumulative usage
    let agg2 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg2.input, 250); // 100 + 150
    assert_eq!(agg2.output, 50); // 20 + 30
    assert_eq!(agg2.requests, 2);
}

#[test]
fn test_partial_final_line_not_committed_until_complete() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_partial.jsonl");

    // Complete line 1 + incomplete line 2 (no trailing newline)
    let line1 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":50,\"output_tokens\":10}}\n";
    let line2_partial = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r2\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":75"; // cut off!

    fs::write(&session_file, format!("{line1}{line2_partial}")).unwrap();

    // Import 1: should ONLY commit line 1
    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 1);
    assert_eq!(res1.end_offset, line1.len() as u64);

    let agg1 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg1.input, 50);
    assert_eq!(agg1.requests, 1);

    // Complete line 2 by appending the rest with a newline
    let line2_rest = ",\"output_tokens\":15}}\n";
    let mut file = OpenOptions::new().append(true).open(&session_file).unwrap();
    file.write_all(line2_rest.as_bytes()).unwrap();
    drop(file);

    // Import 2: should resume from line1 offset and commit line 2
    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.inserted, 1);
    assert_eq!(res2.start_offset, line1.len() as u64);

    let agg2 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg2.input, 125); // 50 + 75
    assert_eq!(agg2.output, 25); // 10 + 15
    assert_eq!(agg2.requests, 2);
}

#[test]
fn test_file_truncation_resets_offset_safely() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_trunc.jsonl");

    // Write 2 lines
    let line1 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":50,\"output_tokens\":10}}\n";
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r2\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":50,\"output_tokens\":10}}\n";
    fs::write(&session_file, format!("{line1}{line2}")).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 2);

    // Truncate file to only line 1
    fs::write(&session_file, line1).unwrap();

    // Import again: should detect file size < last_offset and reset start_offset to 0
    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.start_offset, 0);
    // Line 1 is an existing event_hash so it counts as duplicate, not inserted again
    assert_eq!(res2.duplicates, 1);
    assert_eq!(res2.inserted, 0);
}

#[test]
fn test_file_rotation_resets_offset_safely() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_rot.jsonl");

    // Original file
    let orig = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r_orig\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":100,\"output_tokens\":20}}\n";
    fs::write(&session_file, orig).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 1);

    // Rotate file at same path: completely different content
    let rotated = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r_new\",\"session_id\":\"s_new\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":200,\"output_tokens\":40}}\n";
    fs::write(&session_file, rotated).unwrap();

    // Import again: should detect file_hash mismatch and reset start_offset to 0
    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.start_offset, 0);
    assert_eq!(res2.inserted, 1); // inserts r_new

    let agg = ledger.aggregate_usage().unwrap();
    assert_eq!(agg.input, 300); // 100 + 200
    assert_eq!(agg.output, 60); // 20 + 40
    assert_eq!(agg.requests, 2);
}

#[test]
fn test_checkpoint_persists_turn_and_session_id_when_token_event_omits_them() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_turn_ctx.jsonl");

    // Line 1 establishes turn and session context
    let line1 = "{\"type\":\"turn/started\",\"turn_id\":\"turn_ctx_42\",\"session_id\":\"ses_ctx_alpha\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n";
    fs::write(&session_file, line1).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 0); // turn boundary only, no token usage yet

    // Line 2 arrives after restart without turn_id or session_id
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_no_ctx\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":120,\"output_tokens\":30},\"timestamp\":\"2026-03-30T10:00:01Z\"}\n";
    let mut file = OpenOptions::new().append(true).open(&session_file).unwrap();
    file.write_all(line2.as_bytes()).unwrap();
    drop(file);

    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.inserted, 1);

    // Verify usage event inherited active turn_id and session_id from checkpoint state
    let (s_id, t_id): (String, Option<String>) = ledger
        .connection_mut()
        .query_row(
            "SELECT session_id, turn_id FROM usage_events WHERE request_id = 'req_no_ctx'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert!(s_id.starts_with("ses_"));
    assert!(t_id.is_some());
    assert!(t_id.unwrap().starts_with("turn_"));

    let agg = ledger.aggregate_usage().unwrap();
    assert_eq!(agg.input, 120);
    assert_eq!(agg.output, 30);
    assert_eq!(agg.requests, 1);
}

#[test]
fn test_checkpoint_cumulative_counter_delta_across_restarts() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_cumulative.jsonl");

    // Turn start + first cumulative counter (N = 100 input, 30 output)
    let line1 = "{\"type\":\"turn/started\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n";
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"c1\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":100,\"output_tokens\":30},\"timestamp\":\"2026-03-30T10:00:01Z\"}\n";
    fs::write(&session_file, format!("{line1}{line2}")).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 1);

    let agg1 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg1.input, 100);
    assert_eq!(agg1.output, 30);
    assert_eq!(agg1.requests, 1);

    // Second cumulative counter (N+1 = 150 input, 45 output) after restart
    let line3 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"c2\",\"turn_id\":\"t1\",\"session_id\":\"s1\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":150,\"output_tokens\":45},\"timestamp\":\"2026-03-30T10:00:02Z\"}\n";
    let mut file = OpenOptions::new().append(true).open(&session_file).unwrap();
    file.write_all(line3.as_bytes()).unwrap();
    drop(file);

    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.inserted, 1);

    // Assert exact delta was computed (50 input, 15 output) and total is 150 input, 45 output
    let agg2 = ledger.aggregate_usage().unwrap();
    assert_eq!(agg2.input, 150);
    assert_eq!(agg2.output, 45);
    assert_eq!(agg2.requests, 2);
}

#[test]
fn test_checkpoint_parent_agent_boundary_before_restart_and_child_usage_after() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_agent_boundary.jsonl");

    // Parent agent establishes boundary before restart
    let line1 = "{\"type\":\"turn/started\",\"turn_id\":\"t_sub\",\"session_id\":\"s_sub\",\"agent_id\":\"worker_agent\",\"parent_agent_id\":\"orchestrator_agent\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n";
    fs::write(&session_file, line1).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 0);

    // Child usage arrives after restart without repeating agent_id / parent_agent_id
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_child\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":80,\"output_tokens\":25},\"timestamp\":\"2026-03-30T10:00:01Z\"}\n";
    let mut file = OpenOptions::new().append(true).open(&session_file).unwrap();
    file.write_all(line2.as_bytes()).unwrap();
    drop(file);

    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res2.inserted, 1);

    let (agent, parent_agent): (Option<String>, Option<String>) = ledger
        .connection_mut()
        .query_row(
            "SELECT agent_id, parent_agent_id FROM usage_events WHERE request_id = 'req_child'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(agent.as_deref(), Some("worker_agent"));
    assert_eq!(parent_agent.as_deref(), Some("orchestrator_agent"));
}

#[test]
fn test_line_by_line_restart_matches_uninterrupted_import_byte_for_byte() {
    let lines = vec![
        "{\"type\":\"session/started\",\"session_id\":\"ses_uninterrupted\",\"protocol_version\":\"v1\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n",
        "{\"type\":\"turn/started\",\"turn_id\":\"t1\",\"session_id\":\"ses_uninterrupted\",\"agent_id\":\"agent_main\",\"timestamp\":\"2026-03-30T10:00:01Z\"}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_1\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":20,\"output_tokens\":30,\"reasoning_tokens\":10},\"timestamp\":\"2026-03-30T10:00:02Z\"}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_2\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":150,\"cached_input_tokens\":30,\"output_tokens\":40,\"reasoning_tokens\":15},\"timestamp\":\"2026-03-30T10:00:03Z\"}\n",
        "{\"type\":\"turn/completed\",\"turn_id\":\"t1\",\"timestamp\":\"2026-03-30T10:00:04Z\"}\n",
        "{\"type\":\"turn/started\",\"turn_id\":\"t2\",\"session_id\":\"ses_uninterrupted\",\"agent_id\":\"agent_sub\",\"parent_agent_id\":\"agent_main\",\"timestamp\":\"2026-03-30T10:00:05Z\"}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_3\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":350,\"cached_input_tokens\":60,\"output_tokens\":90,\"reasoning_tokens\":30},\"timestamp\":\"2026-03-30T10:00:06Z\"}\n",
        "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"req_4\",\"model\":\"o3-mini\",\"cumulative_token_usage\":{\"input_tokens\":420,\"cached_input_tokens\":75,\"output_tokens\":115,\"reasoning_tokens\":40},\"timestamp\":\"2026-03-30T10:00:07Z\"}\n",
        "{\"type\":\"turn/completed\",\"turn_id\":\"t2\",\"timestamp\":\"2026-03-30T10:00:08Z\"}\n",
    ];

    let full_content = lines.concat();

    // 1. Uninterrupted import
    let tmp_a = tempdir().unwrap();
    let mut ledger_uninterrupted = Ledger::open(tmp_a.path().join("ledger.db")).unwrap();
    let file_a = tmp_a.path().join("session_a.jsonl");
    fs::write(&file_a, &full_content).unwrap();
    import_codex_file(ledger_uninterrupted.connection_mut(), &file_a).unwrap();

    // 2. Line-by-line resumed import with restart after every line
    let tmp_b = tempdir().unwrap();
    let mut ledger_resumed = Ledger::open(tmp_b.path().join("ledger.db")).unwrap();
    let file_b = tmp_b.path().join("session_b.jsonl");

    let mut current_b = String::new();
    for line in &lines {
        current_b.push_str(line);
        fs::write(&file_b, &current_b).unwrap();
        import_codex_file(ledger_resumed.connection_mut(), &file_b).unwrap();
    }

    // Assert exact equality of aggregate usage
    let agg_uninterrupted = ledger_uninterrupted.aggregate_usage().unwrap();
    let agg_resumed = ledger_resumed.aggregate_usage().unwrap();

    assert_eq!(agg_uninterrupted, agg_resumed);

    // Assert exact equality of usage_events row count
    let count_a: i64 = ledger_uninterrupted
        .connection_mut()
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    let count_b: i64 = ledger_resumed
        .connection_mut()
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_a, count_b);
    assert!(count_a > 0);

    // Assert all event fields match in order
    let events_a: Vec<(String, i64, i64, i64)> = ledger_uninterrupted
        .connection_mut()
        .prepare("SELECT request_id, input_tokens, cached_input_tokens, output_tokens FROM usage_events ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    let events_b: Vec<(String, i64, i64, i64)> = ledger_resumed
        .connection_mut()
        .prepare("SELECT request_id, input_tokens, cached_input_tokens, output_tokens FROM usage_events ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(events_a, events_b);
}

#[test]
fn test_idempotent_anomaly_ingestion_on_replay() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();
    let session_file = tmp.path().join("session_anom.jsonl");

    // Line 1 has an unsupported protocol version -> triggers an anomaly
    let line1 = "{\"type\":\"session_meta\",\"protocol_version\":\"v999.0_future\",\"timestamp\":\"2026-03-30T10:00:00Z\"}\n";
    // Line 2 has valid usage
    let line2 = "{\"type\":\"thread/tokenUsage/updated\",\"request_id\":\"r_ok\",\"session_id\":\"s_anom\",\"model\":\"o3-mini\",\"token_usage\":{\"input_tokens\":50,\"output_tokens\":10},\"timestamp\":\"2026-03-30T10:00:01Z\"}\n";
    fs::write(&session_file, format!("{line1}{line2}")).unwrap();

    let res1 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    assert_eq!(res1.inserted, 1);

    let anom_count_1: i64 = ledger
        .connection_mut()
        .query_row("SELECT count(*) FROM measurement_anomalies", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(anom_count_1, 1);

    let agg1 = ledger.aggregate_usage().unwrap();

    // Simulate replay (e.g. after rotation, truncation, or manual re-ingest with checkpoint reset)
    ledger
        .connection_mut()
        .execute("UPDATE ingestion_checkpoints SET last_offset = 0", [])
        .unwrap();

    let res2 = import_codex_file(ledger.connection_mut(), &session_file).unwrap();
    // Second pass should see existing anomalies and duplicate usage, inserting 0 new ones
    assert_eq!(res2.inserted, 0);
    assert_eq!(res2.duplicates, 1);

    let anom_count_2: i64 = ledger
        .connection_mut()
        .query_row("SELECT count(*) FROM measurement_anomalies", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        anom_count_2, 1,
        "Anomalies must not be duplicated on replay"
    );

    let agg2 = ledger.aggregate_usage().unwrap();
    assert_eq!(
        agg1, agg2,
        "Aggregate usage must remain identical across replay"
    );
}
