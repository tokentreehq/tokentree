// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params};
use tokentree_core::source_kind;

pub fn ensure_session_attribution(connection: &mut Connection, session_id: &str) -> Result<u64> {
    struct SessionRow {
        project_id: Option<String>,
    }

    let session: Option<SessionRow> = connection
        .query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok(SessionRow {
                    project_id: row.get(0)?,
                })
            },
        )
        .ok();

    let Some(session) = session else {
        return Ok(0);
    };

    let now = Utc::now().to_rfc3339();
    let project_id = match session.project_id {
        Some(pid) => pid,
        None => {
            let pid = stable_id("prj", "personal-unassigned");
            connection.execute(
                "INSERT OR IGNORE INTO projects(
                    id, key, display_name, identity_hash, detection_method, confidence, created_at, updated_at
                ) VALUES(?,'personal-unassigned','Personal Unassigned',?,'personal_inbox',0.4,?,?)",
                params![pid, stable_id("identity", "personal-unassigned"), now, now],
            )?;
            connection.execute(
                "UPDATE sessions SET project_id = ? WHERE id = ?",
                params![pid, session_id],
            )?;
            pid
        }
    };

    let work_item_id = stable_id("wi", &format!("{project_id}:uncategorized"));
    connection.execute(
        "INSERT OR IGNORE INTO work_items(
            id, project_id, type, title, status, confidence, classifier_version, created_at
        ) VALUES(?,?,'inbox','Uncategorized','open',0,'pending',?)",
        params![work_item_id, project_id, now],
    )?;

    struct EventRow {
        id: String,
        input_tokens: Option<i64>,
        cached_input_tokens: Option<i64>,
        cache_write_tokens: Option<i64>,
        output_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
    }

    let mut stmt = connection.prepare(&format!(
        "SELECT id, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens
         FROM usage_events
         WHERE session_id = ?1
           AND source_kind NOT IN ({})",
        source_kind::LIFECYCLE_COUNTER_SQL_LIST,
    ))?;

    let events = stmt
        .query_map([session_id], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                input_tokens: row.get(1)?,
                cached_input_tokens: row.get(2)?,
                cache_write_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                reasoning_tokens: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut created = 0u64;
    let transaction = connection.transaction()?;

    for event in events {
        let span_id = stable_id("span", &event.id);
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_spans WHERE id = ?1)",
            [&span_id],
            |row| row.get(0),
        )?;
        if exists {
            continue;
        }

        let is_measured = event.input_tokens.is_some()
            || event.cached_input_tokens.is_some()
            || event.cache_write_tokens.is_some()
            || event.output_tokens.is_some()
            || event.reasoning_tokens.is_some();

        let status_str = if is_measured {
            "measured"
        } else {
            "unavailable"
        };
        let completeness = if is_measured { 100 } else { 0 };
        let measured_json = serde_json::json!({ "usage_event_id": event.id }).to_string();

        transaction.execute(
            "INSERT INTO usage_spans(id, session_id, measurement_status, measured_usage_json, completeness)
             VALUES(?,?,?,?,?)",
            params![span_id, session_id, status_str, measured_json, completeness],
        )?;

        let group_id = stable_id("attr", &format!("{span_id}:initial"));
        transaction.execute(
            "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
             VALUES(?,?,'causal-request',1,?)",
            params![group_id, span_id, now],
        )?;

        transaction.execute(
            "INSERT INTO attributions(
                group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user
            ) VALUES(?,?,?,'primary',10000,'session_default',0,0)",
            params![group_id, project_id, work_item_id],
        )?;

        validate_group_invariant(&transaction, &group_id)?;

        created = created.saturating_add(1);
    }

    transaction.commit()?;
    Ok(created)
}

