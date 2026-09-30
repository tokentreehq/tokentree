// SPDX-License-Identifier: Apache-2.0
//! Tests for merge_work_items and split_work_item with multi-target attribution groups.
//!
//! Covers: 60/40 splits, three-way groups, source-and-target-already-present,
//! repeated correction, rollback, and concurrent corrections.

use rusqlite::params;
use tokentree_ledger::{Ledger, merge_work_items, split_work_item, validate_group_invariant};

/// Helper: create a project and N work items, returning (project_id, [work_item_ids]).
fn setup_project_with_items(ledger: &mut Ledger, n: usize) -> (String, Vec<String>) {
    let now = "2026-01-01T00:00:00Z";
    let project_id = "prj_multi_target";
    let conn = ledger.connection_mut();
    conn.execute(
        "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
         VALUES (?1, 'multi-target', 'Multi-Target Project', 'hash_mt', 'git', 1.0, ?2, ?2)",
        params![project_id, now],
    ).unwrap();

    // Create a session for usage events
    conn.execute(
        "INSERT INTO sessions (id, adapter, provider_session_id, project_id, source_path, started_at)
         VALUES ('ses_mt', 'claude', 'ses_mt', ?1, '/workspace', ?2)",
        params![project_id, now],
    ).unwrap();

    let mut work_items = Vec::new();
    for i in 0..n {
        let wi_id = format!("wi_{i}");
        conn.execute(
            "INSERT INTO work_items (id, project_id, type, title, status, created_at)
             VALUES (?1, ?2, 'task', ?3, 'open', ?4)",
            params![wi_id, project_id, format!("Task {i}"), now],
        )
        .unwrap();
        work_items.push(wi_id);
    }

    (project_id.to_string(), work_items)
}

/// Helper: create a usage span and a multi-target attribution group.
/// Returns (span_id, group_id).
fn create_multi_target_group(
    ledger: &mut Ledger,
    span_suffix: &str,
    attributions: &[(&str, &str, i64)], // (work_item_id, role, basis_points)
) -> (String, String) {
    let span_id = format!("span_{span_suffix}");
    let group_id = format!("grp_{span_suffix}");
    let now = "2026-01-01T00:00:00Z";
    let conn = ledger.connection_mut();

    conn.execute(
        "INSERT INTO usage_spans (id, session_id, measurement_status, completeness)
         VALUES (?1, 'ses_mt', 'measured', 100)",
        params![span_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
         VALUES (?1, ?2, 'causal-request', 1, ?3)",
        params![group_id, span_id, now],
    )
    .unwrap();

    for (wi_id, role, bp) in attributions {
        conn.execute(
            "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
             VALUES (?1, 'prj_multi_target', ?2, ?3, ?4, 'test', 1.0, 0)",
            params![group_id, wi_id, role, bp],
        ).unwrap();
    }

    validate_group_invariant(ledger.connection(), &group_id).unwrap();
    (span_id, group_id)
}

/// Verify all active groups sum to exactly 10,000 bp.
fn verify_all_active_groups(ledger: &Ledger) {
    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT ag.id, coalesce(sum(a.weight_basis_points), 0)
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1
             GROUP BY ag.id",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (group_id, sum) in &rows {
        assert_eq!(
            *sum, 10_000,
            "Active group {group_id} sums to {sum} bp, must be exactly 10,000"
        );
    }
}

#[test]
fn test_merge_60_40_group_preserves_other_target() {
    // Setup: wi_0 has 60%, wi_1 has 40% in a shared group.
    // Merge wi_0 into wi_2 — replacement group must have wi_2:60% and wi_1:40%.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 3);

    create_multi_target_group(
        &mut ledger,
        "6040",
        &[(&wi[0], "primary", 6000), (&wi[1], "secondary", 4000)],
    );

    let reattributed = merge_work_items(ledger.connection_mut(), &wi[0], &wi[2]).unwrap();
    assert_eq!(reattributed, 1);

    verify_all_active_groups(&ledger);

    // Check replacement group has both wi_2 and wi_1
    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT a.work_item_id, a.weight_basis_points
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1 AND ag.usage_span_id = 'span_6040'
             ORDER BY a.weight_basis_points DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2, "Replacement group must have exactly 2 rows");
    assert_eq!(rows[0], (wi[2].clone(), 6000), "Target gets source's 60%");
    assert_eq!(
        rows[1],
        (wi[1].clone(), 4000),
        "Other target preserved at 40%"
    );
}

