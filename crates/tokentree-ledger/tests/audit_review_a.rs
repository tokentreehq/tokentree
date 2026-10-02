// SPDX-License-Identifier: Apache-2.0
//! Adversarial tests for the PR-review blockers (review lane A):
//! 1. Cross-batch truth-ladder replacement (supersede, atomically, append-only).
//! 2. Backward-compatible canonical-identity migration/replay (v1 + v2 schemes,
//!    zero duplicate accounting).
//! 3. Allowlisted, backup-protected, versioned source_kind migration.

use rusqlite::params;
use tokentree_core::{
    MeasurementSource, TokenUsage, UsageObservation, canonical_source_kind, sha256_hex, source_kind,
};
use tokentree_ledger::{
    Ledger, count_source_kind_backup_rows, ingest_observations_tx, migrate_source_kind_vocabulary,
    restore_source_kind_backup, session_stable_id, stable_id,
};

fn obs(
    session: &str,
    turn: Option<&str>,
    source: MeasurementSource,
    request: &str,
    input: u64,
    output: u64,
) -> UsageObservation {
    UsageObservation {
        adapter: "claude".into(),
        source,
        source_subtype: None,
        source_event_id: Some(format!("evt-{request}")),
        provider_session_id: session.into(),
        request_id: Some(request.into()),
        turn_id: turn.map(str::to_string),
        agent_id: None,
        parent_agent_id: None,
        source_timestamp: Some("2026-10-02T00:00:00Z".into()),
        observed_at: "2026-10-02T00:00:00Z".into(),
        model: Some("claude-sonnet-4-6".into()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(input),
            cached_input_tokens: None,
            cache_write_tokens: None,
            output_tokens: Some(output),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: None,
        source_path: "test".into(),
        source_offset: 0,
        adapter_version: "test".into(),
        parser_version: "test".into(),
    }
}

fn poison_obs() -> UsageObservation {
    let mut o = obs(
        "s9",
        None,
        MeasurementSource::TranscriptRequest,
        "poison",
        1,
        1,
    );
    // i64::try_from fails inside ingest -> the whole batch must roll back.
    o.source_offset = u64::MAX;
    o
}

fn event_row_count(ledger: &Ledger) -> i64 {
    ledger
        .connection()
        .query_row("SELECT count(*) FROM usage_events", [], |r| r.get(0))
        .unwrap()
}

fn active_row_count(ledger: &Ledger) -> i64 {
    ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE superseded_by IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn superseded_row_count(ledger: &Ledger) -> i64 {
    ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE superseded_by IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

/// Simulate a row written by a pre-H6 binary: v1 identity scheme, no
/// `logical_event_hash`, old trigger regime never touched it.
fn insert_legacy_v1_row(ledger: &mut Ledger, ob: &UsageObservation) {
    let v1_identity = ob.canonical_identity_v1();
    let session_id = session_stable_id(&ob.adapter, &ob.provider_session_id);
    ledger
        .connection_mut()
        .execute(
            "INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,source_path,started_at)
             VALUES(?,?,?,?,?)",
            params![
                session_id,
                ob.adapter,
                ob.provider_session_id,
                ob.source_path,
                ob.observed_at
            ],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO usage_events(
               id,adapter,source_kind,source_event_id,session_id,request_id,
               source_timestamp,observed_at,ingested_at,model,
               input_tokens,output_tokens,source_path,source_offset,
               event_hash,adapter_version,parser_version
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                stable_id("evt", &v1_identity),
                ob.adapter,
                canonical_source_kind(ob),
                ob.source_event_id,
                session_id,
                ob.request_id,
                ob.source_timestamp,
                ob.observed_at,
                ob.observed_at,
                ob.model,
                ob.usage.input_tokens.map(|v| v as i64),
                ob.usage.output_tokens.map(|v| v as i64),
                ob.source_path,
                0i64,
                sha256_hex(v1_identity.as_bytes()),
                ob.adapter_version,
                ob.parser_version,
            ],
        )
        .unwrap();
}

/// Insert a row with an arbitrary (possibly legacy/garbage) source_kind,
/// simulating a database written by an older or foreign binary.
fn insert_raw_event(ledger: &mut Ledger, suffix: &str, kind: &str) {
    let id = format!("raw-{suffix}");
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO usage_events(
               id,adapter,source_kind,observed_at,ingested_at,source_path,
               source_offset,event_hash,adapter_version,parser_version
             ) VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                id,
                "claude",
                kind,
                "2026-10-02T00:00:00Z",
                "2026-10-02T00:00:00Z",
                "test",
                0i64,
                sha256_hex(id.as_bytes()),
                "test",
                "test",
            ],
        )
        .unwrap();
}

fn clear_migration_version(ledger: &mut Ledger) {
    // Simulate a database that never ran the v1 vocabulary migration.
    ledger
        .connection_mut()
        .execute(
            "DELETE FROM applied_migrations WHERE name = 'normalize_source_kind_vocabulary'",
            [],
        )
        .unwrap();
}

fn kind_of(ledger: &Ledger, id: &str) -> String {
    ledger
        .connection()
        .query_row(
            "SELECT source_kind FROM usage_events WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

// ---------------------------------------------------------------------------
// 1. Cross-batch truth-ladder replacement
// ---------------------------------------------------------------------------

#[test]
fn truth_ladder_supersedes_across_batches() {
    let mut ledger = Ledger::open_memory().unwrap();
    let transcript = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    let telemetry = obs(
        "s1",
        Some("t1"),
        MeasurementSource::OfficialTelemetry,
        "r1",
        12,
        6,
    );

    let s1 = ledger.ingest(vec![transcript]).unwrap();
    assert_eq!((s1.inserted, s1.duplicates, s1.superseded), (1, 0, 0));
    assert_eq!(ledger.aggregate_usage().unwrap().input, 10);

    let s2 = ledger.ingest(vec![telemetry]).unwrap();
    assert_eq!((s2.inserted, s2.duplicates, s2.superseded), (1, 0, 1));

    // Aggregates see exactly the telemetry row: no double counting.
    let agg = ledger.aggregate_usage().unwrap();
    assert_eq!(agg.input, 12);
    assert_eq!(agg.output, 6);
    assert_eq!(agg.requests, 1);

    // Audit trail: both rows present, tombstone links old -> new.
    assert_eq!(event_row_count(&ledger), 2);
    assert_eq!(superseded_row_count(&ledger), 1);
    let linked: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events o
             JOIN usage_events n ON n.id = o.superseded_by
             WHERE o.superseded_by IS NOT NULL AND n.superseded_by IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);

    // Reconcile must not flag the superseded pair as a duplicate request.
    assert_eq!(ledger.reconcile().unwrap().duplicate_request_ids, 0);
}

#[test]
fn same_precedence_reingest_is_idempotent() {
    let mut ledger = Ledger::open_memory().unwrap();
    let first = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    let replay = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    ledger.ingest(vec![first]).unwrap();
    let s = ledger.ingest(vec![replay]).unwrap();
    assert_eq!((s.inserted, s.duplicates, s.superseded), (0, 1, 0));
    assert_eq!(event_row_count(&ledger), 1);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 10);
}

#[test]
fn lower_precedence_arriving_later_does_not_supersede() {
    let mut ledger = Ledger::open_memory().unwrap();
    let telemetry = obs(
        "s1",
        Some("t1"),
        MeasurementSource::OfficialTelemetry,
        "r1",
        12,
        6,
    );
    let transcript = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    ledger.ingest(vec![telemetry]).unwrap();
    let s = ledger.ingest(vec![transcript]).unwrap();
    assert_eq!((s.inserted, s.duplicates, s.superseded), (0, 1, 0));
    // The authoritative row keeps its rank; aggregates untouched.
    assert_eq!(superseded_row_count(&ledger), 0);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 12);
}

