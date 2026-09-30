// SPDX-License-Identifier: Apache-2.0
use std::fs;
use tempfile::tempdir;
use tokentree_ledger::{Ledger, audit_prompt_leakage};

#[test]
fn test_doctor_audit_clean_database_has_zero_leaks() {
    let tmp = tempdir().unwrap();
    let ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    let audit = audit_prompt_leakage(ledger.connection(), None).unwrap();
    assert_eq!(audit.leaks_detected, 0);
    assert!(audit.leak_details.is_empty());
}

#[test]
fn test_doctor_audit_detects_canary_secret_in_database() {
    let tmp = tempdir().unwrap();
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    ledger
        .connection_mut()
        .execute(
            "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
             VALUES ('proj_canary', 'proj-canary', 'Canary Project', 'hash1', 'git', 1.0, '2026-03-30T10:00:00Z', '2026-03-30T10:00:00Z')",
            [],
        )
        .unwrap();

    // Deliberately inject a canary secret into notes table
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO notes (id, project_id, text, created_by, created_at)
             VALUES ('note_leak', 'proj_canary', 'Unauthorized dump: CANARY_SECRET_USER_PROMPT_KEY_XYZ', 'adversary', '2026-03-30T10:00:00Z')",
            [],
        )
        .unwrap();

    let audit = audit_prompt_leakage(ledger.connection(), None).unwrap();
    assert_eq!(audit.leaks_detected, 1);
    assert!(audit.leak_details[0].contains("notes.text"));
}

#[test]
fn test_doctor_audit_detects_prompt_payload_in_spool() {
    let tmp = tempdir().unwrap();
    let ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    let spool_dir = tmp.path().join("spool");
    fs::create_dir_all(&spool_dir).unwrap();
    let leak_spool_file = spool_dir.join("leaked_session.jsonl");

    // Deliberately write an unredacted prompt payload into spool
    fs::write(
        &leak_spool_file,
        r#"{"event":"session_start","prompt":"Classified internal system prompt do not leak"}"#,
    )
    .unwrap();

    let audit = audit_prompt_leakage(ledger.connection(), Some(&spool_dir)).unwrap();
    assert_eq!(audit.leaks_detected, 1);
    assert!(audit.leak_details[0].contains("matched forbidden raw prompt/completion field"));
}
