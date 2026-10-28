//! Embedded MCP HTTP server.
//!
//! `/mcp` is the Streamable HTTP endpoint. `/sse` and `/message` remain as
//! the legacy HTTP+SSE transport for older clients.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, ALLOW, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::{get, post};
use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::bridge::Bridge;
use crate::mcp_tools;
use crate::state::PluginState;

const PROTOCOL_LATEST: &str = "2025-11-25";
const PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const PROTOCOL_LEGACY_SSE: &str = "2024-11-05";
const HEADER_MCP_SESSION_ID: &str = "mcp-session-id";
const HEADER_MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// Streamable HTTP sessions idle longer than this are dropped. Clients get
/// 404 on the next request and must re-initialize, as the spec requires.
const STREAMABLE_SESSION_IDLE_TTL_MS: u64 = 60 * 60 * 1000;

type LegacySessions = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct McpSession {
    pub id: String,
    pub created_at_ms: u64,
    pub last_seen_ms: u64,
    pub protocol_version: String,
    pub initialized: bool,
    pub client_info: Option<Value>,
}

/// Shared state for the embedded MCP HTTP server.
#[derive(Clone)]
pub struct McpState {
    bridge: Bridge,
    plugin_state: PluginState,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    streamable_sessions: Arc<Mutex<HashMap<String, McpSession>>>,
    legacy_sessions: LegacySessions,
}

/// Start the embedded MCP HTTP server.
///
/// Returns the port it's listening on.
pub async fn start(
    bind_address: &str,
    port_range: (u16, u16),
    bridge: Bridge,
    plugin_state: PluginState,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
) -> Result<u16, String> {
    let (std_listener, port) = crate::net::bind_in_range(bind_address, port_range.0, port_range.1)
        .map_err(|e| format!("MCP bind failed: {e}"))?;

    let state = McpState {
        bridge,
        plugin_state,
        app_handle,
        streamable_sessions: Arc::new(Mutex::new(HashMap::new())),
        legacy_sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = router(state);

    let listener = tokio::net::TcpListener::from_std(std_listener)
        .map_err(|e| format!("MCP bind failed: {e}"))?;

    println!(
        "[connector][mcp] HTTP server listening on {bind_address}:{port} (/mcp Streamable HTTP, /sse legacy)"
    );

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("[connector][mcp] Server error: {e}");
        }
    });

    Ok(port)
}

fn router(state: McpState) -> Router {
    Router::new()
        .route(
            "/mcp",
            get(streamable_get)
                .post(streamable_post)
                .delete(streamable_delete),
        )
        .route("/sse", get(legacy_sse_get))
        .route("/message", post(legacy_message_post))
        .with_state(state)
}

/// GET /mcp. Server-initiated Streamable HTTP SSE is not implemented yet, so
/// the endpoint must not emit the legacy `endpoint` event.
async fn streamable_get(headers: HeaderMap) -> AxumResponse {
    if let Err(err) = validate_origin(&headers) {
        return err.into_response();
    }

    let mut resp = StatusCode::METHOD_NOT_ALLOWED.into_response();
    resp.headers_mut()
        .insert(ALLOW, HeaderValue::from_static("POST, DELETE"));
    resp
}

