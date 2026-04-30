use serde::{Deserialize, Serialize};

/// Incoming request from MCP server via external WebSocket.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    // --- Session ---
    Ping,

    // --- JavaScript Execution ---
    ExecuteJs {
        script: String,
        #[serde(default = "default_window")]
        window_id: String,
    },
    BridgeStatus,

    // --- Screenshot ---
    Screenshot {
        #[serde(default = "default_format")]
        format: String,
        #[serde(default = "default_quality")]
        quality: u8,
        #[serde(default)]
        max_width: Option<u32>,
        #[serde(default = "default_window")]
        window_id: String,
        #[serde(default)]
        save: Option<bool>,
        #[serde(default)]
        output_dir: Option<String>,
        #[serde(default)]
        name_hint: Option<String>,
        #[serde(default)]
        overwrite: Option<bool>,
        #[serde(default)]
        selector: Option<String>,
    },

    // --- DOM ---
    DomSnapshot {
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        snapshot_type: Option<String>,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        max_depth: Option<u64>,
        #[serde(default)]
        max_elements: Option<u64>,
        #[serde(default)]
        max_tokens: Option<u64>,
        #[serde(default)]
        no_split: Option<bool>,
        #[serde(default)]
        react_enrich: Option<bool>,
        #[serde(default)]
        follow_portals: Option<bool>,
        #[serde(default)]
        shadow_dom: Option<bool>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    GetCachedDom {
        #[serde(default = "default_window")]
        window_id: String,
    },

    // --- Element Operations ---
    FindElement {
        selector: String,
        #[serde(default = "default_strategy")]
        strategy: String,
        #[serde(default)]
        target: Option<String>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    GetStyles {
        selector: String,
        #[serde(default)]
        properties: Option<Vec<String>>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    SelectElement {
        #[serde(default = "default_window")]
        #[allow(dead_code)]
        window_id: String,
    },
    GetPointedElement {
        #[serde(default = "default_window")]
        #[allow(dead_code)]
        window_id: String,
    },

    // --- Interaction ---
    Interact {
        action: String,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default = "default_strategy")]
        strategy: String,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        direction: Option<String>,
        #[serde(default)]
        distance: Option<f64>,
        #[serde(default, alias = "targetSelector")]
        target_selector: Option<String>,
        #[serde(default, alias = "targetX")]
        target_x: Option<f64>,
        #[serde(default, alias = "targetY")]
        target_y: Option<f64>,
        #[serde(default)]
        steps: Option<u32>,
        #[serde(default, alias = "durationMs")]
        duration_ms: Option<u32>,
        #[serde(default, alias = "dragStrategy")]
        drag_strategy: Option<String>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    Keyboard {
        #[serde(default = "default_keyboard_action")]
        action: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        modifiers: Option<Vec<String>>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    WaitFor {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default = "default_strategy")]
        strategy: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default = "default_timeout")]
        timeout: u64,
        #[serde(default = "default_window")]
        window_id: String,
    },

    // --- Window Management ---
    WindowList,
    WindowInfo {
        #[serde(default = "default_window")]
        window_id: String,
    },
    WindowResize {
        #[serde(default = "default_window")]
        window_id: String,
        width: u32,
        height: u32,
    },

    // --- IPC ---
    BackendState,
    #[allow(clippy::enum_variant_names)]
    IpcExecuteCommand {
        command: String,
        #[serde(default)]
        args: Option<serde_json::Value>,
    },
    IpcMonitor {
        action: String,
    },
    IpcGetCaptured {
        #[serde(default)]
        filter: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default = "default_ipc_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<u64>,
    },
    IpcEmitEvent {
        event_name: String,
        #[serde(default)]
        payload: Option<serde_json::Value>,
    },

    // --- Logs ---
    ConsoleLogs {
        #[serde(default = "default_lines")]
        lines: usize,
        #[serde(default)]
        filter: Option<String>,
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default = "default_window")]
        window_id: String,
    },
    ClearLogs {
        #[serde(default = "default_all")]
        source: String,
    },
    ReadLogFile {
        source: String,
        #[serde(default = "default_read_lines")]
        lines: usize,
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        window_id: Option<String>,
    },

    // --- Event Capture ---
    IpcListen {
        action: String,
        #[serde(default)]
        events: Option<Vec<String>>,
    },
    EventGetCaptured {
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default = "default_ipc_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<u64>,
    },

    // --- Runtime Capture ---
    RuntimeGetCaptured {
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        since_mark: Option<String>,
        #[serde(default = "default_ipc_limit")]
        limit: usize,
        #[serde(default)]
        window_id: Option<String>,
    },

    // --- Artifacts ---
    ArtifactList {
        #[serde(default)]
        kind: Option<String>,
        #[serde(default = "default_ipc_limit")]
        limit: usize,
    },
    ArtifactRead {
        artifact: String,
    },
    ArtifactCompare {
        before: String,
        after: String,
        #[serde(default = "default_threshold")]
        threshold: f64,
    },
    ArtifactPrune {
        #[serde(default = "default_artifact_keep")]
        keep: usize,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default = "default_true")]
        delete_files: bool,
    },

    // --- Debug ---
    DebugMark {
        #[serde(default)]
        label: Option<String>,
    },
    DebugSnapshot {
        #[serde(default = "default_window")]
        window_id: String,
        #[serde(default)]
        include_dom: bool,
        #[serde(default)]
        include_screenshot: bool,
        #[serde(default)]
        include_logs: bool,
        #[serde(default)]
        include_ipc: bool,
        #[serde(default)]
        include_events: bool,
        #[serde(default)]
        include_runtime: bool,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        since_mark: Option<String>,
        #[serde(default)]
        max_tokens: Option<u64>,
        #[serde(default)]
        screenshot_name_hint: Option<String>,
    },
    WebviewActAndVerify {
        action: String,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        target_selector: Option<String>,
        #[serde(default)]
        wait_for_selector: Option<String>,
        #[serde(default)]
        wait_for_text: Option<String>,
        #[serde(default = "default_timeout")]
        timeout: u64,
        #[serde(default)]
        verify_dom: bool,
        #[serde(default)]
        verify_screenshot: bool,
        #[serde(default)]
        include_logs: bool,
        #[serde(default)]
        include_ipc: bool,
        #[serde(default)]
        include_runtime: bool,
        #[serde(default = "default_window")]
        window_id: String,
    },

    // --- Search ---
    SearchSnapshot {
        pattern: String,
        #[serde(default = "default_context")]
        context: usize,
        #[serde(default = "default_snapshot_mode")]
        mode: String,
        #[serde(default = "default_window")]
        window_id: String,
    },
}