#[test]
fn supersede_chain_three_levels_keeps_one_active() {
    let mut ledger = Ledger::open_memory().unwrap();
    let mk = |source, input| obs("s1", Some("t1"), source, "r1", input, 1);
    ledger
        .ingest(vec![mk(MeasurementSource::TranscriptRequest, 10)])
        .unwrap();
    let s2 = ledger
        .ingest(vec![mk(MeasurementSource::ProviderFields, 11)])
        .unwrap();
    assert_eq!(s2.superseded, 1);
    let s3 = ledger
        .ingest(vec![mk(MeasurementSource::OfficialTelemetry, 12)])
        .unwrap();
    assert_eq!(s3.superseded, 1);

    assert_eq!(event_row_count(&ledger), 3);
    assert_eq!(active_row_count(&ledger), 1);
    assert_eq!(superseded_row_count(&ledger), 2);
    // Chain links: A -> B -> C.
    let chain: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events o
             JOIN usage_events n ON n.id = o.superseded_by
             WHERE o.superseded_by IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(chain, 2);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 12);
}

#[test]
fn replay_after_supersede_does_not_reinsert() {
    let mut ledger = Ledger::open_memory().unwrap();
    let transcript = || {
        obs(
            "s1",
            Some("t1"),
            MeasurementSource::TranscriptRequest,
            "r1",
            10,
            5,
        )
    };
    let telemetry = || {
        obs(
            "s1",
            Some("t1"),
            MeasurementSource::OfficialTelemetry,
            "r1",
            12,
            6,
        )
    };
    ledger.ingest(vec![transcript()]).unwrap();
    ledger.ingest(vec![telemetry()]).unwrap();
    // Replaying the superseding observation is idempotent ...
    let s = ledger.ingest(vec![telemetry()]).unwrap();
    assert_eq!((s.inserted, s.duplicates, s.superseded), (0, 1, 0));
    // ... and so is replaying the original lower-precedence one.
    let s = ledger.ingest(vec![transcript()]).unwrap();
    assert_eq!((s.inserted, s.duplicates, s.superseded), (0, 1, 0));
    assert_eq!(event_row_count(&ledger), 2);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 12);
}

