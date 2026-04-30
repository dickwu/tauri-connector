//! MCP tool schema definitions shared by embedded and standalone MCP tests.

use serde_json::{Value, json};

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
                    "annotate": { "type": "boolean", "description": "Overlay numbered labels for @eN refs from the latest ai snapshot" },
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
                "Wait for element selectors, text, URL glob, load state, or a JavaScript condition",
                json!({ "type": "object", "properties": {
                    "selector": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["css", "xpath", "text"] },
                    "text": { "type": "string" },
                    "url": { "type": "string", "description": "Glob pattern matched against location.href" },
                    "loadState": { "type": "string", "enum": ["domcontentloaded", "load", "networkidle"] },
                    "fn": { "type": "string", "description": "JavaScript expression/function/body that returns truthy" },
                    "state": { "type": "string", "enum": ["attached", "detached", "visible", "hidden"], "description": "Selector state to wait for" },
                    "timeout": { "type": "number" },
                    "windowId": { "type": "string" }
                } })
            ),
            tool_def("webview_locator",
                "Find by semantic locator (role/text/label/placeholder/alt/title/testid/name) and optionally act on the match",
                json!({ "type": "object", "properties": {
                    "role": { "type": "string" },
                    "text": { "type": "string" },
                    "label": { "type": "string" },
                    "placeholder": { "type": "string" },
                    "alt": { "type": "string" },
                    "title": { "type": "string" },
                    "testId": { "type": "string" },
                    "name": { "type": "string", "description": "Accessible-name filter" },
                    "exact": { "type": "boolean" },
                    "first": { "type": "boolean" },
                    "last": { "type": "boolean" },
                    "nth": { "type": "number" },
                    "action": { "type": "string", "enum": ["click", "fill", "type", "hover", "focus", "check", "uncheck", "text"] },
                    "value": { "type": "string", "description": "Value for fill/type" },
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
