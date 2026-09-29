// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation, slug_key};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrototypePreview {
    pub records: usize,
    pub measured: usize,
    pub unavailable: usize,
    pub projects: usize,
    pub backup_path: Option<String>,
    pub inserted: Option<usize>,
    pub duplicates: Option<usize>,
}

fn extract_rows(value: &Value) -> Result<Vec<&serde_json::Map<String, Value>>> {
    if let Some(arr) = value.as_array() {
        return Ok(arr.iter().filter_map(Value::as_object).collect());
    }
    if let Some(obj) = value.as_object() {
        for key in ["records", "entries", "events", "usage", "tasks"] {
            if let Some(arr) = obj.get(key).and_then(Value::as_array) {
                return Ok(arr.iter().filter_map(Value::as_object).collect());
            }
        }
        return Ok(obj.values().filter_map(Value::as_object).collect());
    }
    bail!("Prototype ledger must be a JSON object or array");
}

fn str_field<'a>(
    row: &'a serde_json::Map<String, Value>,
    keys: &[&str],
    fallback: &'a str,
) -> &'a str {
    for k in keys {
        if let Some(s) = row.get(*k).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    fallback
}

fn token_field(row: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    for k in keys {
        if let Some(num) = row.get(*k).and_then(Value::as_u64) {
            return Some(num);
        }
    }
    None
}

pub fn preview_prototype(source_path: &Path) -> Result<PrototypePreview> {
    let content = fs::read_to_string(source_path)
        .with_context(|| format!("read prototype source at {}", source_path.display()))?;
    let parsed: Value = serde_json::from_str(&content).context("parse prototype JSON")?;
    let items = extract_rows(&parsed)?;

    let mut measured = 0;
    let mut project_names = HashSet::new();

    for row in &items {
        let is_m = token_field(row, &["input_tokens", "input"]).is_some()
            || token_field(row, &["output_tokens", "output"]).is_some()
            || token_field(row, &["cache_read", "cached_input_tokens"]).is_some();
        if is_m {
            measured += 1;
        }
        let prj = str_field(row, &["project", "project_name"], "Prototype import");
        project_names.insert(prj.to_owned());
    }

    Ok(PrototypePreview {
        records: items.len(),
        measured,
        unavailable: items.len() - measured,
        projects: project_names.len(),
        backup_path: None,
        inserted: None,
        duplicates: None,
    })
}

