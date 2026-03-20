//! MCP tool definitions and dispatch for tauri-connector.

use connector_client::ConnectorClient;
use serde_json::{json, Value};

use crate::protocol::text_content;

/// Return the list of all tool definitions for `tools/list`.
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            tool_def("driver_session",
                "Start/stop connection to a running Tauri app",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "stop", "status"] },
                        "host": { "type": "string" },
                        "port": { "type": "number" }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("webview_execute_js",
                "Execute JavaScript in the Tauri webview. Use IIFE for return values: \"(() => { return value; })()\"",
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string" },
                        "windowId": { "type": "string" }
                    },
                    "required": ["script"]
                })
            ),
            tool_def("webview_screenshot",
                "Take a screenshot of the Tauri window using native xcap capture (cross-platform)",
                json!({
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "enum": ["png", "jpeg", "webp"] },
                        "quality": { "type": "number", "minimum": 0, "maximum": 100 },
                        "maxWidth": { "type": "number" },
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("webview_dom_snapshot",
                "Get structured DOM snapshot. Mode 'ai' (default) includes ref IDs for interaction, React component names, and stitches portals. Mode 'accessibility' shows ARIA roles/names. Mode 'structure' shows tags/classes.",
                json!({
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["ai", "accessibility", "structure"], "default": "ai" },
                        "selector": { "type": "string", "description": "CSS selector to scope snapshot to a subtree" },
                        "maxDepth": { "type": "number", "description": "Maximum tree depth (0 = unlimited)" },
                        "maxElements": { "type": "number", "description": "Maximum elements to include (0 = unlimited)" },
                        "reactEnrich": { "type": "boolean", "description": "Include React component names (default: true)" },
                        "followPortals": { "type": "boolean", "description": "Stitch portals to their triggers (default: true)" },
                        "shadowDom": { "type": "boolean", "description": "Traverse shadow DOM (default: false)" },
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("get_cached_dom",
                "Get cached DOM snapshot pushed from frontend via invoke(). Faster and more LLM-friendly than webview_dom_snapshot.",
                json!({
                    "type": "object",
                    "properties": {
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("webview_find_element",
                "Find elements by CSS, XPath, text, or regex pattern",
                json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string" },
                        "strategy": { "type": "string", "enum": ["css", "xpath", "text", "regex"] },
                        "target": { "type": "string", "enum": ["text", "class", "id", "attr", "all"], "description": "What regex matches against (regex strategy only)" },
                        "windowId": { "type": "string" }
                    },
                    "required": ["selector"]
                })
            ),
            tool_def("webview_get_styles",
                "Get computed CSS styles for an element",
                json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string" },
                        "properties": { "type": "array", "items": { "type": "string" } },
                        "windowId": { "type": "string" }
                    },
                    "required": ["selector"]
                })
            ),
            tool_def("webview_interact",
                "Perform gestures on elements. hover fires full pointer+mouse event sequence; hover-off fires leave events to dismiss dropdowns/tooltips",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["click", "double-click", "dblclick", "focus", "scroll", "hover", "hover-off"] },
                        "selector": { "type": "string" },
                        "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                        "distance": { "type": "number" },
                        "windowId": { "type": "string" }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("webview_keyboard",
                "Type text or press keys with optional modifiers",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["type", "press"] },
                        "text": { "type": "string" },
                        "key": { "type": "string" },
                        "modifiers": { "type": "array", "items": { "type": "string", "enum": ["ctrl", "shift", "alt", "meta"] } },
                        "windowId": { "type": "string" }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("webview_wait_for",
                "Wait for element selectors or text content to appear",
                json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string" },
                        "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                        "text": { "type": "string" },
                        "timeout": { "type": "number" },
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("webview_get_pointed_element",
                "Get metadata for the element the user Alt+Shift+Clicked",
                json!({
                    "type": "object",
                    "properties": {
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("webview_select_element",
                "Activate visual element picker in the webview",
                json!({
                    "type": "object",
                    "properties": {
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("manage_window",
                "List windows, get window info, or resize a window",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["list", "info", "resize"] },
                        "windowId": { "type": "string" },
                        "width": { "type": "number" },
                        "height": { "type": "number" }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("ipc_get_backend_state",
                "Get Tauri app metadata, version, environment, and window info",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("ipc_execute_command",
                "Execute any Tauri IPC command via invoke()",
                json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "args": { "type": "object" }
                    },
                    "required": ["command"]
                })
            ),
            tool_def("ipc_monitor",
                "Start or stop IPC monitoring to capture invoke() calls",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "stop"] }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("ipc_get_captured",
                "Retrieve captured IPC traffic. Supports regex pattern and timestamp filtering.",
                json!({
                    "type": "object",
                    "properties": {
                        "filter": { "type": "string", "description": "Substring match on command name" },
                        "pattern": { "type": "string", "description": "Regex on full entry (overrides filter)" },
                        "limit": { "type": "number" },
                        "since": { "type": "number", "description": "Only entries after this epoch ms" }
                    }
                })
            ),
            tool_def("ipc_emit_event",
                "Emit a custom Tauri event for testing event handlers",
                json!({
                    "type": "object",
                    "properties": {
                        "eventName": { "type": "string" },
                        "payload": {}
                    },
                    "required": ["eventName"]
                })
            ),
            tool_def("read_logs",
                "Read console logs. Supports level filtering (error,warn) and regex patterns on messages.",
                json!({
                    "type": "object",
                    "properties": {
                        "lines": { "type": "number", "description": "Max entries to return (default 50)" },
                        "filter": { "type": "string", "description": "Substring match on message (backward compat)" },
                        "pattern": { "type": "string", "description": "Regex pattern on message (overrides filter)" },
                        "level": { "type": "string", "description": "Filter by level: error, warn, info, log, debug. Comma-separated." },
                        "windowId": { "type": "string" }
                    }
                })
            ),
            tool_def("clear_logs",
                "Clear log files. Specify source: console, ipc, events, or all.",
                json!({
                    "type": "object",
                    "properties": {
                        "source": { "type": "string", "enum": ["console", "ipc", "events", "all"], "default": "all" }
                    }
                })
            ),
            tool_def("read_log_file",
                "Read historical log files (persisted across app restarts). Supports regex and timestamp filtering.",
                json!({
                    "type": "object",
                    "properties": {
                        "source": { "type": "string", "enum": ["console", "ipc", "events"] },
                        "lines": { "type": "number", "description": "Max entries from tail (default 100)" },
                        "level": { "type": "string", "description": "Level filter (console only)" },
                        "pattern": { "type": "string", "description": "Regex on serialized entry" },
                        "since": { "type": "number", "description": "Epoch ms floor" },
                        "windowId": { "type": "string" }
                    },
                    "required": ["source"]
                })
            ),
            tool_def("ipc_listen",
                "Listen for Tauri events. Start captures events to events.log, stop removes all listeners.",
                json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "stop"] },
                        "events": { "type": "array", "items": { "type": "string" }, "description": "Event names to listen for" }
                    },
                    "required": ["action"]
                })
            ),
            tool_def("event_get_captured",
                "Retrieve captured Tauri events from events.log. Filter by event name, regex, or timestamp.",
                json!({
                    "type": "object",
                    "properties": {
                        "event": { "type": "string", "description": "Filter by event name (exact)" },
                        "pattern": { "type": "string", "description": "Regex on full entry" },
                        "limit": { "type": "number" },
                        "since": { "type": "number", "description": "Epoch ms floor" }
                    }
                })
            ),
            tool_def("webview_search_snapshot",
                "Search DOM snapshot with regex. Returns matched lines with context. Uses cached snapshot if fresh (<10s).",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "context": { "type": "number", "description": "Lines of context (default 2, max 10)" },
                        "mode": { "type": "string", "enum": ["ai", "accessibility", "structure"] },
                        "windowId": { "type": "string" }
                    },
                    "required": ["pattern"]
                })
            ),
            tool_def("get_setup_instructions",
                "Get setup instructions for integrating tauri-plugin-connector into a Tauri app",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("list_devices",
                "List running Tauri app instances that the connector can reach",
                json!({ "type": "object", "properties": {} })
            ),
        ]
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
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
        "webview_execute_js" => handle_execute_js(client, args).await,
        "webview_screenshot" => handle_screenshot(client, args).await,
        "webview_dom_snapshot" => handle_dom_snapshot(client, args).await,
        "get_cached_dom" => handle_cached_dom(client, args).await,
        "webview_find_element" => handle_find_element(client, args).await,
        "webview_get_styles" => handle_get_styles(client, args).await,
        "webview_interact" => handle_interact(client, args).await,
        "webview_keyboard" => handle_keyboard(client, args).await,
        "webview_wait_for" => handle_wait_for(client, args).await,
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
        "webview_search_snapshot" => handle_search_snapshot(client, args).await,
        "get_setup_instructions" => Ok(json!(SETUP_INSTRUCTIONS)),
        "list_devices" => handle_list_devices(host, port).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(data) => text_content(&data),
        Err(e) => text_content(&json!({ "error": e })),
    }
}

