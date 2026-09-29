// SPDX-License-Identifier: Apache-2.0
//! Tests that subagent_tokens_already_in_parent affects actual token accounting.
//!
//! Ingests identical parent/child fixture under three capability states (true, false, unknown)
//! and asserts exact, different token totals demonstrating correct behavior.

use rusqlite::params;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use tokentree_ledger::{Ledger, ensure_session_attribution, load_project_trees};

/// Build a parent+child fixture in a fresh ledger.
/// Parent: 500 input, 100 output. Child: 200 input, 40 output.
/// Sets capability to the given state/detail, then returns the ledger.
fn build_fixture(cap_state: &str, cap_detail: Option<&str>) -> Ledger {
    let mut ledger = Ledger::open_memory().unwrap();
    let project_id = "prj_accounting_test";
    let conn = ledger.connection_mut();

    conn.execute(
        "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
         VALUES (?1, 'accounting-proj', 'Accounting Project', 'hash_acct', 'git', 1.0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![project_id],
    ).unwrap();

    // Set capability
    if !cap_state.is_empty() {
        conn.execute(
            "INSERT INTO adapter_capabilities (adapter, adapter_version, host_version, capability, state, detail, checked_at)
             VALUES ('claude', '2.1.0', '1.0.0', 'subagent_tokens_already_in_parent', ?1, ?2, '2026-01-01T00:00:00Z')",
            params![cap_state, cap_detail],
        ).unwrap();
    }

    let _ = conn;

    // Parent observation: 500 input, 100 output
    let obs_parent = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_event_id: Some("evt_parent_1".to_string()),
        provider_session_id: "parent_ses".to_string(),
        request_id: Some("req_parent_1".to_string()),
        turn_id: None,
        agent_id: Some("agent_main".to_string()),
        parent_agent_id: None,
        source_timestamp: Some("2026-01-01T00:01:00Z".to_string()),
        observed_at: "2026-01-01T00:01:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(500),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(100),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: None,
        source_path: "/workspace/parent.jsonl".to_string(),
        source_offset: 0,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    // Child observation: 200 input, 40 output
    let obs_child = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_event_id: Some("evt_child_1".to_string()),
        provider_session_id: "child_ses".to_string(),
        request_id: Some("req_child_1".to_string()),
        turn_id: None,
        agent_id: Some("agent_child".to_string()),
        parent_agent_id: Some("agent_main".to_string()),
        source_timestamp: Some("2026-01-01T00:02:00Z".to_string()),
        observed_at: "2026-01-01T00:02:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(200),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(40),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: None,
        source_path: "/workspace/child.jsonl".to_string(),
        source_offset: 0,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    ledger.ingest(vec![obs_parent, obs_child]).unwrap();

    // Link parent session to project
    let ses_parent_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'parent_ses'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let ses_child_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'child_ses'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // Mark parent session with project, child with root_session_id
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
            params![project_id, ses_parent_id, ses_child_id],
        )
        .unwrap();

    ensure_session_attribution(ledger.connection_mut(), &ses_parent_id).unwrap();
    ensure_session_attribution(ledger.connection_mut(), &ses_child_id).unwrap();

    ledger
}

#[test]
fn test_subagent_accounting_true_excludes_child_tokens() {
    // When subagent_tokens_already_in_parent = true,
    // child tokens are already in the parent's numbers.
    // aggregate_usage must exclude child session events.
    let ledger = build_fixture("available", Some("true"));

    let usage = ledger.aggregate_usage().unwrap();
    // Only parent: 500 input, 100 output (child excluded)
    assert_eq!(
        usage.input, 500,
        "AlreadyInParent: must exclude child session (500 input expected), got {}",
        usage.input
    );
    assert_eq!(
        usage.output, 100,
        "AlreadyInParent: must exclude child session (100 output expected), got {}",
        usage.output
    );
    assert_eq!(
        usage.requests, 1,
        "AlreadyInParent: only parent request counted"
    );
    assert!(
        !usage.completeness_degraded,
        "AlreadyInParent: completeness should not be degraded"
    );

    // Project trees should also reflect the same exclusion
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].totals.input, 500);
    assert_eq!(trees[0].totals.output, 100);
    assert_eq!(trees[0].totals.requests, 1);
    assert_eq!(trees[0].totals.measured, 1);
    assert_eq!(trees[0].totals.unavailable, 0);
}