/// POST /mcp - Streamable HTTP JSON-RPC request endpoint.
async fn streamable_post(
    headers: HeaderMap,
    State(state): State<McpState>,
    body: String,
) -> AxumResponse {
    for check in [
        validate_origin(&headers),
        validate_content_type(&headers),
        validate_accept_post(&headers),
        validate_protocol_header_supported(&headers),
    ] {
        if let Err(err) = check {
            return err.into_response();
        }
    }

    let value: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return jsonrpc_http_error(
                StatusCode::BAD_REQUEST,
                Value::Null,
                -32700,
                &format!("Parse error: {e}"),
            );
        }
    };

    if value.is_array() {
        return jsonrpc_http_error(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32600,
            "Batch JSON-RPC bodies are not supported on this Streamable HTTP endpoint; send a single message",
        );
    }

    let message = match classify_jsonrpc(&value) {
        Ok(kind) => kind,
        Err(err) => return err.into_response(),
    };

    match message {
        JsonRpcMessageKind::Request { id, method, params } => {
            if method == "initialize" {
                return handle_streamable_initialize(state, headers, id, params).await;
            }

            let session = match require_streamable_session(&state, &headers, &id).await {
                Ok(session) => session,
                Err(err) => return err.into_response(),
            };

            let response =
                dispatch_jsonrpc_request(state, id, &method, params, &session.protocol_version)
                    .await;
            jsonrpc_response_with_session(response, &session)
        }
        JsonRpcMessageKind::Notification {
            method: _,
            params: _,
        } => {
            let null_id = Value::Null;
            if let Err(err) = require_streamable_session(&state, &headers, &null_id).await {
                return err.into_response();
            }
            StatusCode::ACCEPTED.into_response()
        }
        JsonRpcMessageKind::Response {
            id,
            result_or_error: _,
        } => {
            if let Err(err) = require_streamable_session(&state, &headers, &id).await {
                return err.into_response();
            }
            StatusCode::ACCEPTED.into_response()
        }
    }
}

async fn streamable_delete(headers: HeaderMap, State(state): State<McpState>) -> AxumResponse {
    if let Err(err) = validate_origin(&headers) {
        return err.into_response();
    }

    let Some(session_id) = session_id_from_headers(&headers) else {
        return jsonrpc_http_error(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32001,
            "Missing MCP-Session-Id header",
        );
    };

    if !is_visible_ascii(&session_id) {
        return jsonrpc_http_error(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32001,
            "Invalid MCP-Session-Id header",
        );
    }

    let removed = state.streamable_sessions.lock().await.remove(&session_id);
    if removed.is_none() {
        return jsonrpc_http_error(
            StatusCode::NOT_FOUND,
            Value::Null,
            -32001,
            "Unknown MCP session",
        );
    }

    StatusCode::NO_CONTENT.into_response()
}

/// GET /sse - legacy SSE event stream for older MCP clients.
async fn legacy_sse_get(headers: HeaderMap, State(state): State<McpState>) -> AxumResponse {
    if let Err(err) = validate_origin(&headers) {
        return err.into_response();
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    state
        .legacy_sessions
        .lock()
        .await
        .insert(session_id.clone(), tx);

    // Unregisters the session when the client disconnects and axum drops
    // the response stream.
    let guard = LegacySessionGuard {
        id: session_id.clone(),
        sessions: state.legacy_sessions.clone(),
    };

    // First event: tell the client where to POST
    let endpoint_event = stream::once(async move {
        let data = format!("/message?sessionId={session_id}");
        Ok::<_, Infallible>(Event::default().event("endpoint").data(data))
    });

    // Subsequent events: JSON-RPC responses
    let response_stream = UnboundedReceiverStream::new(rx).map(move |data| {
        let _ = &guard;
        Ok::<_, Infallible>(Event::default().event("message").data(data))
    });

    Sse::new(endpoint_event.chain(response_stream))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// POST /message?sessionId=xxx - legacy HTTP+SSE JSON-RPC request endpoint.
async fn legacy_message_post(
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<McpState>,
    body: String,
) -> AxumResponse {
    if let Err(err) = validate_origin(&headers) {
        return err.into_response();
    }

    // In the legacy HTTP+SSE transport, responses travel only over the SSE
    // stream; the POST itself is acknowledged with 202 and an empty body.
    let Some(tx) = legacy_session_sender(&state, params.get("sessionId")).await else {
        return jsonrpc_http_error(
            StatusCode::NOT_FOUND,
            Value::Null,
            -32001,
            "Unknown or missing legacy SSE sessionId",
        );
    };

    if let Some(response) = handle_legacy_jsonrpc_request(state, body).await {
        // A closed receiver means the SSE stream is gone; its guard removes
        // the session, so there is nobody left to deliver to.
        let _ = tx.send(response);
    }
    StatusCode::ACCEPTED.into_response()
}

async fn legacy_session_sender(
    state: &McpState,
    session_id: Option<&String>,
) -> Option<mpsc::UnboundedSender<String>> {
    let session_id = session_id?;
    let sessions = state.legacy_sessions.lock().await;
    sessions
        .get(session_id)
        .filter(|tx| !tx.is_closed())
        .cloned()
}

/// Removes a legacy SSE session from the registry when dropped.
struct LegacySessionGuard {
    id: String,
    sessions: LegacySessions,
}

impl Drop for LegacySessionGuard {
    fn drop(&mut self) {
        let id = std::mem::take(&mut self.id);
        let sessions = self.sessions.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                sessions.lock().await.remove(&id);
            });
        }
    }
}

