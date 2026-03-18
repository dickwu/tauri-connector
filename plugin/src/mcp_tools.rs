//! MCP tool definitions and dispatch.
//!
//! Calls existing handler functions directly — no WebSocket hop.

use serde_json::{json, Value};

use crate::bridge::Bridge;
use crate::handlers;
use crate::protocol::{Response, ResponsePayload};
use crate::state::PluginState;

/// Convert a handler Response to MCP content format.
fn to_mcp_content(response: Response) -> Value {
    match response.payload {
        ResponsePayload::Success { result } => {
            let text = match &result {
                Value::String(s) => s.clone(),
                _ => serde_json::to_string_pretty(&result).unwrap_or_default(),
            };
            json!({ "content": [{ "type": "text", "text": text }] })
        }
        ResponsePayload::Error { error } => {
            json!({ "content": [{ "type": "text", "text": error }], "isError": true })
        }
    }
}

/// Convert a screenshot Response to MCP image content (or fall back to text).
fn to_mcp_image_or_text(response: Response) -> Value {
    match response.payload {
        ResponsePayload::Success { ref result } => {
            if let (Some(base64), Some(mime)) = (
                result.get("base64").and_then(|v| v.as_str()),
                result.get("mimeType").and_then(|v| v.as_str()),
            ) {
                json!({
                    "content": [{
                        "type": "image",
                        "data": base64,
                        "mimeType": mime,
                    }]
                })
            } else {
                to_mcp_content(response)
            }
        }
        _ => to_mcp_content(response),
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn num_arg(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn window_id(args: &Value) -> String {
    str_arg(args, "windowId").unwrap_or_else(|| "main".to_string())
}

/// Dispatch an MCP tool call to the appropriate handler.
pub async fn call_tool(
    name: &str,
    args: &Value,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Value {
    let id = "mcp";

    let response = match name {
        "webview_execute_js" => {
            let script = str_arg(args, "script").unwrap_or_default();
            let wid = window_id(args);
            handlers::execute_js(id, &script, &wid, bridge).await
        }

        "webview_screenshot" => {
            let format = str_arg(args, "format").unwrap_or_else(|| "png".to_string());
            let quality = num_arg(args, "quality").unwrap_or(80.0) as u8;
            let max_width = num_arg(args, "maxWidth").map(|n| n as u32);
            let wid = window_id(args);
            let resp = handlers::screenshot(id, &format, quality, max_width, &wid, bridge, app).await;
            // Return image content if base64 data is present
            return to_mcp_image_or_text(resp);
        }

        "webview_dom_snapshot" => {
            let snapshot_type = str_arg(args, "type").unwrap_or_else(|| "accessibility".to_string());
            let selector = str_arg(args, "selector");
            let wid = window_id(args);
            handlers::dom_snapshot(id, &snapshot_type, selector.as_deref(), &wid, bridge).await
        }

        "get_cached_dom" => {
            let wid = window_id(args);
            handlers::get_cached_dom(id, &wid, state).await
        }

        "webview_find_element" => {
            let selector = str_arg(args, "selector").unwrap_or_default();
            let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
            let wid = window_id(args);
            handlers::find_element(id, &selector, &strategy, &wid, bridge).await
        }

        "webview_get_styles" => {
            let selector = str_arg(args, "selector").unwrap_or_default();
            let properties: Option<Vec<String>> = args
                .get("properties")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let wid = window_id(args);
            handlers::get_styles(id, &selector, properties.as_deref(), &wid, bridge).await
        }

        "webview_interact" => {
            let action = str_arg(args, "action").unwrap_or_default();
            let selector = str_arg(args, "selector");
            let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
            let x = num_arg(args, "x");
            let y = num_arg(args, "y");
            let direction = str_arg(args, "direction");
            let distance = num_arg(args, "distance");
            let wid = window_id(args);
            handlers::interact(
                id,
                &action,
                selector.as_deref(),
                &strategy,
                x,
                y,
                direction.as_deref(),
                distance,
                &wid,
                bridge,
            )
            .await
        }

        "webview_keyboard" => {
            let action = str_arg(args, "action").unwrap_or_else(|| "type".to_string());
            let text = str_arg(args, "text");
            let key = str_arg(args, "key");
            let modifiers: Option<Vec<String>> = args
                .get("modifiers")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let wid = window_id(args);
            handlers::keyboard(
                id,
                &action,
                text.as_deref(),
                key.as_deref(),
                modifiers.as_deref(),
                &wid,
                bridge,
            )
            .await
        }

        "webview_wait_for" => {
            let selector = str_arg(args, "selector");
            let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
            let text = str_arg(args, "text");
            let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
            let wid = window_id(args);
            handlers::wait_for(id, selector.as_deref(), &strategy, text.as_deref(), timeout, &wid, bridge).await
        }

        "webview_get_pointed_element" => {
            handlers::get_pointed_element(id, state).await
        }

        "webview_select_element" => {
            Response::error(id.to_string(), "Select element (visual picker) not yet implemented")
        }

        "manage_window" => {
            let action = str_arg(args, "action").unwrap_or_default();
            let wid = window_id(args);
            match action.as_str() {
                "list" => handlers::window_list(id, app).await,
                "info" => handlers::window_info(id, &wid, app).await,
                "resize" => {
                    let width = num_arg(args, "width").unwrap_or(800.0) as u32;
                    let height = num_arg(args, "height").unwrap_or(600.0) as u32;
                    handlers::window_resize(id, &wid, width, height, app).await
                }
                _ => Response::error(id.to_string(), format!("Unknown window action: {action}")),
            }
        }

        "ipc_get_backend_state" => {
            handlers::backend_state(id, app).await
        }

        "ipc_execute_command" => {
            let command = str_arg(args, "command").unwrap_or_default();
            let cmd_args = args.get("args").cloned();
            handlers::ipc_execute_command(id, &command, cmd_args.as_ref(), "main", bridge).await
        }

        "ipc_monitor" => {
            let action = str_arg(args, "action").unwrap_or_default();
            handlers::ipc_monitor(id, &action, state).await
        }

        "ipc_get_captured" => {
            let filter = str_arg(args, "filter");
            let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
            handlers::ipc_get_captured(id, filter.as_deref(), limit, state).await
        }

        "ipc_emit_event" => {
            let event_name = str_arg(args, "eventName").unwrap_or_default();
            let payload = args.get("payload").cloned();
            handlers::ipc_emit_event(id, &event_name, payload.as_ref(), app).await
        }

        "read_logs" => {
            let lines = num_arg(args, "lines").unwrap_or(50.0) as usize;
            let filter = str_arg(args, "filter");
            let wid = window_id(args);
            handlers::console_logs(id, lines, filter.as_deref(), &wid, state).await
        }

        "get_setup_instructions" => {
            return json!({
                "content": [{ "type": "text", "text": SETUP_INSTRUCTIONS }]
            });
        }

        "list_devices" => {
            return json!({
                "content": [{ "type": "text", "text": "This MCP server is embedded in the Tauri app. The app is running." }]
            });
        }

        _ => {
            return json!({
                "content": [{ "type": "text", "text": format!("Unknown tool: {name}") }],
                "isError": true,
            });
        }
    };

    to_mcp_content(response)
}

/// Return the list of tool definitions for `tools/list`.
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            tool_def("webview_execute_js",
                "Execute JavaScript in the Tauri webview. Use IIFE for return values: \"(() => { return value; })()\"",
                json!({ "type": "object", "properties": {
                    "script": { "type": "string" },
                    "windowId": { "type": "string" }
                }, "required": ["script"] })
            ),
            tool_def("webview_screenshot",
                "Take a screenshot of the Tauri webview",
                json!({ "type": "object", "properties": {
                    "format": { "type": "string", "enum": ["png", "jpeg"] },
                    "quality": { "type": "number", "minimum": 0, "maximum": 100 },
                    "maxWidth": { "type": "number" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("webview_dom_snapshot",
                "Get structured DOM snapshot (accessibility or structure tree) via JS bridge",
                json!({ "type": "object", "properties": {
                    "type": { "type": "string", "enum": ["accessibility", "structure"] },
                    "selector": { "type": "string" },
                    "windowId": { "type": "string" }
                }, "required": ["type"] })
            ),
            tool_def("get_cached_dom",
                "Get cached DOM snapshot pushed from frontend. Faster and more LLM-friendly.",
                json!({ "type": "object", "properties": { "windowId": { "type": "string" } } })
            ),
            tool_def("webview_find_element",
                "Find elements by CSS selector, XPath, or text content",
                json!({ "type": "object", "properties": {
                    "selector": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                    "windowId": { "type": "string" }
                }, "required": ["selector"] })
            ),
            tool_def("webview_get_styles",
                "Get computed CSS styles for an element",
                json!({ "type": "object", "properties": {
                    "selector": { "type": "string" },
                    "properties": { "type": "array", "items": { "type": "string" } },
                    "windowId": { "type": "string" }
                }, "required": ["selector"] })
            ),
            tool_def("webview_interact",
                "Perform gestures: click, double-click, focus, scroll, hover on elements",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["click", "double-click", "dblclick", "focus", "scroll", "hover"] },
                    "selector": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                    "x": { "type": "number" }, "y": { "type": "number" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "distance": { "type": "number" },
                    "windowId": { "type": "string" }
                }, "required": ["action"] })
            ),
            tool_def("webview_keyboard",
                "Type text or press keys with optional modifiers",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["type", "press"] },
                    "text": { "type": "string" },
                    "key": { "type": "string" },
                    "modifiers": { "type": "array", "items": { "type": "string", "enum": ["ctrl", "shift", "alt", "meta"] } },
                    "windowId": { "type": "string" }
                }, "required": ["action"] })
            ),
            tool_def("webview_wait_for",
                "Wait for element selectors or text content to appear",
                json!({ "type": "object", "properties": {
                    "selector": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                    "text": { "type": "string" },
                    "timeout": { "type": "number" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("webview_get_pointed_element",
                "Get metadata for the element the user Alt+Shift+Clicked",
                json!({ "type": "object", "properties": { "windowId": { "type": "string" } } })
            ),
            tool_def("webview_select_element",
                "Activate visual element picker in the webview",
                json!({ "type": "object", "properties": { "windowId": { "type": "string" } } })
            ),
            tool_def("manage_window",
                "List windows, get window info, or resize a window",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["list", "info", "resize"] },
                    "windowId": { "type": "string" },
                    "width": { "type": "number" },
                    "height": { "type": "number" }
                }, "required": ["action"] })
            ),
            tool_def("ipc_get_backend_state",
                "Get Tauri app metadata, version, environment, and window info",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("ipc_execute_command",
                "Execute any Tauri IPC command via invoke()",
                json!({ "type": "object", "properties": {
                    "command": { "type": "string" },
                    "args": { "type": "object" }
                }, "required": ["command"] })
            ),
            tool_def("ipc_monitor",
                "Start or stop IPC monitoring to capture invoke() calls",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] }
                }, "required": ["action"] })
            ),
            tool_def("ipc_get_captured",
                "Retrieve captured IPC traffic (requires monitoring started)",
                json!({ "type": "object", "properties": {
                    "filter": { "type": "string" },
                    "limit": { "type": "number" }
                } })
            ),
            tool_def("ipc_emit_event",
                "Emit a custom Tauri event for testing event handlers",
                json!({ "type": "object", "properties": {
                    "eventName": { "type": "string" },
                    "payload": {}
                }, "required": ["eventName"] })
            ),
            tool_def("read_logs",
                "Read captured console logs from the webview",
                json!({ "type": "object", "properties": {
                    "lines": { "type": "number" },
                    "filter": { "type": "string" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("get_setup_instructions",
                "Get setup instructions for tauri-plugin-connector",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("list_devices",
                "List running Tauri app instances",
                json!({ "type": "object", "properties": {} })
            ),
        ]
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

const SETUP_INSTRUCTIONS: &str = r#"## tauri-plugin-connector Setup

### 1. Add the plugin dependency
In your Tauri app's `src-tauri/Cargo.toml`:
```toml
[dependencies]
tauri-plugin-connector = "0.2"
```

### 2. Register the plugin (debug-only)
In `src-tauri/src/lib.rs`, add before `.invoke_handler()`:
```rust
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

### 3. Add permissions
In `src-tauri/capabilities/default.json`:
```json
{ "permissions": ["connector:default"] }
```

### 4. Set withGlobalTauri (required)
In `src-tauri/tauri.conf.json`:
```json
{ "app": { "withGlobalTauri": true } }
```

### 5. Configure Claude Code
In `.mcp.json`:
```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

The MCP server starts automatically when the Tauri app runs. No separate command needed.
"#;
