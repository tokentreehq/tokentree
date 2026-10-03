// SPDX-License-Identifier: Apache-2.0
use anyhow::{Context, Result, anyhow};
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokentree_ledger::{
    Ledger, add_note, attach_session, detach_session, load_project_trees, merge_work_items,
    move_work_item, rename_work_item, split_work_item,
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

/// Constant-time string equality: compares every byte so the comparison
/// time reveals nothing about how many leading bytes matched. Length is
/// checked first; all tokens compared here are fixed-length hex digests,
/// so the length check leaks no secret content.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn extract_cookie_token(headers: &HeaderMap) -> Option<&str> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("tokentree_session=") {
            return Some(val.trim());
        }
    }
    None
}

/// Verify authentication for API endpoints.
///
/// Accepts session tokens via Cookie, Authorization: Bearer, or x-tokentree-token header.
/// Query string tokens are explicitly rejected on all `/api/*` endpoints to eliminate
/// token leakage into proxy access logs, browser history, or Referer headers.
fn verify_api_token(state: &AppState, headers: &HeaderMap) -> bool {
    if let Some(cookie_token) = extract_cookie_token(headers) {
        if constant_time_eq(cookie_token, &state.session_token) {
            return true;
        }
    }
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(bearer) = auth.strip_prefix("Bearer ") {
            if constant_time_eq(bearer.trim(), &state.session_token) {
                return true;
            }
        }
    }
    if let Some(token_hdr) = headers
        .get("x-tokentree-token")
        .and_then(|v| v.to_str().ok())
    {
        if constant_time_eq(token_hdr.trim(), &state.session_token) {
            return true;
        }
    }
    false
}

/// Verify authentication for the initial HTML page load (`/`).
///
/// Allows a one-time query parameter token for the initial navigation from `tokentree dashboard`,
/// setting a secure `HttpOnly` session cookie so all subsequent operations do not require URL tokens.
fn verify_index_token(state: &AppState, headers: &HeaderMap, query: &AuthQuery) -> bool {
    if verify_api_token(state, headers) {
        return true;
    }
    if let Some(t) = &query.token {
        if constant_time_eq(t, &state.session_token) {
            return true;
        }
    }
    false
}

/// Run blocking SQLite work on Tokio's blocking thread pool instead of an
/// async worker (H13/H14 fix). Opening the ledger and running queries blocks
/// the calling thread; doing that directly inside an async handler stalls
/// every other in-flight request sharing the runtime worker.
async fn blocking_db<T, F>(home: &Path, work: F) -> std::result::Result<T, DbError>
where
    T: Send + 'static,
    F: FnOnce(&mut Ledger) -> Result<T> + Send + 'static,
{
    let db_path = home.join("ledger.db");
    tokio::task::spawn_blocking(move || {
        let mut ledger = Ledger::open(&db_path).map_err(DbError::Open)?;
        work(&mut ledger).map_err(DbError::Op)
    })
    .await
    .map_err(|e| DbError::Open(anyhow!("database worker failed: {e}")))?
}

/// Error from [`blocking_db`]. Preserves the handlers' original distinction:
/// failing to *open* the ledger is a 500, failing the *operation* is a 400.
enum DbError {
    Open(anyhow::Error),
    Op(anyhow::Error),
}

impl DbError {
    fn status_code(&self) -> StatusCode {
        match self {
            DbError::Open(_) => StatusCode::INTERNAL_SERVER_ERROR,
            DbError::Op(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn message(&self) -> String {
        match self {
            DbError::Open(e) | DbError::Op(e) => e.to_string(),
        }
    }
}

fn db_error_response(e: DbError) -> Response {
    apply_security_headers(
        (e.status_code(), json!({"error": e.message()}).to_string()).into_response(),
    )
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
    if !verify_index_token(&state, &headers, &query) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                "401 Unauthorized: Invalid or missing token.",
            )
                .into_response(),
        );
    }

    let html = render_dashboard_spa(&state.session_token);
    let mut response = Html(html).into_response();
    let cookie_val = format!(
        "tokentree_session={}; HttpOnly; SameSite=Strict; Path=/",
        state.session_token
    );
    if let Ok(cookie_hdr) = HeaderValue::from_str(&cookie_val) {
        response.headers_mut().insert("set-cookie", cookie_hdr);
    }
    apply_security_headers(response)
}

