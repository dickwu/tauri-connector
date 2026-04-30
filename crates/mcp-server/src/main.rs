//! MCP stdio server for tauri-plugin-connector.
//!
//! Reads JSON-RPC 2.0 requests from stdin, dispatches them to the
//! Tauri app via WebSocket, and writes responses to stdout.

use std::io::{self, BufRead, Write};

use connector_client::discovery::{self, ConnectionOptions};
use connector_client::ConnectorClient;
use serde_json::{json, Value};

mod protocol;
mod tools;

use protocol::{JsonRpcRequest, JsonRpcResponse};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 9555;
const PROTOCOL_LATEST: &str = "2025-11-25";
const PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

#[tokio::main]
async fn main() {
    let resolved = discovery::resolve_connection(ConnectionOptions::from_current_dir())
        .await
        .unwrap_or_else(|e| {
            eprintln!("[tauri-connector-mcp] Discovery failed: {e}");
            let host =
                std::env::var("TAURI_CONNECTOR_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
            let port = std::env::var("TAURI_CONNECTOR_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            discovery::ResolvedConnection {
                host,
                port,
                source: discovery::ConnectionSource::Env,
                instance: None,
            }
        });
    let host = resolved.host;
    let port = resolved.port;

    let mut client = ConnectorClient::new();

    eprintln!("[tauri-connector-mcp] Server started on stdio (target: {host}:{port})");

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {e}"));
                write_response(&stdout, &resp);
                continue;
            }
        };

        // Notifications (no id) don't get a response
        let Some(id) = request.id.clone() else {
            // Handle notification silently
            handle_notification(&request.method).await;
            continue;
        };

        let response = handle_request(&mut client, &host, port, id, &request).await;
        write_response(&stdout, &response);
    }
}

async fn handle_notification(method: &str) {
    match method {
        "notifications/initialized" => {
            eprintln!("[tauri-connector-mcp] Client initialized");
        }
        "notifications/cancelled" => {
            eprintln!("[tauri-connector-mcp] Request cancelled");
        }
        _ => {
            eprintln!("[tauri-connector-mcp] Unknown notification: {method}");
        }
    }
}

async fn handle_request(
    client: &mut ConnectorClient,
    host: &str,
    port: u16,
    id: Value,
    request: &JsonRpcRequest,
) -> JsonRpcResponse {
    match request.method.as_str() {
        "initialize" => {
            let requested = request
                .params
                .as_ref()
                .and_then(|params| params.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_LATEST);
            if !PROTOCOL_SUPPORTED.contains(&requested) {
                return JsonRpcResponse::error(
                    id,
                    -32002,
                    format!("Unsupported protocol version: {requested}"),
                );
            }
            let result = json!({
                "protocolVersion": requested,
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
                "instructions": "Use webview_dom_snapshot, webview_interact, read_logs, ipc_* and bridge_status to debug the running Tauri app."
            });
            JsonRpcResponse::success(id, result)
        }

        "tools/list" => {
            let result = tools::tool_definitions();
            JsonRpcResponse::success(id, result)
        }

        "tools/call" => {
            let params = request.params.as_ref().cloned().unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            // Auto-connect if not connected (except for driver_session)
            if tool_name != "driver_session" && !client.is_connected() {
                if let Err(e) = client.connect(host, port).await {
                    return JsonRpcResponse::success(
                        id,
                        protocol::text_content(&json!({
                            "error": format!("Auto-connect failed: {e}. Use driver_session to connect manually.")
                        })),
                    );
                }
            }

            let result = tools::call_tool(client, host, port, tool_name, &arguments).await;
            JsonRpcResponse::success(id, result)
        }

        "ping" => JsonRpcResponse::success(id, json!({})),

        _ => JsonRpcResponse::error(id, -32601, format!("Method not found: {}", request.method)),
    }
}

fn write_response(stdout: &io::Stdout, response: &JsonRpcResponse) {
    let json = serde_json::to_string(response).unwrap_or_default();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{json}");
    let _ = out.flush();
}
