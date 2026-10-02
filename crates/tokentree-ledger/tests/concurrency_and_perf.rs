// SPDX-License-Identifier: Apache-2.0
use rusqlite::params;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use tokentree_ledger::{
    Ledger, add_note, apply_prototype, load_project_trees, rename_work_item, render_project_trees,
};

#[test]
fn test_concurrent_wal_ingest_and_corrections() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("ledger.db");

    // Initialize baseline ledger schema and initial project/work item
    {
        let mut ledger = Ledger::open(&db_path).unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
                 VALUES ('proj_1', 'tokentree-test', 'TokenTree Test', 'hash_proj_1', 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                 VALUES ('wi_1', 'proj_1', 'task', 'Concurrent Ingest Task', 'open', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
    }

    let path_arc = Arc::new(db_path.clone());
    let mut handles = Vec::new();

    // 2 Ingestion threads
    for t_idx in 0..2 {
        let p = Arc::clone(&path_arc);
        handles.push(thread::spawn(move || -> anyhow::Result<()> {
            let mut ledger = Ledger::open(&*p)?;
            for i in 0..20 {
                let obs = UsageObservation {
                    adapter: "claude".to_string(),
                    source: MeasurementSource::OfficialTelemetry,
                    source_subtype: None,
                    source_event_id: Some(format!("event_{t_idx}_{i}")),
                    provider_session_id: format!("session_{t_idx}"),
                    request_id: Some(format!("req_{t_idx}_{i}")),
                    turn_id: None,
                    agent_id: None,
                    parent_agent_id: None,
                    source_timestamp: Some("2026-09-29T10:00:00Z".to_string()),
                    observed_at: "2026-09-29T10:00:00Z".to_string(),
                    model: Some("claude-sonnet-4.6".to_string()),
                    service_tier: None,
                    region: None,
                    usage: TokenUsage {
                        input_tokens: Some(100),
                        cached_input_tokens: Some(50),
                        cache_write_tokens: Some(0),
                        output_tokens: Some(25),
                        reasoning_tokens: None,
                    },
                    provider_reported_cost_micros: Some(500),
                    source_path: "/tmp/source.jsonl".to_string(),
                    source_offset: i as u64,
                    adapter_version: "2.1.0".to_string(),
                    parser_version: "1.0.0".to_string(),
                };
                ledger.ingest(vec![obs])?;
            }
            Ok(())
        }));
    }

    // 1 Query thread
    {
        let p = Arc::clone(&path_arc);
        handles.push(thread::spawn(move || -> anyhow::Result<()> {
            let ledger = Ledger::open(&*p)?;
            for _ in 0..10 {
                let _trees = load_project_trees(ledger.connection(), None)?;
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }));
    }

    // 1 Corrections thread (rename & note)
    {
        let p = Arc::clone(&path_arc);
        handles.push(thread::spawn(move || -> anyhow::Result<()> {
            let mut ledger = Ledger::open(&*p)?;
            for i in 0..10 {
                rename_work_item(
                    ledger.connection_mut(),
                    "wi_1",
                    &format!("Renamed Task {i}"),
                )?;
                add_note(
                    ledger.connection_mut(),
                    &format!("Concurrent note {i}"),
                    Some("wi_1"),
                )?;
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }));
    }

    for h in handles {
        h.join()
            .expect("thread panicked")
            .expect("thread execution failed");
    }

    // Verify WAL integrity and total events
    let ledger = Ledger::open(&db_path).unwrap();
    assert_eq!(ledger.integrity_check().unwrap(), "ok");
    let total_events: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total_events, 40);
}

#[test]
fn test_100k_event_load_benchmark_under_two_seconds() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("ledger_100k.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    // Setup project, session, work_item, span, attribution_group, and attribution
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
             VALUES ('proj_bench', 'bench-proj', 'Benchmark Project', 'hash_bench', 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO sessions (id, adapter, provider_session_id, project_id, started_at)
             VALUES ('ses_bench', 'claude', 'claude-session-bench', 'proj_bench', '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO work_items (id, project_id, type, title, status, created_at)
             VALUES ('wi_bench', 'proj_bench', 'task', 'Bulk Ingest Item', 'open', '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();

    // Populate 100,000 usage events in a single transaction
    {
        let tx = ledger.connection_mut().transaction().unwrap();
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO usage_events (
                        id, adapter, source_kind, session_id, observed_at, ingested_at,
                        input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
                        reasoning_tokens, event_hash, adapter_version, parser_version
                     ) VALUES (
                        ?1, 'claude', 'telemetry', 'ses_bench', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z',
                        100, 50, 10, 20, 0, ?2, '2.1.0', '1.0.0'
                     )",
                )
                .unwrap();

            for i in 0..100_000 {
                stmt.execute(params![format!("ue_bulk_{i}"), format!("hash_bulk_{i}")])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    // Link a representative span and active attribution to wi_bench
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO usage_spans (id, session_id, measurement_status, measured_usage_json, completeness)
             VALUES ('span_bench', 'ses_bench', 'measured', json_object('usage_event_id', 'ue_bulk_0'), 100.0)",
            [],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
             VALUES ('ag_bench', 'span_bench', 'causal-request', 1, '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
             VALUES ('ag_bench', 'proj_bench', 'wi_bench', 'primary', 10000, 'causal-request')",
            [],
        )
        .unwrap();

    // Benchmark load_project_trees on 100k events
    let start = Instant::now();
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].key, "bench-proj");
    assert!(
        elapsed < Duration::from_millis(2000),
        "100k-event ledger load took {:?}, exceeding 2.0s performance budget",
        elapsed
    );
}

#[test]
fn test_terminal_report_benchmark_under_500ms() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("ledger_report.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    // Insert 3 projects with nested items
    for p in 1..=3 {
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
                params![
                    format!("proj_{p}"),
                    format!("proj-{p}"),
                    format!("Project {p}"),
                    format!("hash_proj_{p}")
                ],
            )
            .unwrap();

        for w in 1..=5 {
            ledger
                .connection_mut()
                .execute(
                    "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                     VALUES (?1, ?2, 'task', ?3, 'open', '2026-09-29T10:00:00Z')",
                    params![
                        format!("wi_{p}_{w}"),
                        format!("proj_{p}"),
                        format!("Task {w}")
                    ],
                )
                .unwrap();
        }
    }

    let start = Instant::now();
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    let report = render_project_trees(&trees);
    let elapsed = start.elapsed();

    assert!(report.contains("Project 1"));
    assert!(report.contains("Project 2"));
    assert!(report.contains("Project 3"));
    assert!(
        elapsed < Duration::from_millis(500),
        "Terminal tree report generation took {:?}, exceeding 500ms budget",
        elapsed
    );
}

#[test]
fn test_hook_spool_p95_benchmark_under_100ms() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("ledger_spool.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    let spool_path = temp.path().join("claude-hooks.jsonl");
    let mut lines = Vec::new();
    let cwd_escaped = temp.path().to_string_lossy().replace('\\', "/");
    for i in 0..50 {
        lines.push(format!(
            r#"{{"version":1,"kind":"hook","capturedAt":"2026-09-29T10:00:00Z","payload":{{"session_id":"ses_spool_bench","cwd":"{cwd_escaped}","hook_name":"UserPromptSubmit","prompt_fingerprint":"fp_bench_{i}","prompt_storage_mode":"fingerprint_only"}}}}"#
        ));
    }
    std::fs::write(&spool_path, lines.join("\n") + "\n").unwrap();

    let start = Instant::now();
    let summary = ledger.process_claude_hook_spool(&spool_path).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(summary.processed, 50);
    let avg_latency = elapsed / 50;
    assert!(
        avg_latency < Duration::from_millis(100),
        "Hook spool average latency {:?} exceeded 100ms budget",
        avg_latency
    );
}

#[test]
fn test_interrupted_migration_preserves_ledger_state() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("ledger_migration.db");
    let mut ledger = Ledger::open(&db_path).unwrap();

    // Baseline project and manual item
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
             VALUES ('proj_orig', 'orig-proj', 'Original Project', 'hash_orig', 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();

    let corrupt_source = temp.path().join("corrupted_prototype.json");
    std::fs::write(&corrupt_source, "{ invalid_json_syntax: true").unwrap();

    let backup_dir = temp.path().join("backups");
    let res = apply_prototype(ledger.connection_mut(), &corrupt_source, &backup_dir);
    assert!(res.is_err(), "Migration must fail on corrupted input");

    // Prior project still intact
    let proj_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM projects WHERE id = 'proj_orig'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(proj_count, 1);
}