async fn handle_api_projects(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let trees = match blocking_db(&state.home, |ledger| {
        load_project_trees(ledger.connection(), None)
    })
    .await
    {
        Ok(t) => t,
        Err(e) => {
            return db_error_response(e);
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
    axum::Json(payload): axum::Json<RenamePayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let RenamePayload { task, title } = payload;

    match blocking_db(&state.home, move |ledger| {
        rename_work_item(ledger.connection_mut(), &task, &title)
    })
    .await
    {
        Ok(_) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true}).to_string()).into_response(),
        ),
        Err(e) => db_error_response(e),
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
    axum::Json(payload): axum::Json<MovePayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let MovePayload { task, parent } = payload;

    match blocking_db(&state.home, move |ledger| {
        move_work_item(ledger.connection_mut(), &task, parent.as_deref())
    })
    .await
    {
        Ok(_) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true}).to_string()).into_response(),
        ),
        Err(e) => db_error_response(e),
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
    axum::Json(payload): axum::Json<NotePayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let NotePayload { task, text } = payload;

    match blocking_db(&state.home, move |ledger| {
        add_note(ledger.connection_mut(), &text, task.as_deref())
    })
    .await
    {
        Ok(id) => apply_security_headers(
            (StatusCode::OK, json!({"ok": true, "id": id}).to_string()).into_response(),
        ),
        Err(e) => db_error_response(e),
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
    axum::Json(payload): axum::Json<AttachPayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let AttachPayload { session, task } = payload;

    match blocking_db(&state.home, move |ledger| {
        attach_session(ledger.connection_mut(), &session, &task)
    })
    .await
    {
        Ok(changed) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "changed": changed}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => db_error_response(e),
    }
}

#[derive(Deserialize)]
pub struct DetachPayload {
    pub session: String,
}

async fn handle_detach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<DetachPayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let DetachPayload { session } = payload;

    match blocking_db(&state.home, move |ledger| {
        detach_session(ledger.connection_mut(), &session)
    })
    .await
    {
        Ok(changed) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "changed": changed}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => db_error_response(e),
    }
}

#[derive(Deserialize)]
pub struct MergePayload {
    pub source: String,
    pub target: String,
}

async fn handle_merge(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<MergePayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let MergePayload { source, target } = payload;

    match blocking_db(&state.home, move |ledger| {
        merge_work_items(ledger.connection_mut(), &source, &target)
    })
    .await
    {
        Ok(spans) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "spans_reattributed": spans}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => db_error_response(e),
    }
}

#[derive(Deserialize)]
pub struct SplitPayload {
    pub source: String,
    pub title: String,
    pub spans: Vec<String>,
}

async fn handle_split(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<SplitPayload>,
) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let SplitPayload {
        source,
        title,
        spans,
    } = payload;

    match blocking_db(&state.home, move |ledger| {
        split_work_item(ledger.connection_mut(), &source, &title, &spans)
    })
    .await
    {
        Ok(new_id) => apply_security_headers(
            (
                StatusCode::OK,
                json!({"ok": true, "new_work_item_id": new_id}).to_string(),
            )
                .into_response(),
        ),
        Err(e) => db_error_response(e),
    }
}

