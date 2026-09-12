//! MCP tool definitions and dispatch for tauri-connector.

use connector_client::ConnectorClient;
use serde_json::{json, Value};

use crate::protocol::text_content;

// Vendored byte-for-byte from plugin/src/mcp_tool_schema.rs (the canonical copy) so the
// published crate is self-contained; the vendored_schema_matches_plugin_source test enforces it.
#[path = "mcp_tool_schema.rs"]
mod embedded_mcp_tool_schema;

pub use embedded_mcp_tool_schema::server_instructions;

/// Return the list of all tool definitions for `tools/list`.
///
/// Composed from the shared schema in `embedded_mcp_tool_schema` plus the
/// standalone-only `driver_session` tool, so the common tools have a single
/// source of truth.
pub fn tool_definitions() -> Value {
    let mut defs = embedded_mcp_tool_schema::tool_definitions();
    if let Some(tools) = defs["tools"].as_array_mut() {
        tools.push(json!({
            "name": "driver_session",
            "description": "Start/stop connection to a running Tauri app",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop", "status"] },
                    "host": { "type": "string" },
                    "port": { "type": "integer", "minimum":1, "maximum":65535 },
                    "appInstanceId":{"type":"string"}
                },
                "required": ["action"]
            }
        }));
    }
    defs
}

/// Dispatch a tool call to the appropriate handler.
pub async fn call_tool(
    client: &mut ConnectorClient,
    host: &str,
    port: u16,
    name: &str,
    args: &Value,
) -> Value {
    let result = match name {
        "driver_session" => handle_driver_session(client, host, port, args).await,
        "batch_actions" => handle_batch_actions(client, host, port, args).await,
        _ => dispatch_tool(client, host, port, name, args).await,
    };

    match result {
        Ok(data) => {
            if name == "webview_select_element" {
                return connector_client::inspection::picker_mcp_content(
                    data,
                    args.get("includeImage")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                );
            }
            if name == "webview_screenshot"
                || name == "artifact_read"
                    && connector_client::inspection::protected_artifact_request(name, args)
            {
                let protected = data.get("storage").and_then(Value::as_str)
                    == Some("protected_memory")
                    || connector_client::inspection::protected_artifact_request(name, args);
                return connector_client::inspection::image_mcp_content(data, protected);
            }
            let mut content = text_content(&data);
            if name.starts_with("workflow_")
                && name != "workflow_capabilities"
                && workflow_result_exit_code(&data) == 1
            {
                content["isError"] = json!(true);
            }
            if name == "batch_actions" && data.get("ok") == Some(&json!(false)) {
                content["isError"] = json!(true);
            }
            if data.is_object() {
                content["structuredContent"] = data;
            }
            content
        }
        Err(e) => {
            let data = serde_json::from_str::<Value>(&e).unwrap_or_else(|_| json!({ "error": e }));
            let mut content = text_content(&data);
            content["isError"] = json!(true);
            if data.is_object() {
                content["structuredContent"] = data;
            }
            content
        }
    }
}

/// Dispatch a single non-session tool call and return its raw result.
///
/// Shared by `call_tool`, `batch_actions`, and the CLI `batch` command; only
/// `driver_session` (which mutates the connection) and `batch_actions` itself
/// live outside this table.
pub async fn dispatch_tool(
    client: &ConnectorClient,
    host: &str,
    port: u16,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    if connector_client::inspection::protected_artifact_request(name, args) {
        return client
            .inspect(
                name,
                &connector_client::inspection::protected_artifact_args(args)
                    .map_err(|e| e.to_string())?,
            )
            .await;
    }
    match name {
        "app_identity" | "runtime_health" | "ipc_capture" | "ipc_query" => {
            client.inspect(name, args).await
        }
        "workflow_run"
        | "workflow_get"
        | "workflow_cancel"
        | "workflow_resume"
        | "workflow_capabilities" => handle_workflow(client, name, args).await,
        "driver_session" => Err("driver_session is not allowed inside batch_actions".to_string()),
        "batch_actions" => Err("batch_actions cannot be nested".to_string()),
        "webview_execute_js" => handle_execute_js(client, args).await,
        "bridge_status" => handle_bridge_status(client).await,
        "webview_screenshot" => handle_screenshot(client, args).await,
        "webview_dom_snapshot" => handle_dom_snapshot(client, args).await,
        "get_cached_dom" => handle_cached_dom(client, args).await,
        "webview_find_element" => handle_find_element(client, args).await,
        "webview_get_styles" => handle_get_styles(client, args).await,
        "webview_interact" => handle_interact(client, args).await,
        "webview_keyboard" => handle_keyboard(client, args).await,
        "webview_wait_for" => handle_wait_for(client, args).await,
        "webview_locator" => handle_locator(client, args).await,
        "webview_get_pointed_element" => handle_get_pointed_element(client, args).await,
        "webview_select_element" => handle_select_element(client, args).await,
        "manage_window" => handle_manage_window(client, args).await,
        "ipc_get_backend_state" => handle_backend_state(client).await,
        "ipc_execute_command" => handle_ipc_execute_command(client, args).await,
        "ipc_monitor" => handle_ipc_monitor(client, args).await,
        "ipc_get_captured" => handle_ipc_get_captured(client, args).await,
        "ipc_emit_event" => handle_ipc_emit_event(client, args).await,
        "read_logs" => handle_read_logs(client, args).await,
        "clear_logs" => handle_clear_logs(client, args).await,
        "read_log_file" => handle_read_log_file(client, args).await,
        "ipc_listen" => handle_ipc_listen(client, args).await,
        "event_get_captured" => handle_event_get_captured(client, args).await,
        "runtime_get_captured" => handle_runtime_get_captured(client, args).await,
        "runtime_clear" => handle_clear_logs_source(client, "runtime").await,
        "artifact_list" => handle_artifact_list(client, args).await,
        "artifact_read" => handle_artifact_read(client, args).await,
        "artifact_compare" => handle_artifact_compare(client, args).await,
        "artifact_prune" => handle_artifact_prune(client, args).await,
        "debug_mark" => handle_debug_mark(client, args).await,
        "debug_snapshot" => handle_debug_snapshot(client, args).await,
        "webview_act_and_verify" => handle_act_and_verify(client, args).await,
        "webview_search_snapshot" => handle_search_snapshot(client, args).await,
        "get_setup_instructions" => Ok(json!(SETUP_INSTRUCTIONS)),
        "list_devices" => handle_list_devices(host, port).await,
        _ => Err(format!("Unknown tool: {name}")),
    }
}

