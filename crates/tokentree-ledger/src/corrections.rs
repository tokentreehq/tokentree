// SPDX-License-Identifier: Apache-2.0
use crate::stable_id;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params};

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

    let mut stmt = connection.prepare(
        "SELECT id, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens
         FROM usage_events WHERE session_id = ?1",
    )?;

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

    let mut created = 0;
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

        created += 1;
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

    let mut stmt = connection.prepare("SELECT id FROM usage_events WHERE session_id = ?1")?;
    let event_ids = stmt
        .query_map([session_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let now = Utc::now().to_rfc3339();
    let mut changed = 0;
    let transaction = connection.transaction()?;

    for event_id in event_ids {
        let span_id = stable_id("span", &event_id);
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

        changed += 1;
    }

    transaction.execute(
        "UPDATE sessions SET project_id = ? WHERE id = ?",
        params![target_project_id, session_id],
    )?;

    transaction.commit()?;
    Ok(changed)
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
}