async fn handle_api_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_api_token(&state, &headers) {
        return apply_security_headers(
            (
                StatusCode::UNAUTHORIZED,
                json!({"error": "Unauthorized"}).to_string(),
            )
                .into_response(),
        );
    }

    let (session_count, event_count, project_count, work_item_count) =
        match blocking_db(&state.home, |ledger| {
            let conn = ledger.connection();
            let count = |sql: &str| -> Result<i64> {
                Ok(conn.query_row(sql, [], |row| row.get(0)).unwrap_or(0))
            };
            Ok::<_, anyhow::Error>((
                count("SELECT count(*) FROM sessions")?,
                count("SELECT count(*) FROM usage_events WHERE superseded_by IS NULL")?,
                count("SELECT count(*) FROM projects")?,
                count("SELECT count(*) FROM work_items")?,
            ))
        })
        .await
        {
            Ok(counts) => counts,
            Err(e) => {
                return db_error_response(e);
            }
        };

    let status_json = json!({
        // P3: no absolute ledger_path — the dashboard is loopback-only today,
        // but the payload must stay safe if ever reused on a non-local surface.
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

/// Generate a 256-bit dashboard session token from the OS CSPRNG (H1 fix).
///
/// The previous implementation hashed `time:pid`, which any local observer
/// could predict. Fails fast if the OS CSPRNG is unavailable: falling back
/// to weak randomness would silently defeat the token.
pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS CSPRNG failure generating dashboard session token");
    hex::encode(bytes)
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle_index))
        .route("/api/projects", get(handle_api_projects))
        .route("/api/status", get(handle_api_status))
        .route("/api/corrections/rename", post(handle_rename))
        .route("/api/corrections/move", post(handle_move))
        .route("/api/corrections/note", post(handle_note))
        .route("/api/corrections/attach", post(handle_attach))
        .route("/api/corrections/detach", post(handle_detach))
        .route("/api/corrections/merge", post(handle_merge))
        .route("/api/corrections/split", post(handle_split))
        .with_state(state)
}