/// Classify only the workflow report contract, never arbitrary business JSON.
/// 0 = completed, 1 = known failure, 2 = running/paused/unknown.
pub fn workflow_result_exit_code(report: &Value) -> i32 {
    embedded_mcp_tool_schema::workflow_report_exit_code(report)
}

async fn handle_workflow(
    client: &ConnectorClient,
    operation: &str,
    args: &Value,
) -> Result<Value, String> {
    let bridge = handle_bridge_status(client).await?;
    if bridge
        .get("workflowProtocolVersion")
        .and_then(Value::as_u64)
        != Some(1)
    {
        return Err("capability_unavailable: connected app does not support workflow protocol v1; upgrade the plugin".into());
    }
    let mut args = args.clone();
    if !args.is_object() {
        return Err("invalid_spec: workflow arguments must be an object".into());
    }
    if operation != "workflow_capabilities" && args.get("authToken").is_none() {
        if let Ok(token) = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN") {
            args["authToken"] = json!(token);
        }
    }
    if operation != "workflow_capabilities"
        && bridge
            .get("inspectionProtocolVersion")
            .and_then(Value::as_u64)
            == Some(1)
    {
        client
            .verify_app_identity(args.get("authToken").and_then(Value::as_str))
            .await?;
    }
    let timeout = args
        .get("waitMs")
        .and_then(Value::as_u64)
        .unwrap_or(1000)
        .min(30_000)
        + 5000;
    client.send_with_timeout(json!({ "type": "workflow", "operation": operation, "args": args }), timeout).await
        .map_err(|error| if error.contains("unknown variant") && error.contains("workflow") {
            "capability_unavailable: connected app does not support workflow; upgrade the plugin".into()
        } else { error })
}

async fn handle_batch_actions(
    client: &ConnectorClient,
    host: &str,
    port: u16,
    args: &Value,
) -> Result<Value, String> {
    let report = connector_client::batch::run_from_value(args, |tool, targs| async move {
        if tool.starts_with("workflow_") {
            return Err("workflow lifecycle operations cannot be nested in batch_actions".into());
        }
        dispatch_tool(client, host, port, &tool, &targs).await
    })
    .await?;
    serde_json::to_value(&report).map_err(|e| format!("Failed to serialize batch report: {e}"))
}