/// Drop Streamable HTTP sessions that have been idle past the TTL.
fn prune_idle_sessions(sessions: &mut HashMap<String, McpSession>, now: u64) {
    sessions.retain(|_, session| {
        now.saturating_sub(session.last_seen_ms) <= STREAMABLE_SESSION_IDLE_TTL_MS
    });
}

async fn handle_streamable_initialize(
    state: McpState,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> AxumResponse {
    let negotiated = match negotiate_protocol(&params) {
        Ok(version) => version,
        Err(message) => {
            return jsonrpc_response(StatusCode::OK, jsonrpc_error(id, -32002, &message));
        }
    };

    if let Some(header_version) = protocol_version_from_headers(&headers)
        && header_version != negotiated
    {
        return jsonrpc_http_error(
            StatusCode::BAD_REQUEST,
            id,
            -32002,
            "MCP-Protocol-Version header does not match initialized protocolVersion",
        );
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let now = now_ms();
    let session = McpSession {
        id: session_id.clone(),
        created_at_ms: now,
        last_seen_ms: now,
        protocol_version: negotiated.clone(),
        initialized: true,
        client_info: params.get("clientInfo").cloned(),
    };
    {
        let mut sessions = state.streamable_sessions.lock().await;
        prune_idle_sessions(&mut sessions, now);
        sessions.insert(session_id.clone(), session);
    }

    let result = with_app_meta(initialize_result(&negotiated), &state).await;
    let mut response = jsonrpc_response(StatusCode::OK, jsonrpc_success(id, result));
    insert_header(&mut response, HEADER_MCP_SESSION_ID, &session_id);
    insert_header(&mut response, HEADER_MCP_PROTOCOL_VERSION, &negotiated);
    response
}

/// Run one legacy JSON-RPC message; `None` for notifications.
async fn handle_legacy_jsonrpc_request(state: McpState, body: String) -> Option<String> {
    let request: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            let resp = jsonrpc_error(Value::Null, -32700, &format!("Parse error: {e}"));
            return serde_json::to_string(&resp).ok();
        }
    };

    let id = request.get("id").cloned();
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");

    // Notifications (no id) — no response
    let id = id?;
    let params_val = request.get("params").cloned().unwrap_or(json!({}));

    let response =
        dispatch_jsonrpc_request(state, id, method, params_val, PROTOCOL_LEGACY_SSE).await;
    serde_json::to_string(&response).ok()
}