pub async fn run_dashboard(home: &Path, port: Option<u16>, no_open: bool) -> Result<()> {
    let session_token = generate_session_token();
    let state = Arc::new(AppState {
        home: home.to_path_buf(),
        session_token: session_token.clone(),
    });

    let app = create_router(state);

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

      <!-- Merge Work Items -->
      <div class="form-card">
        <h2 class="form-title">Merge Work Items</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Source Task ID (will be merged)</label>
            <input type="text" id="mergeSource" class="form-input" placeholder="e.g. wi_source">
          </div>
          <div class="form-group">
            <label class="form-label">Target Task ID (receives usage &amp; children)</label>
            <input type="text" id="mergeTarget" class="form-input" placeholder="e.g. wi_target">
          </div>
        </div>
        <button class="btn-primary" onclick="submitMerge()">Merge Work Items</button>
        <div id="mergeFeedback" class="feedback-msg"></div>
      </div>

      <!-- Split Work Item -->
      <div class="form-card">
        <h2 class="form-title">Split Work Item at Selected Spans</h2>
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label">Source Task ID</label>
            <input type="text" id="splitSource" class="form-input" placeholder="e.g. wi_source">
          </div>
          <div class="form-group">
            <label class="form-label">New Work Item Title</label>
            <input type="text" id="splitTitle" class="form-input" placeholder="e.g. Extracted Subtask">
          </div>
          <div class="form-group" style="grid-column: 1 / -1;">
            <label class="form-label">Span IDs to Move (comma-separated)</label>
            <input type="text" id="splitSpans" class="form-input" placeholder="e.g. span_1, span_2">
          </div>
        </div>
        <button class="btn-primary" onclick="submitSplit()">Split Work Item</button>
        <div id="splitFeedback" class="feedback-msg"></div>
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

    // C7 fix: escape every dynamic value interpolated into HTML. Project
    // titles, work-item titles/ids, status strings, and error messages all
    // originate from the ledger or the network and must never be parsed as
    // markup. Dynamic text goes through esc(); static markup stays literal.
    function esc(s) {{
      return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
    }}

    // Titles keyed by node id for the Rename action. A Map (not a plain
    // object) so a hostile id such as "__proto__" cannot pollute anything.
    const nodeTitles = new Map();

    function showTab(tabId) {{
      document.querySelectorAll('.view-panel').forEach(el => el.classList.remove('active'));
      document.querySelectorAll('.nav-btn').forEach(el => el.classList.remove('active'));
      document.getElementById(tabId).classList.add('active');
      event.target.classList.add('active');

      if (tabId === 'projectsView') loadProjects();
      if (tabId === 'statusView') loadStatus();
    }}

    function apiFetch(url, options = {{}}) {{
      // The session token never appears in API URLs: it lives only in memory
      // (the TOKEN constant above, embedded by the server in this already-
      // authenticated page) and travels via the Authorization header. The
      // query string is stripped on load (see below) so the credential never
      // lands in browser history, server logs, or Referer headers.
      return fetch(url, {{
        ...options,
        headers: {{
          'Content-Type': 'application/json',
          'Authorization': 'Bearer ' + TOKEN,
          ...(options.headers || {{}})
        }}
      }});
    }}

    // Strip the one-time ?token= credential from the address bar immediately
    // after the first authenticated navigation. The token remains available
    // to this page via the in-memory TOKEN constant; it is deliberately never
    // written to localStorage (XSS-readable) or left in the URL.
    if (window.location.search.indexOf('token=') !== -1) {{
      history.replaceState(null, '', window.location.pathname);
    }}

    function renderNode(node, depth) {{
      nodeTitles.set(node.id, node.title);
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
              <span class="node-title">${{esc(node.title)}}</span>
              <span class="node-id">${{esc(node.id)}}</span>
              <button class="action-btn" data-rename-id="${{esc(node.id)}}">Rename</button>
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
            <div class="project-card" data-key="${{esc(t.key)}}">
              <div class="project-header">
                <div>
                  <span class="project-title">${{esc(t.title)}}</span>
                  <span class="project-key">${{esc(t.key)}}</span>
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
        document.getElementById('projectsList').innerHTML = '<div style="color: #EF4444; padding: 24px;">Failed to load projects: ' + esc(err.message) + '</div>';
      }}
    }}

    async function loadStatus() {{
      try {{
        const res = await apiFetch('/api/status');
        const s = await res.json();
        document.getElementById('statusDetails').innerHTML = `
          <div>Schema Version: <strong>${{esc(s.schema_version)}}</strong></div>
          <div>Total Sessions: <strong>${{esc(s.session_count)}}</strong></div>
          <div>Total Usage Events: <strong>${{esc(s.event_count)}}</strong></div>
          <div>Projects: <strong>${{esc(s.project_count)}}</strong></div>
          <div>Work Items: <strong>${{esc(s.work_item_count)}}</strong></div>
          <div>Loopback Security: <strong>Enforced (127.0.0.1)</strong></div>
          <div>Capture Mode: <strong>${{esc(s.capture_mode)}}</strong></div>
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

    async function submitMerge() {{
      const source = document.getElementById('mergeSource').value.trim();
      const target = document.getElementById('mergeTarget').value.trim();
      const fb = document.getElementById('mergeFeedback');
      if (!source || !target) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Source and Target task IDs required'; return; }}
      try {{
        const res = await apiFetch('/api/corrections/merge', {{
          method: 'POST',
          body: JSON.stringify({{ source, target }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Merged successfully! ' + data.spans_reattributed + ' spans reattributed.';
          loadProjects();
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Merge failed';
        }}
      }} catch (e) {{
        fb.className = 'feedback-msg feedback-err'; fb.innerText = e.message;
      }}
    }}

    async function submitSplit() {{
      const source = document.getElementById('splitSource').value.trim();
      const title = document.getElementById('splitTitle').value.trim();
      const spansInput = document.getElementById('splitSpans').value.trim();
      const fb = document.getElementById('splitFeedback');
      if (!source || !title || !spansInput) {{ fb.className = 'feedback-msg feedback-err'; fb.innerText = 'Source, Title, and at least one Span ID required'; return; }}
      const spans = spansInput.split(',').map(s => s.trim()).filter(Boolean);
      try {{
        const res = await apiFetch('/api/corrections/split', {{
          method: 'POST',
          body: JSON.stringify({{ source, title, spans }})
        }});
        const data = await res.json();
        if (res.ok) {{
          fb.className = 'feedback-msg feedback-ok'; fb.innerText = 'Split successfully! New item ID: ' + data.new_work_item_id;
          loadProjects();
        }} else {{
          fb.className = 'feedback-msg feedback-err'; fb.innerText = data.error || 'Split failed';
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

    // Rename buttons are rendered dynamically. Delegate clicks instead of an
    // inline onclick so node ids/titles never pass through the dangerous
    // HTML-attribute + JS-string double context (C7).
    document.addEventListener('click', function(e) {{
      const btn = e.target.closest('[data-rename-id]');
      if (!btn) return;
      const id = btn.getAttribute('data-rename-id');
      populateRename(id, nodeTitles.get(id) || '');
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
    fn served_spa_never_puts_reusable_credential_in_urls() {
        // The session token is a reusable credential: it must not appear in
        // API URLs (browser history, server logs, Referer) and must not be
        // persisted to XSS-readable storage.
        let html = render_dashboard_spa("test_token_12345");
        // 1. The one-time ?token= is stripped from the address bar on load.
        assert!(
            html.contains("history.replaceState"),
            "SPA must strip the query-string token on first load"
        );
        // 2. API calls carry the token via the Authorization header only.
        assert!(
            html.contains("'Authorization': 'Bearer ' + TOKEN"),
            "API calls must use the Bearer header"
        );
        assert!(
            !html.contains("'token=' + encodeURIComponent"),
            "API URLs must not embed the token in the query string"
        );
        // 3. No persistence to XSS-readable storage.
        assert!(
            !html.contains("localStorage.setItem"),
            "token must not be written to localStorage"
        );
    }

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

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_app() -> (Router, String, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let ledger_path = temp.path().join("ledger.db");
        let ledger = Ledger::open(&ledger_path).unwrap();
        drop(ledger);
        let token = "test_secret_session_token_12345678901234567890123456789012".to_string();
        let state = Arc::new(AppState {
            home: temp.path().to_path_buf(),
            session_token: token.clone(),
        });
        (create_router(state), token, temp)
    }

    #[tokio::test]
    async fn auth_token_in_query_param_succeeds_on_index_and_sets_cookie() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri(format!("/?token={token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res
            .headers()
            .get("set-cookie")
            .expect("index response must set session cookie")
            .to_str()
            .unwrap();
        assert!(cookie.contains(&format!("tokentree_session={token}")));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
    }

    #[tokio::test]
    async fn auth_token_in_cookie_succeeds() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri("/api/status")
            .header("Cookie", format!("tokentree_session={token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_token_in_query_param_rejected_on_api_endpoints() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri(format!("/api/status?token={token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_token_in_bearer_header_succeeds() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri("/api/status")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_token_in_custom_header_succeeds() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri("/api/status")
            .header("x-tokentree-token", &token)
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_missing_token_returns_401() {
        let (app, _token, _temp) = test_app();
        let req = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_invalid_token_returns_401() {
        let (app, _token, _temp) = test_app();
        let req = Request::builder()
            .uri("/?token=invalid_forged_token")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn security_headers_enforced_on_all_responses() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .uri(format!("/?token={token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let headers = res.headers();
        let csp = headers
            .get("Content-Security-Policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("default-src 'self' 'unsafe-inline' data:"));
        assert!(csp.contains("connect-src 'self'"));
        assert!(csp.contains("object-src 'none'"));

        assert_eq!(headers.get("X-Frame-Options").unwrap(), "DENY");
        assert_eq!(headers.get("X-Content-Type-Options").unwrap(), "nosniff");
        assert_eq!(headers.get("Referrer-Policy").unwrap(), "no-referrer");
    }

    #[test]
    fn zero_external_cdn_assets() {
        let token = "test_token";
        let html = render_dashboard_spa(token);
        // Ensure no external scripts, CDNs, or external stylesheet links
        assert!(!html.contains("cdn.jsdelivr.net"));
        assert!(!html.contains("unpkg.com"));
        assert!(!html.contains("cdnjs.cloudflare.com"));
        assert!(!html.contains("fonts.googleapis.com"));
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
    }

    #[tokio::test]
    async fn route_denial_no_transcript_or_traversal_endpoints() {
        let (app1, token, _temp) = test_app();
        let req1 = Request::builder()
            .uri("/api/transcripts")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res1 = app1.oneshot(req1).await.unwrap();
        assert_eq!(res1.status(), StatusCode::NOT_FOUND);

        let (app2, token, _temp) = test_app();
        let req2 = Request::builder()
            .uri("/transcripts")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res2 = app2.oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::NOT_FOUND);

        let (app3, token, _temp) = test_app();
        let req3 = Request::builder()
            .uri("/api/raw")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res3 = app3.oneshot(req3).await.unwrap();
        assert_eq!(res3.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_malformed_json_payload() {
        let (app, token, _temp) = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/api/corrections/rename")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from("{malformed_json:"))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert!(res.status().is_client_error());
    }

    #[test]
    fn escapes_work_item_titles_against_xss() {
        let evil_title = "<script>alert('xss')</script>\"'><img src=x onerror=alert(1)>";
        let escaped = tokentree_ledger::html_escape(evil_title);
        assert!(!escaped.contains("<script>"));
        assert!(!escaped.contains("<img"));
        assert!(escaped.contains("&lt;script&gt;"));
        assert!(escaped.contains("&quot;"));
    }

    #[test]
    fn constant_time_eq_matches_str_eq() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
        // 64-hex tokens, the actual shape compared in verify_token
        let t = generate_session_token();
        assert!(constant_time_eq(&t, &t));
        assert!(!constant_time_eq(&t, &generate_session_token()));
    }

    /// C7 regression: the SPA template must route every dynamic interpolation
    /// through esc(). This statically audits the rendered template for raw
    /// (unescaped) interpolations and for the old inline-onclick pattern.
    #[test]
    fn spa_template_escapes_all_dynamic_interpolations() {
        let html = render_dashboard_spa("test_token");
        // The esc() helper must exist and be defined before use.
        assert!(html.contains("function esc(s)"));
        // No raw interpolations of ledger-controlled data may remain.
        for raw in [
            "${node.title}",
            "${node.id}",
            "${t.title}",
            "${t.key}",
            "${s.capture_mode}",
            "err.message + '</div>'",
            "onclick=\"populateRename('",
        ] {
            assert!(
                !html.contains(raw),
                "unescaped dynamic interpolation remains in SPA template: {raw}"
            );
        }
        // The escaped forms must be present.
        for escaped in [
            "${esc(node.title)}",
            "${esc(node.id)}",
            "${esc(t.title)}",
            "${esc(t.key)}",
            // P3: s.ledger_path was removed from /api/status and the template.
            "${esc(s.capture_mode)}",
            "esc(err.message)",
            "data-rename-id=\"${esc(node.id)}\"",
        ] {
            assert!(
                html.contains(escaped),
                "expected escaped interpolation missing from SPA template: {escaped}"
            );
        }
    }

    #[tokio::test]
    async fn merge_and_split_endpoints_work_correctly() {
        let (app, token, temp) = test_app();
        let db_path = temp.path().join("ledger.db");
        let mut ledger = Ledger::open(&db_path).unwrap();

        // Seed project and two tasks
        let run = tokentree_ledger::start_manual(
            ledger.connection_mut(),
            tokentree_ledger::ManualStartInput {
                project_key: "endpoint-proj",
                project_title: Some("Endpoint Project"),
                task_title: "Source Task",
                parent_title: None,
                cwd: "/tmp",
            },
        )
        .unwrap();

        let target_wi = "wi_target_endpoint";
        ledger
            .connection_mut()
            .execute(
                "INSERT INTO work_items (id, project_id, type, title, status, created_at)
                 VALUES (?1, ?2, 'task', 'Target Task', 'open', '2026-09-29T10:00:00Z')",
                [target_wi, &run.project_id],
            )
            .unwrap();

        drop(ledger);

        // Test merge endpoint
        let merge_req = Request::builder()
            .method("POST")
            .uri("/api/corrections/merge")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "source": run.work_item_id,
                    "target": target_wi,
                })
                .to_string(),
            ))
            .unwrap();

        let merge_res = app.clone().oneshot(merge_req).await.unwrap();
        assert_eq!(merge_res.status(), StatusCode::OK);

        // Test invalid merge endpoint (cross-project / non-existent)
        let bad_merge_req = Request::builder()
            .method("POST")
            .uri("/api/corrections/merge")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "source": "nonexistent_source",
                    "target": target_wi,
                })
                .to_string(),
            ))
            .unwrap();

        let bad_merge_res = app.clone().oneshot(bad_merge_req).await.unwrap();
        assert_eq!(bad_merge_res.status(), StatusCode::BAD_REQUEST);

        // Test invalid split endpoint (empty spans)
        let bad_split_req = Request::builder()
            .method("POST")
            .uri("/api/corrections/split")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({
                    "source": target_wi,
                    "title": "New Extracted Item",
                    "spans": []
                })
                .to_string(),
            ))
            .unwrap();

        let bad_split_res = app.oneshot(bad_split_req).await.unwrap();
        assert_eq!(bad_split_res.status(), StatusCode::BAD_REQUEST);
    }
}