#[test]
fn supersede_is_atomic_with_its_batch() {
    let mut ledger = Ledger::open_memory().unwrap();
    let transcript = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    ledger.ingest(vec![transcript]).unwrap();

    // Drive two ingest calls inside ONE outer transaction: stage a supersede,
    // then fail the batch. The rollback must undo the staged supersede too.
    let tx = ledger.connection_mut().transaction().unwrap();
    let staged = ingest_observations_tx(
        &tx,
        &[obs(
            "s1",
            Some("t1"),
            MeasurementSource::OfficialTelemetry,
            "r1",
            12,
            6,
        )],
    )
    .unwrap();
    assert_eq!(staged.superseded, 1);
    let failed = ingest_observations_tx(&tx, &[poison_obs()]);
    assert!(failed.is_err());
    drop(tx); // rollback

    // Zero partial state: old row still active, no new rows, totals unchanged.
    assert_eq!(event_row_count(&ledger), 1);
    assert_eq!(superseded_row_count(&ledger), 0);
    assert_eq!(active_row_count(&ledger), 1);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 10);
}

#[test]
fn failed_ingest_inserts_nothing() {
    let mut ledger = Ledger::open_memory().unwrap();
    let good = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    let res = ledger.ingest(vec![good, poison_obs()]);
    assert!(res.is_err());
    assert_eq!(event_row_count(&ledger), 0);
}

#[test]
fn append_only_trigger_still_blocks_data_rewrites() {
    let mut ledger = Ledger::open_memory().unwrap();
    let ob = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    ledger.ingest(vec![ob]).unwrap();
    let id: String = ledger
        .connection()
        .query_row("SELECT id FROM usage_events", [], |r| r.get(0))
        .unwrap();

    // Rewriting measurement data is still forbidden ...
    let denied = ledger.connection_mut().execute(
        "UPDATE usage_events SET input_tokens = 999 WHERE id = ?1",
        [&id],
    );
    assert!(denied.is_err());

    // ... deleting is still forbidden ...
    let denied = ledger
        .connection_mut()
        .execute("DELETE FROM usage_events WHERE id = ?1", [&id]);
    assert!(denied.is_err());

    // ... but the supersede tombstone transition NULL -> non-NULL is allowed ...
    ledger
        .connection_mut()
        .execute(
            "UPDATE usage_events SET superseded_by = 'evt_new' WHERE id = ?1",
            [&id],
        )
        .unwrap();

    // ... exactly once: re-superseding or clearing is forbidden.
    let denied = ledger.connection_mut().execute(
        "UPDATE usage_events SET superseded_by = 'evt_newer' WHERE id = ?1",
        [&id],
    );
    assert!(denied.is_err());
    let denied = ledger.connection_mut().execute(
        "UPDATE usage_events SET superseded_by = NULL WHERE id = ?1",
        [&id],
    );
    assert!(denied.is_err());
}

// ---------------------------------------------------------------------------
// 2. Backward-compatible identity migration / replay (v1 + v2)
// ---------------------------------------------------------------------------

#[test]
fn v1_row_reingest_is_duplicate_with_zero_drift() {
    let mut ledger = Ledger::open_memory().unwrap();
    let ob = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    insert_legacy_v1_row(&mut ledger, &ob);
    assert_eq!(event_row_count(&ledger), 1);
    let before = ledger.aggregate_usage().unwrap();

    // Re-ingesting the same file under the v2 scheme: no new row, no drift.
    let s = ledger.ingest(vec![ob]).unwrap();
    assert_eq!((s.inserted, s.duplicates, s.superseded), (0, 1, 0));
    assert_eq!(event_row_count(&ledger), 1);
    assert_eq!(ledger.aggregate_usage().unwrap(), before);
}

