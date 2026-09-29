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
