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
mod capabilities;
pub(crate) mod capture;
mod handlers;
mod health;
mod identity;
mod mcp;
mod mcp_tool_schema;
mod mcp_tools;
mod net;
pub(crate) mod picker;
mod project;
mod protocol;
mod runtime;
pub(crate) mod screenshot;
mod server;
mod state;
mod webview_url;
mod workflow;

use bridge::Bridge;
use server::Server;

/// Bridge port shared with the `on_page_load` re-injection hook. Set once when
/// the bridge starts; a single bridge exists per process (the plugin has a
/// `links` key, so only one instance can be registered).
static BRIDGE_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
static BRIDGE_HANDLE: std::sync::OnceLock<Bridge> = std::sync::OnceLock::new();
/// The `.connector.json` this process wrote, removed on `RunEvent::Exit`.
static PID_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
use state::{DomEntry, EventEntry, IpcEvent, LogEntry, PluginState, RuntimeEntry};

#[doc(hidden)]
pub fn __connector_mcp_tool_definitions() -> serde_json::Value {
    mcp_tools::tool_definitions()
}

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";
const DEFAULT_PORT_RANGE: (u16, u16) = connector_client::ports::DEFAULT_WS_RANGE;
const DEFAULT_MCP_PORT_RANGE: (u16, u16) = connector_client::ports::DEFAULT_MCP_RANGE;

/// One listener's port plan after environment overrides (see
/// [`connector_client::ports`] for the precedence). An invalid range
/// variable is logged and skipped; an invalid exact variable is an error.
fn listener_plan(
    exact_var: &str,
    range_var: &str,
    builder_exact: Option<u16>,
    builder_range: Option<(u16, u16)>,
    default: (u16, u16),
) -> Result<connector_client::ports::PortPlan, String> {
    let (plan, invalid) = connector_client::ports::env_port(
        exact_var,
        range_var,
        builder_exact,
        builder_range,
        default,
    )?;
    if let Some(error) = invalid {
        eprintln!("[connector] ignoring {error}; using {}", plan.describe());
    }
    Ok(plan)
}

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

    let _generation = state
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
            generation: 0,
            explicit: false,
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

/// Bounded diagnostic roundtrip. It has no business-state effect.
#[tauri::command]
fn capture_self_test(nonce: String) -> Result<String, String> {
    if nonce.len() > 128 {
        Err("Capture self-test nonce exceeds limit".into())
    } else {
        Ok(nonce)
    }
}

