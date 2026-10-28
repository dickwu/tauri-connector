//! CLI-only formatting and clap adapters; all session state belongs to the application.
use clap::{Args, Subcommand};
use connector_client::ConnectorClient;
use serde_json::{json, Value};

#[derive(Debug, Args)]
pub struct PickerStart {
    /// Idempotency key; generated when omitted. Never replay with a different key after an uncertain response.
    #[arg(long)]
    pub request_key: Option<String>,
    #[arg(long,default_value_t=60000,value_parser=clap::value_parser!(u64).range(5000..=120000))]
    pub timeout_ms: u64,
    #[arg(long,value_parser=clap::value_parser!(u64).range(0..=10000))]
    pub wait_ms: Option<u64>,
    /// Skip screenshot capture entirely.
    #[arg(long)]
    pub no_screenshot: bool,
    #[arg(long,value_parser=["auto","webview_native","window_native","dom_rendering"],conflicts_with="no_screenshot")]
    pub screenshot_source: Option<String>,
}
#[derive(Debug, Subcommand)]
pub enum PickerCommand {
    /// Start active selection. Returns one app-owned handle; exit 2 means still waiting.
    Start(PickerStart),
    /// Query a retained handle; this never selects or captures again.
    Get {
        picker_id: String,
        #[arg(long,default_value_t=0,value_parser=clap::value_parser!(u64).range(0..=10000))]
        wait_ms: u64,
    },
    /// Cancel idempotently and report cleanup; selection already committed is retained.
    Cancel { picker_id: String },
}
impl PickerCommand {
    pub fn handle(&self) -> Option<&str> {
        match self {
            Self::Get { picker_id, .. } | Self::Cancel { picker_id } => Some(picker_id),
            _ => None,
        }
    }
}
impl PickerStart {
    pub fn args(self, window: &str, convenience: bool) -> Value {
        let request_key = self
            .request_key
            .unwrap_or_else(connector_client::inspection::new_request_key);
        let mut args = json!({"windowId":window,"requestKey":request_key,"timeoutMs":self.timeout_ms,"captureScreenshot":!self.no_screenshot});
        if !convenience {
            args["action"] = json!("start");
        }
        if let Some(wait) = self.wait_ms {
            args["waitMs"] = json!(wait);
        }
        if let Some(source) = self.screenshot_source {
            args["screenshotSource"] = json!(source);
        }
        args
    }
}
pub async fn picker(client: &ConnectorClient, args: Value) -> Result<(), String> {
    let request =
        connector_client::inspection::PickerRequest::parse(&args).map_err(|e| e.to_string())?;
    if let Some(key) = &request.request_key {
        eprintln!("requestKey: {key}; recover this request explicitly after a lost response");
    }
    let report = match client.inspect("webview_select_element", &args).await {
        Ok(report) => report,
        Err(error) => {
            let report = serde_json::from_str::<Value>(&error)
                .unwrap_or_else(|_| json!({"status":"failed","error":error}));
            print_json(&report);
            return Err(error);
        }
    };
    if let Some(id) = report.get("pickerId").and_then(Value::as_str) {
        eprintln!("pickerId: {id}");
    }
    let code = connector_client::inspection::picker_exit_code(&report, request.action == "cancel");
    print_json(&report);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
pub async fn lifecycle(
    client: &ConnectorClient,
    command: PickerCommand,
    window: &str,
) -> Result<(), String> {
    let args = match command {
        PickerCommand::Start(start) => start.args(window, false),
        PickerCommand::Get { picker_id, wait_ms } => {
            json!({"action":"get","pickerId":picker_id,"waitMs":wait_ms})
        }
        PickerCommand::Cancel { picker_id } => json!({"action":"cancel","pickerId":picker_id}),
    };
    picker(client, args).await
}

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    /// Start a bounded invoke capture, sharing a single page hook with other sessions.
    Start {
        #[arg(long,default_value="metadata",value_parser=["metadata","preview"])]
        result_policy: String,
        #[arg(long,default_value="metadata",value_parser=["metadata","preview"])]
        argument_policy: String,
        #[arg(long)]
        follow_pages: bool,
        #[arg(long, value_delimiter = ',')]
        commands: Vec<String>,
    },
    Status {
        capture_session_id: String,
    },
    Stop {
        capture_session_id: String,
    },
}
#[derive(Debug, Args)]
pub struct Query {
    pub capture_session_id: String,
    #[arg(long)]
    /// Opaque nextCursor returned by the previous IPC query; pass it unchanged.
    pub cursor: Option<String>,
    #[arg(long)]
    pub invocation_id: Option<String>,
    #[arg(long,value_parser=["started","succeeded","failed"])]
    pub phase: Option<String>,
    #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u64).range(1..=500))]
    pub limit: u64,
    #[arg(long,default_value_t=32768,value_parser=clap::value_parser!(u64).range(1024..=65536))]
    pub max_bytes: u64,
}
pub async fn capture(
    client: &ConnectorClient,
    command: CaptureCommand,
    window: &str,
) -> Result<(), String> {
    let args = match command {
        CaptureCommand::Start {
            result_policy,
            argument_policy,
            follow_pages,
            commands,
        } => {
            let mut options = json!({"resultPolicy":result_policy,"argumentPolicy":argument_policy,"followPages":follow_pages});
            if !commands.is_empty() {
                options["commands"] = json!(commands);
            }
            json!({"action":"start","windowId":window,"options":options})
        }
        CaptureCommand::Status { capture_session_id } => {
            json!({"action":"status","captureSessionId":capture_session_id})
        }
        CaptureCommand::Stop { capture_session_id } => {
            json!({"action":"stop","captureSessionId":capture_session_id})
        }
    };
    call(client, "ipc_capture", &args).await
}
pub async fn query(client: &ConnectorClient, query: Query) -> Result<(), String> {
    let mut args = json!({"captureSessionId":query.capture_session_id,"limit":query.limit,"maxBytes":query.max_bytes});
    if let Some(v) = query.cursor {
        args["cursor"] = json!(v);
    }
    if let Some(v) = query.invocation_id {
        args["invocationId"] = json!(v);
    }
    if let Some(v) = query.phase {
        args["phase"] = json!(v);
    }
    call(client, "ipc_query", &args).await
}
pub async fn call(client: &ConnectorClient, operation: &str, args: &Value) -> Result<(), String> {
    let report = client.inspect(operation, args).await?;
    print_json(&report);
    Ok(())
}
/// `identity`: the plugin's `app_identity`, with an explicit note when the
/// plugin predates `pluginVersion`/`appName` (0.17.5).
pub async fn identity(client: &ConnectorClient) -> Result<(), String> {
    let report = client.inspect("app_identity", &json!({})).await?;
    print_json(&annotate_identity(report));
    Ok(())
}

