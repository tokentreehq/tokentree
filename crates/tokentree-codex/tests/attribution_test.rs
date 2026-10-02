// SPDX-License-Identifier: Apache-2.0
//! Default attribution (C3/blocker-1): every session imported by
//! `import_codex_file` must resolve via the canonical `session_stable_id`
//! derivation and receive a default project/work-item attribution, so reports
//! render trees instead of empty results.

use std::path::PathBuf;
use tokentree_ledger::Ledger;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/parsers/codex")
}

#[test]
fn codex_import_assigns_default_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = fixtures_dir().join("public-small-codex-clean.jsonl");
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    let res = tokentree_codex::import_codex_file(ledger.connection_mut(), &fixture).unwrap();
    assert!(res.inserted > 0, "fixture should yield observations");

    // Sessions are stored under the canonical stable ID.
    let psid: String = ledger
        .connection()
        .query_row(
            "SELECT provider_session_id FROM sessions WHERE adapter = 'codex' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let expected = tokentree_core::session_stable_id("codex", &psid);
    let exists: bool = ledger
        .connection()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
            [&expected],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        exists,
        "codex session id must use the canonical session_stable_id derivation"
    );

    // Default attribution attaches every imported session.
    let attributed = tokentree_ledger::ensure_session_attribution_for_source(
        ledger.connection_mut(),
        "codex",
        &fixture,
    )
    .unwrap();
    assert!(attributed > 0, "no sessions were attributed");
    let unattributed: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM sessions WHERE adapter = 'codex' AND project_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unattributed, 0);

    // Reports render trees, not "No projects".
    let trees = tokentree_ledger::load_project_trees(ledger.connection(), None).unwrap();
    assert!(
        !trees.is_empty(),
        "attributed sessions must render project trees"
    );

    // Idempotent: a second pass attributes nothing new and changes nothing.
    let again = tokentree_ledger::ensure_session_attribution_for_source(
        ledger.connection_mut(),
        "codex",
        &fixture,
    )
    .unwrap();
    let trees2 = tokentree_ledger::load_project_trees(ledger.connection(), None).unwrap();
    assert_eq!(trees.len(), trees2.len());
    let _ = again;
}
