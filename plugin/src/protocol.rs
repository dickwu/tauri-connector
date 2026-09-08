use serde::{Deserialize, Serialize};

/// Incoming request from MCP server via external WebSocket.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    // --- Session ---
    Ping,

    /// Workflow lifecycle operations execute only in the application service.
    Workflow {
        operation: String,
        #[serde(default)]
        args: serde_json::Value,
    },

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
        #[serde(default)]
        annotate: Option<bool>,
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
        #[serde(default)]
        url: Option<String>,
        #[serde(default, alias = "loadState")]
        load_state: Option<String>,
        #[serde(default, alias = "fn", alias = "condition")]
        function: Option<String>,
        #[serde(default)]
        state: Option<String>,
        #[serde(default = "default_timeout")]
        timeout: u64,
        #[serde(default = "default_window")]
        window_id: String,
    },
    Locator {
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        alt: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default, alias = "testId", alias = "testid")]
        test_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        exact: Option<bool>,
        #[serde(default)]
        first: Option<bool>,
        #[serde(default)]
        last: Option<bool>,
        #[serde(default)]
        nth: Option<usize>,
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        value: Option<String>,
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
        #[serde(default = "default_window", alias = "windowId")]
        window_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<connector_client::outcome::ExecutionOutcome>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponsePayload {
    Success { result: serde_json::Value },
    Error { error: String },
}

impl Response {
    pub fn rejected(id: String, error: &serde_json::Value) -> Self {
        let mut error_details = connector_client::outcome::WorkflowError::new(
            error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("execution_failed"),
            error
                .get("stage")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("preparing"),
            error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Operation was rejected before dispatch"),
        );
        error_details.retryable_before_dispatch = error
            .get("retryableBeforeDispatch")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Self::outcome_error(
            id,
            connector_client::outcome::ExecutionOutcome::not_dispatched(error_details),
        )
    }

    pub fn requires_quarantine(&self) -> bool {
        use connector_client::outcome::{EffectStatus, ExecutionStatus};
        self.outcome.as_ref().is_some_and(|outcome| {
            (outcome.execution == ExecutionStatus::OutcomeUnknown
                && outcome.effect != EffectStatus::None)
                || (outcome.execution == ExecutionStatus::Failed
                    && outcome.effect == EffectStatus::Possible)
        })
    }

    pub fn not_dispatched(id: String, code: &str, message: impl Into<String>) -> Self {
        let outcome = connector_client::outcome::ExecutionOutcome::not_dispatched(
            connector_client::outcome::WorkflowError::new(code, "preparing", message),
        );
        Self::outcome_error(id, outcome)
    }

    /// Keep typed execution facts intact; classify only registered legacy tool contracts.
    pub fn into_outcome(self, tool: &str) -> connector_client::outcome::ExecutionOutcome {
        use connector_client::outcome::{ExecutionOutcome, WorkflowError, legacy_outcome};
        if let Some(outcome) = self.outcome {
            return outcome;
        }
        match self.payload {
            ResponsePayload::Success { result } => legacy_outcome(tool, result),
            // A legacy handler error carries no dispatch acknowledgement. It cannot
            // justify releasing a possibly active write's lease or replaying it.
            ResponsePayload::Error { error } => ExecutionOutcome::unknown(WorkflowError::new(
                "outcome_unknown",
                "dispatching",
                error,
            )),
        }
    }

    pub fn success(id: String, result: serde_json::Value) -> Self {
        Self {
            id,
            payload: ResponsePayload::Success { result },
            outcome: None,
        }
    }

    pub fn error(id: String, error: impl Into<String>) -> Self {
        Self {
            id,
            payload: ResponsePayload::Error {
                error: error.into(),
            },
            outcome: None,
        }
    }

    pub fn outcome_error(id: String, outcome: connector_client::outcome::ExecutionOutcome) -> Self {
        let error = outcome
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "Tool execution or verification failed".into());
        Self {
            id,
            payload: ResponsePayload::Error { error },
            outcome: Some(outcome),
        }
    }

    pub fn with_outcome(id: String, outcome: connector_client::outcome::ExecutionOutcome) -> Self {
        if !outcome.is_success(false) {
            return Self::outcome_error(id, outcome);
        }
        Self {
            id,
            payload: ResponsePayload::Success {
                result: outcome.data.clone(),
            },
            outcome: Some(outcome),
        }
    }
}

#[cfg(test)]
mod workflow_protocol_tests {
    use super::*;
    use connector_client::outcome::{EffectStatus, ExecutionOutcome, WorkflowError};
    use serde_json::json;

    #[test]
    fn workflow_preserves_camel_case_service_arguments() {
        let request: Request = serde_json::from_value(json!({
            "id":"request-1", "type":"workflow", "operation":"workflow_resume",
            "args":{"runId":"run-1","expectedRevision":7,"checkpointId":"checkpoint-1","intent":"reconcile"}
        })).unwrap();
        let Command::Workflow { operation, args } = request.command else {
            panic!("wrong command");
        };
        assert_eq!(operation, "workflow_resume");
        assert_eq!(args["expectedRevision"], 7);
        assert_eq!(args["checkpointId"], "checkpoint-1");
    }

    #[test]
    fn response_failure_preserves_typed_outcome_and_legacy_error() {
        let mut outcome = ExecutionOutcome::failed(
            WorkflowError::new("condition_timeout", "verifying", "condition did not match"),
            EffectStatus::None,
        );
        outcome.data = json!({"found":false,"timeout":true});
        let wire =
            serde_json::to_value(Response::outcome_error("request-1".into(), outcome)).unwrap();
        assert_eq!(wire["error"], "condition did not match");
        assert_eq!(wire["outcome"]["error"]["code"], "condition_timeout");
        assert_eq!(wire["outcome"]["data"]["timeout"], true);
        assert!(wire.get("result").is_none());
    }

    #[test]
    fn rejected_requests_preserve_machine_code_and_quarantine_tracks_possible_effects() {
        let rejected = Response::rejected(
            "test".into(),
            &json!({"code":"persistence_unavailable","stage":"acquiring","message":"Storage unavailable"}),
        );
        assert_eq!(
            rejected
                .outcome
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .code,
            "persistence_unavailable"
        );
        assert!(!rejected.requires_quarantine());
        let failed = ExecutionOutcome::failed(
            WorkflowError::new("execution_failed", "dispatching", "partial effect"),
            EffectStatus::Possible,
        );
        assert!(Response::outcome_error("test".into(), failed).requires_quarantine());
        let mut read = ExecutionOutcome::unknown(WorkflowError::new(
            "outcome_unknown",
            "observing",
            "lost read",
        ));
        read.effect = EffectStatus::None;
        assert!(!Response::outcome_error("test".into(), read).requires_quarantine());
    }

    #[test]
    fn ipc_monitor_accepts_legacy_default_and_explicit_window() {
        for (args, expected) in [
            (json!({"type":"ipc_monitor","action":"start"}), "main"),
            (
                json!({"type":"ipc_monitor","action":"start","windowId":"secondary"}),
                "secondary",
            ),
        ] {
            let Command::IpcMonitor { window_id, .. } = serde_json::from_value(args).unwrap()
            else {
                panic!("wrong command");
            };
            assert_eq!(window_id, expected);
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
