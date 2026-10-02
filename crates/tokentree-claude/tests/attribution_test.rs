// SPDX-License-Identifier: Apache-2.0
//! Default attribution (C3/blocker-1): sessions imported through the Claude
//! path (`parse_session` + ledger ingest, mirroring `import_claude`) must
//! receive a default project/work-item attribution via
//! `ensure_session_attribution_for_source`.

use std::path::PathBuf;
use tokentree_ledger::Ledger;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/parsers/claude")
}

#[test]
fn claude_import_assigns_default_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = fixtures_dir().join("public-small-v2.1.80.jsonl");
    let mut ledger = Ledger::open(tmp.path().join("ledger.db")).unwrap();

    // Mirror apps/rust-cli import_claude: parse, ingest, then attribute via
    // the shared source-path helper (same as the other three adapters).
    let parsed = tokentree_claude::parse_session(&fixture).unwrap();
    assert!(!parsed.observations.is_empty());
    let summary = ledger.ingest(parsed.observations).unwrap();
    assert!(summary.inserted > 0);
    let attributed = tokentree_ledger::ensure_session_attribution_for_source(
        ledger.connection_mut(),
        "claude",
        &fixture,
    )
    .unwrap();
    assert!(attributed > 0, "no sessions were attributed");

    let unattributed: i64 = ledger
        .connection()
        .query_row(
            "SELECT count(*) FROM sessions WHERE adapter = 'claude' AND project_id IS NULL",
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