#[test]
fn v1_row_superseded_by_higher_precedence_v2_observation() {
    let mut ledger = Ledger::open_memory().unwrap();
    let transcript = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        5,
    );
    insert_legacy_v1_row(&mut ledger, &transcript);

    let telemetry = obs(
        "s1",
        Some("t1"),
        MeasurementSource::OfficialTelemetry,
        "r1",
        12,
        6,
    );
    let s = ledger.ingest(vec![telemetry]).unwrap();
    assert_eq!((s.inserted, s.superseded), (1, 1));
    assert_eq!(event_row_count(&ledger), 2);
    assert_eq!(ledger.aggregate_usage().unwrap().input, 12);
}

#[test]
fn interleaved_old_and_new_rows_stay_single_counted() {
    let mut ledger = Ledger::open_memory().unwrap();
    let mk = || {
        obs(
            "s1",
            Some("t1"),
            MeasurementSource::TranscriptRequest,
            "r1",
            10,
            5,
        )
    };
    insert_legacy_v1_row(&mut ledger, &mk());
    for _ in 0..3 {
        let s = ledger.ingest(vec![mk()]).unwrap();
        assert_eq!((s.inserted, s.duplicates), (0, 1));
    }
    assert_eq!(event_row_count(&ledger), 1);
    let agg = ledger.aggregate_usage().unwrap();
    assert_eq!((agg.input, agg.requests), (10, 1));
}

#[test]
fn partial_file_replay_with_mixed_schemes() {
    let mut ledger = Ledger::open_memory().unwrap();
    // r1, r2 were ingested by the old binary; r3 is new.
    let r1 = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r1",
        10,
        1,
    );
    let r2 = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r2",
        20,
        2,
    );
    let r3 = obs(
        "s1",
        Some("t1"),
        MeasurementSource::TranscriptRequest,
        "r3",
        30,
        3,
    );
    insert_legacy_v1_row(&mut ledger, &r1);
    insert_legacy_v1_row(&mut ledger, &r2);

    let s = ledger.ingest(vec![r1, r2, r3]).unwrap();
    assert_eq!((s.inserted, s.duplicates), (1, 2));
    assert_eq!(event_row_count(&ledger), 3);
    let agg = ledger.aggregate_usage().unwrap();
    assert_eq!(agg.input, 60);
    assert_eq!(agg.requests, 3);
}

// ---------------------------------------------------------------------------
// 3. Allowlisted, backup-protected, versioned source_kind migration
// ---------------------------------------------------------------------------

#[test]
fn migration_rewrites_only_allowlisted_values() {
    let mut ledger = Ledger::open_memory().unwrap();
    clear_migration_version(&mut ledger);
    insert_raw_event(&mut ledger, "a", "assistant");
    insert_raw_event(&mut ledger, "u", "user");
    insert_raw_event(&mut ledger, "s", "summary");
    insert_raw_event(&mut ledger, "c", "claude_transcript");
    insert_raw_event(&mut ledger, "p", "provider_usage");
    insert_raw_event(&mut ledger, "x", "banana");
    insert_raw_event(&mut ledger, "e", "");
    insert_raw_event(&mut ledger, "q", "x'; DROP TABLE usage_events;--");

    let rewritten = migrate_source_kind_vocabulary(ledger.connection_mut()).unwrap();
    assert_eq!(rewritten, 5);

    assert_eq!(kind_of(&ledger, "raw-a"), source_kind::TRANSCRIPT_REQUEST);
    assert_eq!(kind_of(&ledger, "raw-u"), source_kind::TRANSCRIPT_REQUEST);
    assert_eq!(kind_of(&ledger, "raw-s"), source_kind::TRANSCRIPT_REQUEST);
    assert_eq!(kind_of(&ledger, "raw-c"), source_kind::TRANSCRIPT_REQUEST);
    assert_eq!(kind_of(&ledger, "raw-p"), source_kind::PROVIDER_FIELDS);
    // Unknown values are untouched, however adversarial.
    assert_eq!(kind_of(&ledger, "raw-x"), "banana");
    assert_eq!(kind_of(&ledger, "raw-e"), "");
    assert_eq!(kind_of(&ledger, "raw-q"), "x'; DROP TABLE usage_events;--");
    // ... and the table still exists (no injection).
    let tables: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_events'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);

    // Backup holds the originals.
    let backup_rows: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM source_kind_migration_backup",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(backup_rows, 5);
    let backup_kind: String = ledger
        .connection()
        .query_row(
            "SELECT old_source_kind FROM source_kind_migration_backup WHERE event_id = 'raw-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(backup_kind, "assistant");

    // Anomalies logged for the untouched values.
    let anomalies: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM measurement_anomalies WHERE type = 'unmapped_source_kind'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(anomalies, 3);

    // Version recorded exactly once.
    let versions: Vec<(String, i64, i64)> = ledger
        .connection()
        .prepare("SELECT name, version, rows_affected FROM applied_migrations")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        versions,
        vec![("normalize_source_kind_vocabulary".to_string(), 1, 5)]
    );

    // Second run is a no-op.
    let again = migrate_source_kind_vocabulary(ledger.connection_mut()).unwrap();
    assert_eq!(again, 0);
    let anomalies_after: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM measurement_anomalies WHERE type = 'unmapped_source_kind'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(anomalies_after, 3);
}

