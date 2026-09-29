// SPDX-License-Identifier: Apache-2.0
use anyhow::Result;
use rusqlite::Connection;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LeakageAuditResult {
    pub leaks_detected: usize,
    pub audited_tables: usize,
    pub audited_spool_records: usize,
    pub leak_details: Vec<String>,
}

pub fn audit_prompt_leakage(
    connection: &Connection,
    spool_dir: Option<&Path>,
) -> Result<LeakageAuditResult> {
    let mut result = LeakageAuditResult::default();

    // 1. Audit text columns in the SQLite database
    let tables_and_columns: &[(&str, &[&str])] = &[
        ("work_items", &["title", "description"]),
        ("sessions", &["provider_session_id", "source_path"]),
        ("turns", &["prompt_storage_mode"]),
        (
            "usage_events",
            &[
                "source_kind",
                "source_event_id",
                "model",
                "service_tier",
                "region",
                "source_path",
            ],
        ),
        ("usage_spans", &["measured_usage_json"]),
        ("attributions", &["role", "explanation", "method"]),
        ("classification_events", &["rationale", "extracted_intent"]),
        ("cost_calculations", &["coverage_json"]),
        (
            "measurement_anomalies",
            &["source_values_json", "resolution"],
        ),
        ("notes", &["text"]),
    ];

    for &(table, columns) in tables_and_columns {
        result.audited_tables += 1;
        for &col in columns {
            let query = format!(
                "SELECT rowid, {} FROM {} WHERE {} IS NOT NULL",
                col, table, col
            );
            let mut stmt = match connection.prepare(&query) {
                Ok(stmt) => stmt,
                Err(_) => continue,
            };
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                let text: String = row.get(1)?;
                if contains_leak_pattern(&text) {
                    result.leaks_detected += 1;
                    result.leak_details.push(format!(
                        "{}.{} rowid {}: matched forbidden prompt/canary pattern",
                        table, col, rowid
                    ));
                }
            }
        }
    }

    // 2. Audit spool records if spool_dir exists
    if let Some(dir) = spool_dir {
        if dir.exists() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file()
                        && (path
                            .extension()
                            .is_some_and(|e| e == "json" || e == "jsonl"))
                    {
                        if let Ok(content) = fs::read_to_string(&path) {
                            for (line_no, line) in content.lines().enumerate() {
                                if line.trim().is_empty() {
                                    continue;
                                }
                                result.audited_spool_records += 1;
                                if contains_spool_leak_pattern(line) {
                                    result.leaks_detected += 1;
                                    result.leak_details.push(format!(
                                        "{}:{}: matched forbidden raw prompt/completion field",
                                        path.display(),
                                        line_no + 1
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

fn contains_leak_pattern(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // Canary secrets injected for testing
    if text.contains("CANARY_SECRET")
        || text.contains("CANARY_LEAK")
        || text.contains("SUPER_SECRET")
    {
        return true;
    }
    // API keys and private keys
    if text.contains("sk-proj-") || text.contains("ghp_") || text.contains("BEGIN PRIVATE KEY") {
        return true;
    }
    // Raw prompt or completion payload markers (except prompt_storage_mode)
    if (lower.contains("\"raw_prompt\"")
        || lower.contains("\"raw_completion\"")
        || lower.contains("\"user_prompt_content\""))
        && !text.contains("\"prompt_storage_mode\"")
    {
        return true;
    }
    false
}

fn contains_spool_leak_pattern(line: &str) -> bool {
    if contains_leak_pattern(line) {
        return true;
    }
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
        if val
            .get("prompt")
            .is_some_and(|p| !p.is_null() && p.as_str().is_some_and(|s| !s.is_empty()))
        {
            return true;
        }
        if val
            .get("completion")
            .is_some_and(|c| !c.is_null() && c.as_str().is_some_and(|s| !s.is_empty()))
        {
            return true;
        }
        if val.get("raw_prompt").is_some() || val.get("raw_completion").is_some() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE work_items (id TEXT PRIMARY KEY, title TEXT NOT NULL, description TEXT);
            CREATE TABLE sessions (id TEXT PRIMARY KEY, provider_session_id TEXT NOT NULL, source_path TEXT);
            CREATE TABLE turns (id TEXT PRIMARY KEY, prompt_storage_mode TEXT NOT NULL);
            CREATE TABLE usage_events (id TEXT PRIMARY KEY, source_kind TEXT NOT NULL, source_event_id TEXT, model TEXT, service_tier TEXT, region TEXT, source_path TEXT);
            CREATE TABLE usage_spans (id TEXT PRIMARY KEY, measured_usage_json TEXT);
            CREATE TABLE attributions (group_id TEXT NOT NULL, role TEXT NOT NULL, explanation TEXT, method TEXT NOT NULL);
            CREATE TABLE classification_events (id TEXT PRIMARY KEY, rationale TEXT, extracted_intent TEXT);
            CREATE TABLE cost_calculations (rowid INTEGER PRIMARY KEY, coverage_json TEXT);
            CREATE TABLE measurement_anomalies (id TEXT PRIMARY KEY, source_values_json TEXT NOT NULL, resolution TEXT);
            CREATE TABLE notes (id TEXT PRIMARY KEY, text TEXT NOT NULL);
            "#
        ).unwrap();
        conn
    }

    #[test]
    fn test_clean_database_has_zero_leaks() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO work_items (id, title, description) VALUES ('w1', 'Feature task', 'Safe description')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, text) VALUES ('n1', 'Regular work item note')",
            [],
        )
        .unwrap();

        let audit = audit_prompt_leakage(&conn, None).unwrap();
        assert_eq!(audit.leaks_detected, 0);
        assert!(audit.leak_details.is_empty());
    }

    #[test]
    fn test_injected_canary_secret_in_notes_detected() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO notes (id, text) VALUES ('n1', 'Leaked CANARY_SECRET_OPENAI_KEY_12345')",
            [],
        )
        .unwrap();

        let audit = audit_prompt_leakage(&conn, None).unwrap();
        assert_eq!(audit.leaks_detected, 1);
        assert!(audit.leak_details[0].contains("notes.text"));
    }

    #[test]
    fn test_injected_prompt_payload_in_spool_detected() {
        let conn = create_test_db();
        let tmp = tempfile::tempdir().unwrap();
        let spool_file = tmp.path().join("spool.jsonl");
        fs::write(
            &spool_file,
            r#"{"event":"prompt_event","prompt":"Here is the full proprietary user prompt"}"#,
        )
        .unwrap();

        let audit = audit_prompt_leakage(&conn, Some(tmp.path())).unwrap();
        assert_eq!(audit.leaks_detected, 1);
        assert!(audit.leak_details[0].contains("matched forbidden raw prompt/completion field"));
    }
}
