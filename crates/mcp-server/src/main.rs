//! MCP stdio server for tauri-plugin-connector.
//!
//! Reads JSON-RPC 2.0 requests from stdin, dispatches them to the
//! Tauri app via WebSocket, and writes responses to stdout.
//!
//! The app is discovered lazily on the first tool call, so the server starts
//! (and answers `initialize`/`tools/list`) before the app is running. Tool
//! calls run concurrently and can be stopped with `notifications/cancelled`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use connector_client::discovery::{self, ConnectionOptions};
use connector_client::ConnectorClient;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio::task::{AbortHandle, JoinSet};

use connector_mcp_server::protocol::{self, JsonRpcRequest, JsonRpcResponse};
use connector_mcp_server::tools;

const PROTOCOL_LATEST: &str = "2025-11-25";
const PROTOCOL_SUPPORTED: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Used for `driver_session` defaults when discovery finds nothing.
const FALLBACK_TARGET: (&str, u16) = ("127.0.0.1", 9555);

#[tokio::main]
async fn main() {
    let server = Arc::new(Server::new());
    let in_flight = InFlight::default();
    let mut tasks = JoinSet::new();

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err()
                || stdout.write_all(b"\n").await.is_err()
                || stdout.flush().await.is_err()
            {
                break;
            }
        }
    });

    eprintln!("[tauri-connector-mcp] Server started on stdio (app discovered on first tool call)");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        while tasks.try_join_next().is_some() {}

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {e}"));
                send_response(&out_tx, &resp);
                continue;
            }
        };

        // Notifications (no id) don't get a response
        let Some(id) = request.id.clone() else {
            handle_notification(&request, &in_flight);
            continue;
        };

        if request.method != "tools/call" {
            // Cheap and order-sensitive (initialize sets the protocol), so
            // answer these inline.
            let response = server.handle_request(id, &request).await;
            send_response(&out_tx, &response);
            continue;
        }

        let key = request_key(&id);
        let server = server.clone();
        let out_tx = out_tx.clone();
        let tracker = in_flight.clone();
        let task_key = key.clone();
        // Hold the task until it is registered, so `finish` always finds its
        // entry unless the request was cancelled.
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        let abort = tasks.spawn(async move {
            let _ = ready_rx.await;
            let response = server.handle_request(id, &request).await;
            if tracker.finish(&task_key) {
                send_response(&out_tx, &response);
            }
        });
        in_flight.start(key, abort);
        let _ = ready_tx.send(());
    }

    // stdin closed: let in-flight calls finish, then flush their output.
    while tasks.join_next().await.is_some() {}
    drop(out_tx);
    let _ = writer.await;
}

fn send_response(out: &mpsc::UnboundedSender<String>, response: &JsonRpcResponse) {
    let _ = out.send(serde_json::to_string(response).unwrap_or_default());
}

fn handle_notification(request: &JsonRpcRequest, in_flight: &InFlight) {
    match request.method.as_str() {
        "notifications/initialized" => {
            eprintln!("[tauri-connector-mcp] Client initialized");
        }
        "notifications/cancelled" => {
            let cancelled = request
                .params
                .as_ref()
                .and_then(|params| params.get("requestId"))
                .is_some_and(|id| in_flight.cancel(&request_key(id)));
            eprintln!("[tauri-connector-mcp] Request cancelled (in flight: {cancelled})");
        }
        method => {
            eprintln!("[tauri-connector-mcp] Unknown notification: {method}");
        }
    }
}

fn request_key(id: &Value) -> String {
    id.to_string()
}

/// Abort handles for running `tools/call` requests, keyed by JSON-RPC id.
#[derive(Clone, Default)]
struct InFlight(Arc<StdMutex<HashMap<String, AbortHandle>>>);

impl InFlight {
    fn start(&self, key: String, handle: AbortHandle) {
        self.0.lock().unwrap().insert(key, handle);
    }

    /// Mark a request done. Returns false if it was cancelled, in which case
    /// no response may be sent (the abort can land after the last await).
    fn finish(&self, key: &str) -> bool {
        self.0.lock().unwrap().remove(key).is_some()
    }

    fn cancel(&self, key: &str) -> bool {
        match self.0.lock().unwrap().remove(key) {
            Some(handle) => {
                handle.abort();
                true
            }
            None => false,
        }
    }
}

/// Connection state shared by all requests.
struct Server {
    client: RwLock<ConnectorClient>,
    /// Discovered (or last used) app endpoint; `None` until discovery succeeds.
    target: Mutex<Option<(String, u16)>>,
    negotiated_protocol: StdMutex<Option<String>>,
}

