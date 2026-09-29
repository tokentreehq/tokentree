// SPDX-License-Identifier: Apache-2.0
use rusqlite::params;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use tokentree_ledger::{Ledger, ensure_session_attribution, load_project_trees};

#[test]
fn test_subagent_reconciliation_capabilities_and_duplicate_counter_detection() {
    let mut ledger = Ledger::open_memory().unwrap();

    let project_id = "prj_subagent_test";
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
             VALUES (?1, 'subagent-proj', 'Subagent Project', 'hash_subagent', 'git', 1.0, '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
            params![project_id],
        )
        .unwrap();

    // -------------------------------------------------------------
    // Part 1: Capability missing or unknown -> reports degraded
    // -------------------------------------------------------------
    let rec_missing = ledger.reconcile().unwrap();
    assert!(
        rec_missing
            .subagent_reconciliation
            .contains("degraded: capability unknown or missing"),
        "Expected degraded status when capability is missing, got: {}",
        rec_missing.subagent_reconciliation
    );

    ledger
        .connection_mut()
        .execute(
            "INSERT INTO adapter_capabilities (adapter, adapter_version, host_version, capability, state, detail, checked_at)
             VALUES ('claude', '2.1.0', '1.0.0', 'subagent_tokens_already_in_parent', 'unknown', NULL, '2026-09-29T10:00:00Z')",
            [],
        )
        .unwrap();

    let rec_unknown = ledger.reconcile().unwrap();
    assert!(
        rec_unknown
            .subagent_reconciliation
            .contains("degraded: subagent capability state is unknown"),
        "Expected degraded status when capability is unknown, got: {}",
        rec_unknown.subagent_reconciliation
    );

    // -------------------------------------------------------------
    // Part 2: Capability TRUE -> child tokens already in parent
    // -------------------------------------------------------------
    ledger
        .connection_mut()
        .execute(
            "UPDATE adapter_capabilities
             SET state = 'available', detail = 'true', checked_at = '2026-09-29T10:05:00Z'
             WHERE capability = 'subagent_tokens_already_in_parent'",
            [],
        )
        .unwrap();

    let rec_true = ledger.reconcile().unwrap();
    assert!(
        rec_true
            .subagent_reconciliation
            .contains("verified: child tokens included in parent (no double-counting)"),
        "Expected no-double-counting verified status, got: {}",
        rec_true.subagent_reconciliation
    );

    // -------------------------------------------------------------
    // Part 3: Capability FALSE -> child tokens independent (rolls up)
    // -------------------------------------------------------------
    ledger
        .connection_mut()
        .execute(
            "UPDATE adapter_capabilities
             SET state = 'available', detail = 'false', checked_at = '2026-09-29T10:10:00Z'
             WHERE capability = 'subagent_tokens_already_in_parent'",
            [],
        )
        .unwrap();

    let rec_false = ledger.reconcile().unwrap();
    assert!(
        rec_false
            .subagent_reconciliation
            .contains("verified: child tokens independent (rolled up to parent)"),
        "Expected rolled-up status, got: {}",
        rec_false.subagent_reconciliation
    );

    // Ingest parent session and subagent session
    let obs_parent = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_event_id: Some("evt_parent_turn_1".to_string()),
        provider_session_id: "parent_session_1".to_string(),
        request_id: Some("req_parent_1".to_string()),
        turn_id: None,
        source_timestamp: Some("2026-09-29T10:10:00Z".to_string()),
        observed_at: "2026-09-29T10:10:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(500),
            cached_input_tokens: Some(100),
            cache_write_tokens: Some(0),
            output_tokens: Some(100),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: Some(2500),
        source_path: "/workspace/parent.jsonl".to_string(),
        source_offset: 10,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    let obs_subagent = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_event_id: Some("evt_subagent_turn_1".to_string()),
        provider_session_id: "subagent_session_1".to_string(),
        request_id: Some("req_subagent_1".to_string()),
        turn_id: None,
        source_timestamp: Some("2026-09-29T10:11:00Z".to_string()),
        observed_at: "2026-09-29T10:11:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(200),
            cached_input_tokens: Some(50),
            cache_write_tokens: Some(0),
            output_tokens: Some(40),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: Some(1000),
        source_path: "/workspace/subagent.jsonl".to_string(),
        source_offset: 10,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    ledger.ingest(vec![obs_parent, obs_subagent]).unwrap();

    let ses_parent_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'parent_session_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let ses_subagent_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'subagent_session_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // Link subagent session to root_session_id and both to project
    ledger
        .connection_mut()
        .execute(
            "UPDATE sessions SET project_id = ?1, cwd = '/workspace' WHERE id = ?2",
            params![project_id, ses_parent_id],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "UPDATE sessions SET project_id = ?1, root_session_id = ?2, cwd = '/workspace' WHERE id = ?3",
            params![project_id, ses_parent_id, ses_subagent_id],
        )
        .unwrap();

    ensure_session_attribution(ledger.connection_mut(), &ses_parent_id).unwrap();
    ensure_session_attribution(ledger.connection_mut(), &ses_subagent_id).unwrap();

    // Check project rollup: child tokens roll up into total project usage (500 + 200 = 700 input)
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].totals.input, 700);
    assert_eq!(trees[0].totals.output, 140);

    // -------------------------------------------------------------
    // Part 4: Injected duplicate final-request counters vs request-level events
    // -------------------------------------------------------------
    // Inject a final-request counter event for the same session and request
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO usage_events (
                id, adapter, source_kind, source_event_id, session_id, request_id,
                observed_at, ingested_at, model, input_tokens, output_tokens,
                source_path, source_offset, event_hash, adapter_version, parser_version
            ) VALUES (
                'evt_duplicate_final_counter_1', 'claude', 'final_request_counter', 'counter_1', ?1, 'req_parent_1',
                '2026-09-29T10:10:00Z', '2026-09-29T10:10:00Z', 'claude-sonnet-4.6', 500, 100,
                '/workspace/parent.jsonl', 999, 'hash_counter_1', '2.1.0', '1.0.0'
            )",
            params![ses_parent_id],
        )
        .unwrap();

    // Reconcile must detect the duplicate subagent counter
    let rec_with_counter = ledger.reconcile().unwrap();
    assert!(
        rec_with_counter.duplicate_subagent_counters >= 1,
        "Reconcile must detect injected duplicate final-request counter, got: {}",
        rec_with_counter.duplicate_subagent_counters
    );

    // Request-level events remain authoritative:
    // aggregate_usage must NOT add the final-request counter (still exactly 700 input, not 1200)
    let usage = ledger.aggregate_usage().unwrap();
    assert_eq!(
        usage.input, 700,
        "Authoritative request-level events must be used without double-counting counter events"
    );
    assert_eq!(usage.output, 140);
}
