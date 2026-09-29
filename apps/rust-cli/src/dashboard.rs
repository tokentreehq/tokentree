// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokentree_ledger::{
    Ledger, add_note, attach_session, detach_session, load_project_trees, move_work_item,
    rename_work_item,
};

#[derive(Clone)]
pub struct AppState {
    pub home: PathBuf,
    pub session_token: String,
}

#[derive(Deserialize)]
pub struct AuthQuery {
    pub token: Option<String>,
}

fn verify_token(state: &AppState, headers: &HeaderMap, query: &AuthQuery) -> bool {
    if let Some(t) = &query.token {
        if t == &state.session_token {
            return true;
        }
    }
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(bearer) = auth.strip_prefix("Bearer ") {
            if bearer.trim() == state.session_token {
                return true;
            }
        }
    }
    if let Some(token_hdr) = headers
        .get("x-tokentree-token")
        .and_then(|v| v.to_str().ok())
    {
        if token_hdr.trim() == state.session_token {
            return true;
        }
    }
    false
}

fn apply_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "Content-Security-Policy",
        HeaderValue::from_static(
            "default-src 'self' 'unsafe-inline' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none';",
        ),
    );
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    headers.insert("Referrer-Policy", HeaderValue::from_static("no-referrer"));
    response
}

async fn handle_index(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                "401 Unauthorized: Invalid or missing token.",
            )
                .into_response(),
        );
    }

    let html = render_dashboard_spa(&state.session_token);
    apply_security_headers(Html(html).into_response())
}

async fn handle_api_projects(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let db_path = state.home.join("ledger.db");
    let ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": e.to_string()}).to_string(),
                )
                    .into_response(),
            );
        }
    };

    let trees = match load_project_trees(ledger.connection(), None) {
        Ok(t) => t,
        Err(e) => {
            return apply_security_headers(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": e.to_string()}).to_string(),
                )
                    .into_response(),
            );
        }
    };

    apply_security_headers(
        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            serde_json::to_string(&trees).unwrap_or_default(),
        )
            .into_response(),
    )
}

#[derive(Deserialize)]
pub struct RenamePayload {
    pub task: String,
    pub title: String,
}

async fn handle_rename(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    axum::Json(payload): axum::Json<RenamePayload>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let mut ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    match rename_work_item(ledger.connection_mut(), &payload.task, &payload.title) {
        Ok(_) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true}).to_string()).into_response(),
        ),
        Err(e) => apply_security_headers(
            (
                StatusCode::BAD_REQUEST,
                json!({"error": e.to_string()}).to_string(),
            )
                .into_response(),
        ),
    }
}

#[derive(Deserialize)]
pub struct MovePayload {
    pub task: String,
    pub parent: Option<String>,
}

async fn handle_move(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    axum::Json(payload): axum::Json<MovePayload>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let mut ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    match move_work_item(
        ledger.connection_mut(),
        &payload.task,
        payload.parent.as_deref(),
    ) {
        Ok(_) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true}).to_string()).into_response(),
        ),
        Err(e) => apply_security_headers(
            (
                StatusCode::BAD_REQUEST,
                json!({"error": e.to_string()}).to_string(),
            )
                .into_response(),
        ),
    }
}

#[derive(Deserialize)]
pub struct NotePayload {
    pub task: Option<String>,
    pub text: String,
}

async fn handle_note(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    axum::Json(payload): axum::Json<NotePayload>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let mut ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    match add_note(
        ledger.connection_mut(),
        &payload.text,
        payload.task.as_deref(),
    ) {
        Ok(id) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true, "id": id}).to_string()).into_response(),
        ),
        Err(e) => apply_security_headers(
            (
                StatusCode::BAD_REQUEST,
                json!({"error": e.to_string()}).to_string(),
            )
                .into_response(),
        ),
    }
}

#[derive(Deserialize)]
pub struct AttachPayload {
    pub session: String,
    pub task: String,
}

async fn handle_attach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    axum::Json(payload): axum::Json<AttachPayload>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let mut ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    match attach_session(ledger.connection_mut(), &payload.session, &payload.task) {
        Ok(changed) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "changed": changed}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => apply_security_headers(
            (
                StatusCode::BAD_REQUEST,
                json!({"error": e.to_string()}).to_string(),
            )
                .into_response(),
        ),
    }
}

#[derive(Deserialize)]
pub struct DetachPayload {
    pub session: String,
}

