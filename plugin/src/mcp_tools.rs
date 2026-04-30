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
            let annotate = args
                .get("annotate")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
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
                annotate,
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
            let url = str_arg(args, "url");
            let load_state = str_arg(args, "loadState").or_else(|| str_arg(args, "load_state"));
            let function = str_arg(args, "fn")
                .or_else(|| str_arg(args, "function"))
                .or_else(|| str_arg(args, "condition"));
            let selector_state = str_arg(args, "state");
            let timeout = num_arg(args, "timeout").unwrap_or(5000.0) as u64;
            let wid = window_id(args);
            handlers::wait_for(
                id,
                selector.as_deref(),
                &strategy,
                text.as_deref(),
                url.as_deref(),
                load_state.as_deref(),
                function.as_deref(),
                selector_state.as_deref(),
                timeout,
                &wid,
                bridge,
                state,
            )
            .await
        }

        "webview_locator" => {
            let wid = window_id(args);
            handlers::locator(
                id,
                str_arg(args, "role").as_deref(),
                str_arg(args, "text").as_deref(),
                str_arg(args, "label").as_deref(),
                str_arg(args, "placeholder").as_deref(),
                str_arg(args, "alt").as_deref(),
                str_arg(args, "title").as_deref(),
                str_arg(args, "testId")
                    .or_else(|| str_arg(args, "testid"))
                    .or_else(|| str_arg(args, "test_id"))
                    .as_deref(),
                str_arg(args, "name").as_deref(),
                args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false),
                args.get("first").and_then(|v| v.as_bool()).unwrap_or(false),
                args.get("last").and_then(|v| v.as_bool()).unwrap_or(false),
                num_arg(args, "nth").map(|n| n as usize),
                str_arg(args, "action").as_deref(),
                str_arg(args, "value").as_deref(),
                &wid,
                bridge,
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

pub use crate::mcp_tool_schema::tool_definitions;

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

    #[test]
    fn screenshot_wait_and_locator_schema_expose_agent_oriented_args() {
        let screenshot = tool("webview_screenshot");
        assert!(
            screenshot["inputSchema"]["properties"]
                .get("annotate")
                .is_some()
        );

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
}