/// Source labels come from Tauri's invoking WebView, never from page JSON.
#[tauri::command]
async fn push_capture_events(
    app: AppHandle,
    webview: tauri::Webview,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if payload.to_string().len() > 262144 {
        return Err("Capture ingress payload limit exceeded".into());
    }
    let state = app.state::<PluginState>();
    Ok(state
        .capture
        .ingest(webview.label(), payload, &state.workflow))
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushRuntimePayload {
    kind: String,
    level: String,
    message: String,
    timestamp: u64,
    #[serde(default = "default_main")]
    window_id: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[tauri::command]
async fn push_runtime(app: AppHandle, payload: PushRuntimePayload) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state
        .push_runtime(RuntimeEntry {
            kind: payload.kind,
            level: payload.level,
            message: payload.message,
            timestamp: payload.timestamp,
            window_id: payload.window_id,
            data: payload.data,
        })
        .await;
    Ok(())
}

// ============ Plugin Builder ============

/// Plugin builder with configuration options.
pub struct ConnectorBuilder {
    bind_address: String,
    port: Option<u16>,
    port_range: Option<(u16, u16)>,
    mcp_port: Option<u16>,
    mcp_port_range: Option<(u16, u16)>,
    bridge_port: Option<u16>,
    mcp_enabled: bool,
    workflow_token: Option<String>,
    capture_preview_commands: Vec<String>,
    capture_preview_paths: Vec<String>,
    project_name: Option<String>,
}

fn capture_setting(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .take(50)
        .collect()
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
            port: None,
            port_range: None,
            mcp_port: None,
            mcp_port_range: None,
            bridge_port: None,
            mcp_enabled: true,
            workflow_token: std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN").ok(),
            capture_preview_commands: capture_setting("TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS"),
            capture_preview_paths: capture_setting("TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS"),
            project_name: None,
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

    /// Set the inclusive port range for the WebSocket server. Default: 9555-9655.
    /// `TAURI_CONNECTOR_PORT` (exact) or `TAURI_CONNECTOR_PORT_RANGE`
    /// (`START-END`) in the app's environment override it.
    pub fn port_range(self, start: u16, end: u16) -> Self {
        Self {
            port_range: Some((start, end)),
            ..self
        }
    }

    /// Bind the WebSocket server on exactly `port` (0.17.7): if it is busy
    /// the connector does not start, with no fallback to a range. Wins over
    /// `TAURI_CONNECTOR_PORT` and every range.
    pub fn port(self, port: u16) -> Self {
        Self {
            port: Some(port),
            ..self
        }
    }

    /// Bind the embedded MCP server on exactly `port` (0.17.7), like
    /// [`Self::port`]. Wins over `TAURI_CONNECTOR_MCP_PORT`.
    pub fn mcp_port(self, port: u16) -> Self {
        Self {
            mcp_port: Some(port),
            ..self
        }
    }

    /// Bind the internal webview bridge on exactly `port` (0.17.7). Default
    /// range 9300-9400; also `TAURI_CONNECTOR_BRIDGE_PORT` (exact) and
    /// `TAURI_CONNECTOR_BRIDGE_PORT_RANGE`.
    pub fn bridge_port(self, port: u16) -> Self {
        Self {
            bridge_port: Some(port),
            ..self
        }
    }

    /// Name this app's project in `status`, the identity response, the PID
    /// file and MCP `initialize` (0.17.5+). Default: `productName`, then the
    /// app crate's package name, then its directory. Set it when two
    /// checkouts or apps would otherwise look alike; the CLI's `--project`
    /// (and `TAURI_CONNECTOR_PROJECT`) select by it.
    pub fn project_name(self, name: &str) -> Self {
        Self {
            project_name: Some(name.to_string()),
            ..self
        }
    }

    /// Set the inclusive port range for the embedded MCP HTTP server. Default: 9556-9656.
    /// `TAURI_CONNECTOR_MCP_PORT` (exact) or `TAURI_CONNECTOR_MCP_PORT_RANGE` override it.
    pub fn mcp_port_range(self, start: u16, end: u16) -> Self {
        Self {
            mcp_port_range: Some((start, end)),
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

    /// Enable authenticated workflows with a host-owned token (minimum 32 bytes).
    /// Legacy diagnostic tools retain their existing transport contract.
    pub fn workflow_token(mut self, token: impl Into<String>) -> Self {
        self.workflow_token = Some(token.into());
        self
    }

    /// Host allowlist for IPC command names whose redacted result previews may be captured.
    pub fn capture_preview_commands(mut self, commands: Vec<String>) -> Self {
        self.capture_preview_commands = commands;
        self
    }

    /// Host allowlist of JSON paths eligible for IPC preview extraction.
    pub fn capture_preview_paths(mut self, paths: Vec<String>) -> Self {
        self.capture_preview_paths = paths;
        self
    }

    /// Build the plugin.
    pub fn build(self) -> TauriPlugin<Wry> {
        let bind_address = self.bind_address;
        use connector_client::ports;
        // Resolved once at build; an invalid exact port keeps the connector
        // off rather than binding a guess.
        let plans = (|| {
            Ok::<_, String>((
                listener_plan(
                    ports::WS_PORT_VAR,
                    ports::WS_RANGE_VAR,
                    self.port,
                    self.port_range,
                    DEFAULT_PORT_RANGE,
                )?,
                listener_plan(
                    ports::MCP_PORT_VAR,
                    ports::MCP_RANGE_VAR,
                    self.mcp_port,
                    self.mcp_port_range,
                    DEFAULT_MCP_PORT_RANGE,
                )?,
                listener_plan(
                    ports::BRIDGE_PORT_VAR,
                    ports::BRIDGE_RANGE_VAR,
                    self.bridge_port,
                    None,
                    ports::DEFAULT_BRIDGE_RANGE,
                )?,
            ))
        })();
        let mcp_enabled = self.mcp_enabled;
        let workflow_token = self.workflow_token;
        let preview_commands = self.capture_preview_commands;
        let preview_paths = self.capture_preview_paths;
        if let Some(name) = self.project_name.as_deref() {
            project::set_explicit_name(name);
        }

        PluginBuilder::<Wry>::new("connector")
            .js_init_script(format!("{}\n{}", bridge::early_init_script(), runtime::initialization_script()))
            .on_webview_ready(|webview| { identity::window_instance_id(webview.label()); })
            .on_event(|app, event| {
                if let tauri::RunEvent::Exit = event
                    && let Some(path) = PID_FILE.get()
                {
                    remove_own_pid_file(path, std::process::id());
                }
                if let tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } = event {
                    identity::window_destroyed(label);
                    if let Some(bridge)=BRIDGE_HANDLE.get() { bridge.window_destroyed(label); }
                    if let Some(state)=app.try_state::<PluginState>() { state.capture.window_closed(label); state.picker.window_closed(label); }
                }
            })
            .invoke_handler(tauri::generate_handler![
                push_dom,
                push_logs,
                set_pointed_element,
                push_ipc_event,
                push_capture_events,
                capture_self_test,
                push_event,
                push_runtime,
            ])
            .on_page_load(|webview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                    identity::page_navigation(webview.label());
                    if let Some(state)=webview.app_handle().try_state::<PluginState>() {state.capture.window_closed(webview.label());state.picker.page_changed(webview.label());}
                }
                if matches!(payload.event(),tauri::webview::PageLoadEvent::Started) {return;}
                identity::page_loaded(webview.label());
                if !identity::origin_allowed(webview.app_handle(), payload.url()) { return; }
                // Re-inject after each completed document load: the
                // one-shot eval at setup/webview-created does not survive
                // reloads or navigations (e.g. dev-server full reloads), which
                // silently killed the bridge and snapshot engine until app
                // restart. The __CONNECTOR_BRIDGE__ guard makes this idempotent.
                if let Some(port) = BRIDGE_PORT.get() {
                    let label = webview.label().to_string();
                    let _ = webview.eval(bridge::bridge_init_script(*port, &label));
                }
                if matches!(payload.event(),tauri::webview::PageLoadEvent::Finished)
                    && let (Some(bridge),Some(state))=(BRIDGE_HANDLE.get(),webview.app_handle().try_state::<PluginState>()) {
                        let bridge=bridge.clone();let state=(*state).clone();let label=webview.label().to_owned();
                        tauri::async_runtime::spawn(async move {state.capture.page_changed(&label,&state,&bridge).await;});
                }
            })
            .setup(move |app, _api| {
                if bind_address == "0.0.0.0" || bind_address == "::" {
                    eprintln!(
                        "[connector][security] Remote debug exposed on {bind_address}; prefer 127.0.0.1 unless this is intentional"
                    );
                }
                identity::app_instance_id();
                identity::started_at();
                let application_directory = app.path().app_data_dir();
                let workflow_storage_available = application_directory.is_ok();
                let log_dir = application_directory
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join(".tauri-connector");

                let mut plugin_state = match PluginState::new(log_dir.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[connector] Failed to init log dir: {e}, falling back to temp dir");
                        PluginState::new(std::env::temp_dir().join(".tauri-connector"))
                            .expect("temp dir should be writable")
                    }
                };
                // Logging fallback must never create a fresh workflow identity namespace.
                plugin_state.workflow = std::sync::Arc::new(workflow::WorkflowService::new(log_dir.join("workflow")));
                if !workflow_storage_available {plugin_state.workflow.disable_storage();}
                plugin_state.workflow.set_token(workflow_token.clone());
                plugin_state.capture.set_preview_commands(preview_commands);
                plugin_state.capture.set_preview_paths(preview_paths);
                app.manage(plugin_state.clone());

                let handle = app.clone();
                let log_dir_for_pid = log_dir.clone();
                let late_arbiter = plugin_state.workflow.resources.clone();

                tauri::async_runtime::spawn(async move {
                    let (port_plan, mcp_port_plan, bridge_port_plan) = match plans {
                        Ok(plans) => plans,
                        Err(e) => {
                            eprintln!("[connector] Not starting: {e}");
                            return;
                        }
                    };
                    // 1. Start internal bridge (JS <-> plugin via WebSocket)
                    let bridge = match Bridge::start_on(&bridge_port_plan) {
                        Ok(b) => b,
                        Err(e) => {
                            eprintln!("[connector] Failed to start bridge: {e}");
                            return;
                        }
                    };

                    // 1a. Results that arrive after a timeout settle their quarantine.
                    bridge.set_late_settlement_hook(std::sync::Arc::new(move |request_id, outcome| {
                        if late_arbiter.settle_late(request_id, outcome) {
                            println!("[connector] Late result for {request_id} released its resource quarantine");
                        }
                    }));

                    // 1b. Set app handle on bridge for eval fallback
                    bridge.set_app_handle(handle.clone()).await;
                    let _ = BRIDGE_PORT.set(bridge.port());
                    let _ = BRIDGE_HANDLE.set(bridge.clone());

                    // 2. Inject bridge JS into all current webviews
                    for (label, window) in handle.webview_windows() {
                        if !webview_url::current(&window).is_ok_and(|url|identity::origin_allowed(&handle, &url)) { continue; }
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
                            if !webview_url::current(&window).is_ok_and(|url|identity::origin_allowed(&handle_for_event, &url)) { continue; }
                            let script = bridge::bridge_init_script(bridge_port, &label);
                            let _ = window.eval(&script);
                        }
                    });

                    // 4. Shared app handle for both servers
                    let app_handle = std::sync::Arc::new(tokio::sync::Mutex::new(
                        Some(handle.clone()),
                    ));

                    // 5. Bind the external WebSocket server first, so a busy
                    // exact port stops startup before anything else listens.
                    let server = match Server::new(
                        &bind_address,
                        &port_plan,
                        bridge.clone(),
                        plugin_state.clone(),
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("[connector] Failed to create WS server: {e}");
                            return;
                        }
                    };

                    // 5a. Start embedded MCP HTTP server (if enabled)
                    let mut mcp_port_actual: Option<u16> = None;
                    if mcp_enabled {
                        match mcp::start(
                            &bind_address,
                            &mcp_port_plan,
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
                            Err(e) if mcp_port_plan.exact.is_some() => {
                                // An exact port is a contract: stop rather
                                // than run without the MCP the user pinned.
                                eprintln!("[connector][mcp] Not starting: {e}");
                                return;
                            }
                            Err(e) => {
                                eprintln!("[connector][mcp] Failed to start MCP server: {e}");
                            }
                        }
                    }

                    // 6. External WebSocket server (for CLI): bound before MCP.

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
                    if let Some(path) = write_pid_file(
                        ws_port,
                        mcp_port_actual,
                        bridge_port,
                        &config.product_name.clone().unwrap_or_default(),
                        &config.identifier,
                        project::current(&handle),
                        &log_dir_for_pid,
                    ) {
                        let _ = PID_FILE.set(path);
                    }

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

/// Directory for `.connector.json`: the nearest `target` ancestor of the
/// executable (covers `target/debug`, `target/<triple>/release` and `.app`
/// bundles built under `target/`), otherwise `fallback` (the log dir), so the
/// file never lands inside an app bundle or next to an installed binary.
fn pid_file_dir(exe: &std::path::Path, fallback: &std::path::Path) -> std::path::PathBuf {
    exe.ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == "target"))
        .unwrap_or(fallback)
        .to_path_buf()
}