async fn dispatch_jsonrpc_request(
    state: McpState,
    id: Value,
    method: &str,
    params_val: Value,
    protocol_version: &str,
) -> Value {
    match method {
        "initialize" if protocol_version == PROTOCOL_LEGACY_SSE => jsonrpc_success(
            id,
            json!({
                "protocolVersion": PROTOCOL_LEGACY_SSE,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tauri-connector", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),

        "initialize" => jsonrpc_success(
            id,
            with_app_meta(initialize_result(PROTOCOL_LATEST), &state).await,
        ),

        "tools/list" => jsonrpc_success(id, mcp_tools::tool_definitions()),

        "tools/call" => {
            let tool_name = params_val
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params_val.get("arguments").cloned().unwrap_or(json!({}));

            // Clone the handle out so the lock is not held across tool
            // execution — batch_actions runs tools concurrently.
            let app = state.app_handle.lock().await.clone();
            let result = mcp_tools::call_tool(
                tool_name,
                &arguments,
                &state.bridge,
                app.as_ref(),
                &state.plugin_state,
            )
            .await;
            jsonrpc_success(
                id,
                connector_client::inspection::shape_mcp_result(result, Some(protocol_version)),
            )
        }

        "ping" => jsonrpc_success(id, json!({})),

        _ => jsonrpc_error(id, -32601, &format!("Method not found: {method}")),
    }
}

#[allow(dead_code)]
#[derive(Debug)]
enum JsonRpcMessageKind {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result_or_error: Value,
    },
}

fn classify_jsonrpc(value: &Value) -> Result<JsonRpcMessageKind, HttpJsonRpcError> {
    let Some(obj) = value.as_object() else {
        return Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32600,
            "JSON-RPC body must be an object",
        ));
    };

    if let Some(version) = obj.get("jsonrpc").and_then(|v| v.as_str())
        && version != "2.0"
    {
        return Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            obj.get("id").cloned().unwrap_or(Value::Null),
            -32600,
            "jsonrpc must be 2.0",
        ));
    }

    if let Some(method) = obj.get("method").and_then(|v| v.as_str()) {
        let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
        return match obj.get("id") {
            Some(Value::Null) | None => Ok(JsonRpcMessageKind::Notification {
                method: method.to_string(),
                params,
            }),
            Some(id) => Ok(JsonRpcMessageKind::Request {
                id: id.clone(),
                method: method.to_string(),
                params,
            }),
        };
    }

    if let Some(id) = obj.get("id")
        && (obj.contains_key("result") || obj.contains_key("error"))
    {
        let result_or_error = obj
            .get("result")
            .or_else(|| obj.get("error"))
            .cloned()
            .unwrap_or(Value::Null);
        return Ok(JsonRpcMessageKind::Response {
            id: id.clone(),
            result_or_error,
        });
    }

    Err(HttpJsonRpcError::new(
        StatusCode::BAD_REQUEST,
        obj.get("id").cloned().unwrap_or(Value::Null),
        -32600,
        "Invalid JSON-RPC message",
    ))
}

async fn require_streamable_session(
    state: &McpState,
    headers: &HeaderMap,
    id: &Value,
) -> Result<McpSession, HttpJsonRpcError> {
    let Some(session_id) = session_id_from_headers(headers) else {
        return Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            id.clone(),
            -32001,
            "Missing MCP-Session-Id header",
        ));
    };

    if !is_visible_ascii(&session_id) {
        return Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            id.clone(),
            -32001,
            "Invalid MCP-Session-Id header",
        ));
    }

    let header_protocol = protocol_version_from_headers(headers);
    let now = now_ms();
    let mut sessions = state.streamable_sessions.lock().await;
    prune_idle_sessions(&mut sessions, now);
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(HttpJsonRpcError::new(
            StatusCode::NOT_FOUND,
            id.clone(),
            -32001,
            "Unknown MCP session",
        ));
    };

    if let Some(version) = header_protocol
        && version != session.protocol_version
    {
        return Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            id.clone(),
            -32002,
            "MCP-Protocol-Version header does not match session protocolVersion",
        ));
    }

    session.last_seen_ms = now;
    Ok(session.clone())
}

fn validate_origin(headers: &HeaderMap) -> Result<(), HttpJsonRpcError> {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    if origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
        || origin == "tauri://localhost"
    {
        Ok(())
    } else {
        Err(HttpJsonRpcError::new(
            StatusCode::FORBIDDEN,
            Value::Null,
            -32000,
            "Origin not allowed",
        ))
    }
}

