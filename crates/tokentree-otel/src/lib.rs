// SPDX-License-Identifier: Apache-2.0
use anyhow::{Result, bail};
use axum::extract::Request;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokentree_core::{MeasurementSource, TokenUsage, UsageObservation, source_kind};
use tokentree_ledger::Ledger;

const MAX_OTLP_BODY: usize = 1_048_576;

/// Shared-secret bearer auth for the loopback OTLP receiver (H2 fix).
///
/// Loopback binding alone does not stop a malicious local process from
/// POSTing poisoned telemetry: the ledger is append-only, so a bad row can
/// never be removed. Every ingest request must therefore present
/// `Authorization: Bearer <secret>`, where the secret is 256 bits from the
/// OS CSPRNG generated at receiver startup. The CLI prints the secret and
/// the `OTEL_EXPORTER_OTLP_HEADERS` export the sender must set; operators
/// can pin a secret for automation via the `TOKENTREE_OTLP_TOKEN`
/// environment variable.
#[derive(Clone)]
pub struct OtlpAuth {
    pub bearer_token: String,
}

impl OtlpAuth {
    /// Generate a fresh 256-bit bearer secret from the OS CSPRNG.
    pub fn generate() -> OtlpAuth {
        OtlpAuth {
            bearer_token: generate_bearer_token(),
        }
    }
}

/// 256 bits of OS randomness, hex-encoded (64 chars). Fails fast if the
/// OS CSPRNG is unavailable: silently falling back to weak randomness
/// would defeat the purpose of the secret.
pub fn generate_bearer_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS CSPRNG failure generating OTLP bearer token");
    hex::encode(bytes)
}

/// Constant-time string equality: compares every byte so the comparison
/// time reveals nothing about how many leading bytes matched. Length is
/// checked first; all bearer tokens are fixed-length hex, so the length
/// check leaks no secret content.
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

fn valid_bearer(headers: &HeaderMap, expected: &str) -> bool {
    let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(provided) = auth.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(provided.trim(), expected)
}

/// Reject unauthenticated requests before the body is parsed, so an
/// unauthenticated sender cannot even reach the JSON extractor.
async fn require_bearer(State(auth): State<OtlpAuth>, req: Request, next: Next) -> Response {
    if !valid_bearer(req.headers(), &auth.bearer_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing or invalid bearer token"})),
        )
            .into_response();
    }
    next.run(req).await
}

#[derive(Clone)]
struct ReceiverState {
    ledger: Arc<Mutex<Ledger>>,
}

pub fn create_router(auth: OtlpAuth, ledger: Ledger) -> Router {
    let state = ReceiverState {
        ledger: Arc::new(Mutex::new(ledger)),
    };
    Router::new()
        .route("/v1/logs", post(receive_logs))
        .route_layer(middleware::from_fn_with_state(auth, require_bearer))
        .layer(DefaultBodyLimit::max(MAX_OTLP_BODY))
        .with_state(state)
}

pub async fn serve(address: SocketAddr, ledger: Ledger, auth: OtlpAuth) -> Result<()> {
    if !address.ip().is_loopback() {
        bail!("OTLP receiver must bind to a loopback address");
    }
    let app = create_router(auth, ledger);
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn receive_logs(
    State(state): State<ReceiverState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let observations = extract_api_requests(&payload);
    let count = observations.len();
    let mut ledger = state.ledger.lock().map_err(internal_error)?;
    let summary = ledger.ingest(observations).map_err(internal_error)?;
    Ok(Json(
        json!({"accepted": count, "inserted": summary.inserted, "duplicates": summary.duplicates}),
    ))
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": error.to_string()})),
    )
}

