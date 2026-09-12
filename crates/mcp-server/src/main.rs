//! MCP stdio server for tauri-plugin-connector.
//!
//! Reads JSON-RPC 2.0 requests from stdin, dispatches them to the
//! Tauri app via WebSocket, and writes responses to stdout.

use std::io::{self, BufRead, Write};

use connector_client::discovery::{self, ConnectionOptions};
use connector_client::ConnectorClient;
use serde_json::{json, Value};

use connector_mcp_server::protocol::{self, JsonRpcRequest, JsonRpcResponse};
use connector_mcp_server::tools;

const PROTOCOL_LATEST: &str = "2025-11-25";
const PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

#[tokio::main]
async fn main() {
    let resolved = discovery::resolve_connection(ConnectionOptions::from_current_dir())
        .await
        .unwrap_or_else(|e| {
            eprintln!("[tauri-connector-mcp] Discovery failed: {e}");
            std::process::exit(1);
        });
    let host = resolved.host;
    let port = resolved.port;

    let mut client = ConnectorClient::new();
    if let Some(identity) = &resolved.identity {
        if let Err(error) = client.bind_instance(&identity.app_instance_id) {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }

    let mut negotiated_protocol: Option<String> = None;

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

        let response = handle_request(
            &mut client,
            &host,
            port,
            id,
            &request,
            &mut negotiated_protocol,
        )
        .await;
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
    negotiated_protocol: &mut Option<String>,
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
            *negotiated_protocol = Some(requested.to_owned());
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
                "instructions": tools::server_instructions()
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
                    let mut content = protocol::text_content(
                        &json!({"error":format!("Auto-connect failed: {e}. Use driver_session to connect manually.")}),
                    );
                    content["isError"] = json!(true);
                    return JsonRpcResponse::success(id, content);
                }
            }

            let result = tools::call_tool(client, host, port, tool_name, &arguments).await;
            JsonRpcResponse::success(
                id,
                connector_client::inspection::shape_mcp_result(
                    result,
                    negotiated_protocol.as_deref(),
                ),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn standalone_uses_initialized_protocol_to_shape_tool_results() {
        for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
            let mut client = ConnectorClient::new();
            let mut negotiated = None;
            let init:JsonRpcRequest=serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version}})).unwrap();
            let response = handle_request(
                &mut client,
                "127.0.0.1",
                9555,
                json!(1),
                &init,
                &mut negotiated,
            )
            .await;
            assert_eq!(response.result.unwrap()["protocolVersion"], version);
            let call:JsonRpcRequest=serde_json::from_value(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"driver_session","arguments":{"action":"start","port":-1}}})).unwrap();
            let result = handle_request(
                &mut client,
                "127.0.0.1",
                9555,
                json!(2),
                &call,
                &mut negotiated,
            )
            .await
            .result
            .unwrap();
            assert_eq!(result["isError"], true);
            assert_eq!(result["content"][0]["type"], "text");
            assert_eq!(
                result.get("structuredContent").is_some(),
                version != "2025-03-26"
            );
        }
    }
}
