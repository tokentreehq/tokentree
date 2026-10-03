// SPDX-License-Identifier: Apache-2.0
//! P1 privacy guard: `classification_events.extracted_intent` / `rationale`
//! exist in the schema but must never receive prompt-derived text in this
//! product. `tokentree_core::classifier::redacted_label` takes raw prompt
//! text and emits prompt fragments; wiring its output into those columns
//! would be a prompt-content leak into the ledger. This test scans every
//! non-test Rust source under `crates/` and fails if any INSERT/UPDATE
//! statement targets those columns.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Remove `#[cfg(test)] mod ... { ... }` blocks via brace matching so the
/// guard only inspects production code.
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(idx) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..idx]);
        let after_attr = &rest[idx + "#[cfg(test)]".len()..];
        let brace_offset = match after_attr.find('{') {
            Some(offset) => offset,
            None => break,
        };
        let mut depth = 0usize;
        let mut end = None;
        for (i, ch) in after_attr[brace_offset..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(brace_offset + i + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => rest = &after_attr[e..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// True if the (lowercased, test-stripped) source contains a SQL statement
/// that writes to `classification_events.extracted_intent`/`rationale`.
fn writes_prompt_column(prod_lower: &str) -> bool {
    for col in ["extracted_intent", "rationale"] {
        let mut start = 0;
        while let Some(relative) = prod_lower[start..].find(col) {
            let idx = start + relative;
            // Bound the backward scan to the current statement.
            let stmt_start = prod_lower[..idx].rfind(';').map(|i| i + 1).unwrap_or(0);
            let stmt_head = &prod_lower[stmt_start..idx];
            let is_insert =
                stmt_head.contains("insert") && stmt_head.contains("classification_events");
            let is_update = stmt_head.rfind("update").is_some_and(|u| {
                let after_update = &stmt_head[u..];
                after_update.contains("classification_events") && after_update.contains("set")
            });
            if is_insert || is_update {
                return true;
            }
            start = idx + col.len();
        }
    }
    false
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn classifier_prompt_columns_never_written_in_production() {
    let crates_dir = workspace_root().join("crates");
    assert!(crates_dir.is_dir(), "expected crates/ under workspace root");

    let mut files = Vec::new();
    collect_rs_files(&crates_dir, &mut files);
    assert!(!files.is_empty(), "no Rust sources found under crates/");

    let mut violations = Vec::new();
    for path in &files {
        // Integration-test fixtures may mention the columns (e.g. privacy
        // audit fixtures); only production code is guarded.
        if path.to_string_lossy().contains("/tests/") {
            continue;
        }
        let src = fs::read_to_string(path).unwrap();
        if writes_prompt_column(&strip_test_modules(&src).to_lowercase()) {
            violations.push(path.to_string_lossy().into_owned());
        }
    }

    assert!(
        violations.is_empty(),
        "P1 privacy guard: production code must never write prompt-derived text to \
         classification_events.extracted_intent/rationale. Offending files: {violations:?}"
    );
}