// ─── Helpers ───

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
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
    let p = num_arg(args, "port").map(|n| n as u16).unwrap_or(port);

    match action.as_str() {
        "start" => {
            client.connect(&h, p).await?;
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

async fn handle_execute_js(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let script = str_arg(args, "script").ok_or("Missing 'script' parameter")?;
    let wid = window_id(args);
    client.send(json!({ "type": "execute_js", "script": script, "window_id": wid })).await
}

async fn handle_screenshot(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
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
    client.send(cmd).await
}

async fn handle_dom_snapshot(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let mode = args.get("mode").or_else(|| args.get("type"))
        .and_then(|v| v.as_str()).unwrap_or("ai");
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
    client.send(cmd).await
}

async fn handle_cached_dom(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    client.send(json!({ "type": "get_cached_dom", "window_id": wid })).await
}

async fn handle_find_element(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let selector = str_arg(args, "selector").ok_or("Missing 'selector' parameter")?;
    let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "find_element",
        "selector": selector,
        "strategy": strategy,
        "window_id": wid,
    });
    if let Some(t) = str_arg(args, "target") { cmd["target"] = json!(t); }
    client.send(cmd).await
}

async fn handle_get_styles(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
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

async fn handle_interact(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "interact",
        "action": action,
        "strategy": str_arg(args, "strategy").unwrap_or_else(|| "css".to_string()),
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "selector") { cmd["selector"] = json!(s); }
    if let Some(v) = num_arg(args, "x") { cmd["x"] = json!(v); }
    if let Some(v) = num_arg(args, "y") { cmd["y"] = json!(v); }
    if let Some(s) = str_arg(args, "direction") { cmd["direction"] = json!(s); }
    if let Some(v) = num_arg(args, "distance") { cmd["distance"] = json!(v); }
    client.send(cmd).await
}

