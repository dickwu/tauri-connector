//! Shared WebSocket client for connecting to tauri-plugin-connector.
//!
//! Both the MCP server and CLI use this crate to communicate with the
//! running Tauri app's connector plugin over WebSocket.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub mod batch;
pub mod discovery;
pub mod identity;
pub mod inspection;
pub mod outcome;
pub mod workflow;

const DEFAULT_TIMEOUT_MS: u64 = 35_000;

type _WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct PendingRequest {
    tx: oneshot::Sender<Result<Value, String>>,
}

type PendingMap = Arc<Mutex<HashMap<String, PendingRequest>>>;

/// Removing a local waiter never means the remote operation was cancelled.
struct PendingGuard {
    pending: PendingMap,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

fn reject_pending(pending: &PendingMap, reason: &str) {
    for (id, req) in pending.lock().unwrap().drain() {
        let _ = req.tx.send(Err(format!(
            "outcome_unknown: {reason} (requestId: {id}); remote execution may continue"
        )));
    }
}

/// WebSocket client that communicates with tauri-plugin-connector.
pub struct ConnectorClient {
    write_tx: Option<mpsc::UnboundedSender<String>>,
    pending: PendingMap,
    _reader_handle: Option<tokio::task::JoinHandle<()>>,
    expected_instance: Mutex<Option<String>>,
}

impl ConnectorClient {
    pub fn new() -> Self {
        Self {
            write_tx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            _reader_handle: None,
            expected_instance: Mutex::new(None),
        }
    }

