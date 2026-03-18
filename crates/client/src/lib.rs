//! Shared WebSocket client for connecting to tauri-plugin-connector.
//!
//! Both the MCP server and CLI use this crate to communicate with the
//! running Tauri app's connector plugin over WebSocket.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const DEFAULT_TIMEOUT_MS: u64 = 35_000;

type _WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct PendingRequest {
    tx: oneshot::Sender<Result<Value, String>>,
}

/// WebSocket client that communicates with tauri-plugin-connector.
pub struct ConnectorClient {
    write_tx: Option<mpsc::UnboundedSender<String>>,
    pending: Arc<Mutex<HashMap<String, PendingRequest>>>,
    _reader_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ConnectorClient {
    pub fn new() -> Self {
        Self {
            write_tx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            _reader_handle: None,
        }
    }

    /// Connect to the plugin's WebSocket server.
    pub async fn connect(&mut self, host: &str, port: u16) -> Result<(), String> {
        self.disconnect().await;

        let url = format!("ws://{host}:{port}");
        let (ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| format!("WebSocket connection failed: {e}"))?;

        let (ws_write, ws_read) = ws.split();

        // Writer task: forwards messages from channel to WebSocket
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<String>();
        let writer_handle = tokio::spawn(async move {
            let mut ws_write = ws_write;
            while let Some(msg) = write_rx.recv().await {
                if ws_write.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        });

        // Reader task: receives messages from WebSocket and resolves pending requests
        let pending = self.pending.clone();
        let reader_handle = tokio::spawn(async move {
            let mut ws_read = ws_read;
            while let Some(Ok(msg)) = ws_read.next().await {
                if let Message::Text(text) = msg {
                    let text: &str = text.as_ref();
                    if let Ok(response) = serde_json::from_str::<Value>(text) {
                        let id = response
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        let mut pending = pending.lock().await;
                        if let Some(req) = pending.remove(&id) {
                            let result = if let Some(error) = response.get("error") {
                                Err(error
                                    .as_str()
                                    .unwrap_or("Unknown error")
                                    .to_string())
                            } else {
                                Ok(response
                                    .get("result")
                                    .cloned()
                                    .unwrap_or(Value::Null))
                            };
                            let _ = req.tx.send(result);
                        }
                    }
                }
            }
            // Connection closed — reject all pending
            let mut pending = pending.lock().await;
            for (_, req) in pending.drain() {
                let _ = req.tx.send(Err("Connection closed".to_string()));
            }
            drop(writer_handle);
        });

        self.write_tx = Some(write_tx);
        self._reader_handle = Some(reader_handle);

        Ok(())
    }

    /// Disconnect from the WebSocket server.
    pub async fn disconnect(&mut self) {
        self.write_tx = None;
        if let Some(handle) = self._reader_handle.take() {
            handle.abort();
        }
        let mut pending = self.pending.lock().await;
        for (_, req) in pending.drain() {
            let _ = req.tx.send(Err("Disconnected".to_string()));
        }
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        self.write_tx.is_some()
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
        let write_tx = self
            .write_tx
            .as_ref()
            .ok_or_else(|| "Not connected".to_string())?;

        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), PendingRequest { tx });
        }

        // Build the message with the id
        let mut msg = match command {
            Value::Object(map) => map,
            _ => return Err("Command must be a JSON object".to_string()),
        };
        msg.insert("id".to_string(), Value::String(id.clone()));

        let json = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
        write_tx
            .send(json)
            .map_err(|_| "Send failed: connection closed".to_string())?;

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                Err("Response channel closed".to_string())
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err("Request timeout".to_string())
            }
        }
    }
}

impl Default for ConnectorClient {
    fn default() -> Self {
        Self::new()
    }
}
