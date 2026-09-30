// SPDX-License-Identifier: Apache-2.0
use anyhow::{Result, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    pub requests: u64,
    pub measured: u64,
    pub unavailable: u64,
    pub priced: u64,
    pub amount_micros: u64,
}

impl UsageTotals {
    #[must_use]
    pub fn add(&self, other: &Self) -> Self {
        Self {
            input: self.input + other.input,
            cache_read: self.cache_read + other.cache_read,
            cache_write: self.cache_write + other.cache_write,
            output: self.output + other.output,
            reasoning: self.reasoning + other.reasoning,
            requests: self.requests + other.requests,
            measured: self.measured + other.measured,
            unavailable: self.unavailable + other.unavailable,
            priced: self.priced + other.priced,
            amount_micros: self.amount_micros + other.amount_micros,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkTreeNode {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub direct: UsageTotals,
    pub inclusive: UsageTotals,
    pub children: Vec<WorkTreeNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectTree {
    pub id: String,
    pub key: String,
    pub title: String,
    pub roots: Vec<WorkTreeNode>,
    pub totals: UsageTotals,
}

struct ProjectRow {
    id: String,
    key: String,
    display_name: String,
}

pub fn load_project_trees(
    connection: &Connection,
    project_filter: Option<&str>,
) -> Result<Vec<ProjectTree>> {
    let mut projects = Vec::new();
    if let Some(filter) = project_filter {
        let pattern = format!("%{filter}%");
        let mut stmt = connection.prepare(
            "SELECT id, key, display_name FROM projects WHERE key = ?1 OR display_name LIKE ?2 ORDER BY display_name",
        )?;
        let rows = stmt.query_map([filter, &pattern], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                key: row.get(1)?,
                display_name: row.get(2)?,
            })
        })?;
        for r in rows {
            projects.push(r?);
        }
    } else {
        let mut stmt = connection
            .prepare("SELECT id, key, display_name FROM projects ORDER BY display_name")?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                key: row.get(1)?,
                display_name: row.get(2)?,
            })
        })?;
        for r in rows {
            projects.push(r?);
        }
    }

    let mut trees = Vec::new();
    for p in projects {
        trees.push(load_single_project_tree(connection, &p)?);
    }
    Ok(trees)
}