pub fn annotate_identity(mut report: Value) -> Value {
    if let Some(object) = report.as_object_mut() {
        if !object.contains_key("pluginVersion") {
            object.insert("pluginVersion".into(), Value::Null);
            object.insert(
                "pluginVersionNote".into(),
                json!("this tauri-plugin-connector predates 0.17.5 and does not report its version, app name, or app version; upgrade the plugin to see them"),
            );
        }
    }
    report
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into())
    );
}

#[derive(Debug, Default, Args)]
pub struct ScreenshotOptions {
    /// Explicit protected screenshot source. Only auto permits fallback.
    #[arg(long,value_parser=["auto","webview_native","window_native","dom_rendering"])]
    pub source: Option<String>,
    /// Strict structured locator JSON; conflicts with --selector.
    #[arg(long, conflicts_with = "selector")]
    pub target: Option<String>,
    #[arg(long,value_parser=["required"])]
    pub redaction: Option<String>,
    /// Requires host permission; passive host policy rejects this preparation request.
    #[arg(long)]
    pub allow_window_preparation: bool,
    /// Unminimize, show and focus the window before a protected capture.
    /// A hidden page renders no frames, so capture otherwise fails with
    /// window_not_visible. Off by default: capture never moves windows.
    #[arg(long)]
    pub bring_to_front: bool,
}
impl ScreenshotOptions {
    pub fn is_rich(&self) -> bool {
        self.source.is_some()
            || self.target.is_some()
            || self.redaction.is_some()
            || self.allow_window_preparation
            || self.bring_to_front
    }
    pub fn add_args(&self, args: &mut Value) -> Result<(), String> {
        if let Some(source) = &self.source {
            args["source"] = json!(source);
        }
        if let Some(target) = &self.target {
            args["target"] = serde_json::from_str(target)
                .map_err(|_| "invalid_arguments: --target must be structured locator JSON")?;
        }
        if let Some(redaction) = &self.redaction {
            args["redaction"] = json!(redaction);
        }
        args["allowWindowPreparation"] = json!(self.allow_window_preparation);
        if self.bring_to_front {
            args["bringToFront"] = json!(true);
        }
        Ok(())
    }
}
fn write_export(path: &str, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("refusing screenshot export through symlink".into());
        }
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    // Default umask permissions: a screenshot is a debug artifact the user
    // opens in other tools, not a secret (redaction already ran).
    let mut file = options.open(path).map_err(|e| export_error(path, &e))?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|e| format!("capture succeeded but explicit export failed: {e}"))
}