pub fn apply_prototype(
    connection: &mut rusqlite::Connection,
    source_path: &Path,
    backup_dir: &Path,
) -> Result<PrototypePreview> {
    let raw_bytes = fs::read(source_path)
        .with_context(|| format!("read prototype file at {}", source_path.display()))?;
    let parsed: Value = serde_json::from_slice(&raw_bytes).context("parse prototype JSON")?;
    let items = extract_rows(&parsed)?;

    fs::create_dir_all(backup_dir)?;
    let now = Utc::now();
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ledger.json");
    let backup_path = backup_dir.join(format!("prototype-{}-{}", now.timestamp_millis(), filename));
    fs::write(&backup_path, &raw_bytes)?;

    let mut inserted = 0;
    let mut duplicates = 0;
    let mut measured = 0;
    let mut project_names = HashSet::new();

    let source_str = source_path.to_string_lossy().to_string();

    for (index, row) in items.iter().enumerate() {
        let project_name = str_field(row, &["project", "project_name"], "Prototype import");
        project_names.insert(project_name.to_owned());
        let task_name = str_field(row, &["task", "task_name", "name"], "Imported work");
        let project_id = stable_id("prj", &project_name.to_lowercase());
        let work_id = stable_id("wrk", &format!("{project_id}:{task_name}"));
        let default_session = format!("prototype-{index}");
        let session_key = str_field(row, &["session_id", "session"], &default_session);
        let default_ts = "1970-01-01T00:00:00Z";
        let timestamp = str_field(row, &["timestamp", "created_at"], default_ts);
        let model = row.get("model").and_then(Value::as_str).map(str::to_string);

        let usage = TokenUsage {
            input_tokens: token_field(row, &["input_tokens", "input"]),
            cached_input_tokens: token_field(row, &["cached_input_tokens", "cache_read"]),
            cache_write_tokens: token_field(row, &["cache_write_tokens", "cache_write"]),
            output_tokens: token_field(row, &["output_tokens", "output"]),
            reasoning_tokens: token_field(row, &["reasoning_tokens"]),
        };
        let is_m = usage.is_measured();
        if is_m {
            measured += 1;
        }

        let observation = UsageObservation {
            adapter: "prototype".into(),
            source: MeasurementSource::ExplicitCli,
            source_event_id: Some(format!("{source_str}:{index}")),
            provider_session_id: session_key.to_owned(),
            request_id: Some(format!("prototype:{source_str}:{index}")),
            turn_id: None,
            source_timestamp: Some(timestamp.to_owned()),
            observed_at: timestamp.to_owned(),
            model,
            service_tier: None,
            region: None,
            usage,
            provider_reported_cost_micros: None,
            source_path: source_str.clone(),
            source_offset: index as u64,
            adapter_version: "prototype-import-v1".into(),
            parser_version: "prototype-import-v1".into(),
        };

        let prj_key = slug_key(project_name);
        connection.execute(
            "INSERT OR IGNORE INTO projects(
                id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at
            ) VALUES(?,?,?,?,?,?,?,?)",
            params![
                project_id,
                prj_key,
                project_name,
                stable_id("identity", &project_name.to_lowercase()),
                "prototype_import",
                1.0,
                timestamp,
                timestamp,
            ],
        )?;

        connection.execute(
            "INSERT OR IGNORE INTO work_items(
                id, project_id, type, title, status, confidence, classifier_version, created_at
            ) VALUES(?,?,?,?,?,?,?,?)",
            params![
                work_id,
                project_id,
                "legacy_task",
                task_name,
                "open",
                1.0,
                "prototype-import-v1",
                timestamp,
            ],
        )?;

        let sum = crate::ingest_observations(connection, vec![observation])?;
        inserted += sum.inserted as usize;
        duplicates += sum.duplicates as usize;

        let session_id = stable_id("ses", &format!("prototype:{session_key}"));
        connection.execute(
            "UPDATE sessions SET project_id = ? WHERE id = ?",
            params![project_id, session_id],
        )?;

        let event_id = stable_id(
            "evt",
            &format!("prototype:request:prototype:{source_str}:{index}"),
        );
        let span_id = stable_id("span", &event_id);
        let group_id = stable_id("attr", &event_id);

        let status_str = if is_m { "measured" } else { "unavailable" };
        let completeness = if is_m { 100 } else { 0 };
        let measured_json = json!({ "usage_event_id": event_id }).to_string();

        connection.execute(
            "INSERT OR IGNORE INTO usage_spans(id, session_id, measurement_status, measured_usage_json, completeness)
             VALUES(?,?,?,?,?)",
            params![span_id, session_id, status_str, measured_json, completeness],
        )?;

        connection.execute(
            "INSERT OR IGNORE INTO attribution_groups(id, usage_span_id, policy, active, created_at)
             VALUES(?,?,?,1,?)",
            params![group_id, span_id, "causal-request", timestamp],
        )?;

        connection.execute(
            "INSERT OR IGNORE INTO attributions(
                group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user
            ) VALUES(?,?,?,?,10000,?,1,1)",
            params![group_id, project_id, work_id, "primary", "prototype_import"],
        )?;
    }

    // Verify source file was not modified
    let after_bytes = fs::read(source_path)?;
    if after_bytes != raw_bytes {
        bail!("Prototype source file was modified during migration");
    }

    Ok(PrototypePreview {
        records: items.len(),
        measured,
        unavailable: items.len() - measured,
        projects: project_names.len(),
        backup_path: Some(backup_path.to_string_lossy().to_string()),
        inserted: Some(inserted),
        duplicates: Some(duplicates),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    use tempfile::tempdir;

    #[test]
    fn previews_and_applies_prototype() {
        let dir = tempdir().unwrap();
        let proto_file = dir.path().join("ledger.json");
        let backup_dir = dir.path().join("backups");

        let content = json!([
            {
                "project": "Demo",
                "task": "Task 1",
                "input": 100,
                "output": 50,
                "session": "ses-1"
            },
            {
                "project": "Demo",
                "task": "Task 2",
                "session": "ses-2"
            }
        ]);
        fs::write(&proto_file, content.to_string()).unwrap();

        let preview = preview_prototype(&proto_file).unwrap();
        assert_eq!(preview.records, 2);
        assert_eq!(preview.measured, 1);
        assert_eq!(preview.unavailable, 1);
        assert_eq!(preview.projects, 1);

        let mut ledger = Ledger::open_memory().unwrap();
        let applied = apply_prototype(ledger.connection_mut(), &proto_file, &backup_dir).unwrap();
        assert_eq!(applied.records, 2);
        assert_eq!(applied.inserted, Some(2));
        assert!(applied.backup_path.is_some());
    }
}
