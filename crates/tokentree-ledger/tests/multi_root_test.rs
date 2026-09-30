// SPDX-License-Identifier: Apache-2.0
use rusqlite::params;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};
use tokentree_ledger::{
    Ledger, deactivate_project_root, ensure_session_attribution, load_project_trees,
    move_project_root, register_project_root,
};

#[test]
fn test_multi_root_project_identity_deduplication_and_lifecycle() {
    let mut ledger = Ledger::open_memory().unwrap();

    // 1. Create one project
    let project_id = "prj_monorepo";
    ledger
        .connection_mut()
        .execute(
            "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
             VALUES (?1, 'monorepo-app', 'Monorepo App', 'hash_monorepo', 'git', 1.0, '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
            params![project_id],
        )
        .unwrap();

    // 2. Register multiple roots under this single project
    let client_root = "/workspace/client";
    let server_root = "/workspace/server";
    register_project_root(ledger.connection_mut(), project_id, client_root, "git").unwrap();
    register_project_root(ledger.connection_mut(), project_id, server_root, "git").unwrap();

    // Verify both roots are active and point to the same project
    let roots: Vec<(String, i64)> = {
        let mut stmt = ledger
            .connection()
            .prepare("SELECT canonical_path, active FROM project_roots WHERE project_id = ?1 ORDER BY canonical_path")
            .unwrap();
        stmt.query_map([project_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0], (client_root.to_string(), 1));
    assert_eq!(roots[1], (server_root.to_string(), 1));

    // 3. Ingest sessions from both roots
    let obs_client = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_subtype: None,
        source_event_id: Some("evt_client_1".to_string()),
        provider_session_id: "session_client_1".to_string(),
        request_id: Some("req_shared_client_1".to_string()),
        turn_id: None,
        agent_id: None,
        parent_agent_id: None,
        source_timestamp: Some("2026-09-29T10:01:00Z".to_string()),
        observed_at: "2026-09-29T10:01:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(200),
            cached_input_tokens: Some(50),
            cache_write_tokens: Some(0),
            output_tokens: Some(50),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: Some(1500),
        source_path: format!("{client_root}/session.jsonl"),
        source_offset: 10,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    let obs_server = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_subtype: None,
        source_event_id: Some("evt_server_1".to_string()),
        provider_session_id: "session_server_1".to_string(),
        request_id: Some("req_server_1".to_string()),
        turn_id: None,
        agent_id: None,
        parent_agent_id: None,
        source_timestamp: Some("2026-09-29T10:02:00Z".to_string()),
        observed_at: "2026-09-29T10:02:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(300),
            cached_input_tokens: Some(100),
            cache_write_tokens: Some(0),
            output_tokens: Some(80),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: Some(2500),
        source_path: format!("{server_root}/session.jsonl"),
        source_offset: 20,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    // Duplicate observation from client root with lower-ranked source (TranscriptRequest)
    let obs_client_dup = UsageObservation {
        source: MeasurementSource::TranscriptRequest,
        source_offset: 999,
        ..obs_client.clone()
    };

    // Ingest all observations including duplicate
    let summary = ledger
        .ingest(vec![obs_client, obs_server, obs_client_dup])
        .unwrap();
    assert_eq!(summary.inserted, 2);

    // Link sessions to prj_monorepo and set their respective root cwds
    let ses_client_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'session_client_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let ses_server_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'session_server_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    ledger
        .connection_mut()
        .execute(
            "UPDATE sessions SET project_id = ?1, cwd = ?2 WHERE id = ?3",
            params![project_id, client_root, ses_client_id],
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "UPDATE sessions SET project_id = ?1, cwd = ?2 WHERE id = ?3",
            params![project_id, server_root, ses_server_id],
        )
        .unwrap();

    // Ensure attribution creates work items and spans under prj_monorepo
    ensure_session_attribution(ledger.connection_mut(), &ses_client_id).unwrap();
    ensure_session_attribution(ledger.connection_mut(), &ses_server_id).unwrap();

    // 4. Assert single project identity
    let session_projects: Vec<String> = {
        let mut stmt = ledger
            .connection()
            .prepare("SELECT DISTINCT project_id FROM sessions WHERE id IN (?1, ?2)")
            .unwrap();
        stmt.query_map([&ses_client_id, &ses_server_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(session_projects, vec![project_id.to_string()]);

    // 5. Assert single request representation (no duplicates in usage_events)
    let req_count: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM usage_events WHERE request_id = 'req_shared_client_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        req_count, 1,
        "Single request must have exactly one event representation"
    );

    // 6. Assert non-duplicated totals across roots via load_project_trees
    let trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), 1, "Expected single project tree");
    let tree = &trees[0];
    assert_eq!(tree.id, project_id);
    assert_eq!(tree.title, "Monorepo App");
    // Totals: client (200 + 50 = 250) + server (300 + 80 = 380) = 500 input, 130 output, 150 cached
    assert_eq!(tree.totals.input, 500);
    assert_eq!(tree.totals.output, 130);
    assert_eq!(tree.totals.cache_read, 150);

    // 7. Test root deactivation
    deactivate_project_root(ledger.connection_mut(), client_root).unwrap();

    let client_active: i64 = ledger
        .connection()
        .query_row(
            "SELECT active FROM project_roots WHERE canonical_path = ?1",
            [client_root],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(client_active, 0, "Deactivated root must have active = 0");

    let active_roots: Vec<String> = {
        let mut stmt = ledger
            .connection()
            .prepare(
                "SELECT canonical_path FROM project_roots WHERE project_id = ?1 AND active = 1",
            )
            .unwrap();
        stmt.query_map([project_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(active_roots, vec![server_root.to_string()]);

    // Project trees and usage totals remain intact after deactivation
    let trees_after_deact = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees_after_deact[0].totals.input, 500);

    // 8. Test directory move
    let backend_root = "/workspace/backend";
    move_project_root(ledger.connection_mut(), server_root, backend_root).unwrap();

    let old_server_active: i64 = ledger
        .connection()
        .query_row(
            "SELECT active FROM project_roots WHERE canonical_path = ?1",
            [server_root],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_server_active, 0);

    let backend_active: i64 = ledger
        .connection()
        .query_row(
            "SELECT active FROM project_roots WHERE canonical_path = ?1 AND project_id = ?2",
            params![backend_root, project_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(backend_active, 1);

    // Ingest new session from moved root
    let obs_backend = UsageObservation {
        adapter: "claude".to_string(),
        source: MeasurementSource::OfficialTelemetry,
        source_subtype: None,
        source_event_id: Some("evt_backend_1".to_string()),
        provider_session_id: "session_backend_1".to_string(),
        request_id: Some("req_backend_1".to_string()),
        turn_id: None,
        agent_id: None,
        parent_agent_id: None,
        source_timestamp: Some("2026-09-29T10:05:00Z".to_string()),
        observed_at: "2026-09-29T10:05:00Z".to_string(),
        model: Some("claude-sonnet-4.6".to_string()),
        service_tier: None,
        region: None,
        usage: TokenUsage {
            input_tokens: Some(100),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(20),
            reasoning_tokens: None,
        },
        provider_reported_cost_micros: Some(500),
        source_path: format!("{backend_root}/session.jsonl"),
        source_offset: 10,
        adapter_version: "2.1.0".to_string(),
        parser_version: "1.0.0".to_string(),
    };

    ledger.ingest(vec![obs_backend]).unwrap();
    let ses_backend_id: String = ledger
        .connection()
        .query_row(
            "SELECT id FROM sessions WHERE provider_session_id = 'session_backend_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    ledger
        .connection_mut()
        .execute(
            "UPDATE sessions SET project_id = ?1, cwd = ?2 WHERE id = ?3",
            params![project_id, backend_root, ses_backend_id],
        )
        .unwrap();
    ensure_session_attribution(ledger.connection_mut(), &ses_backend_id).unwrap();

    // Verify rolled up total under single project identity now includes backend session
    let final_trees = load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(final_trees.len(), 1);
    assert_eq!(final_trees[0].totals.input, 600);
    assert_eq!(final_trees[0].totals.output, 150);
}