fn export_error(path: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        format!("capture succeeded but explicit export failed: {path} already exists; pass --overwrite to replace it")
    } else {
        format!("capture succeeded but explicit export failed: {error}")
    }
}

pub async fn screenshot(
    client: &ConnectorClient,
    args: Value,
    output: Option<&str>,
    overwrite: bool,
    resolved: Option<&connector_client::discovery::ResolvedConnection>,
) -> Result<(), String> {
    let mut args = args;
    args["includeImage"] = json!(output.is_some());
    let report = crate::commands::capture_screenshot(client, &args, resolved).await?;
    let existed = output.is_some_and(|path| std::path::Path::new(path).exists());
    let bytes = match output {
        Some(path) => {
            let encoded = report
                .get("base64")
                .and_then(Value::as_str)
                .ok_or("captured image unavailable for explicit export")?;
            let bytes = crate::commands::b64_decode(encoded)?;
            write_export(path, &bytes, overwrite)?;
            Some(bytes)
        }
        None => None,
    };
    print_json(&crate::commands::source_screenshot_output(
        report,
        output,
        bytes.as_deref(),
        existed && overwrite,
    ));
    Ok(())
}

#[cfg(test)]
mod cursor_contract_tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct QueryCli {
        #[command(flatten)]
        query: Query,
    }

    #[test]
    fn an_existing_export_path_names_the_overwrite_flag() {
        let exists = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
        let message = export_error("/tmp/shot.png", &exists);
        assert!(message.contains("/tmp/shot.png already exists"));
        assert!(message.contains("pass --overwrite"));
        let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!export_error("/tmp/shot.png", &other).contains("--overwrite"));
    }

    #[cfg(unix)]
    #[test]
    fn screenshot_exports_are_readable_like_other_user_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("connector-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let _ = std::fs::remove_file(&path);
        let path = path.to_str().unwrap().to_owned();
        write_export(&path, b"png", false).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_ne!(mode, 0o600, "exports follow the umask, not owner-only");
        let refused = write_export(&path, b"png", false).unwrap_err();
        assert!(refused.contains("pass --overwrite"));
        write_export(&path, b"png2", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"png2");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ipc_query_cli_accepts_opaque_next_cursor_without_numeric_coercion() {
        // Same process/session/sequence form returned by CaptureStore::query.
        let response = json!({"captureSessionId":"app-instance:capture:session-id","nextCursor":"store-namespace.app-instance:capture:session-id.17"});
        let parsed = QueryCli::try_parse_from([
            "ipc-query",
            response["captureSessionId"].as_str().unwrap(),
            "--cursor",
            response["nextCursor"].as_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(json!(parsed.query.cursor), response["nextCursor"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_notes_an_older_plugin_without_a_version() {
        let old = annotate_identity(json!({"appId":"com.example","pid":1}));
        assert_eq!(old["pluginVersion"], Value::Null);
        assert!(old["pluginVersionNote"]
            .as_str()
            .unwrap()
            .contains("predates 0.17.5"));
        let current = annotate_identity(json!({"appId":"com.example","pluginVersion":"0.17.5"}));
        assert_eq!(current["pluginVersion"], "0.17.5");
        assert!(current.get("pluginVersionNote").is_none());
    }
}