    /// Connect to the plugin's WebSocket server.
    pub async fn connect(&mut self, host: &str, port: u16) -> Result<(), String> {
        self.disconnect().await;
        // An old I/O task finishing concurrently must not reject requests made
        // on the replacement connection.
        self.pending = Arc::new(Mutex::new(HashMap::new()));

        let url = format!("ws://{host}:{port}");
        let (ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| format!("WebSocket connection failed: {e}"))?;

        let (ws_write, ws_read) = ws.split();

        // One task owns both halves: exiting either direction drops the other,
        // closes the queue, and releases all waiters. No detached writer remains.
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<String>();
        let pending = self.pending.clone();
        let reader_handle = tokio::spawn(async move {
            let mut ws_write = ws_write;
            let mut ws_read = ws_read;
            loop {
                tokio::select! {
                    outbound = write_rx.recv() => {
                        match outbound {
                            Some(msg) => {
                                if ws_write.send(Message::Text(msg.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    inbound = ws_read.next() => {
                        match inbound {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(response) = serde_json::from_str::<Value>(&text) {
                                    let id = response.get("id").and_then(Value::as_str).unwrap_or("");
                                    if let Some(req) = pending.lock().unwrap().remove(id) {
                                        let result = if let Some(error) = response.get("error") {
                                            if let Some(outcome) = response.get("outcome") {
                                                Err(serde_json::json!({ "error": error, "outcome": outcome }).to_string())
                                            } else {
                                                Err(error.as_str().unwrap_or("Unknown error").to_string())
                                            }
                                        } else {
                                            Ok(response.get("result").cloned().unwrap_or(Value::Null))
                                        };
                                        let _ = req.tx.send(result);
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                            Some(Ok(_)) => {}
                        }
                    }
                }
            }
            write_rx.close();
            reject_pending(&pending, "Connection closed");
        });

        self.write_tx = Some(write_tx);
        self._reader_handle = Some(reader_handle);
        let pinned = self.expected_instance.lock().unwrap().is_some();
        if pinned {
            let token = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN").ok();
            if let Err(error) = self.verify_app_identity(token.as_deref()).await {
                self.disconnect().await;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Pin the application expected by discovery or a previously returned handle.
    /// Reconnecting does not clear this binding and cannot silently switch applications.
    pub fn bind_instance(&self, instance: &str) -> Result<(), String> {
        let mut expected = self.expected_instance.lock().unwrap();
        if expected.as_deref().is_some_and(|old| old != instance) {
            return Err("app_identity_mismatch: client is bound to another app instance".into());
        }
        *expected = Some(instance.to_owned());
        Ok(())
    }

    pub async fn verify_app_identity(
        &self,
        auth_token: Option<&str>,
    ) -> Result<identity::AppIdentity, String> {
        let mut args = serde_json::json!({});
        if let Some(token) = auth_token {
            args["authToken"] = serde_json::json!(token);
        }
        let identity = identity::AppIdentity::parse(
            self.send_with_timeout(
                serde_json::json!({"type":"inspection","operation":"app_identity","args":args}),
                2000,
            )
            .await?,
        )?;
        self.bind_instance(&identity.app_instance_id)?;
        Ok(identity)
    }

    /// Forward to the application-owned inspection service after capability and identity checks.
    /// A failed response is never retried and no arbitrary JS fallback exists.
    pub async fn inspect(&self, operation: &str, arguments: &Value) -> Result<Value, String> {
        let mut args = arguments.clone();
        if !args.is_object() {
            return Err("invalid_arguments: arguments must be an object".into());
        }
        if args.get("authToken").is_none() {
            if let Ok(token) = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN") {
                args["authToken"] = serde_json::json!(token);
            }
        }
        for field in [
            "pickerId",
            "captureSessionId",
            "artifactId",
            "artifact",
            "before",
            "after",
            "baselineId",
            "currentId",
        ] {
            if let Some(instance) = args
                .get(field)
                .and_then(Value::as_str)
                .and_then(identity::instance_from_handle)
            {
                self.bind_instance(instance)?;
            }
        }
        if operation == "webview_select_element" {
            let request = inspection::PickerRequest::parse(&args).map_err(|e| e.to_string())?;
            if request.action == "start" && request.request_key.is_none() {
                args["requestKey"] = serde_json::json!(inspection::new_request_key());
            }
        }
        let capabilities = self
            .send_with_timeout(serde_json::json!({"type":"bridge_status"}), 2000)
            .await?;
        if capabilities
            .get("inspectionProtocolVersion")
            .and_then(Value::as_u64)
            != Some(inspection::INSPECTION_PROTOCOL_VERSION)
        {
            return Err("capability_unavailable: connected app does not support inspection protocol v1; upgrade the plugin".into());
        }
        let identity = self
            .verify_app_identity(args.get("authToken").and_then(Value::as_str))
            .await?;
        if operation == "app_identity" {
            return Ok(serde_json::json!(identity));
        }
        let wait_ms = args
            .get("waitMs")
            .and_then(Value::as_u64)
            .unwrap_or(10000)
            .min(10000);
        let transport_timeout = if operation == "webview_screenshot" {
            args.get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(10000)
                .min(30000)
                + 5000
        } else {
            wait_ms + 15000
        };
        let recovery_key = args.get("requestKey").cloned();
        self.send_with_timeout(
            serde_json::json!({"type":"inspection","operation":operation,"args":args}),
            transport_timeout,
        )
        .await
        .map_err(|error| {
            if operation != "webview_select_element" {
                return error;
            }
            if let Some(key) = recovery_key {
                let mut report = serde_json::from_str::<Value>(&error)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| serde_json::json!({"error":error}));
                report["requestKey"] = key;
                report.to_string()
            } else {
                error
            }
        })
    }

    /// Disconnect from the WebSocket server.
    pub async fn disconnect(&mut self) {
        self.write_tx = None;
        if let Some(handle) = self._reader_handle.take() {
            handle.abort();
        }
        reject_pending(&self.pending, "Disconnected");
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        self.write_tx.as_ref().is_some_and(|tx| !tx.is_closed())
    }

    /// Send a command and wait for a response.
    pub async fn send(&self, command: Value) -> Result<Value, String> {
        self.send_with_timeout(command, DEFAULT_TIMEOUT_MS).await
    }

    /// Send a command with a custom timeout.
    pub async fn send_with_timeout(
        &self,
        command: Value,
        timeout_ms: u64,
    ) -> Result<Value, String> {
        // Validate and serialize before registering any request state.
        let mut msg = match command {
            Value::Object(map) => map,
            _ => return Err("Command must be a JSON object".to_string()),
        };
        let write_tx = self
            .write_tx
            .as_ref()
            .ok_or_else(|| "Not connected".to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        msg.insert("id".to_string(), Value::String(id.clone()));
        let json = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(id.clone(), PendingRequest { tx });
        let _waiter = PendingGuard {
            pending: self.pending.clone(),
            id: id.clone(),
        };
        write_tx
            .send(json)
            .map_err(|_| "not_dispatched: Send failed: connection closed".to_string())?;

        // Once queued, a timeout or connection loss cannot prove non-execution.
        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("outcome_unknown: Response channel closed (requestId: {id}); remote execution may continue")),
            Err(_) => Err(format!("outcome_unknown: Request timeout (requestId: {id}); remote execution may continue")),
        }
    }
}

impl Drop for ConnectorClient {
    fn drop(&mut self) {
        self.write_tx = None;
        if let Some(handle) = self._reader_handle.take() {
            handle.abort();
        }
        reject_pending(&self.pending, "Client dropped");
    }
}

impl Default for ConnectorClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use serde_json::json;

    fn queued_client() -> (ConnectorClient, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut client = ConnectorClient::new();
        client.write_tx = Some(tx);
        (client, rx)
    }

    #[tokio::test]
    async fn invalid_command_does_not_register_a_waiter() {
        let (client, _rx) = queued_client();
        assert!(client.send(Value::Null).await.is_err());
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_enqueue_removes_waiter() {
        let (client, rx) = queued_client();
        drop(rx);
        assert!(client.send(json!({"command": "test"})).await.is_err());
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropped_send_future_removes_waiter_without_remote_cancellation() {
        let (client, mut rx) = queued_client();
        let client = Arc::new(client);
        let cloned = client.clone();
        let task = tokio::spawn(async move { cloned.send(json!({"command": "test"})).await });
        let dispatched = rx.recv().await.unwrap();
        assert!(serde_json::from_str::<Value>(&dispatched).unwrap()["id"].is_string());
        assert_eq!(client.pending.lock().unwrap().len(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(
            rx.try_recv().is_err(),
            "dropping the waiter must not send another command"
        );
    }

    #[tokio::test]
    async fn timeout_retains_unknown_remote_outcome_and_request_id() {
        let (client, mut rx) = queued_client();
        let error = client
            .send_with_timeout(json!({"command": "test"}), 1)
            .await
            .unwrap_err();
        let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert!(error.contains("outcome_unknown"), "{error}");
        assert!(error.contains(sent["id"].as_str().unwrap()), "{error}");
        assert!(client.pending.lock().unwrap().is_empty());
    }

    async fn fixture_client(
        response: Option<Value>,
    ) -> (ConnectorClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let command = socket.next().await.unwrap().unwrap();
            let command: Value = serde_json::from_str(command.to_text().unwrap()).unwrap();
            if let Some(mut response) = response {
                response["id"] = command["id"].clone();
                socket
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .unwrap();
            }
            socket.close(None).await.unwrap();
        });
        let mut client = ConnectorClient::new();
        client.connect("127.0.0.1", port).await.unwrap();
        (client, server)
    }

    #[tokio::test]
    async fn closed_socket_cleans_pending_and_preserves_unknown_remote_state() {
        let (client, server) = fixture_client(None).await;
        let error = client.send(json!({"command":"write"})).await.unwrap_err();
        assert!(error.contains("outcome_unknown"));
        assert!(error.contains("requestId:"));
        assert!(client.pending.lock().unwrap().is_empty());
        server.await.unwrap();
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn protocol_error_retains_the_explicit_outcome_envelope() {
        let outcome =
            json!({"execution":"completed", "verification":"failed", "effect":"possible"});
        let (client, server) = fixture_client(Some(
            json!({"error":"Postcondition failed", "outcome":outcome}),
        ))
        .await;
        let error = client.send(json!({"command":"write"})).await.unwrap_err();
        let envelope: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(envelope["outcome"], outcome);
        assert_eq!(envelope["error"], "Postcondition failed");
        assert!(client.pending.lock().unwrap().is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn business_error_fields_remain_successful_data() {
        let data = json!({"error":"user content", "found":false, "ok":false});
        let (client, server) = fixture_client(Some(json!({"result":data}))).await;
        assert_eq!(client.send(json!({"command":"read"})).await.unwrap(), data);
        assert!(client.pending.lock().unwrap().is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn explicit_disconnect_releases_every_waiter() {
        let (mut client, _rx) = queued_client();
        let (tx, result) = oneshot::channel();
        client
            .pending
            .lock()
            .unwrap()
            .insert("queued-id".into(), PendingRequest { tx });
        client.disconnect().await;
        assert!(!client.is_connected());
        let error = result.await.unwrap().unwrap_err();
        assert!(error.contains("outcome_unknown"));
        assert!(error.contains("queued-id"));
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropping_client_closes_socket_without_detached_writer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            match tokio::time::timeout(Duration::from_secs(1), socket.next()).await {
                Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => {}
                other => panic!("client drop must close the connection: {other:?}"),
            }
        });
        let mut client = ConnectorClient::new();
        client.connect("127.0.0.1", port).await.unwrap();
        drop(client);
        server.await.unwrap();
    }
}