pub fn attach_session(
    connection: &mut Connection,
    session_id: &str,
    work_item_id: &str,
) -> Result<u64> {
    let target_project_id: String = connection
        .query_row(
            "SELECT project_id FROM work_items WHERE id = ?1",
            [work_item_id],
            |row| row.get(0),
        )
        .with_context(|| format!("Unknown work item {work_item_id}"))?;

    ensure_session_attribution(connection, session_id)?;

    struct AttachEvent {
        id: String,
        source_kind: String,
    }
    let mut stmt =
        connection.prepare("SELECT id, source_kind FROM usage_events WHERE session_id = ?1")?;
    let events = stmt
        .query_map([session_id], |row| {
            Ok(AttachEvent {
                id: row.get(0)?,
                source_kind: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let now = Utc::now().to_rfc3339();
    let mut changed = 0u64;
    let mut skipped_lifecycle: Vec<String> = Vec::new();
    let transaction = connection.transaction()?;

    for event in events {
        // Lifecycle-counter events never get usage_spans (ensure_session_attribution
        // deliberately excludes them): there is no span to re-attribute, and inserting
        // an attribution_group for a nonexistent span would FK-violate. Skip them with
        // a recorded note instead of failing the whole attach.
        if is_lifecycle_counter_kind(&event.source_kind) {
            skipped_lifecycle.push(event.id);
            continue;
        }
        let span_id = stable_id("span", &event.id);
        let active_group: Option<String> = transaction
            .query_row(
                "SELECT id FROM attribution_groups WHERE usage_span_id = ?1 AND active = 1",
                [&span_id],
                |row| row.get(0),
            )
            .ok();

        if let Some(ref gid) = active_group {
            transaction.execute(
                "UPDATE attribution_groups SET active = 0 WHERE id = ?",
                params![gid],
            )?;
        }

        let new_group_id = stable_id("attr", &format!("{span_id}:{work_item_id}:{now}"));
        transaction.execute(
            "INSERT INTO attribution_groups(
                id, usage_span_id, policy, supersedes_group_id, active, created_at
            ) VALUES(?,?,'causal-request',?,1,?)",
            params![new_group_id, span_id, active_group, now],
        )?;

        transaction.execute(
            "INSERT INTO attributions(
                group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user
            ) VALUES(?,?,?,'primary',10000,'manual_attach',1,1)",
            params![new_group_id, target_project_id, work_item_id],
        )?;

        validate_group_invariant(&transaction, &new_group_id)?;

        changed = changed.saturating_add(1);
    }

    if !skipped_lifecycle.is_empty() {
        let note_id = stable_id(
            "anom",
            &format!("attach_skipped_lifecycle:{session_id}:{now}"),
        );
        let skipped_json =
            serde_json::json!({ "skipped_event_ids": skipped_lifecycle }).to_string();
        transaction.execute(
            "INSERT OR IGNORE INTO measurement_anomalies(
                id, session_id, turn_id, type, source_values_json, created_at
            ) VALUES(?,?,?,?,?,?)",
            params![
                note_id,
                session_id,
                Option::<String>::None,
                "attach_skipped_lifecycle_counter",
                skipped_json,
                now,
            ],
        )?;
    }

    transaction.execute(
        "UPDATE sessions SET project_id = ? WHERE id = ?",
        params![target_project_id, session_id],
    )?;

    transaction.commit()?;
    Ok(changed)
}

fn is_lifecycle_counter_kind(kind: &str) -> bool {
    matches!(
        kind,
        source_kind::FINAL_REQUEST_COUNTER
            | source_kind::SUBAGENT_STOP
            | source_kind::SUBAGENT_LIFECYCLE_COUNTER
    )
}

pub fn detach_session(connection: &mut Connection, session_id: &str) -> Result<u64> {
    let mut stmt = connection.prepare("SELECT id FROM usage_spans WHERE session_id = ?1")?;
    let span_ids = stmt
        .query_map([session_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let transaction = connection.transaction()?;
    let mut changed = 0;

    for span_id in span_ids {
        let c = transaction.execute(
            "UPDATE attribution_groups SET active = 0 WHERE usage_span_id = ?1 AND active = 1",
            [&span_id],
        )?;
        changed += c as u64;
    }

    transaction.execute(
        "UPDATE sessions SET project_id = NULL WHERE id = ?1",
        [session_id],
    )?;

    transaction.commit()?;
    Ok(changed)
}

pub fn add_note(
    connection: &mut Connection,
    text: &str,
    work_item_id: Option<&str>,
) -> Result<String> {
    if text.is_empty() || text.len() > 240 {
        bail!("Notes must be 1–240 characters");
    }

    let target_work_item_id = if let Some(wid) = work_item_id {
        wid.to_owned()
    } else {
        let active_work_item: Option<String> = connection
            .query_row(
                "SELECT work_item_id FROM manual_runs WHERE state = 'active' ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        match active_work_item {
            Some(wid) => wid,
            None => bail!("Specify a work item or start a manual run"),
        }
    };

    let project_id: String = connection
        .query_row(
            "SELECT project_id FROM work_items WHERE id = ?1",
            [&target_work_item_id],
            |row| row.get(0),
        )
        .with_context(|| format!("Unknown work item {target_work_item_id}"))?;

    let now = Utc::now().to_rfc3339();
    let note_id = stable_id("note", &format!("{target_work_item_id}:{now}:{text}"));

    connection.execute(
        "INSERT INTO notes(id, project_id, work_item_id, text, created_by, created_at)
         VALUES(?,?,?,?,'user',?)",
        params![note_id, project_id, target_work_item_id, text, now],
    )?;

    Ok(note_id)
}

pub fn rename_work_item(
    connection: &mut Connection,
    work_item_id: &str,
    new_title: &str,
) -> Result<()> {
    if new_title.trim().is_empty() {
        bail!("Work item title cannot be empty");
    }
    let changed = connection.execute(
        "UPDATE work_items SET title = ?1 WHERE id = ?2",
        params![new_title.trim(), work_item_id],
    )?;
    if changed == 0 {
        bail!("Work item {work_item_id} not found");
    }
    Ok(())
}

pub fn move_work_item(
    connection: &mut Connection,
    work_item_id: &str,
    new_parent_id: Option<&str>,
) -> Result<()> {
    if let Some(pid) = new_parent_id {
        if pid == work_item_id {
            bail!("Cannot set work item as its own parent");
        }
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_items WHERE id = ?1)",
            [pid],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("Parent work item {pid} not found");
        }
    }
    let changed = connection.execute(
        "UPDATE work_items SET parent_id = ?1 WHERE id = ?2",
        params![new_parent_id, work_item_id],
    )?;
    if changed == 0 {
        bail!("Work item {work_item_id} not found");
    }
    Ok(())
}

pub fn reclassify_work_item(
    connection: &mut Connection,
    work_item_id: &str,
    new_type: &str,
) -> Result<()> {
    if new_type.trim().is_empty() {
        bail!("Work item type cannot be empty");
    }
    let changed = connection.execute(
        "UPDATE work_items SET type = ?1 WHERE id = ?2",
        params![new_type.trim(), work_item_id],
    )?;
    if changed == 0 {
        bail!("Work item {work_item_id} not found");
    }
    Ok(())
}

pub fn validate_group_invariant(connection: &Connection, group_id: &str) -> Result<()> {
    let is_active: bool = connection
        .query_row(
            "SELECT active FROM attribution_groups WHERE id = ?1",
            [group_id],
            |row| row.get::<_, i64>(0).map(|v| v == 1),
        )
        .with_context(|| format!("Attribution group {group_id} not found"))?;

    if !is_active {
        return Ok(());
    }

    let total_bp: i64 = connection.query_row(
        "SELECT coalesce(sum(weight_basis_points), 0) FROM attributions WHERE group_id = ?1",
        [group_id],
        |row| row.get(0),
    )?;

    if total_bp != 10_000 {
        bail!(
            "Attribution group invariant violated for {group_id}: active weights total {total_bp} bp (must equal exactly 10,000 bp)"
        );
    }

    Ok(())
}

pub fn merge_work_items(
    connection: &mut Connection,
    source_id: &str,
    target_id: &str,
) -> Result<u64> {
    if source_id == target_id {
        bail!("Cannot merge work item into itself");
    }

    struct ItemInfo {
        project_id: String,
        status: String,
    }

    let source: ItemInfo = connection
        .query_row(
            "SELECT project_id, status FROM work_items WHERE id = ?1",
            [source_id],
            |row| {
                Ok(ItemInfo {
                    project_id: row.get(0)?,
                    status: row.get(1)?,
                })
            },
        )
        .with_context(|| format!("Source work item {source_id} not found"))?;

    let target: ItemInfo = connection
        .query_row(
            "SELECT project_id, status FROM work_items WHERE id = ?1",
            [target_id],
            |row| {
                Ok(ItemInfo {
                    project_id: row.get(0)?,
                    status: row.get(1)?,
                })
            },
        )
        .with_context(|| format!("Target work item {target_id} not found"))?;

    if source.project_id != target.project_id {
        bail!("Cross-project merge rejected: source and target must belong to the same project");
    }

    if source.status == "merged" {
        bail!("Source work item {source_id} is already merged");
    }

    // Find all DISTINCT active groups that contain an attribution row for source_id.
    let affected_group_ids: Vec<(String, String)> = {
        let mut stmt = connection.prepare(
            "SELECT DISTINCT ag.id, ag.usage_span_id
             FROM attribution_groups ag
             JOIN attributions a ON a.group_id = ag.id
             WHERE a.work_item_id = ?1 AND ag.active = 1",
        )?;
        stmt.query_map([source_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };

    let now = Utc::now().to_rfc3339();
    let tx = connection.transaction()?;

    // 1. Reparent any child work items to target
    tx.execute(
        "UPDATE work_items SET parent_id = ?1 WHERE parent_id = ?2",
        params![target_id, source_id],
    )?;

    // 2. Reparent any notes to target
    tx.execute(
        "UPDATE notes SET work_item_id = ?1 WHERE work_item_id = ?2",
        params![target_id, source_id],
    )?;

    // 3. For each affected group, reproduce ALL attribution rows,
    //    replacing source → target and combining weights if target already present.
    let mut reattributed_spans = 0u64;
    for (old_group_id, usage_span_id) in &affected_group_ids {
        // Read all rows from the superseded group
        let all_rows = read_group_attributions(&tx, old_group_id)?;

        // Deactivate the old group
        tx.execute(
            "UPDATE attribution_groups SET active = 0 WHERE id = ?1",
            [old_group_id],
        )?;

        // Build merged row set: replace source→target, combine if target already present
        let merged_rows = merge_attribution_rows(&all_rows, source_id, target_id);

        // Create replacement group
        let new_group_id = stable_id(
            "attr",
            &format!("{usage_span_id}:merge:{target_id}:{now}:{reattributed_spans}"),
        );
        tx.execute(
            "INSERT INTO attribution_groups(
                id, usage_span_id, policy, supersedes_group_id, active, created_at
            ) VALUES(?,?,'causal-request',?,1,?)",
            params![new_group_id, usage_span_id, old_group_id, now],
        )?;

        // Insert all replacement attribution rows
        for row in &merged_rows {
            tx.execute(
                "INSERT INTO attributions(
                    group_id, project_id, work_item_id, role, weight_basis_points,
                    method, confidence, verified_by_user
                ) VALUES(?,?,?,?,?,'manual_merge',?,1)",
                params![
                    new_group_id,
                    row.project_id,
                    row.work_item_id,
                    row.role,
                    row.weight_basis_points,
                    row.confidence.unwrap_or(1.0)
                ],
            )?;
        }

        validate_group_invariant(&tx, &new_group_id)?;
        reattributed_spans = reattributed_spans.saturating_add(1);
    }

    // 4. Mark source item as merged
    tx.execute(
        "UPDATE work_items SET status = 'merged' WHERE id = ?1",
        [source_id],
    )?;

    tx.commit()?;
    Ok(reattributed_spans)
}

pub fn split_work_item(
    connection: &mut Connection,
    source_id: &str,
    new_title: &str,
    span_ids: &[String],
) -> Result<String> {
    let title = new_title.trim();
    if title.is_empty() {
        bail!("Split work item title cannot be empty");
    }
    if span_ids.is_empty() {
        bail!("Must select at least one usage span to split");
    }

    struct SourceInfo {
        project_id: String,
        parent_id: Option<String>,
    }

    let source: SourceInfo = connection
        .query_row(
            "SELECT project_id, parent_id FROM work_items WHERE id = ?1",
            [source_id],
            |row| {
                Ok(SourceInfo {
                    project_id: row.get(0)?,
                    parent_id: row.get(1)?,
                })
            },
        )
        .with_context(|| format!("Source work item {source_id} not found"))?;

    // Validate span selections and collect their group info
    struct SpanInfo {
        span_id: String,
        group_id: String,
    }

    let mut span_infos = Vec::new();
    for span_id in span_ids {
        let info: Option<SpanInfo> = connection
            .query_row(
                "SELECT ag.usage_span_id, ag.id
                 FROM attribution_groups ag
                 JOIN attributions a ON a.group_id = ag.id
                 WHERE ag.usage_span_id = ?1 AND a.work_item_id = ?2 AND ag.active = 1",
                params![span_id, source_id],
                |row| {
                    Ok(SpanInfo {
                        span_id: row.get(0)?,
                        group_id: row.get(1)?,
                    })
                },
            )
            .ok();

        let Some(info) = info else {
            bail!(
                "Invalid span selection: span {span_id} is not currently actively attributed to work item {source_id}"
            );
        };
        span_infos.push(info);
    }

    let now = Utc::now().to_rfc3339();
    let new_work_item_id = stable_id("wi", &format!("{}:{}:{}", source.project_id, title, now));

    let tx = connection.transaction()?;

    // 1. Create new work item
    tx.execute(
        "INSERT INTO work_items(id, project_id, parent_id, type, title, status, created_at)
         VALUES(?,?,?,'task',?,'open',?)",
        params![
            new_work_item_id,
            source.project_id,
            source.parent_id,
            title,
            now
        ],
    )?;

    // 2. For each selected span, create a replacement group reproducing ALL rows
    //    but retargeting the source's row to the new work item.
    for (idx, info) in span_infos.iter().enumerate() {
        // Read ALL attribution rows from the superseded group
        let all_rows = read_group_attributions(&tx, &info.group_id)?;

        // Deactivate the old group
        tx.execute(
            "UPDATE attribution_groups SET active = 0 WHERE id = ?1",
            [&info.group_id],
        )?;

        // Build split row set: retarget source → new_work_item_id,
        // preserving all other targets exactly as-is
        let split_rows = split_attribution_rows(&all_rows, source_id, &new_work_item_id);

        // Create replacement group
        let new_group_id = stable_id(
            "attr",
            &format!("{}:split:{new_work_item_id}:{now}:{idx}", info.span_id),
        );
        tx.execute(
            "INSERT INTO attribution_groups(
                id, usage_span_id, policy, supersedes_group_id, active, created_at
            ) VALUES(?,?,'causal-request',?,1,?)",
            params![new_group_id, info.span_id, info.group_id, now],
        )?;

        // Insert all replacement attribution rows
        for row in &split_rows {
            tx.execute(
                "INSERT INTO attributions(
                    group_id, project_id, work_item_id, role, weight_basis_points,
                    method, confidence, verified_by_user
                ) VALUES(?,?,?,?,?,'manual_split',?,1)",
                params![
                    new_group_id,
                    row.project_id,
                    row.work_item_id,
                    row.role,
                    row.weight_basis_points,
                    row.confidence.unwrap_or(1.0)
                ],
            )?;
        }

        validate_group_invariant(&tx, &new_group_id)?;
    }

    tx.commit()?;
    Ok(new_work_item_id)
}