async fn handle_detach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
    axum::Json(payload): axum::Json<DetachPayload>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let mut ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    match detach_session(ledger.connection_mut(), &payload.session) {
        Ok(changed) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "changed": changed}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => apply_security_headers(
            (
                StatusCode::BAD_REQUEST,
                json!({"error": e.to_string()}).to_string(),
            )
                .into_response(),
        ),
    }
}

async fn handle_api_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuthQuery>,
) -> Response {
    if !verify_token(&state, &headers, &query) {
        return apply_security_headers((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let db_path = state.home.join("ledger.db");
    let ledger = match Ledger::open(&db_path) {
        Ok(l) => l,
        Err(e) => {
            return apply_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            );
        }
    };

    let session_count: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap_or(0);
    let event_count: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
        .unwrap_or(0);
    let project_count: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
        .unwrap_or(0);
    let work_item_count: i64 = ledger
        .connection()
        .query_row("SELECT count(*) FROM work_items", [], |row| row.get(0))
        .unwrap_or(0);

    let status_json = json!({
        "ledger_path": db_path.to_string_lossy(),
        "schema_version": 1,
        "session_count": session_count,
        "event_count": event_count,
        "project_count": project_count,
        "work_item_count": work_item_count,
        "loopback_only": true,
        "capture_mode": "automatic",
    });

    apply_security_headers(
        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            status_json.to_string(),
        )
            .into_response(),
    )
}

pub fn generate_session_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let mut hasher = Sha256::new();
    hasher.update(format!("{now}:{pid}:tokentree_dashboard_secret"));
    hex::encode(hasher.finalize())
}

