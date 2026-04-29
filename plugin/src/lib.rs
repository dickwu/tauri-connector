//! tauri-plugin-connector
//!
//! MCP-compatible Tauri plugin with reliable JS execution.
//!
//! **Key difference from `tauri-plugin-mcp-bridge`**: Does NOT rely on
//! `window.__TAURI__` for JS execution results. Uses an internal WebSocket
//! bridge instead. Additionally provides Tauri IPC commands so the frontend
//! can proactively push DOM snapshots for faster, more LLM-friendly access.
//!
//! ## Frontend Integration (optional, enhances DOM access)
//!
//! ```typescript
//! import { invoke } from '@tauri-apps/api/core';
//!
//! // Push current DOM to the plugin for LLM consumption
//! await invoke('plugin:connector|push_dom', {
//!   payload: {
//!     windowId: 'main',
//!     html: document.body.innerHTML.substring(0, 500000),
//!     textContent: document.body.innerText.substring(0, 200000),
//!     snapshot: snapshotResult.snapshot,
//!     snapshotMode: 'ai',
//!     refs: JSON.stringify(snapshotResult.refs),
//!     meta: JSON.stringify(snapshotResult.meta),
//!   }
//! });
//! ```

use serde::Deserialize;
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::{AppHandle, Listener, Manager, Wry};

mod bridge;
mod handlers;
mod mcp;
mod mcp_tools;
mod protocol;
mod server;
mod state;

use bridge::Bridge;
use server::Server;
use state::{DomEntry, EventEntry, IpcEvent, LogEntry, PluginState};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";
const DEFAULT_PORT_RANGE: (u16, u16) = (9555, 9655);
const DEFAULT_MCP_PORT_RANGE: (u16, u16) = (9556, 9656);

// ============ Tauri IPC Commands ============
// These are callable from the frontend via `invoke('plugin:connector|...')`

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushDomPayload {
    #[serde(default = "default_main")]
    window_id: String,
    html: String,
    #[serde(default)]
    text_content: String,
    #[serde(default)]
    snapshot: String,
    #[serde(default)]
    snapshot_mode: String,
    #[serde(default)]
    refs: String,
    #[serde(default)]
    meta: String,
}

fn default_main() -> String {
    "main".to_string()
}