// ─── Helpers ───

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn num_arg(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn window_id(args: &Value) -> String {
    str_arg(args, "windowId").unwrap_or_else(|| "main".to_string())
}

// ─── Tool Handlers ───

async fn handle_driver_session(
    client: &mut ConnectorClient,
    host: &str,
    port: u16,
    args: &Value,
) -> Result<Value, String> {
    let action = str_arg(args, "action").unwrap_or_default();
    let h = str_arg(args, "host").unwrap_or_else(|| host.to_string());
    let p = args
        .get("port")
        .map(|p| {
            p.as_u64()
                .filter(|p| (1..=65535).contains(p))
                .map(|p| p as u16)
                .ok_or("invalid_arguments: port must be an integer in 1..65535")
        })
        .transpose()?
        .unwrap_or(port);

    match action.as_str() {
        "start" => {
            if let Some(expected) = args.get("appInstanceId") {
                client.bind_instance(
                    expected
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or("invalid_arguments: appInstanceId must be nonempty")?,
                )?;
            }
            client.connect(&h, p).await?;
            if args.get("appInstanceId").is_some() {
                let token = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN").ok();
                if let Err(error) = client.verify_app_identity(token.as_deref()).await {
                    client.disconnect().await;
                    return Err(error);
                }
            }
            Ok(json!(format!("Connected to {h}:{p}")))
        }
        "stop" => {
            client.disconnect().await;
            Ok(json!("Disconnected"))
        }
        "status" => {
            let status = if client.is_connected() {
                format!("Connected to {host}:{port}")
            } else {
                "Not connected".to_string()
            };
            Ok(json!(status))
        }
        _ => Err(format!("Unknown action: {action}")),
    }
}

async fn handle_execute_js(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let script = str_arg(args, "script").ok_or("Missing 'script' parameter")?;
    let wid = window_id(args);
    client
        .send(json!({ "type": "execute_js", "script": script, "window_id": wid }))
        .await
}

async fn handle_bridge_status(client: &ConnectorClient) -> Result<Value, String> {
    client.send(json!({ "type": "bridge_status" })).await
}

async fn handle_screenshot(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    if connector_client::inspection::is_rich_screenshot(args) {
        return client.inspect("webview_screenshot", args).await;
    }
    let format = str_arg(args, "format").unwrap_or_else(|| "jpeg".to_string());
    let quality = num_arg(args, "quality").unwrap_or(80.0) as u8;
    let max_width = num_arg(args, "maxWidth").map(|n| n as u32);
    let wid = window_id(args);

    let mut cmd = json!({
        "type": "screenshot",
        "format": format,
        "quality": quality,
        "window_id": wid,
    });
    if let Some(mw) = max_width {
        cmd["max_width"] = json!(mw);
    }
    if let Some(save) = args.get("save").and_then(|v| v.as_bool()) {
        cmd["save"] = json!(save);
    }
    if let Some(output_dir) = str_arg(args, "outputDir") {
        cmd["output_dir"] = json!(output_dir);
    }
    if let Some(name_hint) = str_arg(args, "nameHint") {
        cmd["name_hint"] = json!(name_hint);
    }
    if let Some(overwrite) = args.get("overwrite").and_then(|v| v.as_bool()) {
        cmd["overwrite"] = json!(overwrite);
    }
    if let Some(annotate) = args.get("annotate").and_then(|v| v.as_bool()) {
        cmd["annotate"] = json!(annotate);
    }
    if let Some(selector) = str_arg(args, "selector") {
        cmd["selector"] = json!(selector);
    }
    client.send(cmd).await
}

async fn handle_dom_snapshot(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let mode = args
        .get("mode")
        .or_else(|| args.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("ai");
    let wid = window_id(args);

    let mut cmd = json!({
        "type": "dom_snapshot",
        "mode": mode,
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "selector") {
        cmd["selector"] = json!(s);
    }
    if let Some(v) = num_arg(args, "maxDepth") {
        cmd["max_depth"] = json!(v as u64);
    }
    if let Some(v) = num_arg(args, "maxElements") {
        cmd["max_elements"] = json!(v as u64);
    }
    if let Some(v) = args.get("reactEnrich").and_then(|v| v.as_bool()) {
        cmd["react_enrich"] = json!(v);
    }
    if let Some(v) = args.get("followPortals").and_then(|v| v.as_bool()) {
        cmd["follow_portals"] = json!(v);
    }
    if let Some(v) = args.get("shadowDom").and_then(|v| v.as_bool()) {
        cmd["shadow_dom"] = json!(v);
    }
    if let Some(v) = num_arg(args, "maxTokens") {
        cmd["max_tokens"] = json!(v as u64);
    }
    if let Some(v) = args.get("noSplit").and_then(|v| v.as_bool()) {
        cmd["no_split"] = json!(v);
    }
    client.send(cmd).await
}

async fn handle_cached_dom(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    client
        .send(json!({ "type": "get_cached_dom", "window_id": wid }))
        .await
}

async fn handle_find_element(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let selector = str_arg(args, "selector").ok_or("Missing 'selector' parameter")?;
    let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "find_element",
        "selector": selector,
        "strategy": strategy,
        "window_id": wid,
    });
    if let Some(t) = str_arg(args, "target") {
        cmd["target"] = json!(t);
    }
    client.send(cmd).await
}

async fn handle_get_styles(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let selector = str_arg(args, "selector").ok_or("Missing 'selector' parameter")?;
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "get_styles",
        "selector": selector,
        "window_id": wid,
    });
    if let Some(props) = args.get("properties") {
        cmd["properties"] = props.clone();
    }
    client.send(cmd).await
}