fn validate_accept_post(headers: &HeaderMap) -> Result<(), HttpJsonRpcError> {
    let Some(value) = headers.get(ACCEPT).and_then(|v| v.to_str().ok()) else {
        eprintln!("[connector][mcp] POST /mcp missing Accept header; allowing for compatibility");
        return Ok(());
    };

    if accept_contains(value, "application/json") && accept_contains(value, "text/event-stream") {
        Ok(())
    } else {
        Err(HttpJsonRpcError::new(
            StatusCode::NOT_ACCEPTABLE,
            Value::Null,
            -32000,
            "Accept must include application/json and text/event-stream",
        ))
    }
}

fn validate_content_type(headers: &HeaderMap) -> Result<(), HttpJsonRpcError> {
    let Some(value) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return Err(HttpJsonRpcError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Value::Null,
            -32000,
            "Content-Type must be application/json",
        ));
    };

    let media_type = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media_type == "application/json" {
        Ok(())
    } else {
        Err(HttpJsonRpcError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Value::Null,
            -32000,
            "Content-Type must be application/json",
        ))
    }
}

fn validate_protocol_header_supported(headers: &HeaderMap) -> Result<(), HttpJsonRpcError> {
    let Some(version) = protocol_version_from_headers(headers) else {
        return Ok(());
    };
    if PROTOCOL_SUPPORTED.contains(&version.as_str()) {
        Ok(())
    } else {
        Err(HttpJsonRpcError::new(
            StatusCode::BAD_REQUEST,
            Value::Null,
            -32002,
            "Unsupported MCP-Protocol-Version",
        ))
    }
}

fn negotiate_protocol(params: &Value) -> Result<String, String> {
    let requested = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_LATEST);

    if requested == PROTOCOL_LEGACY_SSE {
        return Err(
            "Protocol version 2024-11-05 is only supported by legacy /sse transport".into(),
        );
    }
    if PROTOCOL_SUPPORTED.contains(&requested) {
        Ok(requested.to_string())
    } else {
        Err(format!("Unsupported protocol version: {requested}"))
    }
}

/// `_meta` key naming the app that owns this MCP endpoint, so `doctor` can
/// tell when `.mcp.json` points at another app's port (0.17.5+).
pub(crate) const APP_META_KEY: &str = "dev.tauri-connector/app";

/// Add the owning app to an initialize result. Omitted when the host set a
/// workflow token, because identity is then an authenticated read.
async fn with_app_meta(mut result: Value, state: &McpState) -> Value {
    if state.plugin_state.workflow.token_configured() {
        return result;
    }
    let identity = state
        .app_handle
        .lock()
        .await
        .as_ref()
        .map(crate::identity::app_identity);
    if let Some(meta) = identity.as_ref().map(app_meta) {
        result["_meta"] = json!({ APP_META_KEY: meta });
    }
    result
}

fn app_meta(identity: &Value) -> Value {
    json!({
        "appId": identity["appId"],
        "appName": identity["appName"],
        "pid": identity["pid"],
        "pluginVersion": identity["pluginVersion"],
        "projectName": identity["projectName"],
        "projectRoot": identity["projectRoot"],
    })
}

fn initialize_result(protocol_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "tauri-connector",
            "title": "Tauri Connector",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": crate::mcp_tool_schema::server_instructions()
    })
}

fn accept_contains(header: &str, expected: &str) -> bool {
    header.split(',').any(|part| {
        let token = part
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        token == expected || token == "*/*"
    })
}