// ── Attribution row helpers ──────────────────────────────────────────────────

/// A single attribution row read from an existing group.
#[derive(Clone, Debug)]
struct AttrRow {
    project_id: Option<String>,
    work_item_id: Option<String>,
    role: String,
    weight_basis_points: i64,
    confidence: Option<f64>,
}

/// Read ALL attribution rows from a group (active or not).
fn read_group_attributions(conn: &Connection, group_id: &str) -> Result<Vec<AttrRow>> {
    let mut stmt = conn.prepare(
        "SELECT project_id, work_item_id, role, weight_basis_points, confidence
         FROM attributions WHERE group_id = ?1
         ORDER BY role",
    )?;
    let rows = stmt
        .query_map([group_id], |row| {
            Ok(AttrRow {
                project_id: row.get(0)?,
                work_item_id: row.get(1)?,
                role: row.get(2)?,
                weight_basis_points: row.get(3)?,
                confidence: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// For merge: reproduce all rows from the old group, but replace any row
/// targeting `source_id` with `target_id`. If `target_id` already has a row,
/// combine the weights (and take the higher confidence).
fn merge_attribution_rows(rows: &[AttrRow], source_id: &str, target_id: &str) -> Vec<AttrRow> {
    let mut result: Vec<AttrRow> = Vec::new();
    let mut target_combined = false;

    for row in rows {
        let is_source = row.work_item_id.as_deref() == Some(source_id);
        let is_target = row.work_item_id.as_deref() == Some(target_id);

        if is_source {
            // Find if target already exists in result or in remaining rows
            if let Some(existing) = result
                .iter_mut()
                .find(|r| r.work_item_id.as_deref() == Some(target_id))
            {
                // Combine: add source weight to existing target row
                existing.weight_basis_points += row.weight_basis_points;
                existing.confidence = match (existing.confidence, row.confidence) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                target_combined = true;
            } else {
                // No target row yet; retarget source → target
                let mut new_row = row.clone();
                new_row.work_item_id = Some(target_id.to_string());
                // Derive a role suffix when combining later
                result.push(new_row);
            }
        } else if is_target && target_combined {
            // Already combined into the retargeted source row above;
            // this shouldn't happen because we process sequentially, but guard.
            continue;
        } else {
            // Keep unmodified
            result.push(row.clone());
        }
    }

    // Second pass: if target appeared AFTER the source row in the original,
    // we need to combine it into the already-pushed retargeted row.
    let mut final_result: Vec<AttrRow> = Vec::new();
    let mut seen_target = false;
    for row in result {
        let is_target = row.work_item_id.as_deref() == Some(target_id);
        if is_target {
            if seen_target {
                // Duplicate target; combine into the first
                if let Some(first) = final_result
                    .iter_mut()
                    .find(|r| r.work_item_id.as_deref() == Some(target_id))
                {
                    first.weight_basis_points += row.weight_basis_points;
                    first.confidence = match (first.confidence, row.confidence) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                    continue;
                }
            }
            seen_target = true;
        }
        final_result.push(row);
    }

    final_result
}

/// For split: reproduce all rows from the old group, but retarget the source's
/// row to `new_work_item_id`, preserving all other active targets exactly.
fn split_attribution_rows(
    rows: &[AttrRow],
    source_id: &str,
    new_work_item_id: &str,
) -> Vec<AttrRow> {
    rows.iter()
        .map(|row| {
            if row.work_item_id.as_deref() == Some(source_id) {
                let mut new_row = row.clone();
                new_row.work_item_id = Some(new_work_item_id.to_string());
                new_row
            } else {
                row.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;
    use crate::manual::{ManualStartInput, start_manual, stop_manual};

    #[test]
    fn attach_detach_and_notes_work_correctly() {
        let mut ledger = Ledger::open_memory().unwrap();
        let run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "space-game",
                project_title: Some("Space Game"),
                task_title: "Fix collision bug",
                parent_title: Some("Build initial game"),
                cwd: "/tmp/game",
            },
        )
        .unwrap();

        stop_manual(ledger.connection_mut(), Default::default()).unwrap();

        // Add note
        let note_id = add_note(
            ledger.connection_mut(),
            "Collision bug reproducible",
            Some(&run.work_item_id),
        )
        .unwrap();
        assert!(note_id.starts_with("note_"));

        // Detach
        let detached = detach_session(ledger.connection_mut(), &run.session_id).unwrap();
        assert_eq!(detached, 1);

        let active_count: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM attribution_groups WHERE active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 0);

        // Attach
        let attached =
            attach_session(ledger.connection_mut(), &run.session_id, &run.work_item_id).unwrap();
        assert_eq!(attached, 1);

        let total_weight: i64 = ledger
            .connection()
            .query_row(
                "SELECT sum(weight_basis_points) FROM attributions a
                 JOIN attribution_groups ag ON ag.id = a.group_id
                 WHERE ag.active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total_weight, 10_000);
    }

    #[test]
    fn test_merge_work_items_preserves_usage_and_enforces_10000_bp() {
        let mut ledger = Ledger::open_memory().unwrap();
        let run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "space-game",
                project_title: Some("Space Game"),
                task_title: "Fix collision bug",
                parent_title: None,
                cwd: "/tmp/game",
            },
        )
        .unwrap();

        stop_manual(ledger.connection_mut(), Default::default()).unwrap();

        // Create target work item in the same project
        let target_id = stable_id("wi", "space-game:target-task");
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                 VALUES (?1, ?2, 'task', 'Target Task', 'open', '2026-09-29T10:00:00Z')",
                params![target_id, run.project_id],
            )
            .unwrap();

        add_note(
            ledger.connection_mut(),
            "Important bug note",
            Some(&run.work_item_id),
        )
        .unwrap();

        // Perform merge
        let reattributed =
            merge_work_items(ledger.connection_mut(), &run.work_item_id, &target_id).unwrap();
        assert_eq!(reattributed, 1);

        // Verify source status is merged
        let source_status: String = ledger
            .connection()
            .query_row(
                "SELECT status FROM work_items WHERE id = ?1",
                [&run.work_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_status, "merged");

        // Verify note reparented
        let note_wi: String = ledger
            .connection()
            .query_row(
                "SELECT work_item_id FROM notes WHERE text = 'Important bug note'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(note_wi, target_id);

        // Verify active attribution points to target with exactly 10,000 bp
        let (active_wi, active_bp): (String, i64) = ledger
            .connection()
            .query_row(
                "SELECT a.work_item_id, a.weight_basis_points
                 FROM attribution_groups ag
                 JOIN attributions a ON a.group_id = ag.id
                 WHERE ag.active = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(active_wi, target_id);
        assert_eq!(active_bp, 10_000);

        // Verify superseded group exists and is inactive
        let inactive_count: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM attribution_groups WHERE active = 0 AND supersedes_group_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(inactive_count, 0); // supersedes_group_id is on the NEW group, not old group!
        let superseded_group_count: i64 = ledger
            .connection()
            .query_row(
                "SELECT count(*) FROM attribution_groups WHERE active = 1 AND supersedes_group_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(superseded_group_count, 1);
    }

    #[test]
    fn test_merge_cross_project_and_self_fail_safely() {
        let mut ledger = Ledger::open_memory().unwrap();

        // Project A
        let run_a = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "proj-a",
                project_title: Some("Project A"),
                task_title: "Task A",
                parent_title: None,
                cwd: "/tmp/a",
            },
        )
        .unwrap();

        // Project B
        let run_b = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "proj-b",
                project_title: Some("Project B"),
                task_title: "Task B",
                parent_title: None,
                cwd: "/tmp/b",
            },
        )
        .unwrap();

        // Cross-project merge fails
        let err = merge_work_items(
            ledger.connection_mut(),
            &run_a.work_item_id,
            &run_b.work_item_id,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Cross-project merge rejected"));

        // Merge into self fails
        let self_err = merge_work_items(
            ledger.connection_mut(),
            &run_a.work_item_id,
            &run_a.work_item_id,
        )
        .unwrap_err();
        assert!(self_err.to_string().contains("into itself"));
    }

    #[test]
    fn test_split_work_item_moves_selected_spans_and_enforces_invariant() {
        let mut ledger = Ledger::open_memory().unwrap();
        let run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "split-proj",
                project_title: Some("Split Project"),
                task_title: "Initial Big Task",
                parent_title: None,
                cwd: "/tmp/split",
            },
        )
        .unwrap();

        // Add a second span to the session
        let span_2_id = stable_id("span", "second-event-span");
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_spans(id, session_id, measurement_status, completeness)
                 VALUES(?1, ?2, 'measured', 100)",
                params![span_2_id, run.session_id],
            )
            .unwrap();

        let ag_2_id = stable_id("attr", "span-2-initial");
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
                 VALUES(?1, ?2, 'causal-request', 1, '2026-09-29T10:00:00Z')",
                params![ag_2_id, span_2_id],
            )
            .unwrap();

        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions(group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
                 VALUES(?1, ?2, ?3, 'primary', 10000, 'session_default', 1.0, 1)",
                params![ag_2_id, run.project_id, run.work_item_id],
            )
            .unwrap();

        // Split span_2 into a new work item
        let new_wi = split_work_item(
            ledger.connection_mut(),
            &run.work_item_id,
            "Extracted Second Task",
            std::slice::from_ref(&span_2_id),
        )
        .unwrap();

        assert_ne!(new_wi, run.work_item_id);

        // Verify span_2 is now attributed to new_wi with 10,000 bp
        let (wi_for_span_2, bp_for_span_2): (String, i64) = ledger
            .connection()
            .query_row(
                "SELECT a.work_item_id, a.weight_basis_points
                 FROM attribution_groups ag
                 JOIN attributions a ON a.group_id = ag.id
                 WHERE ag.usage_span_id = ?1 AND ag.active = 1",
                [&span_2_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(wi_for_span_2, new_wi);
        assert_eq!(bp_for_span_2, 10_000);

        // Invalid span selection fails
        let invalid_err = split_work_item(
            ledger.connection_mut(),
            &run.work_item_id,
            "Failing Task",
            &["nonexistent_span".to_string()],
        )
        .unwrap_err();
        assert!(invalid_err.to_string().contains("Invalid span selection"));
    }

    #[test]
    fn test_attribution_group_invariant_rejections() {
        let mut ledger = Ledger::open_memory().unwrap();

        // Insert incomplete group with 9,999 bp
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
                 VALUES ('p1', 'p1', 'P1', 'h1', 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                 VALUES ('w1', 'p1', 'task', 'W1', 'open', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO sessions (id, adapter, provider_session_id, project_id, started_at)
                 VALUES ('s1', 'claude', 's1', 'p1', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_spans (id, session_id, measurement_status) VALUES ('sp1', 's1', 'measured')",
                [],
            )
            .unwrap();

        // 9,999 bp group
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
                 VALUES ('ag_9999', 'sp1', 'causal-request', 1, '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
                 VALUES ('ag_9999', 'p1', 'w1', 'primary', 9999, 'manual')",
                [],
            )
            .unwrap();

        let err_9999 = validate_group_invariant(ledger.connection(), "ag_9999").unwrap_err();
        assert!(err_9999.to_string().contains("9999 bp"));

        // 10,001 bp group (via two roles totaling 10,001)
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_spans (id, session_id, measurement_status) VALUES ('sp2', 's1', 'measured')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
                 VALUES ('ag_10001', 'sp2', 'causal-request', 1, '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
                 VALUES ('ag_10001', 'p1', 'w1', 'primary', 10000, 'manual')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
                 VALUES ('ag_10001', 'p1', 'w1', 'secondary', 1, 'manual')",
                [],
            )
            .unwrap();

        let err_10001 = validate_group_invariant(ledger.connection(), "ag_10001").unwrap_err();
        assert!(err_10001.to_string().contains("10001 bp"));
    }

    #[test]
    fn test_negative_and_duplicate_attribution_group_rejections() {
        let mut ledger = Ledger::open_memory().unwrap();

        ledger
            .connection_mut()
            .execute(
                "INSERT INTO projects (id, key, display_name, identity_hash, detection_method, created_at, updated_at)
                 VALUES ('p_neg', 'p_neg', 'P Neg', 'h_neg', 'git', '2026-09-29T10:00:00Z', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                 VALUES ('w_neg', 'p_neg', 'task', 'W Neg', 'open', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO sessions (id, adapter, provider_session_id, project_id, started_at)
                 VALUES ('s_neg', 'claude', 's_neg', 'p_neg', '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_spans (id, session_id, measurement_status) VALUES ('sp_neg', 's_neg', 'measured')",
                [],
            )
            .unwrap();
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
                 VALUES ('ag_neg', 'sp_neg', 'causal-request', 1, '2026-09-29T10:00:00Z')",
                [],
            )
            .unwrap();

        // 1. Negative weight is rejected by database CHECK constraint
        let neg_err = ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
                 VALUES ('ag_neg', 'p_neg', 'w_neg', 'primary', -500, 'manual')",
                [],
            );
        assert!(neg_err.is_err(), "Negative basis points must be rejected");

        // 2. Inserting a second active attribution group for the same usage_span_id violates unique index
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions (group_id, project_id, work_item_id, role, weight_basis_points, method)
                 VALUES ('ag_neg', 'p_neg', 'w_neg', 'primary', 10000, 'manual')",
                [],
            )
            .unwrap();

        let dup_active_err = ledger.connection_mut().execute(
            "INSERT INTO attribution_groups (id, usage_span_id, policy, active, created_at)
                 VALUES ('ag_neg_dup', 'sp_neg', 'causal-request', 1, '2026-09-29T10:00:00Z')",
            [],
        );
        assert!(
            dup_active_err.is_err(),
            "Duplicate active group for same span must violate unique index"
        );
    }

    #[test]
    fn test_superseding_group_lineage_and_deactivation() {
        let mut ledger = Ledger::open_memory().unwrap();
        let run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "lineage-proj",
                project_title: Some("Lineage Project"),
                task_title: "Lineage Task 1",
                parent_title: None,
                cwd: "/tmp/lineage",
            },
        )
        .unwrap();

        let target_run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "lineage-proj",
                project_title: Some("Lineage Project"),
                task_title: "Lineage Task 2",
                parent_title: None,
                cwd: "/tmp/lineage",
            },
        )
        .unwrap();

        let span_1_id = stable_id("span", "lineage-span-1");
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO usage_spans(id, session_id, measurement_status, completeness)
                 VALUES(?1, ?2, 'measured', 100)",
                params![span_1_id, run.session_id],
            )
            .unwrap();

        let old_group_id = stable_id("attr", "lineage-span-1-initial");
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
                 VALUES(?1, ?2, 'causal-request', 1, '2026-09-29T10:00:00Z')",
                params![old_group_id, span_1_id],
            )
            .unwrap();

        ledger
            .connection_mut()
            .execute(
                "INSERT INTO attributions(group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
                 VALUES(?1, ?2, ?3, 'primary', 10000, 'session_default', 1.0, 1)",
                params![old_group_id, run.project_id, run.work_item_id],
            )
            .unwrap();

        // Merge Task 1 into Task 2
        let count = merge_work_items(
            ledger.connection_mut(),
            &run.work_item_id,
            &target_run.work_item_id,
        )
        .unwrap();
        assert!(count >= 1);

        // Verify old group is deactivated
        let old_active: i64 = ledger
            .connection()
            .query_row(
                "SELECT active FROM attribution_groups WHERE id = ?1",
                [&old_group_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            old_active, 0,
            "Superseded group must be deactivated (active = 0)"
        );

        // Verify new group supersedes old group and is active
        let (new_group_id, supersedes_id, new_active): (String, Option<String>, i64) = ledger
            .connection()
            .query_row(
                "SELECT id, supersedes_group_id, active FROM attribution_groups WHERE supersedes_group_id = ?1",
                [&old_group_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(supersedes_id, Some(old_group_id));
        assert_eq!(new_active, 1);

        // Verify invariant on replacement group
        validate_group_invariant(ledger.connection(), &new_group_id).unwrap();
    }

    #[test]
    fn test_transactional_rollback_preserves_state() {
        let mut ledger = Ledger::open_memory().unwrap();
        let run = start_manual(
            ledger.connection_mut(),
            ManualStartInput {
                project_key: "rollback-proj",
                project_title: Some("Rollback Project"),
                task_title: "Rollback Task",
                parent_title: None,
                cwd: "/tmp/rollback",
            },
        )
        .unwrap();

        // Invalid span list in split should fail and leave work items unmodified
        let initial_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM work_items", [], |row| row.get(0))
            .unwrap();

        let split_res = split_work_item(
            ledger.connection_mut(),
            &run.work_item_id,
            "Failed Split",
            &[
                "nonexistent_span_1".to_string(),
                "nonexistent_span_2".to_string(),
            ],
        );
        assert!(split_res.is_err());

        let post_count: i64 = ledger
            .connection()
            .query_row("SELECT count(*) FROM work_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            initial_count, post_count,
            "Failed split must rollback work_items insertion"
        );

        // Status of original work item is unchanged
        let status: String = ledger
            .connection()
            .query_row(
                "SELECT status FROM work_items WHERE id = ?1",
                [&run.work_item_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "open");
    }

    #[test]
    fn test_concurrent_corrections_merge_and_split() {
        use std::sync::Arc;
        use std::thread;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let db_path = temp.path().join("concurrent_corrections.db");

        let (run_a_id, run_b_id, span_a_id) = {
            let mut ledger = Ledger::open(&db_path).unwrap();
            let run_a = start_manual(
                ledger.connection_mut(),
                ManualStartInput {
                    project_key: "conc-proj",
                    project_title: Some("Concurrent Project"),
                    task_title: "Task A",
                    parent_title: None,
                    cwd: "/tmp/conc",
                },
            )
            .unwrap();

            let run_b = start_manual(
                ledger.connection_mut(),
                ManualStartInput {
                    project_key: "conc-proj",
                    project_title: Some("Concurrent Project"),
                    task_title: "Task B",
                    parent_title: None,
                    cwd: "/tmp/conc",
                },
            )
            .unwrap();

            let span_a = stable_id("span", "conc-span-a");
            ledger
                .connection_mut()
                .execute(
                    "INSERT INTO usage_spans(id, session_id, measurement_status, completeness)
                     VALUES(?1, ?2, 'measured', 100)",
                    params![span_a, run_a.session_id],
                )
                .unwrap();

            let ag_a = stable_id("attr", "conc-span-a-ag");
            ledger
                .connection_mut()
                .execute(
                    "INSERT INTO attribution_groups(id, usage_span_id, policy, active, created_at)
                     VALUES(?1, ?2, 'causal-request', 1, '2026-09-29T10:00:00Z')",
                    params![ag_a, span_a],
                )
                .unwrap();

            ledger
                .connection_mut()
                .execute(
                    "INSERT INTO attributions(group_id, project_id, work_item_id, role, weight_basis_points, method, confidence, verified_by_user)
                     VALUES(?1, ?2, ?3, 'primary', 10000, 'session_default', 1.0, 1)",
                    params![ag_a, run_a.project_id, run_a.work_item_id],
                )
                .unwrap();

            (run_a.work_item_id, run_b.work_item_id, span_a)
        };

        let path_arc = Arc::new(db_path.clone());
        let p1 = Arc::clone(&path_arc);
        let run_a_clone = run_a_id.clone();
        let span_a_clone = span_a_id.clone();

        let handle_split = thread::spawn(move || -> anyhow::Result<()> {
            let mut ledger = Ledger::open(&*p1)?;
            let new_item = split_work_item(
                ledger.connection_mut(),
                &run_a_clone,
                "Concurrent Split Task",
                &[span_a_clone],
            )?;
            assert!(!new_item.is_empty());
            Ok(())
        });

        let p2 = Arc::clone(&path_arc);
        let run_b_clone = run_b_id.clone();
        let handle_rename = thread::spawn(move || -> anyhow::Result<()> {
            let mut ledger = Ledger::open(&*p2)?;
            rename_work_item(
                ledger.connection_mut(),
                &run_b_clone,
                "Renamed Concurrent Task B",
            )?;
            add_note(
                ledger.connection_mut(),
                "Concurrent note during split",
                Some(&run_b_clone),
            )?;
            Ok(())
        });

        handle_split.join().unwrap().unwrap();
        handle_rename.join().unwrap().unwrap();

        // Verify ledger integrity and invariant across all active groups
        let ledger = Ledger::open(&db_path).unwrap();
        assert_eq!(ledger.integrity_check().unwrap(), "ok");

        let active_groups: Vec<String> = {
            let mut stmt = ledger
                .connection()
                .prepare("SELECT id FROM attribution_groups WHERE active = 1")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        for g in active_groups {
            validate_group_invariant(ledger.connection(), &g).unwrap();
        }
    }
}