async fn handle_interact(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "interact",
        "action": action,
        "strategy": str_arg(args, "strategy").unwrap_or_else(|| "css".to_string()),
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "selector") {
        cmd["selector"] = json!(s);
    }
    if let Some(v) = num_arg(args, "x") {
        cmd["x"] = json!(v);
    }
    if let Some(v) = num_arg(args, "y") {
        cmd["y"] = json!(v);
    }
    if let Some(s) = str_arg(args, "direction") {
        cmd["direction"] = json!(s);
    }
    if let Some(v) = num_arg(args, "distance") {
        cmd["distance"] = json!(v);
    }
    if action == "drag" {
        if let Some(s) = str_arg(args, "targetSelector") {
            cmd["target_selector"] = json!(s);
        }
        if let Some(v) = num_arg(args, "targetX") {
            cmd["target_x"] = json!(v);
        }
        if let Some(v) = num_arg(args, "targetY") {
            cmd["target_y"] = json!(v);
        }
        if let Some(v) = num_arg(args, "steps") {
            cmd["steps"] = json!(v as u32);
        }
        if let Some(v) = num_arg(args, "durationMs") {
            cmd["duration_ms"] = json!(v as u32);
        }
        if let Some(s) = str_arg(args, "dragStrategy") {
            cmd["drag_strategy"] = json!(s);
        }
    }
    client.send(cmd).await
}

async fn handle_keyboard(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "keyboard",
        "action": action,
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "text") {
        cmd["text"] = json!(s);
    }
    if let Some(s) = str_arg(args, "key") {
        cmd["key"] = json!(s);
    }
    if let Some(m) = args.get("modifiers") {
        cmd["modifiers"] = m.clone();
    }
    client.send(cmd).await
}

async fn handle_wait_for(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
    let mut cmd = json!({
        "type": "wait_for",
        "strategy": str_arg(args, "strategy").unwrap_or_else(|| "css".to_string()),
        "timeout": timeout,
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "selector") {
        cmd["selector"] = json!(s);
    }
    if let Some(s) = str_arg(args, "text") {
        cmd["text"] = json!(s);
    }
    if let Some(s) = str_arg(args, "url") {
        cmd["url"] = json!(s);
    }
    if let Some(s) = str_arg(args, "loadState").or_else(|| str_arg(args, "load_state")) {
        cmd["load_state"] = json!(s);
    }
    if let Some(s) = str_arg(args, "fn")
        .or_else(|| str_arg(args, "function"))
        .or_else(|| str_arg(args, "condition"))
    {
        cmd["function"] = json!(s);
    }
    if let Some(s) = str_arg(args, "state") {
        cmd["state"] = json!(s);
    }
    client.send_with_timeout(cmd, timeout + 5000).await
}

async fn handle_locator(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "locator",
        "window_id": wid,
        "exact": args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false),
        "first": args.get("first").and_then(|v| v.as_bool()).unwrap_or(false),
        "last": args.get("last").and_then(|v| v.as_bool()).unwrap_or(false),
    });
    for (in_key, out_key) in [
        ("role", "role"),
        ("text", "text"),
        ("label", "label"),
        ("placeholder", "placeholder"),
        ("alt", "alt"),
        ("title", "title"),
        ("testId", "test_id"),
        ("testid", "test_id"),
        ("test_id", "test_id"),
        ("name", "name"),
        ("action", "action"),
        ("value", "value"),
    ] {
        if let Some(value) = str_arg(args, in_key) {
            cmd[out_key] = json!(value);
        }
    }
    if let Some(nth) = num_arg(args, "nth") {
        cmd["nth"] = json!(nth as usize);
    }
    client.send_with_timeout(cmd, 30_000).await
}

async fn handle_get_pointed_element(
    client: &ConnectorClient,
    args: &Value,
) -> Result<Value, String> {
    let wid = window_id(args);
    client
        .send(json!({ "type": "get_pointed_element", "window_id": wid }))
        .await
}

async fn handle_select_element(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    client.inspect("webview_select_element", args).await
}

async fn handle_manage_window(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);

    match action.as_str() {
        "list" => client.send(json!({ "type": "window_list" })).await,
        "info" => {
            client
                .send(json!({ "type": "window_info", "window_id": wid }))
                .await
        }
        "resize" => {
            let width = num_arg(args, "width").ok_or("Missing 'width'")?;
            let height = num_arg(args, "height").ok_or("Missing 'height'")?;
            client
                .send(json!({
                    "type": "window_resize",
                    "window_id": wid,
                    "width": width as u32,
                    "height": height as u32,
                }))
                .await
        }
        _ => Err(format!("Unknown window action: {action}")),
    }
}

async fn handle_backend_state(client: &ConnectorClient) -> Result<Value, String> {
    client.send(json!({ "type": "backend_state" })).await
}

async fn handle_ipc_execute_command(
    client: &ConnectorClient,
    args: &Value,
) -> Result<Value, String> {
    let command = str_arg(args, "command").ok_or("Missing 'command' parameter")?;
    let mut cmd = json!({ "type": "ipc_execute_command", "command": command });
    if let Some(a) = args.get("args") {
        cmd["args"] = a.clone();
    }
    client.send(cmd).await
}

async fn handle_ipc_monitor(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    client
        .send(json!({ "type": "ipc_monitor", "action": action, "window_id": window_id(args) }))
        .await
}