/// Remove the PID file only if it still describes this process; another
/// instance sharing the target dir may have replaced it.
fn remove_own_pid_file(path: &std::path::Path, pid: u32) {
    let owned = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|info| info.get("pid").and_then(serde_json::Value::as_u64))
        == Some(u64::from(pid));
    if owned {
        let _ = std::fs::remove_file(path);
    }
}

/// Write a `.connector.json` PID file (see [`pid_file_dir`]) so scripts can auto-discover ports.
/// Returns the path if successful (for cleanup on exit).
fn write_pid_file(
    ws_port: u16,
    mcp_port: Option<u16>,
    bridge_port: u16,
    app_name: &str,
    app_id: &str,
    project: &project::Project,
    log_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let target_dir = pid_file_dir(&exe, log_dir);
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        eprintln!("[connector] Failed to create PID file directory: {e}");
        return None;
    }

    let pid_path = target_dir.join(".connector.json");
    let pid = std::process::id();

    let info = serde_json::json!({
        "pid": pid,
        "ws_port": ws_port,
        "mcp_port": mcp_port,
        "bridge_port": bridge_port,
        "app_name": app_name,
        "app_id": app_id,
        "app_instance_id": identity::app_instance_id(),
        "project_name": project.name,
        "project_root": project.root.as_ref().map(|root| root.to_string_lossy().into_owned()),
        "cargo_package": project.cargo_package,
        "log_dir": log_dir.to_string_lossy(),
        "exe": exe.to_string_lossy(),
        "started_at": identity::started_at(),
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

#[cfg(test)]
mod pid_file_tests {
    use super::{pid_file_dir, remove_own_pid_file};
    use std::path::Path;

    #[test]
    fn pid_file_goes_to_the_nearest_target_dir() {
        let fallback = Path::new("/data/logs");
        for (exe, expected) in [
            ("/p/src-tauri/target/debug/app", "/p/src-tauri/target"),
            ("/p/target/aarch64-apple-darwin/release/app", "/p/target"),
            (
                "/p/target/release/bundle/macos/App.app/Contents/MacOS/App",
                "/p/target",
            ),
        ] {
            assert_eq!(pid_file_dir(Path::new(exe), fallback), Path::new(expected));
        }
    }

    #[test]
    fn installed_apps_use_the_fallback_dir() {
        let fallback = Path::new("/data/logs");
        for exe in [
            "/Applications/App.app/Contents/MacOS/App",
            "/usr/local/bin/app",
            "/home/me/target-app/app",
        ] {
            assert_eq!(pid_file_dir(Path::new(exe), fallback), fallback);
        }
    }

    #[test]
    fn exit_cleanup_only_removes_our_own_pid_file() {
        let dir = std::env::temp_dir().join(format!("connector-pid-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".connector.json");

        std::fs::write(&path, r#"{"pid": 41}"#).unwrap();
        remove_own_pid_file(&path, 42);
        assert!(path.exists(), "another instance's file is kept");

        std::fs::write(&path, r#"{"pid": 42}"#).unwrap();
        remove_own_pid_file(&path, 42);
        assert!(!path.exists());

        remove_own_pid_file(&path, 42); // missing file is fine
        let _ = std::fs::remove_dir_all(&dir);
    }
}