pub async fn run_dashboard(home: &Path, port: Option<u16>, no_open: bool) -> Result<()> {
    let session_token = generate_session_token();
    let state = Arc::new(AppState {
        home: home.to_path_buf(),
        session_token: session_token.clone(),
    });

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/api/projects", get(handle_api_projects))
        .route("/api/status", get(handle_api_status))
        .route("/api/corrections/rename", post(handle_rename))
        .route("/api/corrections/move", post(handle_move))
        .route("/api/corrections/note", post(handle_note))
        .route("/api/corrections/attach", post(handle_attach))
        .route("/api/corrections/detach", post(handle_detach))
        .with_state(state);

    let chosen_port = port.unwrap_or(0);
    let bind_addr = SocketAddr::from(([127, 0, 0, 1], chosen_port));
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("bind loopback on {bind_addr}"))?;

    let actual_addr = listener.local_addr()?;
    let url = format!("http://{actual_addr}/?token={session_token}");

    println!("\n  TokenTree Local Dashboard active:");
    println!("  URL: {url}\n");
    println!("  Bound strictly to loopback (127.0.0.1)");
    println!("  Random session token verified on every request.");
    println!("  Press Ctrl+C to stop the dashboard.\n");

    if !no_open {
        open_browser(&url);
    }

    axum::serve(listener, app)
        .await
        .context("run axum server")?;

    Ok(())
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn render_dashboard_spa(token: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>TokenTree — Interactive Dashboard</title>
  <meta http-equiv="Content-Security-Policy" content="default-src 'self' 'unsafe-inline' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none';">
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
      padding: 0;
      min-height: 100vh;
      display: flex;
      flex-direction: column;
    }}
    header {{
      background-color: #1F1F1F;
      border-bottom: 1px solid #313131;
      padding: 16px 32px;
      display: flex;
      justify-content: space-between;
      align-items: center;
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
      width: 32px;
      height: 32px;
      background-color: #272727;
      border: 1px solid #313131;
      border-radius: 8px;
      font-weight: 700;
      color: #3B82F6;
      font-size: 16px;
    }}
    .hero-title {{
      font-size: 18px;
      font-weight: 600;
      background: linear-gradient(to right, #FFFFFF, #9B9B9B);
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
    }}
    nav {{
      display: flex;
      gap: 8px;
    }}
    .nav-btn {{
      background: transparent;
      border: 1px solid transparent;
      color: #9B9B9B;
      padding: 6px 14px;
      border-radius: 8px;
      cursor: pointer;
      font-size: 13px;
      font-weight: 500;
      transition: all 0.2s;
    }}
    .nav-btn:hover {{
      color: #FFFFFF;
      background-color: #272727;
    }}
    .nav-btn.active {{
      color: #FFFFFF;
      background-color: #272727;
      border-color: #313131;
    }}
    main {{
      max-width: 1100px;
      width: 100%;
      margin: 32px auto;
      padding: 0 24px;
      flex: 1;
    }}
    .view-panel {{
      display: none;
    }}
    .view-panel.active {{
      display: block;
    }}
    .stats-bar {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 16px;
      margin-bottom: 32px;
    }}
    .stat-card {{
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 12px;
      padding: 16px 20px;
    }}
    .stat-label {{
      font-size: 12px;
      color: #9B9B9B;
      margin-bottom: 4px;
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }}
    .stat-value {{
      font-size: 22px;
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
      padding: 10px 14px;
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
    .project-title {{
      font-size: 17px;
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
      margin-left: 8px;
    }}
    .project-metrics {{
      display: flex;
      align-items: center;
      gap: 16px;
    }}
    .cost-val {{
      font-family: ui-monospace, monospace;
      font-weight: 600;
      color: #10B981;
    }}
    .tokens-val {{
      font-family: ui-monospace, monospace;
      color: #9B9B9B;
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
    .project-tree-container {{
      padding: 16px 20px;
    }}
    .tree-node {{
      background-color: #272727;
      border: 1px solid #313131;
      border-radius: 8px;
      padding: 10px 14px;
      margin: 6px 0;
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
    .node-title {{
      font-weight: 500;
      color: #FFFFFF;
    }}
    .node-id {{
      font-size: 11px;
      color: #6B7280;
      font-family: ui-monospace, monospace;
    }}
    .action-btn {{
      background-color: #1F1F1F;
      border: 1px solid #313131;
      color: #9B9B9B;
      font-size: 11px;
      padding: 3px 8px;
      border-radius: 4px;
      cursor: pointer;
      margin-left: 8px;
      transition: all 0.2s;
    }}
    .action-btn:hover {{
      color: #FFFFFF;
      border-color: #3B82F6;
    }}
    /* Forms in Review panel */
    .form-card {{
      background-color: #1F1F1F;
      border: 1px solid #313131;
      border-radius: 16px;
      padding: 24px;
      margin-bottom: 24px;
    }}
    .form-title {{
      font-size: 16px;
      font-weight: 600;
      color: #FFFFFF;
      margin-bottom: 16px;
    }}
    .form-grid {{
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 16px;
      margin-bottom: 16px;
    }}
    .form-group {{
      display: flex;
      flex-direction: column;
      gap: 6px;
    }}
    .form-label {{
      font-size: 12px;
      color: #9B9B9B;
    }}
    .form-input {{
      background-color: #272727;
      border: 1px solid #313131;
      border-radius: 8px;
      padding: 8px 12px;
      color: #FFFFFF;
      font-size: 13px;
      outline: none;
    }}
    .form-input:focus {{
      border-color: #3B82F6;
    }}
    .btn-primary {{
      background-color: #3B82F6;
      border: none;
      color: #FFFFFF;
      font-weight: 500;
      padding: 8px 16px;
      border-radius: 8px;
      cursor: pointer;
      font-size: 13px;
      transition: background-color 0.2s;
    }}
    .btn-primary:hover {{
      background-color: #2563EB;
    }}
    .feedback-msg {{
      margin-top: 8px;
      font-size: 12px;
    }}
    .feedback-ok {{
      color: #10B981;
    }}
    .feedback-err {{
      color: #EF4444;
    }}
    footer {{
      border-top: 1px solid #313131;
      padding: 20px;
      text-align: center;
      color: #6B7280;
      font-size: 12px;
      line-height: 18px;
    }}
  </style>
</head>
<body>
  <header>
    <div class="brand">
      <div class="logo-badge">TT</div>
      <div>
        <h1 class="hero-title">TokenTree</h1>
      </div>
    </div>
    <nav>
      <button class="nav-btn active" onclick="showTab('projectsView')">Projects &amp; Trees</button>
      <button class="nav-btn" onclick="showTab('reviewView')">Review &amp; Corrections</button>
      <button class="nav-btn" onclick="showTab('statusView')">Status &amp; Diagnostics</button>
    </nav>
  </header>

  <main>
    <!-- Projects View -->
    <div id="projectsView" class="view-panel active">
      <div class="stats-bar" id="statsBar">
        <div class="stat-card">
          <div class="stat-label">Total Estimated Cost</div>
          <div class="stat-value" id="statCost">—</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Total Tokens</div>
          <div class="stat-value" id="statTokens">—</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Completeness</div>
          <div class="stat-value" id="statCompleteness">—</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Unavailable Requests</div>
          <div class="stat-value" id="statUnavailable">—</div>
        </div>
      </div>

      <div class="search-filter">
        <input type="text" id="searchInput" class="search-input" placeholder="Filter projects and work items..." autocomplete="off">
      </div>

      <div id="projectsList">
        <div style="text-align: center; padding: 48px; color: #9B9B9B;">Loading ledger projects...</div>
      </div>
    </div>

    <!-- Review & Corrections View -->
    <div id="reviewView" class="view-panel">
      <!-- Rename -->
      <div class="form-card">
        <h2 class="form-title">Rename Work Item</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Task ID or Title Match</label>
            <input type="text" id="renameTask" class="form-input" placeholder="e.g. wrk_123 or Fix bug">
          </div>
          <div class="form-group">
            <label class="form-label">New Title</label>
            <input type="text" id="renameTitle" class="form-input" placeholder="e.g. Fix collision bug">
          </div>
        </div>
        <button class="btn-primary" onclick="submitRename()">Rename Work Item</button>
        <div id="renameFeedback" class="feedback-msg"></div>
      </div>

      <!-- Move -->
      <div class="form-card">
        <h2 class="form-title">Move Work Item (Parent Hierarchy)</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Task ID</label>
            <input type="text" id="moveTask" class="form-input" placeholder="e.g. wrk_child">
          </div>
          <div class="form-group">
            <label class="form-label">New Parent Task ID (empty for root)</label>
            <input type="text" id="moveParent" class="form-input" placeholder="e.g. wrk_parent or leave blank">
          </div>
        </div>
        <button class="btn-primary" onclick="submitMove()">Move Item</button>
        <div id="moveFeedback" class="feedback-msg"></div>
      </div>

      <!-- Add Note -->
      <div class="form-card">
        <h2 class="form-title">Add Note to Work Item</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Task ID (Optional)</label>
            <input type="text" id="noteTask" class="form-input" placeholder="Optional work item ID">
          </div>
          <div class="form-group">
            <label class="form-label">Note Text (1–240 characters)</label>
            <input type="text" id="noteText" class="form-input" maxlength="240" placeholder="Verification note or context">
          </div>
        </div>
        <button class="btn-primary" onclick="submitNote()">Add Note</button>
        <div id="noteFeedback" class="feedback-msg"></div>
      </div>

      <!-- Attach Session -->
      <div class="form-card">
        <h2 class="form-title">Attach Session to Work Item</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Session ID</label>
            <input type="text" id="attachSessionId" class="form-input" placeholder="e.g. ses_123">
          </div>
          <div class="form-group">
            <label class="form-label">Work Item ID</label>
            <input type="text" id="attachTaskId" class="form-input" placeholder="e.g. wrk_456">
          </div>
        </div>
        <button class="btn-primary" onclick="submitAttach()">Attach Session</button>
        <div id="attachFeedback" class="feedback-msg"></div>
      </div>
    </div>

    <!-- Status & Diagnostics View -->
    <div id="statusView" class="view-panel">
      <div class="form-card">
        <h2 class="form-title">System Status &amp; Ledger Diagnostics</h2>
        <div id="statusDetails" style="font-family: ui-monospace, monospace; font-size: 13px; line-height: 24px; color: #E0E0E0;">
          Loading diagnostics...
        </div>
      </div>
    </div>
  </main>

  <footer>
    <p>Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.</p>
  </footer>

  <script>
    const TOKEN = "{token}";

    function showTab(tabId) {{
      document.querySelectorAll('.view-panel').forEach(el => el.classList.remove('active'));
      document.querySelectorAll('.nav-btn').forEach(el => el.classList.remove('active'));
      document.getElementById(tabId).classList.add('active');
      event.target.classList.add('active');

      if (tabId === 'projectsView') loadProjects();
      if (tabId === 'statusView') loadStatus();
    }}

    function apiFetch(url, options = {{}}) {{
      const authUrl = url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(TOKEN);
      return fetch(authUrl, {{
        ...options,
        headers: {{
          'Content-Type': 'application/json',
          'X-TokenTree-Token': TOKEN,
          ...(options.headers || {{}})
        }}
      }});
    }}

    function renderNode(node, depth) {{
      const indent = depth * 24;
      const u = node.inclusive;
      const totalTok = u.input + u.cache_read + u.cache_write + u.output + u.reasoning;
      const cost = (u.requests > 0 && u.priced === u.requests)
        ? '$' + (u.amount_micros / 1000000).toFixed(2)
        : 'unavailable';

      let compBadge = '';
      if (u.requests === 0) {{
        compBadge = '<span class="badge badge-muted">no requests</span>';
      }} else if (u.unavailable === 0) {{
        compBadge = '<span class="badge badge-green">100% complete</span>';
      }} else {{
        const pct = (100 * u.measured / (u.measured + u.unavailable)).toFixed(0);
        compBadge = `<span class="badge badge-amber">${{pct}}% complete (${{u.unavailable}} unavail)</span>`;
      }}

      let html = `
        <div style="margin-left: ${{indent}}px;">
          <div class="tree-node">
            <div class="node-left">
              <span class="node-title">${{node.title}}</span>
              <span class="node-id">${{node.id}}</span>
              <button class="action-btn" onclick="populateRename('${{node.id}}', '${{node.title.replace(/'/g, "\\'")}}')">Rename</button>
            </div>
            <div style="display: flex; align-items: center; gap: 12px;">
              <span class="cost-val">${{cost}}</span>
              <span class="tokens-val">${{totalTok}} tok</span>
              ${{compBadge}}
            </div>
          </div>
        </div>
      `;

      if (node.children && node.children.length > 0) {{
        for (const child of node.children) {{
          html += renderNode(child, depth + 1);
        }}
      }}
      return html;
    }}

    async function loadProjects() {{
      try {{
        const res = await apiFetch('/api/projects');
        if (!res.ok) throw new Error('HTTP ' + res.status);
        const trees = await res.json();

        let totalTokens = 0, totalMicros = 0, totalReqs = 0, totalMeasured = 0, totalUnavail = 0;
        let fullyPriced = true;

        for (const t of trees) {{
          const u = t.totals;
          totalTokens += u.input + u.cache_read + u.cache_write + u.output + u.reasoning;
          totalMicros += u.amount_micros;
          totalReqs += u.requests;
          totalMeasured += u.measured;
          totalUnavail += u.unavailable;
          if (u.requests > 0 && u.priced < u.requests) fullyPriced = false;
        }}

        document.getElementById('statCost').innerText = (totalReqs > 0 && fullyPriced)
          ? '$' + (totalMicros / 1000000).toFixed(2)
          : 'Unavailable';
        document.getElementById('statTokens').innerText = totalTokens.toLocaleString();
        document.getElementById('statCompleteness').innerText = totalReqs === 0
          ? '100%'
          : (100 * totalMeasured / (totalMeasured + totalUnavail)).toFixed(0) + '%';
        document.getElementById('statUnavailable').innerText = totalUnavail.toLocaleString();

        const container = document.getElementById('projectsList');
        if (trees.length === 0) {{
          container.innerHTML = '<div style="text-align: center; padding: 48px; color: #9B9B9B;">No projects recorded in the ledger yet.</div>';
          return;
        }}

        let listHtml = '';
        for (const t of trees) {{
          const u = t.totals;
          const totalTok = u.input + u.cache_read + u.cache_write + u.output + u.reasoning;
          const cost = (u.requests > 0 && u.priced === u.requests)
            ? '$' + (u.amount_micros / 1000000).toFixed(2)
            : 'unavailable';

          let compBadge = '';
          if (u.requests === 0) compBadge = '<span class="badge badge-muted">no requests</span>';
          else if (u.unavailable === 0) compBadge = '<span class="badge badge-green">100% complete</span>';
          else {{
            const pct = (100 * u.measured / (u.measured + u.unavailable)).toFixed(0);
            compBadge = `<span class="badge badge-amber">${{pct}}% complete (${{u.unavailable}} unavail)</span>`;
          }}

          let treeHtml = '';
          if (t.roots && t.roots.length > 0) {{
            for (const r of t.roots) treeHtml += renderNode(r, 0);
          }} else {{
            treeHtml = '<div style="color: #6B7280; font-style: italic;">No work items recorded.</div>';
          }}

          listHtml += `
            <div class="project-card" data-key="${{t.key}}">
              <div class="project-header">
                <div>
                  <span class="project-title">${{t.title}}</span>
                  <span class="project-key">${{t.key}}</span>
                </div>
                <div class="project-metrics">
                  <span class="cost-val">${{cost}}</span>
                  <span class="tokens-val">${{totalTok}} tok</span>
                  ${{compBadge}}
                </div>
              </div>
              <div class="project-tree-container">
                ${{treeHtml}}
              </div>
            </div>
          `;
        }}
        container.innerHTML = listHtml;
      }} catch (err) {{
        document.getElementById('projectsList').innerHTML = '<div style="color: #EF4444; padding: 24px;">Failed to load projects: ' + err.message + '</div>';
      }}
    }}

    async function loadStatus() {{
      try {{
        const res = await apiFetch('/api/status');
        const s = await res.json();
        document.getElementById('statusDetails').innerHTML = `
          <div>Ledger Path: <strong>${{s.ledger_path}}</strong></div>
          <div>Schema Version: <strong>${{s.schema_version}}</strong></div>
          <div>Total Sessions: <strong>${{s.session_count}}</strong></div>
          <div>Total Usage Events: <strong>${{s.event_count}}</strong></div>
          <div>Projects: <strong>${{s.project_count}}</strong></div>
          <div>Work Items: <strong>${{s.work_item_count}}</strong></div>
          <div>Loopback Security: <strong>Enforced (127.0.0.1)</strong></div>
          <div>Capture Mode: <strong>${{s.capture_mode}}</strong></div>
        `;
      }} catch (err) {{
        document.getElementById('statusDetails').innerText = 'Failed to load status: ' + err.message;
      }}
    }}

    function populateRename(id, title) {{
      showTab('reviewView');
      document.getElementById('renameTask').value = id;
      document.getElementById('renameTitle').value = title;
    }}

    async function submitRename() {{
      const task = document.getElementById('renameTask').value.trim();
      const title = document.getElementById('renameTitle').value.trim();
      const fb = document.getElementById('renameFeedback');
      if (!task || !title) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Task and new title required'; return; }}
      try {{
        const res = await apiFetch('/api/corrections/rename', {{
          method: 'POST',
          body: JSON.stringify({{ task, title }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Renamed successfully!';
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Rename failed';
        }}
      }} catch (e) {{
        fb.className = 'feedback-msg feedback-err'; fb.innerText = e.message;
      }}
    }}

    async function submitMove() {{
      const task = document.getElementById('moveTask').value.trim();
      const parent = document.getElementById('moveParent').value.trim() || null;
      const fb = document.getElementById('moveFeedback');
      if (!task) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Task required'; return; }}
      try {{
        const res = await apiFetch('/api/corrections/move', {{
          method: 'POST',
          body: JSON.stringify({{ task, parent }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Moved successfully!';
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Move failed';
        }}
      }} catch (e) {{
        fb.className = 'feedback-msg feedback-err'; fb.innerText = e.message;
      }}
    }}

    async function submitNote() {{
      const task = document.getElementById('noteTask').value.trim() || null;
      const text = document.getElementById('noteText').value.trim();
      const fb = document.getElementById('noteFeedback');
      if (!text) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Note text required (1–240 chars)'; return; }}
      try {{
        const res = await apiFetch('/api/corrections/note', {{
          method: 'POST',
          body: JSON.stringify({{ task, text }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Note added with ID ' + data.id;
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Failed to add note';
        }}
      }} catch (e) {{
        fb.className = 'feedback-msg feedback-err'; fb.innerText = e.message;
      }}
    }}

    async function submitAttach() {{
      const session = document.getElementById('attachSessionId').value.trim();
      const task = document.getElementById('attachTaskId').value.trim();
      const fb = document.getElementById('attachFeedback');
      if (!session || !task) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Session and Task ID required'; return; }}
      try {{
        const res = await apiFetch('/api/corrections/attach', {{
          method: 'POST',
          body: JSON.stringify({{ session, task }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Attached successfully!';
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Attach failed';
        }}
      }} catch (e) {{
        fb.className = 'feedback-msg feedback-err'; fb.innerText = e.message;
      }}
    }}

    document.getElementById('searchInput').addEventListener('input', function(e) {{
      const term = e.target.value.toLowerCase().trim();
      document.querySelectorAll('.project-card').forEach(card => {{
        const text = card.textContent.toLowerCase();
        card.style.display = text.includes(term) ? '' : 'none';
      }});
    }});

    // Initialize
    loadProjects();
  </script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_high_entropy_session_token() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        assert_eq!(t1.len(), 64);
        assert_ne!(t1, t2);
    }

    #[test]
    fn renders_dashboard_spa_with_token_and_csp() {
        let token = "test_token_12345";
        let html = render_dashboard_spa(token);
        assert!(html.contains(token));
        assert!(html.contains("Content-Security-Policy"));
        assert!(html.contains("TokenTree"));
        assert!(html.contains("Projects &amp; Trees"));
    }
}