async fn handle_ipc_get_captured(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "ipc_get_captured", "limit": limit });
    if let Some(f) = str_arg(args, "filter") {
        cmd["filter"] = json!(f);
    }
    if let Some(p) = str_arg(args, "pattern") {
        cmd["pattern"] = json!(p);
    }
    if let Some(s) = num_arg(args, "since") {
        cmd["since"] = json!(s as u64);
    }
    if let Some(mark) = str_arg(args, "sinceMark") {
        cmd["since_mark"] = json!(mark);
    }
    client.send(cmd).await
}

async fn handle_ipc_emit_event(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let event_name = str_arg(args, "eventName").ok_or("Missing 'eventName' parameter")?;
    let mut cmd = json!({ "type": "ipc_emit_event", "event_name": event_name });
    if let Some(p) = args.get("payload") {
        cmd["payload"] = p.clone();
    }
    client.send(cmd).await
}

async fn handle_read_logs(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let lines = num_arg(args, "lines").unwrap_or(50.0) as usize;
    let wid = window_id(args);
    let mut cmd = json!({ "type": "console_logs", "lines": lines, "window_id": wid });
    if let Some(f) = str_arg(args, "filter") {
        cmd["filter"] = json!(f);
    }
    if let Some(p) = str_arg(args, "pattern") {
        cmd["pattern"] = json!(p);
    }
    if let Some(l) = str_arg(args, "level") {
        cmd["level"] = json!(l);
    }
    client.send(cmd).await
}

async fn handle_clear_logs(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let source = str_arg(args, "source").unwrap_or_else(|| "all".to_string());
    handle_clear_logs_source(client, &source).await
}

async fn handle_clear_logs_source(client: &ConnectorClient, source: &str) -> Result<Value, String> {
    client
        .send(json!({ "type": "clear_logs", "source": source }))
        .await
}

async fn handle_read_log_file(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let source = str_arg(args, "source").ok_or("Missing 'source' parameter")?;
    let lines = num_arg(args, "lines").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "read_log_file", "source": source, "lines": lines });
    if let Some(l) = str_arg(args, "level") {
        cmd["level"] = json!(l);
    }
    if let Some(p) = str_arg(args, "pattern") {
        cmd["pattern"] = json!(p);
    }
    if let Some(s) = num_arg(args, "since") {
        cmd["since"] = json!(s as u64);
    }
    if let Some(w) = str_arg(args, "windowId") {
        cmd["window_id"] = json!(w);
    }
    client.send(cmd).await
}

async fn handle_ipc_listen(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let mut cmd = json!({ "type": "ipc_listen", "action": action });
    if let Some(events) = args.get("events") {
        cmd["events"] = events.clone();
    }
    client.send(cmd).await
}

async fn handle_event_get_captured(
    client: &ConnectorClient,
    args: &Value,
) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "event_get_captured", "limit": limit });
    if let Some(e) = str_arg(args, "event") {
        cmd["event"] = json!(e);
    }
    if let Some(p) = str_arg(args, "pattern") {
        cmd["pattern"] = json!(p);
    }
    if let Some(s) = num_arg(args, "since") {
        cmd["since"] = json!(s as u64);
    }
    if let Some(mark) = str_arg(args, "sinceMark") {
        cmd["since_mark"] = json!(mark);
    }
    client.send(cmd).await
}

async fn handle_runtime_get_captured(
    client: &ConnectorClient,
    args: &Value,
) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "runtime_get_captured", "limit": limit });
    for (in_key, out_key) in [
        ("kind", "kind"),
        ("level", "level"),
        ("pattern", "pattern"),
        ("windowId", "window_id"),
    ] {
        if let Some(v) = str_arg(args, in_key) {
            cmd[out_key] = json!(v);
        }
    }
    if let Some(s) = num_arg(args, "since") {
        cmd["since"] = json!(s as u64);
    }
    if let Some(mark) = str_arg(args, "sinceMark") {
        cmd["since_mark"] = json!(mark);
    }
    client.send(cmd).await
}

async fn handle_artifact_list(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "artifact_list", "limit": limit });
    if let Some(kind) = str_arg(args, "kind") {
        cmd["kind"] = json!(kind);
    }
    client.send(cmd).await
}

async fn handle_artifact_read(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let artifact = str_arg(args, "artifact")
        .or_else(|| str_arg(args, "artifactId"))
        .ok_or("Missing 'artifact' parameter")?;
    client
        .send(json!({ "type": "artifact_read", "artifact": artifact }))
        .await
}

async fn handle_artifact_compare(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let before = str_arg(args, "before").ok_or("Missing 'before' parameter")?;
    let after = str_arg(args, "after").ok_or("Missing 'after' parameter")?;
    let threshold = num_arg(args, "threshold").unwrap_or(0.0);
    client
        .send(json!({
            "type": "artifact_compare",
            "before": before,
            "after": after,
            "threshold": threshold,
        }))
        .await
}

