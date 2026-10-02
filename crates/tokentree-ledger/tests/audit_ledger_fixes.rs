// SPDX-License-Identifier: Apache-2.0
//! Regression tests for the ledger-lane audit fixes:
//! C2 (source_kind vocabulary), C3 (import attribution), H4 (tree money columns),
//! H8 (saturating accumulation), H10 (attach_session lifecycle skip),
//! H11 (stop_manual atomicity), and the completeness / WAL / cast mediums.

use rusqlite::params;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation, source_kind};
use tokentree_ledger::{
    Ledger, ManualCounts, ManualStartInput, ProjectTree, UsageTotals, WorkTreeNode, attach_session,
    ensure_session_attribution, export_csv, export_html, load_project_trees, session_stable_id,
    stable_id, start_manual, stop_manual,
};

fn observation(
    adapter: &str,
    session: &str,
    source: MeasurementSource,
    subtype: Option<&str>,
    request: &str,
    input: u64,
    output: u64,
) -> UsageObservation {
    UsageObservation {
        adapter: adapter.into(),
        source,
        source_subtype: subtype.map(str::to_string),
        source_event_id: Some(format!("evt-{request}")),
        provider_session_id: session.into(),
        request_id: Some(request.into()),
        turn_id: None,
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

#[test]
fn c2_ingest_canonicalizes_unrecognized_subtypes() {
    let mut ledger = Ledger::open_memory().unwrap();
    // Legacy raw transcript record type: must not reach the DB.
    let legacy = observation(
        "claude",
        "s1",
        MeasurementSource::TranscriptRequest,
        Some("assistant"),
        "req-legacy",
        10,
        5,
    );
    // Recognized vocabulary subtype: preserved verbatim.
    let counter = observation(
        "codex",
        "s2",
        MeasurementSource::SnapshotDelta,
        Some(source_kind::CODEX_TURN_COUNTER),
        "req-counter",
        10,
        5,
    );
    // No subtype: canonical source string.
    let plain = observation(
        "claude",
        "s3",
        MeasurementSource::OfficialTelemetry,
        None,
        "req-plain",
        10,
        5,
    );
    ledger.ingest(vec![legacy, counter, plain]).unwrap();

    let rows: Vec<(String, String)> = ledger
        .connection()
        .prepare("SELECT request_id, source_kind FROM usage_events")
        .unwrap()
        .query_map([], |row| Ok((row.get(0).unwrap(), row.get(1).unwrap())))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let kind_of = |req: &str| {
        rows.iter()
            .find(|(r, _)| r == req)
            .map(|(_, k)| k.clone())
            .unwrap()
    };
    assert_eq!(kind_of("req-legacy"), source_kind::TRANSCRIPT_REQUEST);
    assert_eq!(kind_of("req-counter"), source_kind::CODEX_TURN_COUNTER);
    assert_eq!(kind_of("req-plain"), source_kind::OFFICIAL_TELEMETRY);
}

#[test]
fn c2_session_stable_id_matches_ingest_derivation() {
    let mut ledger = Ledger::open_memory().unwrap();
    ledger
        .ingest(vec![observation(
            "claude",
            "provider-ses-9",
            MeasurementSource::TranscriptRequest,
            None,
            "req-x",
            1,
            1,
        )])
        .unwrap();
    let expected = session_stable_id("claude", "provider-ses-9");
    let count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM sessions WHERE id = ?1",
            [&expected],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "session_stable_id must match ingest's derivation");
}

#[test]
fn c2_legacy_source_kinds_normalized_on_open() {
    let dir = std::env::temp_dir().join(format!("tokentree-c2-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("ledger.db");

    // First open creates the schema; insert legacy rows directly.
    {
        let ledger = Ledger::open(&db_path).unwrap();
        for (id, kind, hash) in [
            ("evt_legacy_1", "assistant", "h1"),
            ("evt_legacy_2", "user", "h2"),
            ("evt_legacy_3", "provider_usage", "h3"),
            ("evt_legacy_4", "claude_transcript", "h4"),
            ("evt_legacy_5", "summary", "h5"),
        ] {
            ledger
                .connection()
                .execute(
                    "INSERT INTO usage_events(id, adapter, source_kind, observed_at, ingested_at, source_path, source_offset, event_hash, adapter_version, parser_version)
                     VALUES(?1, 'claude', ?2, '2026-10-02T00:00:00Z', '2026-10-02T00:00:00Z', 'test', 0, ?3, 't', 't')",
                    params![id, kind, hash],
                )
                .unwrap();
        }
        // A recognized value must survive untouched.
        ledger
            .connection()
            .execute(
                "INSERT INTO usage_events(id, adapter, source_kind, observed_at, ingested_at, source_path, source_offset, event_hash, adapter_version, parser_version)
                 VALUES('evt_ok', 'codex', 'codex_turn_counter', '2026-10-02T00:00:00Z', '2026-10-02T00:00:00Z', 'test', 0, 'h', 't', 't')",
                [],
            )
            .unwrap();
    }
    // Reopen: the versioned migration runs (the version row is cleared first
    // to simulate a database written before the migration existed; without
    // that, reopen is a deliberate no-op).
    {
        let ledger = Ledger::open(&db_path).unwrap();
        ledger
            .connection()
            .execute(
                "DELETE FROM applied_migrations WHERE name = 'normalize_source_kind_vocabulary'",
                [],
            )
            .unwrap();
        drop(ledger);
        let ledger = Ledger::open(&db_path).unwrap();
        let kinds: std::collections::HashMap<String, String> = ledger
            .connection()
            .prepare("SELECT id, source_kind FROM usage_events")
            .unwrap()
            .query_map([], |row| Ok((row.get(0).unwrap(), row.get(1).unwrap())))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kinds["evt_legacy_1"], source_kind::TRANSCRIPT_REQUEST);
        assert_eq!(kinds["evt_legacy_2"], source_kind::TRANSCRIPT_REQUEST);
        assert_eq!(kinds["evt_legacy_3"], source_kind::PROVIDER_FIELDS);
        assert_eq!(kinds["evt_legacy_4"], source_kind::TRANSCRIPT_REQUEST);
        assert_eq!(kinds["evt_legacy_5"], source_kind::TRANSCRIPT_REQUEST);
        assert_eq!(kinds["evt_ok"], source_kind::CODEX_TURN_COUNTER);
    }
    // Third open: already clean, must remain a no-op.
    {
        let ledger = Ledger::open(&db_path).unwrap();
        let count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 6);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn c3_import_flow_creates_attributions_so_trees_are_not_empty() {
    let mut ledger = Ledger::open_memory().unwrap();
    // Simulate `tokentree import claude`: ingest, then ensure attribution per session.
    let obs = vec![
        observation(
            "claude",
            "imported-ses",
            MeasurementSource::TranscriptRequest,
            None,
            "req-a",
            100,
            50,
        ),
        observation(
            "claude",
            "imported-ses",
            MeasurementSource::TranscriptRequest,
            None,
            "req-b",
            200,
            25,
        ),
    ];
    ledger.ingest(obs).unwrap();

    // Before attribution: aggregates see usage but trees are empty (the C3 bug).
    let trees_before = load_project_trees(ledger.connection(), None).unwrap();
    assert!(trees_before.is_empty(), "no attribution yet, so no trees");

    // The wired import path: one ensure_session_attribution per imported session.
    let created = ensure_session_attribution(
        ledger.connection_mut(),
        &session_stable_id("claude", "imported-ses"),
    )
    .unwrap();
    assert_eq!(created, 2);

    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    let totals = &trees[0].totals;
    assert_eq!(totals.requests, 2);
    assert_eq!(totals.input, 300);
    assert_eq!(totals.output, 75);
}

#[test]
fn h4_tree_money_columns_exclude_lifecycle_counters() {
    let mut ledger = Ledger::open_memory().unwrap();
    let normal = observation(
        "claude",
        "money-ses",
        MeasurementSource::TranscriptRequest,
        None,
        "req-normal",
        100,
        50,
    );
    let lifecycle = observation(
        "claude",
        "money-ses",
        MeasurementSource::TranscriptRequest,
        Some(source_kind::FINAL_REQUEST_COUNTER),
        "req-lifecycle",
        100,
        50,
    );
    ledger.ingest(vec![normal, lifecycle]).unwrap();
    ensure_session_attribution(
        ledger.connection_mut(),
        &session_stable_id("claude", "money-ses"),
    )
    .unwrap();

    let conn = ledger.connection_mut();
    let project_id = stable_id("prj", "personal-unassigned");
    let work_item_id = stable_id("wi", &format!("{project_id}:uncategorized"));

    // Wire the lifecycle-counter event into the tree manually (legacy state the
    // H4 exclusion must defend against): span + active group + attribution.
    let lifecycle_event_id: String = conn
        .query_row(
            "SELECT id FROM usage_events WHERE request_id = 'req-lifecycle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let now = "2026-10-02T00:00:00Z";
    let span_id = stable_id("span", &lifecycle_event_id);
    conn.execute(
        "INSERT INTO usage_spans(id, session_id, measurement_status, measured_usage_json, completeness)
         VALUES(?, ?, 'measured', ?, 100)",
        params![
            span_id,
            session_stable_id("claude", "money-ses"),
            serde_json::json!({ "usage_event_id": lifecycle_event_id }).to_string()
        ],
    )
    .unwrap();
    let group_id = stable_id("attr", &format!("{span_id}:h4"));
    conn.execute(
        "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
         VALUES(?, ?, 'causal-request', 1, ?)",
        params![group_id, span_id, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attributions(group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
         VALUES(?, ?, ?, 'primary', 10000, 'test', 0, 0)",
        params![group_id, project_id, work_item_id],
    )
    .unwrap();

    // Price both events (pricing_versions row needed for the FK).
    conn.execute(
        "INSERT INTO pricing_versions(id, provider, model_pattern, effective_from, rates_json, source, retrieved_at, signature_or_hash)
         VALUES('test-v1', 'anthropic', 'claude-sonnet-4-6', '2026-01-01', '{}', 'test', ?, 'hash')",
        [now],
    )
    .unwrap();
    let normal_event_id: String = conn
        .query_row(
            "SELECT id FROM usage_events WHERE request_id = 'req-normal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for (event_id, micros) in [
        (normal_event_id, 1_000_000i64),
        (lifecycle_event_id, 2_000_000i64),
    ] {
        conn.execute(
            "INSERT INTO cost_calculations(usage_event_id, pricing_version_id, amount_micros, currency, cost_type, attribution_policy, coverage_json, calculated_at)
             VALUES(?, 'test-v1', ?, 'USD', 'api_equivalent_estimate', 'causal-request', '{}', ?)",
            params![event_id, micros, now],
        )
        .unwrap();
    }

    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    let totals = &trees[0].totals;
    // Token columns already excluded lifecycle counters; money must match now.
    assert_eq!(totals.amount_micros, 1_000_000);
    assert_eq!(totals.priced, 1);
}

#[test]
fn h8_usage_totals_add_saturates_on_overflow() {
    let maxed = UsageTotals {
        input: u64::MAX,
        cache_read: u64::MAX,
        cache_write: u64::MAX,
        output: u64::MAX,
        reasoning: u64::MAX,
        requests: u64::MAX,
        measured: u64::MAX,
        unavailable: u64::MAX,
        priced: u64::MAX,
        amount_micros: u64::MAX,
    };
    let one = UsageTotals {
        input: 1,
        cache_read: 1,
        cache_write: 1,
        output: 1,
        reasoning: 1,
        requests: 1,
        measured: 1,
        unavailable: 1,
        priced: 1,
        amount_micros: 1,
    };
    let sum = maxed.add(&one);
    assert_eq!(sum.input, u64::MAX);
    assert_eq!(sum.amount_micros, u64::MAX);
    assert_eq!(sum.requests, u64::MAX);

    // Normal addition still exact.
    let small = UsageTotals {
        input: 100,
        output: 50,
        ..UsageTotals::default()
    };
    let doubled = small.add(&small);
    assert_eq!(doubled.input, 200);
    assert_eq!(doubled.output, 100);
}

#[test]
fn h10_attach_session_skips_lifecycle_events_with_note() {
    let mut ledger = Ledger::open_memory().unwrap();
    ledger
        .ingest(vec![
            observation(
                "claude",
                "attach-ses",
                MeasurementSource::TranscriptRequest,
                None,
                "req-attach-normal",
                100,
                50,
            ),
            observation(
                "claude",
                "attach-ses",
                MeasurementSource::TranscriptRequest,
                Some(source_kind::SUBAGENT_LIFECYCLE_COUNTER),
                "req-attach-lifecycle",
                10,
                5,
            ),
        ])
        .unwrap();

    // Target work item in its own project.
    let conn = ledger.connection_mut();
    let now = "2026-10-02T00:00:00Z";
    conn.execute(
        "INSERT INTO projects(id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
         VALUES('prj_target', 'target', 'Target', 'h', 'manual', 1.0, ?, ?)",
        params![now, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO work_items(id, project_id, type, title, status, confidence, classifier_version, created_at)
         VALUES('wi_target', 'prj_target', 'manual_task', 'Target task', 'open', 1, 'manual', ?)",
        [now],
    )
    .unwrap();

    // Must not FK-violate on the lifecycle-counter event; only the real
    // request event is re-attributed.
    let changed = attach_session(
        ledger.connection_mut(),
        &session_stable_id("claude", "attach-ses"),
        "wi_target",
    )
    .unwrap();
    assert_eq!(changed, 1);

    // Skip recorded as a note.
    let anomaly: String = ledger
        .connection()
        .query_row(
            "SELECT type FROM measurement_anomalies WHERE type = 'attach_skipped_lifecycle_counter'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(anomaly, "attach_skipped_lifecycle_counter");

    // The re-attributed span points at the target work item.
    let wi: String = ledger
        .connection()
        .query_row(
            "SELECT a.work_item_id FROM attributions a
             JOIN attribution_groups ag ON ag.id = a.group_id
             WHERE ag.active = 1 AND a.method = 'manual_attach'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wi, "wi_target");
}

#[test]
fn h11_stop_manual_commits_atomically() {
    let mut ledger = Ledger::open_memory().unwrap();
    let started = start_manual(
        ledger.connection_mut(),
        ManualStartInput {
            project_key: "atomic-proj",
            project_title: Some("Atomic Project"),
            task_title: "atomic task",
            parent_title: None,
            cwd: "/tmp",
        },
    )
    .unwrap();

    let stopped = stop_manual(
        ledger.connection_mut(),
        ManualCounts {
            input: Some(100),
            output: Some(50),
            ..ManualCounts::default()
        },
    )
    .unwrap();
    assert_eq!(stopped.run_id, started.run_id);
    assert_eq!(stopped.measurement_status, "measured");

    let conn = ledger.connection();
    // All four writes landed together: event, stopped run, span, attribution.
    let event_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM usage_events WHERE adapter = 'manual'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 1);
    let state: String = conn
        .query_row(
            "SELECT state FROM manual_runs WHERE id = ?1",
            [&started.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "stopped");
    let span_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM usage_spans WHERE session_id = ?1",
            [&started.session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(span_count, 1);
    let attr_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM attributions WHERE work_item_id = ?1",
            [&started.work_item_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attr_count, 1);

    // Second stop fails cleanly on "no active run", not on a duplicate event:
    // there is no partial state left behind to wedge on.
    let err = stop_manual(ledger.connection_mut(), ManualCounts::default()).unwrap_err();
    assert!(
        err.to_string().contains("No active manual run"),
        "unexpected error: {err}"
    );
}

#[test]
fn medium_completeness_neutral_without_signal() {
    // CSV: zero-signal node must not render NaN.
    let empty_node = WorkTreeNode {
        id: "wi_empty".into(),
        title: "Empty".into(),
        parent_id: None,
        direct: UsageTotals::default(),
        inclusive: UsageTotals::default(),
        children: vec![],
    };
    let tree = ProjectTree {
        id: "prj_empty".into(),
        key: "empty".into(),
        title: "Empty".into(),
        roots: vec![empty_node],
        totals: UsageTotals::default(),
    };
    let csv = export_csv(std::slice::from_ref(&tree)).unwrap();
    assert!(
        csv.contains(",unavailable\n") || csv.contains(",unavailable\r\n"),
        "CSV completeness should be 'unavailable', got:\n{csv}"
    );
    assert!(!csv.contains("NaN"), "CSV must never render NaN");

    // HTML: zero requests must not render a green 100%.
    let html = export_html(&[], "disclaimer");
    assert!(
        !html.contains(">100%</div>") && !html.contains(">100%</span>"),
        "HTML must not show 100% completeness with no data"
    );
    assert!(!html.contains("NaN"), "HTML must never render NaN");
}

#[test]
fn medium_wal_journal_mode_honored_on_open() {
    let dir = std::env::temp_dir().join(format!("tokentree-wal-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("ledger.db");
    let ledger = Ledger::open(&db_path).unwrap();
    let mode: String = ledger
        .connection()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    drop(ledger);
    std::fs::remove_dir_all(&dir).ok();
}
