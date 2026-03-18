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
//!   windowId: 'main',
//!   html: document.body.innerHTML,
//!   textContent: document.body.innerText,
//!   accessibilityTree: buildA11yTree(), // your helper
//!   structureTree: buildStructureTree(), // your helper
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
use state::{DomEntry, LogEntry, PluginState};

const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0";
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
    accessibility_tree: String,
    #[serde(default)]
    structure_tree: String,
}

fn default_main() -> String {
    "main".to_string()
}

#[tauri::command]
async fn push_dom(
    app: AppHandle,
    payload: PushDomPayload,
) -> Result<(), String> {
    let state = app.state::<PluginState>();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    state
        .push_dom(DomEntry {
            window_id: payload.window_id,
            html: payload.html,
            text_content: payload.text_content,
            accessibility_tree: payload.accessibility_tree,
            structure_tree: payload.structure_tree,
            timestamp,
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
async fn push_logs(
    app: AppHandle,
    payload: PushLogsPayload,
) -> Result<(), String> {
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
async fn set_pointed_element(
    app: AppHandle,
    payload: PointedElementPayload,
) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state.set_pointed_element(payload.element).await;
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

    /// Set the bind address. Default: "0.0.0.0" (all interfaces).
    /// Use "127.0.0.1" for localhost-only access.
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
            ])
            .setup(move |app, _api| {
                let plugin_state = PluginState::default();
                app.manage(plugin_state.clone());

                let handle = app.clone();

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
                    let init_script = bridge::bridge_init_script(bridge.port());
                    for (_label, window) in handle.webview_windows() {
                        if let Err(e) = window.eval(&init_script) {
                            eprintln!("[connector] Failed to inject bridge script: {e}");
                        }
                    }

                    // 3. Auto-inject into future webviews
                    let bridge_port = bridge.port();
                    let handle_for_event = handle.clone();
                    handle.listen("tauri://webview-created", move |_event| {
                        let script = bridge::bridge_init_script(bridge_port);
                        for (_label, window) in handle_for_event.webview_windows() {
                            let _ = window.eval(&script);
                        }
                    });

                    // 4. Shared app handle for both servers
                    let app_handle = std::sync::Arc::new(tokio::sync::Mutex::new(
                        Some(handle.clone()),
                    ));

                    // 5. Start embedded MCP SSE server (if enabled)
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
                            Ok(mcp_port) => {
                                let config = handle.config();
                                println!(
                                    "[connector][mcp] MCP ready for '{}' — url: http://{}:{}/sse",
                                    config.product_name.clone().unwrap_or_default(),
                                    bind_address,
                                    mcp_port,
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

                    let config = handle.config();
                    println!(
                        "[connector] Plugin ready for '{}' ({}) — WS on {}:{}",
                        config.product_name.clone().unwrap_or_default(),
                        config.identifier,
                        bind_address,
                        server.port()
                    );

                    server.set_app_handle(handle);

                    if let Err(e) = server.run(bind_address).await {
                        eprintln!("[connector] Server error: {e}");
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
