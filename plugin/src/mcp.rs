//! Embedded MCP server using SSE transport.
//!
//! Starts an HTTP server inside the plugin so Claude Code can connect
//! via `"url": "http://127.0.0.1:PORT/sse"` — no separate process needed.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::TcpListener;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream::{self, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::bridge::Bridge;
use crate::mcp_tools;
use crate::state::PluginState;

/// Shared state for the MCP SSE server.
#[derive(Clone)]
pub struct McpState {
    bridge: Bridge,
    plugin_state: PluginState,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
}

/// Start the embedded MCP SSE server.
///
/// Returns the port it's listening on.
pub async fn start(
    bind_address: &str,
    port_range: (u16, u16),
    bridge: Bridge,
    plugin_state: PluginState,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
) -> Result<u16, String> {
    let port = find_available_port(bind_address, port_range.0, port_range.1)
        .ok_or_else(|| format!("No MCP port in range {}-{}", port_range.0, port_range.1))?;

    let state = McpState {
        bridge,
        plugin_state,
        app_handle,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("{bind_address}:{port}"))
        .await
        .map_err(|e| format!("MCP bind failed: {e}"))?;

    println!("[connector][mcp] SSE server listening on {bind_address}:{port}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("[connector][mcp] Server error: {e}");
        }
    });

    Ok(port)
}

/// GET /sse — SSE event stream for an MCP client session.
async fn sse_handler(
    State(state): State<McpState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), tx);

    // First event: tell the client where to POST
    let endpoint_event = stream::once(async move {
        let data = format!("/message?sessionId={session_id}");
        Ok::<_, Infallible>(Event::default().event("endpoint").data(data))
    });

    // Subsequent events: JSON-RPC responses
    let response_stream = UnboundedReceiverStream::new(rx).map(|data| {
        Ok::<_, Infallible>(Event::default().event("message").data(data))
    });

    Sse::new(endpoint_event.chain(response_stream)).keep_alive(KeepAlive::default())
}

/// POST /message?sessionId=xxx — handle a JSON-RPC request.
async fn message_handler(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<McpState>,
    body: String,
) -> String {
    let session_id = params.get("sessionId").cloned().unwrap_or_default();

    let request: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            let resp = jsonrpc_error(Value::Null, -32700, &format!("Parse error: {e}"));
            return serde_json::to_string(&resp).unwrap_or_default();
        }
    };

    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Notifications (no id) — no response
    if id.is_none() {
        return String::new();
    }

    let id = id.unwrap_or(Value::Null);
    let params_val = request.get("params").cloned().unwrap_or(json!({}));

    let response = match method {
        "initialize" => jsonrpc_success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tauri-connector", "version": "0.1.0" }
            }),
        ),

        "tools/list" => jsonrpc_success(id, mcp_tools::tool_definitions()),

        "tools/call" => {
            let tool_name = params_val
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params_val.get("arguments").cloned().unwrap_or(json!({}));

            let app = state.app_handle.lock().await;
            let result = mcp_tools::call_tool(
                tool_name,
                &arguments,
                &state.bridge,
                app.as_ref(),
                &state.plugin_state,
            )
            .await;
            jsonrpc_success(id, result)
        }

        "ping" => jsonrpc_success(id, json!({})),

        _ => jsonrpc_error(id, -32601, &format!("Method not found: {method}")),
    };

    let response_str = serde_json::to_string(&response).unwrap_or_default();

    // Also push to SSE stream
    if let Some(tx) = state.sessions.lock().await.get(&session_id) {
        let _ = tx.send(response_str.clone());
    }

    response_str
}

fn jsonrpc_success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn find_available_port(addr: &str, start: u16, end: u16) -> Option<u16> {
    (start..end).find(|&port| TcpListener::bind((addr, port)).is_ok())
}