#[must_use]
pub fn extract_api_requests(payload: &Value) -> Vec<UsageObservation> {
    let mut output = Vec::new();
    let Some(resource_logs) = payload.get("resourceLogs").and_then(Value::as_array) else {
        return output;
    };
    for resource in resource_logs {
        let resource_attributes = attributes(resource.pointer("/resource/attributes"));
        let Some(scopes) = resource.get("scopeLogs").and_then(Value::as_array) else {
            continue;
        };
        for scope in scopes {
            let Some(records) = scope.get("logRecords").and_then(Value::as_array) else {
                continue;
            };
            for record in records {
                let mut fields = resource_attributes.clone();
                fields.extend(attributes(record.get("attributes")));
                let body = otlp_value(record.get("body"));
                let event_name = fields
                    .get("event.name")
                    .and_then(Value::as_str)
                    .or_else(|| body.as_ref().and_then(Value::as_str));
                if !matches!(event_name, Some("api_request" | "claude_code.api_request")) {
                    continue;
                }
                let session = string_field(&fields, &["session.id", "session_id"]);
                let request = string_field(&fields, &["request_id", "gen_ai.response.id"]);
                let Some(session) = session else { continue };
                let Some(request) = request else { continue };
                let timestamp = string_field(&fields, &["event.timestamp"]).or_else(|| {
                    record
                        .get("timeUnixNano")
                        .and_then(Value::as_str)
                        .map(|value| format!("unix-ns:{value}"))
                });
                output.push(UsageObservation {
                    adapter: "claude".into(),
                    source: MeasurementSource::OfficialTelemetry,
                    source_subtype: Some(source_kind::OTEL_API_REQUEST.into()),
                    source_event_id: sequence_field(&fields)
                        .map(|seq| format!("otel:{session}:{seq}")),
                    provider_session_id: session,
                    request_id: Some(request),
                    turn_id: string_field(&fields, &["prompt.id", "turn.id"]),
                    agent_id: string_field(&fields, &["agent.id", "agent_id"]),
                    parent_agent_id: string_field(&fields, &["parent_agent.id", "parent_agent_id"]),
                    source_timestamp: timestamp,
                    observed_at: Utc::now().to_rfc3339(),
                    model: string_field(&fields, &["model", "gen_ai.request.model"]),
                    service_tier: string_field(&fields, &["service_tier"]),
                    region: string_field(&fields, &["inference_geo", "region"]),
                    usage: TokenUsage {
                        input_tokens: u64_field(&fields, "input_tokens"),
                        cached_input_tokens: u64_field(&fields, "cache_read_tokens"),
                        cache_write_tokens: u64_field(&fields, "cache_creation_tokens"),
                        output_tokens: u64_field(&fields, "output_tokens"),
                        reasoning_tokens: u64_field(&fields, "reasoning_tokens"),
                    },
                    provider_reported_cost_micros: u64_field(&fields, "cost_usd_micros"),
                    source_path: "otel:http/json:/v1/logs".into(),
                    source_offset: sequence_field(&fields).unwrap_or(0),
                    adapter_version: concat!(env!("CARGO_PKG_VERSION"), "-rust").into(),
                    parser_version: "otel-http-json-v1".into(),
                });
            }
        }
    }
    output
}

fn attributes(value: Option<&Value>) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    let Some(entries) = value.and_then(Value::as_array) else {
        return result;
    };
    for entry in entries {
        let Some(key) = entry.get("key").and_then(Value::as_str) else {
            continue;
        };
        if let Some(value) = otlp_value(entry.get("value")) {
            result.insert(key.into(), value);
        }
    }
    result
}
fn otlp_value(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    for key in ["stringValue", "intValue", "doubleValue", "boolValue"] {
        if let Some(inner) = value.get(key) {
            return Some(inner.clone());
        }
    }
    None
}
fn string_field(fields: &HashMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(Value::as_str).map(str::to_owned))
}
fn u64_field(fields: &HashMap<String, Value>, key: &str) -> Option<u64> {
    fields
        .get(key)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}
