// SPDX-License-Identifier: Apache-2.0
//! CLI safety gates: destructive ledger mutations must refuse without `--yes`
//! and preview faithfully with `--dry-run`.
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn bin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join("tokentree");
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

fn run_with_home(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin_path())
        .env("TOKENTREE_HOME", home)
        .args(args)
        .output()
        .expect("execute tokentree")
}

/// Create two stopped manual tasks in the same project; returns their work item ids.
fn two_tasks(home: &std::path::Path) -> (String, String) {
    let mut ids = Vec::new();
    for task in ["task-a", "task-b"] {
        let out = run_with_home(home, &["start", "--project", "p1", "--task", task]);
        assert!(out.status.success(), "start failed: {out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        ids.push(v["work_item_id"].as_str().unwrap().to_string());
        let out = run_with_home(home, &["stop"]);
        assert!(out.status.success(), "stop failed: {out:?}");
    }
    (ids.remove(0), ids.remove(0))
}

#[test]
fn merge_refuses_without_yes() {
    let dir = tempdir().unwrap();
    let (a, b) = two_tasks(dir.path());
    let out = run_with_home(dir.path(), &["merge", "--source", &a, "--target", &b]);
    assert!(!out.status.success(), "merge without --yes must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--yes"),
        "refusal must name --yes, got: {stderr}"
    );
}

#[test]
fn merge_dry_run_previews_without_writing() {
    let dir = tempdir().unwrap();
    let (a, b) = two_tasks(dir.path());
    let out = run_with_home(
        dir.path(),
        &["merge", "--source", &a, "--target", &b, "--dry-run"],
    );
    assert!(out.status.success(), "dry-run failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["source"], a);
    assert_eq!(v["target"], b);
    assert_eq!(v["attribution_groups"], 1);

    // Nothing was written: a second dry-run reports the same plan.
    let out2 = run_with_home(
        dir.path(),
        &["merge", "--source", &a, "--target", &b, "--dry-run"],
    );
    let v2: Value = serde_json::from_slice(&out2.stdout).unwrap();
    assert_eq!(v2, v);
}

#[test]
fn merge_yes_applies() {
    let dir = tempdir().unwrap();
    let (a, b) = two_tasks(dir.path());
    let out = run_with_home(
        dir.path(),
        &["merge", "--source", &a, "--target", &b, "--yes"],
    );
    assert!(out.status.success(), "merge --yes failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["merged"], true);
    assert_eq!(v["spans_reattributed"], 1);

    // Source is now merged: planning the same merge fails.
    let out = run_with_home(
        dir.path(),
        &["merge", "--source", &a, "--target", &b, "--dry-run"],
    );
    assert!(!out.status.success());
}

#[test]
fn merge_dry_run_validates_like_apply() {
    let dir = tempdir().unwrap();
    two_tasks(dir.path());
    let out = run_with_home(
        dir.path(),
        &[
            "merge",
            "--source",
            "no-such-item",
            "--target",
            "no-such-target",
            "--dry-run",
        ],
    );
    assert!(!out.status.success(), "dry-run must validate inputs");
}

#[test]
fn split_refuses_without_yes() {
    let dir = tempdir().unwrap();
    let (a, _) = two_tasks(dir.path());
    let out = run_with_home(
        dir.path(),
        &["split", "--source", &a, "--title", "t", "--spans", "s1"],
    );
    assert!(!out.status.success(), "split without --yes must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--yes"),
        "refusal must name --yes, got: {stderr}"
    );
}

#[test]
fn split_dry_run_validates_like_apply() {
    let dir = tempdir().unwrap();
    let (a, _) = two_tasks(dir.path());
    // Empty title must fail even in dry-run.
    let out = run_with_home(
        dir.path(),
        &[
            "split",
            "--source",
            &a,
            "--title",
            "   ",
            "--spans",
            "s1",
            "--dry-run",
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn restore_source_kind_backup_refuses_without_yes() {
    let dir = tempdir().unwrap();
    let out = run_with_home(dir.path(), &["repair", "restore-source-kind-backup"]);
    assert!(
        !out.status.success(),
        "restore-source-kind-backup without --yes must refuse"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--yes"),
        "refusal must name --yes, got: {stderr}"
    );
}

#[test]
fn restore_source_kind_backup_dry_run_reports_zero_on_fresh_home() {
    let dir = tempdir().unwrap();
    let out = run_with_home(
        dir.path(),
        &["repair", "restore-source-kind-backup", "--dry-run"],
    );
    assert!(out.status.success(), "dry-run failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["rows"], 0);
}

#[test]
fn restore_source_kind_backup_yes_is_noop_on_fresh_home() {
    let dir = tempdir().unwrap();
    let out = run_with_home(
        dir.path(),
        &["repair", "restore-source-kind-backup", "--yes"],
    );
    assert!(out.status.success(), "apply failed: {out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["restored"], false);
    assert!(v["reason"].as_str().unwrap().contains("nothing to do"));
}