async fn handle_artifact_prune(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let keep = num_arg(args, "keep").unwrap_or(50.0) as usize;
    let mut cmd = json!({
        "type": "artifact_prune",
        "keep": keep,
        "delete_files": args.get("deleteFiles").and_then(|v| v.as_bool()).unwrap_or(true),
    });
    if let Some(kind) = str_arg(args, "kind") {
        cmd["kind"] = json!(kind);
    }
    client.send(cmd).await
}

async fn handle_debug_mark(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let mut cmd = json!({ "type": "debug_mark" });
    if let Some(label) = str_arg(args, "label") {
        cmd["label"] = json!(label);
    }
    client.send(cmd).await
}

async fn handle_debug_snapshot(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let mut cmd = json!({
        "type": "debug_snapshot",
        "window_id": window_id(args),
    });
    for (in_key, out_key) in [
        ("includeDom", "include_dom"),
        ("includeScreenshot", "include_screenshot"),
        ("includeLogs", "include_logs"),
        ("includeIpc", "include_ipc"),
        ("includeEvents", "include_events"),
        ("includeRuntime", "include_runtime"),
    ] {
        if let Some(v) = args.get(in_key).and_then(|v| v.as_bool()) {
            cmd[out_key] = json!(v);
        }
    }
    if let Some(s) = num_arg(args, "since") {
        cmd["since"] = json!(s as u64);
    }
    if let Some(s) = str_arg(args, "sinceMark") {
        cmd["since_mark"] = json!(s);
    }
    if let Some(n) = num_arg(args, "maxTokens") {
        cmd["max_tokens"] = json!(n as u64);
    }
    if let Some(s) = str_arg(args, "screenshotNameHint") {
        cmd["screenshot_name_hint"] = json!(s);
    }
    client.send_with_timeout(cmd, 60_000).await
}

async fn handle_act_and_verify(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let mut cmd = json!({
        "type": "webview_act_and_verify",
        "action": action,
        "window_id": window_id(args),
    });
    for (in_key, out_key) in [
        ("selector", "selector"),
        ("text", "text"),
        ("key", "key"),
        ("targetSelector", "target_selector"),
        ("waitForSelector", "wait_for_selector"),
        ("waitForText", "wait_for_text"),
    ] {
        if let Some(v) = str_arg(args, in_key) {
            cmd[out_key] = json!(v);
        }
    }
    for (in_key, out_key) in [
        ("verifyDom", "verify_dom"),
        ("verifyScreenshot", "verify_screenshot"),
        ("includeLogs", "include_logs"),
        ("includeIpc", "include_ipc"),
        ("includeRuntime", "include_runtime"),
    ] {
        if let Some(v) = args.get(in_key).and_then(|v| v.as_bool()) {
            cmd[out_key] = json!(v);
        }
    }
    let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
    cmd["timeout"] = json!(timeout);
    client.send_with_timeout(cmd, timeout + 60_000).await
}

async fn handle_search_snapshot(client: &ConnectorClient, args: &Value) -> Result<Value, String> {
    let pattern = str_arg(args, "pattern").ok_or("Missing 'pattern' parameter")?;
    let context = num_arg(args, "context").unwrap_or(2.0) as usize;
    let mode = str_arg(args, "mode").unwrap_or_else(|| "ai".to_string());
    let wid = window_id(args);
    client
        .send(json!({
            "type": "search_snapshot",
            "pattern": pattern,
            "context": context,
            "mode": mode,
            "window_id": wid,
        }))
        .await
}

async fn handle_list_devices(host: &str, port: u16) -> Result<Value, String> {
    let mut devices = Vec::new();
    let mut unverified = 0usize;
    for p in port..=port.saturating_add(100) {
        match connector_client::discovery::endpoint_identity(host,p,100).await {
            Ok(identity)=>devices.push(json!({"host":host,"port":p,"appId":identity.app_id,"appInstanceId":identity.app_instance_id,"pid":identity.pid,"identityVerified":true})),
            Err(_)=>unverified+=1,
        }
    }
    Ok(
        json!({"devices":devices,"count":devices.len(),"coverage":"authenticated_identity_only","unverifiedEndpoints":unverified,"limitations":["An unavailable or unauthorized endpoint is unobserved, not proof no application is running"]}),
    )
}

const SETUP_INSTRUCTIONS: &str = r#"
## tauri-plugin-connector Setup

### 1. Add the plugin dependency

In your Tauri app's `src-tauri/Cargo.toml`:

```toml
[dependencies]
tauri-plugin-connector = "0.14"
```

### 2. Register the plugin

In `src-tauri/src/lib.rs`:

```rust
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_connector::init())
        .run(tauri::generate_context!())
        .expect("error running app");
}
```

### 3. Add permissions

In `src-tauri/capabilities/default.json`, add:

```json
{
  "permissions": ["connector:default"]
}
```

### 4. Configure the MCP server