fn sequence_field(fields: &HashMap<String, Value>) -> Option<u64> {
    u64_field(fields, "event.sequence")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_official_api_request_without_raw_bodies() {
        let attr = |key: &str, value: Value| json!({"key":key,"value":value});
        let payload = json!({"resourceLogs":[{"resource":{"attributes":[attr("session.id",json!({"stringValue":"s"}))]},"scopeLogs":[{"logRecords":[{"attributes":[
            attr("event.name",json!({"stringValue":"api_request"})),attr("request_id",json!({"stringValue":"req_1"})),attr("model",json!({"stringValue":"claude-sonnet-4-6"})),attr("input_tokens",json!({"intValue":"10"})),attr("output_tokens",json!({"intValue":"2"})),attr("cache_read_tokens",json!({"intValue":"5"})),attr("event.sequence",json!({"intValue":"7"}))
        ]},{"body":{"stringValue":"claude_code.api_request_body"},"attributes":[attr("body",json!({"stringValue":"secret prompt"}))]}]}]}]});
        let rows = extract_api_requests(&payload);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, MeasurementSource::OfficialTelemetry);
        assert_eq!(rows[0].usage.input_tokens, Some(10));
        assert_eq!(rows[0].usage.cached_input_tokens, Some(5));
    }
    #[tokio::test]
    async fn rejects_non_loopback_bind() {
        let ledger = Ledger::open_memory().unwrap();
        let result = serve("0.0.0.0:0".parse().unwrap(), ledger, OtlpAuth::generate()).await;
        assert!(result.unwrap_err().to_string().contains("loopback"));
    }

    fn authed_router() -> (Router, String) {
        let auth = OtlpAuth::generate();
        let token = auth.bearer_token.clone();
        let ledger = Ledger::open_memory().unwrap();
        (create_router(auth, ledger), token)
    }

    fn ingest_request(token: Option<&str>, body: &str) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/logs")
            .header("Content-Type", "application/json");
        if let Some(token) = token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    const VALID_OTLP_BODY: &str = r#"{"resourceLogs":[{"resource":{"attributes":[{"key":"session.id","value":{"stringValue":"s"}}]},"scopeLogs":[{"logRecords":[{"attributes":[
        {"key":"event.name","value":{"stringValue":"api_request"}},
        {"key":"request_id","value":{"stringValue":"req_1"}},
        {"key":"input_tokens","value":{"intValue":"10"}},
        {"key":"output_tokens","value":{"intValue":"2"}}
    ]}]}]}]}"#;

    #[tokio::test]
    async fn rejects_ingest_without_bearer_token() {
        use tower::ServiceExt;
        let (router, _token) = authed_router();
        let res = router
            .oneshot(ingest_request(None, VALID_OTLP_BODY))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_ingest_with_wrong_bearer_token() {
        use tower::ServiceExt;
        let (router, _token) = authed_router();
        let res = router
            .oneshot(ingest_request(Some("wrong-token"), VALID_OTLP_BODY))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_ingest_with_valid_bearer_token() {
        use tower::ServiceExt;
        let (router, token) = authed_router();
        let res = router
            .oneshot(ingest_request(Some(&token), VALID_OTLP_BODY))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["accepted"], 1);
        assert_eq!(parsed["inserted"], 1);
    }

    #[tokio::test]
    async fn malformed_bearer_scheme_rejected() {
        use tower::ServiceExt;
        let (router, token) = authed_router();
        // "Token" scheme instead of "Bearer" must not authenticate.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/logs")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Token {token}"))
            .body(axum::body::Body::from(VALID_OTLP_BODY.to_string()))
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn bearer_tokens_are_high_entropy() {
        let t1 = generate_bearer_token();
        let t2 = generate_bearer_token();
        assert_eq!(t1.len(), 64);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t1, t2);
    }

    #[test]
    fn constant_time_eq_behaves_like_eq() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "abc"));
        assert!(constant_time_eq("", ""));
    }
}