fn load_single_project_tree(connection: &Connection, project: &ProjectRow) -> Result<ProjectTree> {
    struct RawWorkItemRow {
        id: String,
        parent_id: Option<String>,
        title: String,
        input: i64,
        cache_read: i64,
        cache_write: i64,
        output: i64,
        reasoning: i64,
        requests: i64,
        measured: i64,
        unavailable: i64,
        priced: i64,
        amount_micros: i64,
    }

    let policy = crate::resolve_subagent_policy(connection);
    let policy_str = match policy {
        crate::SubagentPolicy::AlreadyInParent => "AlreadyInParent",
        crate::SubagentPolicy::Independent => "Independent",
        crate::SubagentPolicy::Unknown => "Unknown",
    };

    let is_covered = "(ue.source_kind IN ('codex_turn_counter', 'turn_counter', 'turn_summary', 'cumulative_turn_counter') AND EXISTS (SELECT 1 FROM usage_events d WHERE d.session_id = ue.session_id AND d.turn_id IS NOT NULL AND d.turn_id = ue.turn_id AND d.source_kind NOT IN ('codex_turn_counter', 'turn_counter', 'turn_summary', 'cumulative_turn_counter', 'final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter')))";

    let query = format!(
        "SELECT wi.id, wi.parent_id, wi.title,
                coalesce(sum(CASE
                    WHEN ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(ue.input_tokens * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) input,
                coalesce(sum(CASE
                    WHEN ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(ue.cached_input_tokens * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) cache_read,
                coalesce(sum(CASE
                    WHEN ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(ue.cache_write_tokens * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) cache_write,
                coalesce(sum(CASE
                    WHEN ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(ue.output_tokens * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) output,
                coalesce(sum(CASE
                    WHEN ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(ue.reasoning_tokens * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) reasoning,
                coalesce(sum(CASE
                    WHEN ue.id IS NULL OR ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE 1
                END), 0) requests,
                coalesce(sum(CASE
                    WHEN ue.id IS NULL OR ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN us.measurement_status='measured' THEN 1
                    ELSE 0
                END), 0) measured,
                coalesce(sum(CASE
                    WHEN ue.id IS NULL OR ue.source_kind IN ('final_request_counter', 'subagent_stop', 'subagent_lifecycle_counter') THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 1
                    WHEN us.measurement_status<>'measured' THEN 1
                    ELSE 0
                END), 0) unavailable,
                count(CASE
                    WHEN cc.usage_event_id IS NULL THEN NULL
                    WHEN {is_covered} THEN NULL
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN NULL
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN NULL
                    ELSE cc.usage_event_id
                END) priced,
                coalesce(sum(CASE
                    WHEN cc.amount_micros IS NULL THEN 0
                    WHEN {is_covered} THEN 0
                    WHEN ?2 = 'AlreadyInParent' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    WHEN ?2 = 'Unknown' AND (ue.parent_agent_id IS NOT NULL OR ue.source_kind LIKE '%subagent%' OR ue.session_id IN (SELECT s.id FROM sessions s WHERE s.root_session_id IS NOT NULL AND s.root_session_id <> s.id)) THEN 0
                    ELSE CAST(ROUND(cc.amount_micros * a.weight_basis_points / 10000.0) AS INTEGER)
                END), 0) amount_micros
         FROM work_items wi
         LEFT JOIN attributions a ON a.work_item_id = wi.id
         LEFT JOIN attribution_groups ag ON ag.id = a.group_id AND ag.active = 1
         LEFT JOIN usage_spans us ON us.id = ag.usage_span_id
         LEFT JOIN usage_events ue ON ue.id = json_extract(us.measured_usage_json, '$.usage_event_id')
         LEFT JOIN cost_calculations cc ON cc.rowid = (
             SELECT c2.rowid FROM cost_calculations c2 WHERE c2.usage_event_id = ue.id ORDER BY c2.calculated_at DESC LIMIT 1
         )
         WHERE wi.project_id = ?1
         GROUP BY wi.id
         ORDER BY wi.created_at, wi.title"
    );
    let mut stmt = connection.prepare(&query)?;

    let raw_rows = stmt
        .query_map([&project.id, policy_str], |row| {
            Ok(RawWorkItemRow {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                title: row.get(2)?,
                input: row.get(3)?,
                cache_read: row.get(4)?,
                cache_write: row.get(5)?,
                output: row.get(6)?,
                reasoning: row.get(7)?,
                requests: row.get(8)?,
                measured: row.get(9)?,
                unavailable: row.get(10)?,
                priced: row.get(11)?,
                amount_micros: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut nodes_map: HashMap<String, WorkTreeNode> = HashMap::new();
    let mut node_order: Vec<String> = Vec::new();

    for r in raw_rows {
        let direct = UsageTotals {
            input: r.input.max(0) as u64,
            cache_read: r.cache_read.max(0) as u64,
            cache_write: r.cache_write.max(0) as u64,
            output: r.output.max(0) as u64,
            reasoning: r.reasoning.max(0) as u64,
            requests: r.requests.max(0) as u64,
            measured: r.measured.max(0) as u64,
            unavailable: r.unavailable.max(0) as u64,
            priced: r.priced.max(0) as u64,
            amount_micros: r.amount_micros.max(0) as u64,
        };
        node_order.push(r.id.clone());
        nodes_map.insert(
            r.id.clone(),
            WorkTreeNode {
                id: r.id,
                title: r.title,
                parent_id: r.parent_id,
                direct,
                inclusive: UsageTotals::default(),
                children: Vec::new(),
            },
        );
    }

    // Build children structure
    let mut child_ids_by_parent: HashMap<String, Vec<String>> = HashMap::new();
    let mut root_ids: Vec<String> = Vec::new();

    for id in &node_order {
        let node = &nodes_map[id];
        if let Some(pid) = &node.parent_id {
            if nodes_map.contains_key(pid) {
                child_ids_by_parent
                    .entry(pid.clone())
                    .or_default()
                    .push(id.clone());
                continue;
            }
        }
        root_ids.push(id.clone());
    }

    fn assemble_tree(
        id: &str,
        nodes_map: &HashMap<String, WorkTreeNode>,
        children_map: &HashMap<String, Vec<String>>,
    ) -> WorkTreeNode {
        let node = &nodes_map[id];
        let mut assembled_children = Vec::new();
        if let Some(cids) = children_map.get(id) {
            for cid in cids {
                assembled_children.push(assemble_tree(cid, nodes_map, children_map));
            }
        }
        let mut inclusive = node.direct.clone();
        for child in &assembled_children {
            inclusive = inclusive.add(&child.inclusive);
        }
        WorkTreeNode {
            id: node.id.clone(),
            title: node.title.clone(),
            parent_id: node.parent_id.clone(),
            direct: node.direct.clone(),
            inclusive,
            children: assembled_children,
        }
    }

    let mut roots = Vec::new();
    let mut project_totals = UsageTotals::default();
    for rid in root_ids {
        let root = assemble_tree(&rid, &nodes_map, &child_ids_by_parent);
        project_totals = project_totals.add(&root.inclusive);
        roots.push(root);
    }

    Ok(ProjectTree {
        id: project.id.clone(),
        key: project.key.clone(),
        title: project.display_name.clone(),
        roots,
        totals: project_totals,
    })
}

#[must_use]
pub fn format_totals(value: &UsageTotals) -> String {
    let tokens =
        value.input + value.cache_read + value.cache_write + value.output + value.reasoning;
    let complete = if value.requests == 0 {
        None
    } else {
        Some(100.0 * value.measured as f64 / (value.measured + value.unavailable) as f64)
    };
    let cost = if value.requests > 0 && value.priced == value.requests {
        format!(
            "${:.2} est. API-equivalent",
            value.amount_micros as f64 / 1_000_000.0
        )
    } else {
        "cost unavailable".to_owned()
    };
    let comp_str = complete.map_or_else(|| "—".into(), |c| format!("{c:.0}% complete"));
    format!(
        "{cost} · {tokens} tok · {comp_str} · {} unavailable",
        value.unavailable
    )
}

#[must_use]
pub fn render_project_trees(trees: &[ProjectTree]) -> String {
    if trees.is_empty() {
        return "No projects in the ledger.".to_owned();
    }
    let mut lines = Vec::new();
    for tree in trees {
        lines.push(format!("{} — {}", tree.title, format_totals(&tree.totals)));
        walk_render(&tree.roots, "", &mut lines);
    }
    lines.join("\n")
}

fn walk_render(nodes: &[WorkTreeNode], prefix: &str, lines: &mut Vec<String>) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let branch = if is_last { "└── " } else { "├── " };
        lines.push(format!(
            "{prefix}{branch}{}  {}",
            node.title,
            format_totals(&node.inclusive)
        ));
        let next_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
        walk_render(&node.children, &next_prefix, lines);
    }
}

pub fn query_ledger(
    connection: &Connection,
    project_filter: Option<&str>,
    work_item_filter: Option<&str>,
    include_descendants: bool,
) -> Result<serde_json::Value> {
    let trees = load_project_trees(connection, project_filter)?;
    if trees.is_empty() {
        bail!("Project not found");
    }
    if trees.len() > 1 {
        let names: Vec<&str> = trees.iter().map(|t| t.title.as_str()).collect();
        bail!("Project is ambiguous ({})", names.join(", "));
    }
    let tree = &trees[0];
    let Some(work_item_query) = work_item_filter else {
        return Ok(serde_json::to_value(tree)?);
    };

    let query_lower = work_item_query.to_ascii_lowercase();
    let mut matches = Vec::new();

    fn find_matches(node: &WorkTreeNode, q: &str, acc: &mut Vec<WorkTreeNode>) {
        if node.title.to_ascii_lowercase().contains(q) {
            acc.push(node.clone());
        }
        for child in &node.children {
            find_matches(child, q, acc);
        }
    }

    for root in &tree.roots {
        find_matches(root, &query_lower, &mut matches);
    }

    if matches.is_empty() {
        bail!("Work item not found");
    }
    if matches.len() > 1 {
        let titles: Vec<&str> = matches.iter().map(|m| m.title.as_str()).collect();
        bail!("Work item is ambiguous ({})", titles.join(", "));
    }

    let mut match_node = matches.remove(0);
    if !include_descendants {
        match_node.children.clear();
        match_node.inclusive = match_node.direct.clone();
    }
    Ok(serde_json::to_value(match_node)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;

    #[test]
    fn loads_empty_tree() {
        let ledger = Ledger::open_memory().unwrap();
        let trees = load_project_trees(ledger.connection(), None).unwrap();
        assert!(trees.is_empty());
        assert_eq!(render_project_trees(&trees), "No projects in the ledger.");
    }
}