#[test]
fn migration_handles_very_long_and_unicode_kinds() {
    let mut ledger = Ledger::open_memory().unwrap();
    clear_migration_version(&mut ledger);
    let long = "k".repeat(10_000);
    insert_raw_event(&mut ledger, "long", &long);
    insert_raw_event(&mut ledger, "uni", "tôken✓種");

    let rewritten = migrate_source_kind_vocabulary(ledger.connection_mut()).unwrap();
    assert_eq!(rewritten, 0);
    assert_eq!(kind_of(&ledger, "raw-long"), long);
    assert_eq!(kind_of(&ledger, "raw-uni"), "tôken✓種");
    let anomalies: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM measurement_anomalies WHERE type = 'unmapped_source_kind'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(anomalies, 2);
}

#[test]
fn backup_restore_roundtrip() {
    let mut ledger = Ledger::open_memory().unwrap();
    clear_migration_version(&mut ledger);
    insert_raw_event(&mut ledger, "a", "assistant");
    insert_raw_event(&mut ledger, "p", "provider_usage");
    insert_raw_event(&mut ledger, "x", "banana");

    migrate_source_kind_vocabulary(ledger.connection_mut()).unwrap();
    assert_eq!(kind_of(&ledger, "raw-a"), source_kind::TRANSCRIPT_REQUEST);

    let restored = restore_source_kind_backup(ledger.connection_mut()).unwrap();
    assert_eq!(restored, 2);
    assert_eq!(kind_of(&ledger, "raw-a"), "assistant");
    assert_eq!(kind_of(&ledger, "raw-p"), "provider_usage");
    // Untouched values were never backed up and are unaffected.
    assert_eq!(kind_of(&ledger, "raw-x"), "banana");
    // Backup table retained for auditability; restore is repeatable.
    let restored_again = restore_source_kind_backup(ledger.connection_mut()).unwrap();
    assert_eq!(restored_again, 2);
}

#[test]
fn migration_records_zero_row_version_on_clean_db() {
    // Fresh databases record the version row too, so "ran" is auditable
    // even when there was nothing to rewrite.
    let ledger = Ledger::open_memory().unwrap();
    let row: Option<(String, i64, i64)> = ledger
        .connection()
        .query_row(
            "SELECT name, version, rows_affected FROM applied_migrations
             WHERE name = 'normalize_source_kind_vocabulary'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    assert_eq!(
        row,
        Some(("normalize_source_kind_vocabulary".to_string(), 1, 0))
    );
}

#[test]
fn count_source_kind_backup_rows_tracks_migration() {
    let mut ledger = Ledger::open_memory().unwrap();
    // Fresh ledger: migration ran at init but rewrote nothing.
    assert_eq!(
        count_source_kind_backup_rows(ledger.connection()).unwrap(),
        0
    );

    clear_migration_version(&mut ledger);
    insert_raw_event(&mut ledger, "a", "assistant");
    insert_raw_event(&mut ledger, "p", "provider_usage");
    insert_raw_event(&mut ledger, "x", "banana");

    let rewritten = migrate_source_kind_vocabulary(ledger.connection_mut()).unwrap();
    assert_eq!(rewritten, 2);
    assert_eq!(
        count_source_kind_backup_rows(ledger.connection()).unwrap(),
        2
    );

    // Restore keeps the backup table (auditable), so the count is unchanged.
    let restored = restore_source_kind_backup(ledger.connection_mut()).unwrap();
    assert_eq!(restored, 2);
    assert_eq!(
        count_source_kind_backup_rows(ledger.connection()).unwrap(),
        2
    );
}