fn session_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER_MCP_SESSION_ID)
        .or_else(|| headers.get("Mcp-Session-Id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn protocol_version_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER_MCP_PROTOCOL_VERSION)
        .or_else(|| headers.get("Mcp-Protocol-Version"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn is_visible_ascii(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct HttpJsonRpcError {
    status: StatusCode,
    id: Value,
    code: i64,
    message: String,
}

impl HttpJsonRpcError {
    fn new(status: StatusCode, id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            status,
            id,
            code,
            message: message.into(),
        }
    }

    fn into_response(self) -> AxumResponse {
        jsonrpc_http_error(self.status, self.id, self.code, &self.message)
    }
}

fn jsonrpc_success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn jsonrpc_http_error(status: StatusCode, id: Value, code: i64, message: &str) -> AxumResponse {
    jsonrpc_response(status, jsonrpc_error(id, code, message))
}

fn jsonrpc_response(status: StatusCode, value: Value) -> AxumResponse {
    json_string_response(status, serde_json::to_string(&value).unwrap_or_default())
}

fn jsonrpc_response_with_session(value: Value, session: &McpSession) -> AxumResponse {
    let mut response = jsonrpc_response(StatusCode::OK, value);
    insert_header(&mut response, HEADER_MCP_SESSION_ID, &session.id);
    insert_header(
        &mut response,
        HEADER_MCP_PROTOCOL_VERSION,
        &session.protocol_version,
    );
    response
}

fn json_string_response(status: StatusCode, body: String) -> AxumResponse {
    (status, [(CONTENT_TYPE, "application/json")], body).into_response()
}

fn insert_header(response: &mut AxumResponse, name: &'static str, value: &str) {
    let Ok(value) = HeaderValue::from_str(value) else {
        return;
    };
    response
        .headers_mut()
        .insert(HeaderName::from_static(name), value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    fn test_state() -> McpState {
        let bridge = Bridge::start().expect("bridge starts");
        let log_dir =
            std::env::temp_dir().join(format!("tauri-connector-mcp-test-{}", uuid::Uuid::new_v4()));
        let plugin_state = PluginState::new(log_dir).expect("plugin state");
        McpState {
            bridge,
            plugin_state,
            app_handle: Arc::new(Mutex::new(None)),
            streamable_sessions: Arc::new(Mutex::new(HashMap::new())),
            legacy_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn test_router() -> Router {
        router(test_state())
    }

    fn legacy_message(session: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(format!("/message?sessionId={session}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn mcp_request(body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    async fn body_json(response: AxumResponse) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn post_mcp_initialize_negotiates_latest_and_sets_session_headers() {
        let app = test_router();
        let response = app
            .oneshot(mcp_request(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            })))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(HEADER_MCP_SESSION_ID));
        assert_eq!(
            response
                .headers()
                .get(HEADER_MCP_PROTOCOL_VERSION)
                .unwrap()
                .to_str()
                .unwrap(),
            "2025-11-25"
        );
        let body = body_json(response).await;
        assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(body["result"]["serverInfo"]["title"], "Tauri Connector");
        let meta = app_meta(&json!({"appId":"com.example","appName":"Example","pid":7,
            "pluginVersion":"0.17.5","workspacePath":"/private","projectName":"Example",
            "projectRoot":"/src/example"}));
        assert_eq!(meta["projectName"], "Example");
        assert_eq!(meta["projectRoot"], "/src/example");
        assert_eq!(meta["appId"], "com.example");
        assert_eq!(meta["appName"], "Example");
        assert!(meta.get("workspacePath").is_none());
    }

    #[tokio::test]
    async fn post_mcp_notification_returns_accepted_empty_body() {
        let app = test_router();
        let init_response = app
            .clone()
            .oneshot(mcp_request(json!({
                "jsonrpc": "2.0",
                "id": "init",
                "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" }
            })))
            .await
            .unwrap();
        let session = init_response
            .headers()
            .get(HEADER_MCP_SESSION_ID)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(CONTENT_TYPE, "application/json")
                    .header(HEADER_MCP_SESSION_ID, session)
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn get_mcp_does_not_emit_legacy_endpoint_event() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/mcp")
                    .header(ACCEPT, "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers().get(ALLOW).unwrap(), "POST, DELETE");
    }

    #[tokio::test]
    async fn invalid_origin_is_forbidden() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(CONTENT_TYPE, "application/json")
                    .header("origin", "https://example.com")
                    .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn unsupported_protocol_header_is_bad_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(CONTENT_TYPE, "application/json")
                    .header(HEADER_MCP_PROTOCOL_VERSION, "1900-01-01")
                    .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn deleted_session_cannot_be_reused() {
        let app = test_router();
        let init_response = app
            .clone()
            .oneshot(mcp_request(json!({
                "jsonrpc": "2.0",
                "id": "init",
                "method": "initialize",
                "params": {}
            })))
            .await
            .unwrap();
        let session = init_response
            .headers()
            .get(HEADER_MCP_SESSION_ID)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/mcp")
                    .header(HEADER_MCP_SESSION_ID, session.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let ping_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(CONTENT_TYPE, "application/json")
                    .header(HEADER_MCP_SESSION_ID, session)
                    .body(Body::from(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ping_response.status(), StatusCode::NOT_FOUND);
    }
    #[tokio::test]
    async fn negotiated_tool_content_keeps_legacy_text_without_unnegotiated_structured_fields() {
        for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
            let app = test_router();
            let init=app.clone().oneshot(mcp_request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version}}))).await.unwrap();
            let session = init.headers().get(HEADER_MCP_SESSION_ID).unwrap().clone();
            let mut request = mcp_request(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"webview_select_element","arguments":{"action":"get","pickerId":"missing"}}}),
            );
            request.headers_mut().insert(HEADER_MCP_SESSION_ID, session);
            let response = body_json(app.oneshot(request).await.unwrap()).await;
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(response["result"]["content"][0]["type"], "text");
            assert_eq!(
                response["result"].get("structuredContent").is_some(),
                version != "2025-03-26"
            );
        }
        let state = test_state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        state
            .legacy_sessions
            .lock()
            .await
            .insert("legacy-test".into(), tx);
        let response = router(state)
            .oneshot(legacy_message(
                "legacy-test",
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"webview_select_element","arguments":{"action":"get","pickerId":"missing"}}}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert!(body["result"].get("structuredContent").is_none());
        assert_eq!(body["result"]["content"][0]["type"], "text");
    }

    #[tokio::test]
    async fn legacy_message_replies_once_over_sse_with_empty_post_body() {
        let state = test_state();
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.legacy_sessions.lock().await.insert("s1".into(), tx);
        let response = router(state)
            .oneshot(legacy_message(
                "s1",
                json!({"jsonrpc":"2.0","id":7,"method":"ping"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(bytes.is_empty());
        let sse: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(sse["id"], 7);
        assert!(rx.try_recv().is_err(), "response must be delivered once");
    }

    #[tokio::test]
    async fn legacy_message_with_unknown_session_is_rejected() {
        let app = test_router();
        for uri in ["/message?sessionId=nope", "/message"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(uri)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn legacy_sse_session_is_removed_when_stream_is_dropped() {
        let state = test_state();
        let sessions = state.legacy_sessions.clone();
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/sse")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(sessions.lock().await.len(), 1);
        drop(response);
        for _ in 0..50 {
            if sessions.lock().await.is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("legacy session leaked after the SSE stream was dropped");
    }

    #[test]
    fn idle_streamable_sessions_are_pruned() {
        let session = |id: &str, last_seen_ms: u64| McpSession {
            id: id.into(),
            created_at_ms: 0,
            last_seen_ms,
            protocol_version: PROTOCOL_LATEST.into(),
            initialized: true,
            client_info: None,
        };
        let now = STREAMABLE_SESSION_IDLE_TTL_MS * 3;
        let mut sessions = HashMap::from([
            ("fresh".to_string(), session("fresh", now - 1)),
            (
                "edge".to_string(),
                session("edge", now - STREAMABLE_SESSION_IDLE_TTL_MS),
            ),
            ("stale".to_string(), session("stale", 0)),
        ]);
        prune_idle_sessions(&mut sessions, now);
        let mut kept: Vec<_> = sessions.keys().cloned().collect();
        kept.sort();
        assert_eq!(kept, ["edge", "fresh"]);
    }
}