impl Server {
    fn new() -> Self {
        Self {
            client: RwLock::new(ConnectorClient::new()),
            target: Mutex::new(None),
            negotiated_protocol: StdMutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_target(host: &str, port: u16) -> Self {
        let server = Self::new();
        *server.target.try_lock().unwrap() = Some((host.to_owned(), port));
        server
    }

    async fn handle_request(&self, id: Value, request: &JsonRpcRequest) -> JsonRpcResponse {
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
                *self.negotiated_protocol.lock().unwrap() = Some(requested.to_owned());
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

            "tools/list" => JsonRpcResponse::success(id, tools::tool_definitions()),

            "tools/call" => {
                let params = request.params.as_ref().cloned().unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = self.call_tool(tool_name, &arguments).await;
                let negotiated = self.negotiated_protocol.lock().unwrap().clone();
                JsonRpcResponse::success(
                    id,
                    connector_client::inspection::shape_mcp_result(result, negotiated.as_deref()),
                )
            }

            "ping" => JsonRpcResponse::success(id, json!({})),

            _ => {
                JsonRpcResponse::error(id, -32601, format!("Method not found: {}", request.method))
            }
        }
    }

    async fn call_tool(&self, name: &str, arguments: &Value) -> Value {
        if name == "driver_session" {
            let mut client = self.client.write().await;
            let (host, port) = match self.cached_target().await {
                Some(target) => target,
                None => match self.discover(&client).await {
                    Ok(target) => target,
                    Err(_) => (FALLBACK_TARGET.0.to_owned(), FALLBACK_TARGET.1),
                },
            };
            return tools::call_tool(&mut client, &host, port, name, arguments).await;
        }

        let (host, port) = match self.ensure_connected().await {
            Ok(target) => target,
            Err(e) => {
                let mut content = protocol::text_content(&json!({
                    "error": format!("Auto-connect failed: {e}. Start the app or use driver_session to connect manually.")
                }));
                content["isError"] = json!(true);
                return content;
            }
        };
        let client = self.client.read().await;
        tools::call_shared_tool(&client, &host, port, name, arguments).await
    }

    async fn cached_target(&self) -> Option<(String, u16)> {
        self.target.lock().await.clone()
    }

    /// Connect if needed, discovering (or re-discovering) the app on demand.
    async fn ensure_connected(&self) -> Result<(String, u16), String> {
        if self.client.read().await.is_connected() {
            if let Some(target) = self.cached_target().await {
                return Ok(target);
            }
        }

        let mut client = self.client.write().await;
        let cached = self.cached_target().await;
        if client.is_connected() {
            if let Some(target) = cached {
                return Ok(target);
            }
        }
        if let Some((host, port)) = cached {
            if client.connect(&host, port).await.is_ok() {
                return Ok((host, port));
            }
        }
        // Nothing cached, or the app moved (e.g. restarted on another port).
        let (host, port) = self.discover(&client).await?;
        client.connect(&host, port).await?;
        Ok((host, port))
    }

    async fn discover(&self, client: &ConnectorClient) -> Result<(String, u16), String> {
        let resolved = discovery::resolve_connection(ConnectionOptions::from_current_dir())
            .await
            .map_err(|e| format!("discovery failed: {e}"))?;
        if let Some(identity) = &resolved.identity {
            client.bind_instance(&identity.app_instance_id)?;
        }
        let target = (resolved.host, resolved.port);
        *self.target.lock().await = Some(target.clone());
        eprintln!(
            "[tauri-connector-mcp] Discovered app at {}:{}",
            target.0, target.1
        );
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn request(value: Value) -> JsonRpcRequest {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn standalone_uses_initialized_protocol_to_shape_tool_results() {
        for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
            let server = Server::with_target("127.0.0.1", 9555);
            let init = request(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version}}),
            );
            let response = server.handle_request(json!(1), &init).await;
            assert_eq!(response.result.unwrap()["protocolVersion"], version);
            let call = request(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"driver_session","arguments":{"action":"start","port":-1}}}),
            );
            let result = server.handle_request(json!(2), &call).await.result.unwrap();
            assert_eq!(result["isError"], true);
            assert_eq!(result["content"][0]["type"], "text");
            assert_eq!(
                result.get("structuredContent").is_some(),
                version != "2025-03-26"
            );
        }
    }

    #[tokio::test]
    async fn initialize_and_tools_list_do_not_need_the_app() {
        // No target and no discovery: these must answer without touching the app.
        let server = Server::new();
        let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        let response = server.handle_request(json!(1), &init).await;
        assert_eq!(response.result.unwrap()["protocolVersion"], PROTOCOL_LATEST);
        let list = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let tools = server.handle_request(json!(2), &list).await.result.unwrap();
        assert!(tools["tools"].as_array().is_some_and(|t| !t.is_empty()));
        assert!(server.cached_target().await.is_none());
    }

    #[tokio::test]
    async fn cancelling_aborts_an_in_flight_request() {
        let in_flight = InFlight::default();
        let mut tasks = JoinSet::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<&str>();
        let tracker = in_flight.clone();
        let abort = tasks.spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if tracker.finish("7") {
                let _ = tx.send("late response");
            }
        });
        in_flight.start(request_key(&json!(7)), abort);

        let cancel = request(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}),
        );
        handle_notification(&cancel, &in_flight);
        let joined = tasks.join_next().await.unwrap();
        assert!(joined.unwrap_err().is_cancelled());
        assert!(
            rx.recv().await.is_none(),
            "cancelled request must not reply"
        );
        assert!(!in_flight.cancel("7"), "entry is removed after cancel");
    }

    #[tokio::test]
    async fn finished_requests_still_reply_and_unregister() {
        let in_flight = InFlight::default();
        let mut tasks = JoinSet::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<&str>();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let tracker = in_flight.clone();
        let abort = tasks.spawn(async move {
            let _ = go_rx.await;
            if tracker.finish("\"a\"") {
                let _ = tx.send("response");
            }
        });
        in_flight.start(request_key(&json!("a")), abort);
        go_tx.send(()).unwrap();
        tasks.join_next().await.unwrap().unwrap();
        assert_eq!(rx.recv().await, Some("response"));
        assert!(!in_flight.cancel("\"a\""));
    }
}