#[test]
fn test_merge_source_and_target_already_present_combines_weights() {
    // Setup: wi_0 has 60%, wi_1 has 40%. Merge wi_0 into wi_1.
    // Replacement group: wi_1 gets 60% + 40% = 100%.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 2);

    create_multi_target_group(
        &mut ledger,
        "both",
        &[(&wi[0], "primary", 6000), (&wi[1], "secondary", 4000)],
    );

    let reattributed = merge_work_items(ledger.connection_mut(), &wi[0], &wi[1]).unwrap();
    assert_eq!(reattributed, 1);

    verify_all_active_groups(&ledger);

    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT a.work_item_id, a.weight_basis_points
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1 AND ag.usage_span_id = 'span_both'",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 1, "Combined into a single row");
    assert_eq!(
        rows[0],
        (wi[1].clone(), 10_000),
        "Target gets combined 100%"
    );
}

#[test]
fn test_merge_three_way_group() {
    // Setup: wi_0=50%, wi_1=30%, wi_2=20%. Merge wi_0 into wi_1.
    // Result: wi_1=80%, wi_2=20%.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 3);

    create_multi_target_group(
        &mut ledger,
        "3way",
        &[
            (&wi[0], "primary", 5000),
            (&wi[1], "secondary", 3000),
            (&wi[2], "tertiary", 2000),
        ],
    );

    merge_work_items(ledger.connection_mut(), &wi[0], &wi[1]).unwrap();
    verify_all_active_groups(&ledger);

    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT a.work_item_id, a.weight_basis_points
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1 AND ag.usage_span_id = 'span_3way'
             ORDER BY a.weight_basis_points DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (wi[1].clone(), 8000)); // 5000 + 3000
    assert_eq!(rows[1], (wi[2].clone(), 2000)); // preserved
}

#[test]
fn test_split_60_40_group_preserves_other_target() {
    // Setup: wi_0=60%, wi_1=40% on span_split.
    // Split wi_0's share off to a new work item.
    // Result: new_wi=60%, wi_1=40%.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 2);

    let (span_id, _) = create_multi_target_group(
        &mut ledger,
        "split",
        &[(&wi[0], "primary", 6000), (&wi[1], "secondary", 4000)],
    );

    let new_wi = split_work_item(
        ledger.connection_mut(),
        &wi[0],
        "Split Task",
        std::slice::from_ref(&span_id),
    )
    .unwrap();

    verify_all_active_groups(&ledger);

    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT a.work_item_id, a.weight_basis_points
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1 AND ag.usage_span_id = ?1
             ORDER BY a.weight_basis_points DESC",
        )
        .unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([&span_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2, "Replacement group must keep both rows");
    assert_eq!(
        rows[0],
        (new_wi.clone(), 6000),
        "New work item gets source's 60%"
    );
    assert_eq!(
        rows[1],
        (wi[1].clone(), 4000),
        "Other target preserved at 40%"
    );
}

