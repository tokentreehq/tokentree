// SPDX-License-Identifier: Apache-2.0
//! Default attribution (blocker-1): every session imported by
//! `import_grok_file` must resolve via the canonical `session_stable_id`
//! derivation and receive a default project/work-item attribution.

use std::path::PathBuf;
use tokentree_ledger::Ledger;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/parsers/grok")
}

#[test]
fn grok_import_assigns_default_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = fixtures_dir().join("multi-turn.json");
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    let res = tokentree_grok::import_grok_file(ledger.connection_mut(), &fixture).unwrap();
    assert!(res.inserted > 0, "fixture should yield observations");

    let psid: String = ledger
        .connection()
        .query_row(
            "SELECT provider_session_id FROM sessions WHERE adapter = 'grok' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let expected = tokentree_core::session_stable_id("grok", &psid);
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
        "grok session id must use the canonical session_stable_id derivation"
    );

    let attributed = tokentree_ledger::ensure_session_attribution_for_source(
        ledger.connection_mut(),
        "grok",
        &fixture,
    )
    .unwrap();
    assert!(attributed > 0, "no sessions were attributed");
    let unattributed: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM sessions WHERE adapter = 'grok' AND project_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unattributed, 0);

    let trees = tokentree_ledger::load_project_trees(ledger.connection(), None).unwrap();
    assert!(
        !trees.is_empty(),
        "attributed sessions must render project trees"
    );
}