#[tauri::command]
async fn push_dom(app: AppHandle, payload: PushDomPayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let refs: state::RefMap =
        serde_json::from_str(&payload.refs).map_err(|e| format!("Invalid refs JSON: {e}"))?;
    let meta: state::SnapshotMeta = serde_json::from_str(&payload.meta).unwrap_or_default();

    state
        .push_dom(DomEntry {
            window_id: payload.window_id,
            html: payload.html,
            text_content: payload.text_content,
            snapshot: payload.snapshot,
            snapshot_mode: payload.snapshot_mode,
            refs,
            meta,
            timestamp,
            search_text: String::new(),
            snapshot_id: None,
        })
        .await;

    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushLogsPayload {
    entries: Vec<LogEntryPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogEntryPayload {
    level: String,
    message: String,
    timestamp: u64,
    #[serde(default = "default_main")]
    window_id: String,
}

#[tauri::command]
async fn push_logs(app: AppHandle, payload: PushLogsPayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    let entries: Vec<LogEntry> = payload
        .entries
        .into_iter()
        .map(|e| LogEntry {
            level: e.level,
            message: e.message,
            timestamp: e.timestamp,
            window_id: e.window_id,
        })
        .collect();

    state.push_logs(entries).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointedElementPayload {
    element: serde_json::Value,
}

#[tauri::command]
async fn set_pointed_element(app: AppHandle, payload: PointedElementPayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state.set_pointed_element(payload.element).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushIpcEventPayload {
    command: String,
    #[serde(default)]
    args: serde_json::Value,
    timestamp: u64,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

#[tauri::command]
async fn push_ipc_event(app: AppHandle, payload: PushIpcEventPayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state
        .push_ipc_event(IpcEvent {
            command: payload.command,
            args: payload.args,
            timestamp: payload.timestamp,
            duration_ms: payload.duration_ms,
            error: payload.error,
        })
        .await;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushEventPayload {
    event: String,
    #[serde(default)]
    payload: serde_json::Value,
    timestamp: u64,
    #[serde(default = "default_main")]
    window_id: String,
}

#[tauri::command]
async fn push_event(app: AppHandle, payload: PushEventPayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state
        .push_event(EventEntry {
            event: payload.event,
            payload: payload.payload,
            timestamp: payload.timestamp,
            window_id: payload.window_id,
        })
        .await;
    Ok(())
}

// ============ Plugin Builder ============

/// Plugin builder with configuration options.
pub struct ConnectorBuilder {
    bind_address: String,
    port_range: (u16, u16),
    mcp_port_range: (u16, u16),
    mcp_enabled: bool,
}

impl Default for ConnectorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectorBuilder {
    pub fn new() -> Self {
        Self {
            bind_address: DEFAULT_BIND_ADDRESS.to_string(),
            port_range: DEFAULT_PORT_RANGE,
            mcp_port_range: DEFAULT_MCP_PORT_RANGE,
            mcp_enabled: true,
        }
    }

    /// Set the bind address. Default: "127.0.0.1" (localhost only).
    /// Passing "0.0.0.0" exposes the debug connector to the network.
    pub fn bind_address(self, addr: &str) -> Self {
        Self {
            bind_address: addr.to_string(),
            ..self
        }
    }

    /// Set the port range for the WebSocket server. Default: 9555-9655.
    pub fn port_range(self, start: u16, end: u16) -> Self {
        Self {
            port_range: (start, end),
            ..self
        }
    }

    /// Set the port range for the embedded MCP SSE server. Default: 9556-9656.
    pub fn mcp_port_range(self, start: u16, end: u16) -> Self {
        Self {
            mcp_port_range: (start, end),
            ..self
        }
    }

    /// Disable the embedded MCP server. Default: enabled.
    pub fn disable_mcp(self) -> Self {
        Self {
            mcp_enabled: false,
            ..self
        }
    }

    /// Build the plugin.
    pub fn build(self) -> TauriPlugin<Wry> {
        let bind_address = self.bind_address;
        let port_range = self.port_range;
        let mcp_port_range = self.mcp_port_range;
        let mcp_enabled = self.mcp_enabled;

        PluginBuilder::<Wry>::new("connector")
            .invoke_handler(tauri::generate_handler![
                push_dom,
                push_logs,
                set_pointed_element,
                push_ipc_event,
                push_event,
            ])
            .setup(move |app, _api| {
                if bind_address == "0.0.0.0" || bind_address == "::" {
                    eprintln!(
                        "[connector][security] Remote debug exposed on {bind_address}; prefer 127.0.0.1 unless this is intentional"
                    );
                }
                let log_dir = app.path().app_data_dir()
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join(".tauri-connector");

                let plugin_state = match PluginState::new(log_dir.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[connector] Failed to init log dir: {e}, falling back to temp dir");
                        PluginState::new(std::env::temp_dir().join(".tauri-connector"))
                            .expect("temp dir should be writable")
                    }
                };
                app.manage(plugin_state.clone());

                let handle = app.clone();
                let log_dir_for_pid = log_dir.clone();

                tauri::async_runtime::spawn(async move {
                    // 1. Start internal bridge (JS <-> plugin via WebSocket)
                    let bridge = match Bridge::start() {
                        Ok(b) => b,
                        Err(e) => {
                            eprintln!("[connector] Failed to start bridge: {e}");
                            return;
                        }
                    };

                    // 1b. Set app handle on bridge for eval fallback
                    bridge.set_app_handle(handle.clone()).await;

                    // 2. Inject bridge JS into all current webviews
                    for (label, window) in handle.webview_windows() {
                        let init_script = bridge::bridge_init_script(bridge.port(), &label);
                        if let Err(e) = window.eval(&init_script) {
                            eprintln!("[connector] Failed to inject bridge script: {e}");
                        }
                    }

                    // 3. Auto-inject into future webviews
                    let bridge_port = bridge.port();
                    let handle_for_event = handle.clone();
                    handle.listen("tauri://webview-created", move |_event| {
                        for (label, window) in handle_for_event.webview_windows() {
                            let script = bridge::bridge_init_script(bridge_port, &label);
                            let _ = window.eval(&script);
                        }
                    });

                    // 4. Shared app handle for both servers
                    let app_handle = std::sync::Arc::new(tokio::sync::Mutex::new(
                        Some(handle.clone()),
                    ));

                    // 5. Start embedded MCP SSE server (if enabled)
                    let mut mcp_port_actual: Option<u16> = None;
                    if mcp_enabled {
                        match mcp::start(
                            &bind_address,
                            mcp_port_range,
                            bridge.clone(),
                            plugin_state.clone(),
                            app_handle.clone(),
                        )
                        .await
                        {
                            Ok(port) => {
                                mcp_port_actual = Some(port);
                                let config = handle.config();
                                println!(
                                    "[connector][mcp] MCP ready for '{}' — url: http://{}:{}/mcp (/sse legacy)",
                                    config.product_name.clone().unwrap_or_default(),
                                    bind_address,
                                    port,
                                );
                            }
                            Err(e) => {
                                eprintln!("[connector][mcp] Failed to start MCP server: {e}");
                            }
                        }
                    }

                    // 6. Start external WebSocket server (for CLI)
                    let server =
                        match Server::new(&bind_address, port_range, bridge, plugin_state) {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("[connector] Failed to create WS server: {e}");
                                return;
                            }
                        };

                    let ws_port = server.port();
                    let config = handle.config();
                    println!(
                        "[connector] Plugin ready for '{}' ({}) — WS on {}:{}",
                        config.product_name.clone().unwrap_or_default(),
                        config.identifier,
                        bind_address,
                        ws_port,
                    );

                    // 7. Write PID file so bun scripts can auto-discover ports
                    let pid_file = write_pid_file(
                        ws_port,
                        mcp_port_actual,
                        bridge_port,
                        &config.product_name.clone().unwrap_or_default(),
                        &config.identifier,
                        &log_dir_for_pid,
                    );

                    server.set_app_handle(handle);

                    if let Err(e) = server.run(bind_address).await {
                        eprintln!("[connector] Server error: {e}");
                    }

                    // Clean up PID file on exit
                    if let Some(path) = pid_file {
                        let _ = std::fs::remove_file(&path);
                    }
                });

                Ok(())
            })
            .build()
    }
}

/// Initialize the plugin with default settings.
pub fn init() -> TauriPlugin<Wry> {
    ConnectorBuilder::new().build()
}

/// Write a `.connector.json` PID file to `target/` so bun scripts can auto-discover ports.
/// Returns the path if successful (for cleanup on exit).
fn write_pid_file(
    ws_port: u16,
    mcp_port: Option<u16>,
    bridge_port: u16,
    app_name: &str,
    app_id: &str,
    log_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // exe = .../target/debug/app-name  ->  target/ is 2 levels up
    let target_dir = exe.parent()?.parent()?;

    let pid_path = target_dir.join(".connector.json");
    let pid = std::process::id();

    let info = serde_json::json!({
        "pid": pid,
        "ws_port": ws_port,
        "mcp_port": mcp_port,
        "bridge_port": bridge_port,
        "app_name": app_name,
        "app_id": app_id,
        "log_dir": log_dir.to_string_lossy(),
        "exe": exe.to_string_lossy(),
        "started_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "pid_file": pid_path.to_string_lossy(),
    });

    let tmp_path = pid_path.with_extension("json.tmp");
    match std::fs::write(&tmp_path, serde_json::to_string_pretty(&info).ok()?)
        .and_then(|_| std::fs::rename(&tmp_path, &pid_path))
    {
        Ok(()) => {
            println!("[connector] PID file: {}", pid_path.display());
            Some(pid_path)
        }
        Err(e) => {
            eprintln!("[connector] Failed to write PID file: {e}");
            None
        }
    }
}