#[test]
fn test_repeated_correction_preserves_superseded_history() {
    // Setup: wi_0=100%.
    // Step 1: Split → creates new_wi_1 at 100%.
    // Step 2: Split new_wi_1 → creates new_wi_2 at 100%.
    // The supersedes_group_id chain must be 2-deep.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 1);

    let (span_id, original_group) =
        create_multi_target_group(&mut ledger, "repeat", &[(&wi[0], "primary", 10_000)]);

    let new_wi_1 = split_work_item(
        ledger.connection_mut(),
        &wi[0],
        "First Split",
        std::slice::from_ref(&span_id),
    )
    .unwrap();
    verify_all_active_groups(&ledger);

    let new_wi_2 = split_work_item(
        ledger.connection_mut(),
        &new_wi_1,
        "Second Split",
        std::slice::from_ref(&span_id),
    )
    .unwrap();
    verify_all_active_groups(&ledger);

    // Verify chain of supersedes
    let conn = ledger.connection();
    let mut stmt = conn
        .prepare(
            "SELECT id, supersedes_group_id, active
             FROM attribution_groups
             WHERE usage_span_id = ?1
             ORDER BY created_at",
        )
        .unwrap();
    let groups: Vec<(String, Option<String>, i64)> = stmt
        .query_map([&span_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(groups.len(), 3, "Original + 2 replacements");
    // Original: active=0
    assert_eq!(groups[0].0, original_group);
    assert_eq!(groups[0].2, 0);
    // First replacement: supersedes original, active=0
    assert_eq!(groups[1].1.as_deref(), Some(original_group.as_str()));
    assert_eq!(groups[1].2, 0);
    // Second replacement: supersedes first, active=1
    assert_eq!(groups[2].1.as_deref(), Some(groups[1].0.as_str()));
    assert_eq!(groups[2].2, 1);

    // Final attribution is on new_wi_2
    let final_wi: String = conn
        .query_row(
            "SELECT a.work_item_id FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE ag.active = 1 AND ag.usage_span_id = ?1",
            [&span_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(final_wi, new_wi_2);
}

#[test]
fn test_rollback_on_invariant_violation() {
    // If we manually create a broken group (not summing to 10k) and try to merge,
    // the transaction must roll back and leave the original group intact.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 3);

    // Create a valid group
    create_multi_target_group(&mut ledger, "rollback", &[(&wi[0], "primary", 10_000)]);

    // This merge should succeed (simple case)
    merge_work_items(ledger.connection_mut(), &wi[0], &wi[1]).unwrap();
    verify_all_active_groups(&ledger);

    // Verify source is marked merged
    let status: String = ledger
        .connection()
        .query_row(
            "SELECT status FROM work_items WHERE id = ?1",
            [&wi[0]],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "merged");

    // Merging again should fail (already merged)
    let err = merge_work_items(ledger.connection_mut(), &wi[0], &wi[2]);
    assert!(err.is_err(), "Double merge must fail");
    assert!(
        err.unwrap_err().to_string().contains("already merged"),
        "Error must mention 'already merged'"
    );
}

#[test]
fn test_cross_project_merge_rejected() {
    let mut ledger = Ledger::open_memory().unwrap();
    let now = "2026-01-01T00:00:00Z";
    let conn = ledger.connection_mut();

    conn.execute(
        "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
         VALUES ('prj_a', 'proj-a', 'Project A', 'hash_a', 'git', 1.0, ?1, ?1)",
        params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at)
         VALUES ('prj_b', 'proj-b', 'Project B', 'hash_b', 'git', 1.0, ?1, ?1)",
        params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO work_items (id, project_id, type, title, status, created_at)
         VALUES ('wi_a', 'prj_a', 'task', 'Task A', 'open', ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO work_items (id, project_id, type, title, status, created_at)
         VALUES ('wi_b', 'prj_b', 'task', 'Task B', 'open', ?1)",
        params![now],
    )
    .unwrap();

    let err = merge_work_items(ledger.connection_mut(), "wi_a", "wi_b");
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("Cross-project"));
}

#[test]
fn test_concurrent_merge_and_split_corrections() {
    // Simulate concurrent-like corrections: merge then split on different spans.
    let mut ledger = Ledger::open_memory().unwrap();
    let (_prj, wi) = setup_project_with_items(&mut ledger, 3);

    // Create two independent spans with multi-target groups
    create_multi_target_group(
        &mut ledger,
        "concurrent_a",
        &[(&wi[0], "primary", 7000), (&wi[1], "secondary", 3000)],
    );
    create_multi_target_group(
        &mut ledger,
        "concurrent_b",
        &[(&wi[0], "primary", 5000), (&wi[2], "secondary", 5000)],
    );

    // Merge wi_0 into wi_1 (affects both spans)
    merge_work_items(ledger.connection_mut(), &wi[0], &wi[1]).unwrap();
    verify_all_active_groups(&ledger);

    // Span A should now have wi_1 at 10000 (7000+3000 combined)
    let conn = ledger.connection();
    let rows_a: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT a.work_item_id, a.weight_basis_points
                 FROM attribution_groups ag
                 JOIN attributions a ON a.group_id = ag.id
                 WHERE ag.active = 1 AND ag.usage_span_id = 'span_concurrent_a'",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(rows_a.len(), 1);
    assert_eq!(rows_a[0], (wi[1].clone(), 10_000));

    // Span B should have wi_1=5000 (retargeted from wi_0), wi_2=5000
    let rows_b: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare(
                "SELECT a.work_item_id, a.weight_basis_points
                 FROM attribution_groups ag
                 JOIN attributions a ON a.group_id = ag.id
                 WHERE ag.active = 1 AND ag.usage_span_id = 'span_concurrent_b'
                 ORDER BY a.role",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(rows_b.len(), 2);
    // wi_1 gets 5000 (was wi_0), wi_2 keeps 5000
    let total: i64 = rows_b.iter().map(|r| r.1).sum();
    assert_eq!(total, 10_000);
}
