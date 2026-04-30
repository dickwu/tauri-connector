//! MCP tool definitions and dispatch.
//!
//! Calls existing handler functions directly — no WebSocket hop.

use serde_json::{Value, json};

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
                let mut content = vec![json!({
                    "type": "image",
                    "data": base64,
                    "mimeType": mime,
                })];
                if let Some(artifact) = result.get("artifact") {
                    content.push(json!({
                        "type": "text",
                        "text": serde_json::to_string_pretty(artifact).unwrap_or_default(),
                    }));
                }
                json!({
                    "content": content
                })
            } else {
                to_mcp_content(response)
            }
        }
        _ => to_mcp_content(response),
    }
}

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

        "bridge_status" => Response::success(id.to_string(), bridge.status().await),

        "webview_screenshot" => {
            let format = str_arg(args, "format").unwrap_or_else(|| "png".to_string());
            let quality = num_arg(args, "quality").unwrap_or(80.0) as u8;
            let max_width = num_arg(args, "maxWidth").map(|n| n as u32);
            let wid = window_id(args);
            let save = args.get("save").and_then(|v| v.as_bool()).unwrap_or(false);
            let output_dir = str_arg(args, "outputDir");
            let name_hint = str_arg(args, "nameHint");
            let overwrite = args
                .get("overwrite")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let selector = str_arg(args, "selector");
            let resp = handlers::screenshot(
                id,
                &format,
                quality,
                max_width,
                &wid,
                bridge,
                app,
                state,
                save,
                output_dir.as_deref(),
                name_hint.as_deref(),
                overwrite,
                selector.as_deref(),
            )
            .await;
            // Return image content if base64 data is present
            return to_mcp_image_or_text(resp);
        }

        "webview_dom_snapshot" => {
            // Backward compat: accept "type" as alias for "mode"
            let mode = str_arg(args, "mode")
                .or_else(|| str_arg(args, "type"))
                .unwrap_or_else(|| "ai".to_string());
            let selector = str_arg(args, "selector");
            let max_depth = num_arg(args, "max_depth")
                .or_else(|| num_arg(args, "maxDepth"))
                .map(|n| n as u64);
            let max_elements = num_arg(args, "max_elements")
                .or_else(|| num_arg(args, "maxElements"))
                .map(|n| n as u64);
            // Default maxTokens to 4000 for MCP callers
            let max_tokens = num_arg(args, "max_tokens")
                .or_else(|| num_arg(args, "maxTokens"))
                .map(|n| n as u64)
                .or(Some(4000));
            let react_enrich = args
                .get("react_enrich")
                .or_else(|| args.get("reactEnrich"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let follow_portals = args
                .get("follow_portals")
                .or_else(|| args.get("followPortals"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let shadow_dom = args
                .get("shadow_dom")
                .or_else(|| args.get("shadowDom"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let wid = window_id(args);
            handlers::dom_snapshot(
                id,
                &mode,
                selector.as_deref(),
                max_depth,
                max_elements,
                max_tokens,
                react_enrich,
                follow_portals,
                shadow_dom,
                &wid,
                bridge,
                state,
            )
            .await
        }

        "get_cached_dom" => {
            let wid = window_id(args);
            handlers::get_cached_dom(id, &wid, state).await
        }

        "webview_find_element" => {
            let selector = str_arg(args, "selector").unwrap_or_default();
            let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
            let target = str_arg(args, "target");
            let wid = window_id(args);
            handlers::find_element(id, &selector, &strategy, target.as_deref(), &wid, bridge).await
        }

        "webview_get_styles" => {
            let selector = str_arg(args, "selector").unwrap_or_default();
            let properties: Option<Vec<String>> = args
                .get("properties")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let wid = window_id(args);
            handlers::get_styles(id, &selector, properties.as_deref(), &wid, bridge, state).await
        }

        "webview_interact" => {
            let action = str_arg(args, "action").unwrap_or_default();
            let selector = str_arg(args, "selector");
            let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
            let x = num_arg(args, "x");
            let y = num_arg(args, "y");
            let wid = window_id(args);

            if action == "drag" {
                let target_selector = str_arg(args, "targetSelector");
                let target_x = num_arg(args, "targetX");
                let target_y = num_arg(args, "targetY");
                let steps = num_arg(args, "steps").unwrap_or(10.0) as u32;
                let duration_ms = num_arg(args, "durationMs").unwrap_or(300.0) as u32;
                let drag_strategy =
                    str_arg(args, "dragStrategy").unwrap_or_else(|| "auto".to_string());
                handlers::drag(
                    id,
                    selector.as_deref(),
                    &strategy,
                    x,
                    y,
                    target_selector.as_deref(),
                    target_x,
                    target_y,
                    steps,
                    duration_ms,
                    &drag_strategy,
                    &wid,
                    bridge,
                    state,
                )
                .await
            } else {
                let direction = str_arg(args, "direction");
                let distance = num_arg(args, "distance");
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
                    state,
                )
                .await
            }
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
            handlers::wait_for(
                id,
                selector.as_deref(),
                &strategy,
                text.as_deref(),
                timeout,
                &wid,
                bridge,
                state,
            )
            .await
        }

        "webview_get_pointed_element" => handlers::get_pointed_element(id, state).await,

        "webview_select_element" => Response::error(
            id.to_string(),
            "Select element (visual picker) not yet implemented",
        ),

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

        "ipc_get_backend_state" => handlers::backend_state(id, app).await,

        "ipc_execute_command" => {
            let command = str_arg(args, "command").unwrap_or_default();
            let cmd_args = args.get("args").cloned();
            handlers::ipc_execute_command(id, &command, cmd_args.as_ref(), "main", bridge).await
        }

        "ipc_monitor" => {
            let action = str_arg(args, "action").unwrap_or_default();
            handlers::ipc_monitor(id, &action, state, bridge).await
        }

        "ipc_get_captured" => {
            let filter = str_arg(args, "filter");
            let pattern = str_arg(args, "pattern");
            let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
            let since = num_arg(args, "since").map(|n| n as u64);
            handlers::ipc_get_captured(
                id,
                filter.as_deref(),
                pattern.as_deref(),
                limit,
                since,
                state,
            )
            .await
        }

        "ipc_emit_event" => {
            let event_name = str_arg(args, "eventName").unwrap_or_default();
            let payload = args.get("payload").cloned();
            handlers::ipc_emit_event(id, &event_name, payload.as_ref(), app).await
        }

        "read_logs" => {
            let lines = num_arg(args, "lines").unwrap_or(50.0) as usize;
            let filter = str_arg(args, "filter");
            let pattern = str_arg(args, "pattern");
            let level = str_arg(args, "level");
            let wid = window_id(args);
            handlers::console_logs(
                id,
                lines,
                filter.as_deref(),
                pattern.as_deref(),
                level.as_deref(),
                &wid,
                state,
            )
            .await
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

        "clear_logs" => {
            let source = str_arg(args, "source").unwrap_or_else(|| "all".to_string());
            handlers::clear_logs(id, &source, state).await
        }

        "read_log_file" => {
            let source = str_arg(args, "source").unwrap_or_else(|| "console".to_string());
            let lines = num_arg(args, "lines").unwrap_or(100.0) as usize;
            let level = str_arg(args, "level");
            let pattern = str_arg(args, "pattern");
            let since = num_arg(args, "since").map(|n| n as u64);
            let wid = str_arg(args, "windowId");
            handlers::read_log_file(
                id,
                &source,
                lines,
                level.as_deref(),
                pattern.as_deref(),
                since,
                wid.as_deref(),
                state,
            )
            .await
        }

        "ipc_listen" => {
            let action = str_arg(args, "action").unwrap_or_default();
            let events: Option<Vec<String>> = args
                .get("events")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            handlers::ipc_listen(id, &action, events.as_deref(), state, bridge).await
        }

        "event_get_captured" => {
            let event = str_arg(args, "event");
            let pattern = str_arg(args, "pattern");
            let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
            let since = num_arg(args, "since").map(|n| n as u64);
            handlers::event_get_captured(
                id,
                event.as_deref(),
                pattern.as_deref(),
                limit,
                since,
                state,
            )
            .await
        }

        "runtime_get_captured" => {
            let kind = str_arg(args, "kind");
            let level = str_arg(args, "level");
            let pattern = str_arg(args, "pattern");
            let since = num_arg(args, "since").map(|n| n as u64);
            let since_mark = str_arg(args, "sinceMark");
            let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
            let wid = str_arg(args, "windowId");
            handlers::runtime_get_captured(
                id,
                kind.as_deref(),
                level.as_deref(),
                pattern.as_deref(),
                since,
                since_mark.as_deref(),
                limit,
                wid.as_deref(),
                state,
            )
            .await
        }

        "runtime_clear" => handlers::clear_logs(id, "runtime", state).await,

        "artifact_list" => {
            let kind = str_arg(args, "kind");
            let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
            handlers::artifact_list(id, kind.as_deref(), limit, state).await
        }

        "artifact_read" => {
            let artifact = str_arg(args, "artifact")
                .or_else(|| str_arg(args, "artifactId"))
                .unwrap_or_default();
            handlers::artifact_read(id, &artifact, state).await
        }

        "artifact_compare" => {
            let before = str_arg(args, "before").unwrap_or_default();
            let after = str_arg(args, "after").unwrap_or_default();
            let threshold = num_arg(args, "threshold").unwrap_or(0.0);
            handlers::artifact_compare(id, &before, &after, threshold, state).await
        }

        "artifact_prune" => {
            let keep = num_arg(args, "keep").unwrap_or(50.0) as usize;
            let kind = str_arg(args, "kind");
            let delete_files = args
                .get("deleteFiles")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            handlers::artifact_prune(id, keep, kind.as_deref(), delete_files, state).await
        }

        "debug_mark" => {
            let label = str_arg(args, "label");
            handlers::debug_mark(id, label.as_deref(), state).await
        }

        "debug_snapshot" => {
            let wid = window_id(args);
            let since = num_arg(args, "since").map(|n| n as u64);
            let since_mark = str_arg(args, "sinceMark");
            let max_tokens = num_arg(args, "maxTokens").map(|n| n as u64);
            let screenshot_name_hint = str_arg(args, "screenshotNameHint");
            handlers::debug_snapshot(
                id,
                &wid,
                args.get("includeDom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                args.get("includeScreenshot")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("includeLogs")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                args.get("includeIpc")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("includeEvents")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("includeRuntime")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                since,
                since_mark.as_deref(),
                max_tokens,
                screenshot_name_hint.as_deref(),
                bridge,
                app,
                state,
            )
            .await
        }

        "webview_act_and_verify" => {
            let action = str_arg(args, "action").unwrap_or_default();
            let selector = str_arg(args, "selector");
            let text = str_arg(args, "text");
            let key = str_arg(args, "key");
            let target_selector = str_arg(args, "targetSelector");
            let wait_for_selector = str_arg(args, "waitForSelector");
            let wait_for_text = str_arg(args, "waitForText");
            let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
            let wid = window_id(args);
            handlers::webview_act_and_verify(
                id,
                &action,
                selector.as_deref(),
                text.as_deref(),
                key.as_deref(),
                target_selector.as_deref(),
                wait_for_selector.as_deref(),
                wait_for_text.as_deref(),
                timeout,
                args.get("verifyDom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("verifyScreenshot")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("includeLogs")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                args.get("includeIpc")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                args.get("includeRuntime")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                &wid,
                bridge,
                app,
                state,
            )
            .await
        }

        "webview_search_snapshot" => {
            let pattern = str_arg(args, "pattern").unwrap_or_default();
            let context = num_arg(args, "context").unwrap_or(2.0) as usize;
            let mode = str_arg(args, "mode").unwrap_or_else(|| "ai".to_string());
            let wid = window_id(args);
            handlers::search_snapshot(id, &pattern, context, &mode, &wid, state, bridge).await
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
            tool_def("bridge_status",
                "Show internal JS bridge clients, pending evals, and fallback availability",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("webview_screenshot",
                "Take a screenshot of the Tauri window using native xcap capture (cross-platform)",
                json!({ "type": "object", "properties": {
                    "format": { "type": "string", "enum": ["png", "jpeg", "webp"] },
                    "quality": { "type": "number", "minimum": 0, "maximum": 100 },
                    "maxWidth": { "type": "number" },
                    "save": { "type": "boolean" },
                    "outputDir": { "type": "string" },
                    "nameHint": { "type": "string" },
                    "overwrite": { "type": "boolean" },
                    "selector": { "type": "string", "description": "CSS selector or @ref for future element captures" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("webview_dom_snapshot",
                "Get structured DOM snapshot. Mode 'ai' (default) includes ref IDs for interaction, React component names, and stitches portals. Mode 'accessibility' shows ARIA roles/names. Mode 'structure' shows tags/classes.",
                json!({ "type": "object", "properties": {
                    "mode": { "type": "string", "enum": ["ai", "accessibility", "structure"], "default": "ai" },
                    "selector": { "type": "string", "description": "CSS selector to scope snapshot to a subtree" },
                    "maxDepth": { "type": "number", "description": "Maximum tree depth (0 = unlimited)" },
                    "maxElements": { "type": "number", "description": "Maximum elements to include (0 = unlimited)" },
                    "reactEnrich": { "type": "boolean", "description": "Include React component names (default: true)" },
                    "followPortals": { "type": "boolean", "description": "Stitch portals to their triggers (default: true)" },
                    "shadowDom": { "type": "boolean", "description": "Traverse shadow DOM (default: false)" },
                    "maxTokens": { "type": "number", "description": "Token budget for inline result (default 4000, 0 for unlimited)" },
                    "noSplit": { "type": "boolean", "description": "Disable snapshot subtree splitting and return the full inline snapshot" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("get_cached_dom",
                "Get cached DOM snapshot pushed from frontend. Faster and more LLM-friendly.",
                json!({ "type": "object", "properties": { "windowId": { "type": "string" } } })
            ),
            tool_def("webview_find_element",
                "Find elements by CSS, XPath, text, or regex pattern",
                json!({ "type": "object", "properties": {
                    "selector": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text", "regex"] },
                    "target": { "type": "string", "enum": ["text", "class", "id", "attr", "all"], "description": "What regex matches against (regex strategy only)" },
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
                "Perform gestures on elements. drag simulates drag-and-drop (pointer-based or HTML5 DnD with auto-detection); hover fires full pointer+mouse event sequence; hover-off fires leave events to dismiss dropdowns/tooltips",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["click", "double-click", "dblclick", "focus", "scroll", "hover", "hover-off", "drag"] },
                    "selector": { "type": "string", "description": "Source element (CSS selector, XPath, or text)" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                    "x": { "type": "number", "description": "Source x coordinate (alternative to selector)" },
                    "y": { "type": "number", "description": "Source y coordinate (alternative to selector)" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "distance": { "type": "number" },
                    "targetSelector": { "type": "string", "description": "Drag target element (CSS selector). Required for drag." },
                    "targetX": { "type": "number", "description": "Drag target x coordinate (alternative to targetSelector)" },
                    "targetY": { "type": "number", "description": "Drag target y coordinate (alternative to targetSelector)" },
                    "steps": { "type": "number", "description": "Number of intermediate move events for drag (default 10)" },
                    "durationMs": { "type": "number", "description": "Total drag duration in ms (default 300)" },
                    "dragStrategy": { "type": "string", "enum": ["auto", "pointer", "html5dnd"], "description": "Drag strategy: auto detects from element draggable attribute (default auto)" },
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
                "Retrieve captured IPC traffic. Supports regex pattern and timestamp filtering.",
                json!({ "type": "object", "properties": {
                    "filter": { "type": "string", "description": "Substring match on command name" },
                    "pattern": { "type": "string", "description": "Regex on full entry (overrides filter)" },
                    "limit": { "type": "number" },
                    "since": { "type": "number", "description": "Only entries after this epoch ms" }
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
                "Read console logs. Supports level filtering (error,warn) and regex patterns on messages.",
                json!({ "type": "object", "properties": {
                    "lines": { "type": "number", "description": "Max entries to return (default 50)" },
                    "filter": { "type": "string", "description": "Substring match on message (backward compat)" },
                    "pattern": { "type": "string", "description": "Regex pattern on message (overrides filter)" },
                    "level": { "type": "string", "description": "Filter by level: error, warn, info, log, debug. Comma-separated." },
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
            tool_def("clear_logs",
                "Clear log files. Specify source: console, ipc, events, runtime, or all.",
                json!({ "type": "object", "properties": {
                    "source": { "type": "string", "enum": ["console", "ipc", "events", "runtime", "all"], "default": "all" }
                } })
            ),
            tool_def("read_log_file",
                "Read historical log files (persisted across app restarts). Supports regex and timestamp filtering.",
                json!({ "type": "object", "properties": {
                    "source": { "type": "string", "enum": ["console", "ipc", "events", "runtime"] },
                    "lines": { "type": "number", "description": "Max entries from tail (default 100)" },
                    "level": { "type": "string", "description": "Level filter (console only)" },
                    "pattern": { "type": "string", "description": "Regex on serialized entry" },
                    "since": { "type": "number", "description": "Epoch ms floor" },
                    "windowId": { "type": "string" }
                }, "required": ["source"] })
            ),
            tool_def("ipc_listen",
                "Listen for Tauri events. Start captures events to events.log, stop removes all listeners.",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] },
                    "events": { "type": "array", "items": { "type": "string" }, "description": "Event names to listen for" }
                }, "required": ["action"] })
            ),
            tool_def("event_get_captured",
                "Retrieve captured Tauri events from events.log. Filter by event name, regex, or timestamp.",
                json!({ "type": "object", "properties": {
                    "event": { "type": "string", "description": "Filter by event name (exact)" },
                    "pattern": { "type": "string", "description": "Regex on full entry" },
                    "limit": { "type": "number" },
                    "since": { "type": "number", "description": "Epoch ms floor" }
                } })
            ),
            tool_def("runtime_get_captured",
                "Retrieve captured frontend runtime failures: window errors, unhandled rejections, network failures, navigation, and resource errors.",
                json!({ "type": "object", "properties": {
                    "kind": { "type": "string", "description": "Filter by kind: window_error, unhandledrejection, network, navigation, resource_error" },
                    "level": { "type": "string", "description": "Filter by level: error, warn, info. Comma-separated." },
                    "pattern": { "type": "string", "description": "Regex on full entry" },
                    "since": { "type": "number", "description": "Epoch ms floor" },
                    "sinceMark": { "type": "string" },
                    "limit": { "type": "number" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("runtime_clear",
                "Clear captured frontend runtime entries.",
                json!({ "type": "object", "properties": {} })
            ),
            tool_def("artifact_list",
                "List connector artifacts from the manifest registry.",
                json!({ "type": "object", "properties": {
                    "kind": { "type": "string" },
                    "limit": { "type": "number" }
                } })
            ),
            tool_def("artifact_read",
                "Read an artifact by artifactId or path.",
                json!({ "type": "object", "properties": {
                    "artifact": { "type": "string" },
                    "artifactId": { "type": "string" }
                } })
            ),
            tool_def("artifact_compare",
                "Compare two screenshot artifacts or paths. Refuses same-path comparisons.",
                json!({ "type": "object", "properties": {
                    "before": { "type": "string" },
                    "after": { "type": "string" },
                    "threshold": { "type": "number" }
                }, "required": ["before", "after"] })
            ),
            tool_def("artifact_prune",
                "Prune old artifact manifest entries and optionally delete artifact files.",
                json!({ "type": "object", "properties": {
                    "keep": { "type": "number" },
                    "kind": { "type": "string" },
                    "deleteFiles": { "type": "boolean" }
                } })
            ),
            tool_def("debug_mark",
                "Create a timestamp mark for later log/ipc/event/runtime filtering.",
                json!({ "type": "object", "properties": {
                    "label": { "type": "string" }
                } })
            ),
            tool_def("debug_snapshot",
                "Collect a bundled debug context: bridge/app state, DOM, screenshot, logs, IPC, events, and runtime captures.",
                json!({ "type": "object", "properties": {
                    "windowId": { "type": "string" },
                    "includeDom": { "type": "boolean" },
                    "includeScreenshot": { "type": "boolean" },
                    "includeLogs": { "type": "boolean" },
                    "includeIpc": { "type": "boolean" },
                    "includeEvents": { "type": "boolean" },
                    "includeRuntime": { "type": "boolean" },
                    "since": { "type": "number" },
                    "sinceMark": { "type": "string" },
                    "maxTokens": { "type": "number" },
                    "screenshotNameHint": { "type": "string" }
                } })
            ),
            tool_def("webview_act_and_verify",
                "Perform an action, wait for text/selector, and collect fresh debug evidence in one call.",
                json!({ "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["click", "fill", "type", "press", "drag", "hover"] },
                    "selector": { "type": "string" },
                    "text": { "type": "string" },
                    "key": { "type": "string" },
                    "targetSelector": { "type": "string" },
                    "waitForSelector": { "type": "string" },
                    "waitForText": { "type": "string" },
                    "timeout": { "type": "number" },
                    "verifyDom": { "type": "boolean" },
                    "verifyScreenshot": { "type": "boolean" },
                    "includeLogs": { "type": "boolean" },
                    "includeIpc": { "type": "boolean" },
                    "includeRuntime": { "type": "boolean" },
                    "windowId": { "type": "string" }
                }, "required": ["action"] })
            ),
            tool_def("webview_search_snapshot",
                "Search DOM snapshot with regex. Returns matched lines with context. Uses cached snapshot if fresh (<10s).",
                json!({ "type": "object", "properties": {
                    "pattern": { "type": "string" },
                    "context": { "type": "number", "description": "Lines of context (default 2, max 10)" },
                    "mode": { "type": "string", "enum": ["ai", "accessibility", "structure"] },
                    "windowId": { "type": "string" }
                }, "required": ["pattern"] })
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
tauri-plugin-connector = "0.11"
```

### 2. Register the plugin (feature-gated dev tooling)
In `src-tauri/src/lib.rs`, add before `.invoke_handler()`:
```rust
#[cfg(feature = "dev-connector")]
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
      "url": "http://127.0.0.1:9556/mcp"
    }
  }
}
```

The MCP server starts automatically when the Tauri app runs. No separate command needed.
"#;

#[cfg(test)]
mod tests {
    use super::*;

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
    fn runtime_schema_supports_mark_and_window_filters() {
        let tool = tool("runtime_get_captured");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("sinceMark").is_some());
        assert!(props.get("windowId").is_some());
    }

    #[test]
    fn artifact_schema_exposes_prune() {
        let tool = tool("artifact_prune");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("keep").is_some());
        assert!(props.get("deleteFiles").is_some());
    }
}