async fn handle_keyboard(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);
    let mut cmd = json!({
        "type": "keyboard",
        "action": action,
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "text") { cmd["text"] = json!(s); }
    if let Some(s) = str_arg(args, "key") { cmd["key"] = json!(s); }
    if let Some(m) = args.get("modifiers") { cmd["modifiers"] = m.clone(); }
    client.send(cmd).await
}

async fn handle_wait_for(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
    let mut cmd = json!({
        "type": "wait_for",
        "strategy": str_arg(args, "strategy").unwrap_or_else(|| "css".to_string()),
        "timeout": timeout,
        "window_id": wid,
    });
    if let Some(s) = str_arg(args, "selector") { cmd["selector"] = json!(s); }
    if let Some(s) = str_arg(args, "text") { cmd["text"] = json!(s); }
    client.send_with_timeout(cmd, timeout + 5000).await
}

async fn handle_get_pointed_element(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    client.send(json!({ "type": "get_pointed_element", "window_id": wid })).await
}

async fn handle_select_element(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let wid = window_id(args);
    client.send(json!({ "type": "select_element", "window_id": wid })).await
}

async fn handle_manage_window(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let wid = window_id(args);

    match action.as_str() {
        "list" => client.send(json!({ "type": "window_list" })).await,
        "info" => client.send(json!({ "type": "window_info", "window_id": wid })).await,
        "resize" => {
            let width = num_arg(args, "width").ok_or("Missing 'width'")?;
            let height = num_arg(args, "height").ok_or("Missing 'height'")?;
            client.send(json!({
                "type": "window_resize",
                "window_id": wid,
                "width": width as u32,
                "height": height as u32,
            })).await
        }
        _ => Err(format!("Unknown window action: {action}")),
    }
}

async fn handle_backend_state(client: &mut ConnectorClient) -> Result<Value, String> {
    client.send(json!({ "type": "backend_state" })).await
}

async fn handle_ipc_execute_command(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let command = str_arg(args, "command").ok_or("Missing 'command' parameter")?;
    let mut cmd = json!({ "type": "ipc_execute_command", "command": command });
    if let Some(a) = args.get("args") { cmd["args"] = a.clone(); }
    client.send(cmd).await
}