fn default_window() -> String {
    "main".to_string()
}
fn default_format() -> String {
    "jpeg".to_string()
}
fn default_quality() -> u8 {
    80
}
fn default_strategy() -> String {
    "css".to_string()
}
fn default_keyboard_action() -> String {
    "type".to_string()
}
fn default_timeout() -> u64 {
    5000
}
fn default_lines() -> usize {
    50
}
fn default_ipc_limit() -> usize {
    100
}
fn default_all() -> String {
    "all".to_string()
}
fn default_read_lines() -> usize {
    100
}
fn default_context() -> usize {
    2
}
fn default_snapshot_mode() -> String {
    "ai".to_string()
}
fn default_threshold() -> f64 {
    0.0
}
fn default_artifact_keep() -> usize {
    50
}
fn default_true() -> bool {
    true
}

/// Response sent back to MCP server.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    #[serde(flatten)]
    pub payload: ResponsePayload,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponsePayload {
    Success { result: serde_json::Value },
    Error { error: String },
}

impl Response {
    pub fn success(id: String, result: serde_json::Value) -> Self {
        Self {
            id,
            payload: ResponsePayload::Success { result },
        }
    }

    pub fn error(id: String, error: impl Into<String>) -> Self {
        Self {
            id,
            payload: ResponsePayload::Error {
                error: error.into(),
            },
        }
    }
}

/// Internal bridge message: plugin → webview JS.
#[derive(Debug, Serialize)]
pub struct BridgeCommand {
    pub id: String,
    pub script: String,
}

/// Internal bridge message: webview JS → plugin.
#[derive(Debug, Deserialize)]
pub struct BridgeResult {
    pub id: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// App metadata for backend_state.
#[derive(Debug, Serialize)]
pub struct BackendState {
    pub app: AppInfo,
    pub tauri: TauriInfo,
    pub environment: EnvInfo,
    pub windows: Vec<WindowEntry>,
    pub timestamp: u128,
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub name: String,
    pub identifier: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct TauriInfo {
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct EnvInfo {
    pub debug: bool,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Serialize)]
pub struct WindowEntry {
    pub label: String,
    pub title: String,
    pub visible: bool,
    pub focused: bool,
}
