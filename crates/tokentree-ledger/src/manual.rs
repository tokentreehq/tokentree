// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation};

#[derive(Clone, Debug)]
pub struct ManualStartInput<'a> {
    pub project_key: &'a str,
    pub project_title: Option<&'a str>,
    pub task_title: &'a str,
    pub parent_title: Option<&'a str>,
    pub cwd: &'a str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManualStartResult {
    pub run_id: String,
    pub project_id: String,
    pub work_item_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct ManualCounts {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManualStopResult {
    pub run_id: String,
    pub measurement_status: String,
}

pub fn start_manual(
    connection: &mut Connection,
    input: ManualStartInput<'_>,
) -> Result<ManualStartResult> {
    let now = Utc::now().to_rfc3339();
    let project_id = stable_id("prj", input.project_key);
    let parent_id = input
        .parent_title
        .map(|pt| stable_id("wi", &format!("{project_id}:{pt}")));
    let work_item_id = stable_id("wi", &format!("{project_id}:{}", input.task_title));

    let run_id = format!(
        "run_{}",
        &tokentree_core::sha256_hex(format!("{}:{}", now, input.task_title).as_bytes())[..16]
    );
    let session_id = stable_id("ses", &format!("manual:{run_id}"));

    let transaction = connection.transaction()?;

    transaction.execute(
        "INSERT OR IGNORE INTO projects(
            id, key, display_name, identity_hash, detection_method, confidence, verified_at, created_at, updated_at
        ) VALUES(?,?,?,?,?,1,?,?,?)",
        params![
            project_id,
            input.project_key,
            input.project_title.unwrap_or(input.project_key),
            stable_id("identity", input.project_key),
            "explicit_override",
            now,
            now,
            now,
        ],
    )?;

    if let Some(pid) = &parent_id {
        transaction.execute(
            "INSERT OR IGNORE INTO work_items(
                id, project_id, type, title, status, confidence, classifier_version, created_at
            ) VALUES(?,?,'objective',?,'open',1,'manual',?)",
            params![pid, project_id, input.parent_title.unwrap_or_default(), now],
        )?;
    }

    transaction.execute(
        "INSERT OR IGNORE INTO work_items(
            id, project_id, parent_id, type, title, status, confidence, classifier_version, created_at
        ) VALUES(?,?,?,'manual_task',?,'open',1,'manual',?)",
        params![work_item_id, project_id, parent_id, input.task_title, now],
    )?;

    transaction.execute(
        "INSERT INTO sessions(id, adapter, provider_session_id, project_id, cwd, started_at)
         VALUES(?,'manual',?,?,?,?)",
        params![session_id, run_id, project_id, input.cwd, now],
    )?;

    transaction.execute(
        "INSERT INTO manual_runs(id, session_id, project_id, work_item_id, started_at, state)
         VALUES(?,?,?,?,?,'active')",
        params![run_id, session_id, project_id, work_item_id, now],
    )?;

    transaction.commit()?;

    Ok(ManualStartResult {
        run_id,
        project_id,
        work_item_id,
        session_id,
    })
}

pub fn stop_manual(connection: &mut Connection, counts: ManualCounts) -> Result<ManualStopResult> {
    struct ActiveRun {
        id: String,
        session_id: String,
        project_id: String,
        work_item_id: String,
    }

    let active_run: Option<ActiveRun> = connection
        .query_row(
            "SELECT id, session_id, project_id, work_item_id
             FROM manual_runs WHERE state = 'active'
             ORDER BY started_at DESC LIMIT 1",
            [],
            |row| {
                Ok(ActiveRun {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    project_id: row.get(2)?,
                    work_item_id: row.get(3)?,
                })
            },
        )
        .ok();

    let Some(run) = active_run else {
        bail!("No active manual run");
    };

    let now = Utc::now().to_rfc3339();
    let usage = TokenUsage {
        input_tokens: counts.input,
        cached_input_tokens: counts.cache_read,
        cache_write_tokens: counts.cache_write,
        output_tokens: counts.output,
        reasoning_tokens: counts.reasoning,
    };
    let is_measured = usage.is_measured();

    let observation = UsageObservation {
        adapter: "manual".into(),
        source: MeasurementSource::ExplicitCli,
        source_subtype: Some(tokentree_core::source_kind::MANUAL_STOP.into()),
        source_event_id: Some(run.id.clone()),
        provider_session_id: run.id.clone(),
        request_id: Some(format!("manual:{}", run.id)),
        turn_id: None,
        agent_id: None,
        parent_agent_id: None,
        source_timestamp: Some(now.clone()),
        observed_at: now.clone(),
        model: counts.model,
        service_tier: None,
        region: None,
        usage,
        provider_reported_cost_micros: None,
        source_path: "manual".into(),
        source_offset: 0,
        adapter_version: "0.2.0".into(),
        parser_version: "manual-v1".into(),
    };

    // V5: derive the event id with the SAME function the insert path uses.
    // The old hand-rolled `manual:request:manual:{run_id}` string diverged
    // from `canonical_identity()`, so `usage_spans.measured_usage_json`
    // referenced a non-existent `usage_events.id` and manual runs vanished
    // from tree rollups.
    let event_id = stable_id("evt", &observation.canonical_identity());
    let span_id = stable_id("span", &event_id);
    let group_id = stable_id("attr", &observation.event_hash());

    // Single atomic transaction: the usage event, the run state change, and the
    // span/attribution rows commit together. A crash can no longer leave the
    // event ingested while the run stays 'active' with no span (previously
    // unrecoverable: retrying bailed on the duplicate event).
    let mut transaction = connection.transaction()?;

    let summary = crate::ingest_observations_tx(&mut transaction, &[observation])?;
    if summary.inserted != 1 {
        bail!("Manual usage event already exists");
    }

    transaction.execute(
        "UPDATE sessions SET ended_at = ? WHERE id = ?",
        params![now, run.session_id],
    )?;

    transaction.execute(
        "UPDATE manual_runs SET stopped_at = ?, state = 'stopped' WHERE id = ?",
        params![now, run.id],
    )?;

    let status_str = if is_measured {
        "measured"
    } else {
        "unavailable"
    };
    let completeness = if is_measured { 100 } else { 0 };

    let measured_json = json!({ "usage_event_id": event_id }).to_string();
    transaction.execute(
        "INSERT INTO usage_spans(id, session_id, measurement_status, measured_usage_json, completeness)
         VALUES(?,?,?,?,?)",
        params![span_id, run.session_id, status_str, measured_json, completeness],
    )?;

    transaction.execute(
        "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
         VALUES(?,?,'causal-request',1,?)",
        params![group_id, span_id, now],
    )?;

    transaction.execute(
        "INSERT INTO attributions(group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
         VALUES(?,?,?,'primary',10000,'manual',1,1)",
        params![group_id, run.project_id, run.work_item_id],
    )?;

    transaction.commit()?;

    Ok(ManualStopResult {
        run_id: run.id,
        measurement_status: status_str.to_owned(),
    })
}