async fn handle_ipc_monitor(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    client.send(json!({ "type": "ipc_monitor", "action": action })).await
}

async fn handle_ipc_get_captured(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "ipc_get_captured", "limit": limit });
    if let Some(f) = str_arg(args, "filter") { cmd["filter"] = json!(f); }
    if let Some(p) = str_arg(args, "pattern") { cmd["pattern"] = json!(p); }
    if let Some(s) = num_arg(args, "since") { cmd["since"] = json!(s as u64); }
    client.send(cmd).await
}

async fn handle_ipc_emit_event(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let event_name = str_arg(args, "eventName").ok_or("Missing 'eventName' parameter")?;
    let mut cmd = json!({ "type": "ipc_emit_event", "event_name": event_name });
    if let Some(p) = args.get("payload") { cmd["payload"] = p.clone(); }
    client.send(cmd).await
}

async fn handle_read_logs(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let lines = num_arg(args, "lines").unwrap_or(50.0) as usize;
    let wid = window_id(args);
    let mut cmd = json!({ "type": "console_logs", "lines": lines, "window_id": wid });
    if let Some(f) = str_arg(args, "filter") { cmd["filter"] = json!(f); }
    if let Some(p) = str_arg(args, "pattern") { cmd["pattern"] = json!(p); }
    if let Some(l) = str_arg(args, "level") { cmd["level"] = json!(l); }
    client.send(cmd).await
}

async fn handle_clear_logs(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let source = str_arg(args, "source").unwrap_or_else(|| "all".to_string());
    client.send(json!({ "type": "clear_logs", "source": source })).await
}

async fn handle_read_log_file(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let source = str_arg(args, "source").ok_or("Missing 'source' parameter")?;
    let lines = num_arg(args, "lines").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "read_log_file", "source": source, "lines": lines });
    if let Some(l) = str_arg(args, "level") { cmd["level"] = json!(l); }
    if let Some(p) = str_arg(args, "pattern") { cmd["pattern"] = json!(p); }
    if let Some(s) = num_arg(args, "since") { cmd["since"] = json!(s as u64); }
    if let Some(w) = str_arg(args, "windowId") { cmd["window_id"] = json!(w); }
    client.send(cmd).await
}

async fn handle_ipc_listen(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let action = str_arg(args, "action").ok_or("Missing 'action' parameter")?;
    let mut cmd = json!({ "type": "ipc_listen", "action": action });
    if let Some(events) = args.get("events") { cmd["events"] = events.clone(); }
    client.send(cmd).await
}

async fn handle_event_get_captured(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let mut cmd = json!({ "type": "event_get_captured", "limit": limit });
    if let Some(e) = str_arg(args, "event") { cmd["event"] = json!(e); }
    if let Some(p) = str_arg(args, "pattern") { cmd["pattern"] = json!(p); }
    if let Some(s) = num_arg(args, "since") { cmd["since"] = json!(s as u64); }
    client.send(cmd).await
}

async fn handle_search_snapshot(client: &mut ConnectorClient, args: &Value) -> Result<Value, String> {
    let pattern = str_arg(args, "pattern").ok_or("Missing 'pattern' parameter")?;
    let context = num_arg(args, "context").unwrap_or(2.0) as usize;
    let mode = str_arg(args, "mode").unwrap_or_else(|| "ai".to_string());
    let wid = window_id(args);
    client.send(json!({
        "type": "search_snapshot",
        "pattern": pattern,
        "context": context,
        "mode": mode,
        "window_id": wid,
    })).await
}

async fn handle_list_devices(host: &str, port: u16) -> Result<Value, String> {
    // Scan the default port range to find running connector instances
    let mut devices = Vec::new();
    let start = port;
    let end = port + 100;

    for p in start..end {
        if let Ok(stream) = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tokio::net::TcpStream::connect(format!("{host}:{p}")),
        ).await {
            if stream.is_ok() {
                devices.push(json!({ "host": host, "port": p }));
            }
        }
    }

    Ok(json!({ "devices": devices, "count": devices.len() }))
}

const SETUP_INSTRUCTIONS: &str = r#"
## tauri-plugin-connector Setup

### 1. Add the plugin dependency

In your Tauri app's `src-tauri/Cargo.toml`:

```toml
[dependencies]
tauri-plugin-connector = "0.6"
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