#[test]
fn test_subagent_accounting_false_includes_all_tokens() {
    // When subagent_tokens_already_in_parent = false,
    // child and parent are independent streams.
    // Both aggregate_usage and load_project_trees must roll up child events exactly once.
    let ledger = build_fixture("available", Some("false"));

    let usage = ledger.aggregate_usage().unwrap();
    // Both: 500 + 200 = 700 input, 100 + 40 = 140 output
    assert_eq!(
        usage.input, 700,
        "Independent: must include both parent and child (700 input expected), got {}",
        usage.input
    );
    assert_eq!(
        usage.output, 140,
        "Independent: must include both parent and child (140 output expected), got {}",
        usage.output
    );
    assert_eq!(usage.requests, 2, "Independent: both requests counted");
    assert_eq!(usage.measured, 2);
    assert_eq!(usage.unavailable, 0);
    assert!(
        !usage.completeness_degraded,
        "Independent: completeness should not be degraded"
    );

    // Project trees roll up both independent streams
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].totals.input, 700);
    assert_eq!(trees[0].totals.output, 140);
    assert_eq!(trees[0].totals.requests, 2);
    assert_eq!(trees[0].totals.measured, 2);
    assert_eq!(trees[0].totals.unavailable, 0);
}

#[test]
fn test_subagent_accounting_unknown_reports_degraded() {
    // When capability is unknown/missing,
    // report degraded completeness rather than silently selecting a policy.
    let ledger = build_fixture("", None);

    let usage = ledger.aggregate_usage().unwrap();
    // Parent measured: 500 input, 100 output; child unverified: unavailable: 1
    assert_eq!(
        usage.input, 500,
        "Unknown: must not add unverified child tokens to measured totals, got {}",
        usage.input
    );
    assert_eq!(
        usage.output, 100,
        "Unknown: must not add unverified child tokens to measured totals, got {}",
        usage.output
    );
    assert_eq!(usage.requests, 2);
    assert_eq!(usage.measured, 1);
    assert_eq!(usage.unavailable, 1);
    assert!(
        usage.completeness_degraded,
        "Unknown: completeness MUST be marked degraded"
    );

    // Project trees report degraded completeness
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1);
    assert_eq!(trees[0].totals.input, 500);
    assert_eq!(trees[0].totals.output, 100);
    assert_eq!(trees[0].totals.requests, 2);
    assert_eq!(trees[0].totals.measured, 1);
    assert_eq!(trees[0].totals.unavailable, 1);
}

#[test]
fn test_subagent_true_and_false_demonstrate_different_totals() {
    // The totals under true and false MUST differ to prove the policy affects accounting.
    let ledger_true = build_fixture("available", Some("true"));
    let ledger_false = build_fixture("available", Some("false"));

    let usage_true = ledger_true.aggregate_usage().unwrap();
    let usage_false = ledger_false.aggregate_usage().unwrap();

    assert_ne!(
        usage_true.input, usage_false.input,
        "AlreadyInParent (input={}) and Independent (input={}) MUST produce different totals",
        usage_true.input, usage_false.input
    );
    assert_ne!(
        usage_true.output, usage_false.output,
        "AlreadyInParent (output={}) and Independent (output={}) MUST produce different totals",
        usage_true.output, usage_false.output
    );

    // true: 500 input (parent only); false: 700 input (parent + child)
    assert_eq!(usage_true.input, 500);
    assert_eq!(usage_false.input, 700);
    assert_eq!(usage_true.output, 100);
    assert_eq!(usage_false.output, 140);
}

#[test]
fn test_subagent_unknown_state_reports_degraded_reconciliation() {
    let ledger = build_fixture("unknown", None);

    let rec = ledger.reconcile().unwrap();
    assert!(
        rec.subagent_reconciliation.contains("degraded"),
        "Unknown state must report degraded reconciliation, got: {}",
        rec.subagent_reconciliation
    );

    let usage = ledger.aggregate_usage().unwrap();
    assert!(
        usage.completeness_degraded,
        "Unknown state: completeness_degraded must be true"
    );
}