In `.mcp.json` (for Claude Code):

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "tauri-connector-mcp",
      "env": {
        "TAURI_CONNECTOR_HOST": "127.0.0.1",
        "TAURI_CONNECTOR_PORT": "9555"
      }
    }
  }
}
```

### 5. Run your Tauri app

The plugin will start a WebSocket server on port 9555 (or next available in range 9555-9655).
The MCP server connects to this WebSocket to bridge Claude Code ↔ your Tauri app.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_exit_codes_keep_pending_and_unknown_distinct_from_success() {
        assert_eq!(
            workflow_result_exit_code(
                &json!({"status":"completed","goalStatus":"passed","originalTestVerdict":"failed","recoveryOccurred":true})
            ),
            1
        );
        for status in [
            "created",
            "running",
            "paused",
            "cancel_requested",
            "interrupted",
            "outcome_unknown",
        ] {
            assert_eq!(workflow_result_exit_code(&json!({"status":status})), 2);
        }
        for status in ["failed", "cancelled", "expired"] {
            assert_eq!(workflow_result_exit_code(&json!({"status":status})), 1);
        }
        assert_eq!(
            workflow_result_exit_code(&json!({"status":"completed","goalStatus":"passed"})),
            0
        );
        assert_eq!(
            workflow_result_exit_code(
                &json!({"status":"completed","goalStatus":"not_requested","data":{"error":"business data"}})
            ),
            0
        );
        assert_eq!(
            workflow_result_exit_code(&json!({"status":"completed","goalStatus":"failed"})),
            1
        );
        assert_eq!(
            workflow_result_exit_code(&json!({"status":"completed","goalStatus":"inconclusive"})),
            2
        );
    }
    use std::collections::BTreeSet;

    #[test]
    fn workflow_schemas_define_app_owned_lifecycle() {
        let get_schema = tool("workflow_get")["inputSchema"].clone();
        assert_eq!(get_schema["properties"]["evidenceId"]["type"], "string");
        assert_eq!(get_schema["properties"]["offset"]["minimum"], 0);
        assert_eq!(
            get_schema["dependentRequired"]["offset"],
            json!(["evidenceId"])
        );
        for name in [
            "workflow_run",
            "workflow_get",
            "workflow_cancel",
            "workflow_resume",
            "workflow_capabilities",
        ] {
            let definition = tool(name);
            assert_eq!(definition["inputSchema"]["type"], "object");
        }
        assert_eq!(
            tool("workflow_run")["inputSchema"]["required"],
            json!(["spec"])
        );
        assert_eq!(
            tool("workflow_run")["inputSchema"]["properties"]["waitMs"]["maximum"],
            30_000
        );
        assert_eq!(
            tool("workflow_resume")["inputSchema"]["required"],
            json!(["runId", "expectedRevision", "checkpointId", "intent"])
        );
        let schema = tool("workflow_run")["inputSchema"].clone();
        assert_eq!(
            schema["properties"]["spec"]["required"],
            json!(["schemaVersion", "runKey", "steps"])
        );
        fn check_references(value: &Value, root: &Value) {
            match value {
                Value::Object(fields) => {
                    if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                        assert!(
                            root.pointer(reference.strip_prefix('#').unwrap()).is_some(),
                            "broken schema reference {reference}"
                        );
                    }
                    for value in fields.values() {
                        check_references(value, root);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        check_references(value, root);
                    }
                }
                _ => {}
            }
        }
        check_references(&schema, &schema);
    }

    #[tokio::test]
    async fn workflow_requests_are_forwarded_even_without_a_local_executor() {
        let client = ConnectorClient::new();
        for operation in [
            "workflow_run",
            "workflow_get",
            "workflow_cancel",
            "workflow_resume",
            "workflow_capabilities",
        ] {
            let error = dispatch_tool(&client, "127.0.0.1", 9555, operation, &json!({}))
                .await
                .unwrap_err();
            assert!(!error.contains("Unknown tool"), "{operation}: {error}");
        }
    }

    #[tokio::test]
    async fn tool_transport_failure_sets_mcp_error_flag() {
        let mut client = ConnectorClient::new();
        let response = call_tool(&mut client, "127.0.0.1", 9555, "bridge_status", &json!({})).await;
        assert_eq!(response["isError"], true);
    }

    fn tool(name: &str) -> Value {
        tool_definitions()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("missing tool {name}"))
    }

    #[test]
    fn interact_schema_contains_drag_args() {
        let tool = tool("webview_interact");
        let props = &tool["inputSchema"]["properties"];
        assert!(props["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("drag")));
        for key in [
            "targetSelector",
            "targetX",
            "targetY",
            "steps",
            "durationMs",
            "dragStrategy",
        ] {
            assert!(props.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn dom_snapshot_schema_contains_budget_args() {
        let tool = tool("webview_dom_snapshot");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("maxTokens").is_some());
        assert!(props.get("noSplit").is_some());
    }

    #[test]
    fn screenshot_schema_contains_artifact_args() {
        let tool = tool("webview_screenshot");
        let props = &tool["inputSchema"]["properties"];
        for key in [
            "save",
            "outputDir",
            "nameHint",
            "overwrite",
            "annotate",
            "selector",
            "windowId",
        ] {
            assert!(props.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn wait_and_locator_schema_expose_agent_oriented_args() {
        let wait = tool("webview_wait_for");
        let wait_props = &wait["inputSchema"]["properties"];
        for key in ["url", "loadState", "fn", "state"] {
            assert!(wait_props.get(key).is_some(), "missing wait prop {key}");
        }

        let locator = tool("webview_locator");
        let locator_props = &locator["inputSchema"]["properties"];
        for key in [
            "role",
            "text",
            "label",
            "placeholder",
            "alt",
            "title",
            "testId",
            "name",
            "action",
        ] {
            assert!(
                locator_props.get(key).is_some(),
                "missing locator prop {key}"
            );
        }
    }

    #[test]
    fn runtime_schema_contains_since_mark_and_window() {
        let tool = tool("runtime_get_captured");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("sinceMark").is_some());
        assert!(props.get("windowId").is_some());
    }

    #[test]
    fn artifact_schema_contains_prune_tool() {
        let tool = tool("artifact_prune");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("keep").is_some());
        assert!(props.get("deleteFiles").is_some());
    }

    fn tool_names(defs: &Value) -> BTreeSet<String> {
        defs["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn standalone_tools_are_embedded_plus_driver_session() {
        let standalone = tool_names(&tool_definitions());
        let embedded = tool_names(&embedded_mcp_tool_schema::tool_definitions());

        // The standalone server exposes exactly the shared tools plus one extra.
        let extras: Vec<_> = standalone.difference(&embedded).cloned().collect();
        assert_eq!(
            extras,
            vec!["driver_session".to_string()],
            "standalone must add exactly the driver_session tool over the shared schema"
        );

        // No shared tool may be dropped from the standalone list.
        let missing: Vec<_> = embedded.difference(&standalone).cloned().collect();
        assert!(
            missing.is_empty(),
            "standalone is missing shared tools: {missing:?}"
        );

        // Pin the totals so accidental additions/deletions are caught.
        assert_eq!(embedded.len(), 46, "shared tool count changed");
        assert_eq!(standalone.len(), 47, "standalone tool count changed");
    }

    #[test]
    fn batch_actions_schema_describes_spec() {
        let tool = tool("batch_actions");
        let props = &tool["inputSchema"]["properties"];
        for key in [
            "mode",
            "stopOnError",
            "maxParallel",
            "timeoutMs",
            "save",
            "actions",
        ] {
            assert!(props.get(key).is_some(), "missing {key}");
        }
        let action_props = &props["actions"]["items"]["properties"];
        for key in ["id", "tool", "args", "dependsOn", "timeoutMs", "omitResult"] {
            assert!(action_props.get(key).is_some(), "missing action prop {key}");
        }
        assert_eq!(tool["inputSchema"]["required"], json!(["actions"]));
    }

    #[tokio::test]
    async fn batch_actions_rejects_nesting_and_driver_session() {
        let client = ConnectorClient::new();
        for forbidden in ["batch_actions", "driver_session"] {
            let args = json!({ "actions": [ { "tool": forbidden } ] });
            let report = handle_batch_actions(&client, "127.0.0.1", 9555, &args)
                .await
                .expect("batch itself succeeds; the inner action fails");
            assert_eq!(report["failed"], json!(1), "{forbidden} must be rejected");
            let error = report["logs"][0]["error"].as_str().unwrap();
            assert!(error.contains(forbidden), "unexpected error: {error}");
        }
    }

    #[tokio::test]
    async fn batch_actions_reports_per_action_logs_when_disconnected() {
        let client = ConnectorClient::new();
        let args = json!({
            "actions": [
                { "id": "status", "tool": "bridge_status" },
                { "id": "after", "tool": "webview_execute_js", "args": { "script": "1" } }
            ]
        });
        let report = handle_batch_actions(&client, "127.0.0.1", 9555, &args)
            .await
            .unwrap();
        assert_eq!(report["total"], json!(2));
        assert_eq!(report["ok"], json!(false));
        // First action errors (not connected), second is skipped by stopOnError.
        assert_eq!(report["logs"][0]["status"], json!("error"));
        assert_eq!(report["logs"][1]["status"], json!("skipped"));
        assert_eq!(report["logs"][0]["id"], json!("status"));
    }

    #[test]
    fn vendored_schema_matches_plugin_source() {
        let vendored = include_str!("mcp_tool_schema.rs");
        let canonical_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugin/src/mcp_tool_schema.rs"
        );
        let canonical = std::fs::read_to_string(canonical_path)
            .expect("plugin schema source readable (parity test must run from the workspace)");
        assert_eq!(
            vendored, canonical,
            "crates/mcp-server/src/mcp_tool_schema.rs is out of sync with \
             plugin/src/mcp_tool_schema.rs — re-copy the canonical file"
        );
    }
}
