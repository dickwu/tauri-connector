//! Command handlers for all supported operations.

use tauri::{Emitter, Manager};

use crate::bridge::Bridge;
use crate::protocol::{AppInfo, BackendState, EnvInfo, Response, TauriInfo, WindowEntry};
use crate::state::PluginState;

// ============ JavaScript Execution ============

pub async fn execute_js(
    id: &str,
    script: &str,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    match bridge.execute_js(script, 30_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), e),
    }
}

// ============ DOM Snapshot (via bridge JS) ============

pub async fn dom_snapshot(
    id: &str,
    snapshot_type: &str,
    selector: Option<&str>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let selector_arg = match selector {
        Some(s) => format!("'{}'", s.replace('\'', "\\'")),
        None => "null".to_string(),
    };

    let script = format!(
        "window.__CONNECTOR_DOM_SNAPSHOT__('{}', {})",
        snapshot_type, selector_arg
    );

    match bridge.execute_js(&script, 15_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("DOM snapshot failed: {e}")),
    }
}

// ============ Cached DOM (pushed from frontend via invoke) ============

pub async fn get_cached_dom(
    id: &str,
    window_id: &str,
    state: &PluginState,
) -> Response {
    match state.get_dom(window_id).await {
        Some(entry) => Response::success(
            id.to_string(),
            serde_json::json!({
                "window_id": entry.window_id,
                "html": entry.html,
                "text_content": entry.text_content,
                "accessibility_tree": entry.accessibility_tree,
                "structure_tree": entry.structure_tree,
                "timestamp": entry.timestamp,
            }),
        ),
        None => Response::error(
            id.to_string(),
            format!("No cached DOM for window '{window_id}'. Call connector_push_dom from your frontend first."),
        ),
    }
}

// ============ Element Operations ============

pub async fn find_element(
    id: &str,
    selector: &str,
    strategy: &str,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let script = match strategy {
        "xpath" => format!(
            r#"(() => {{
                const result = document.evaluate("{sel}", document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
                const elements = [];
                for (let i = 0; i < result.snapshotLength; i++) {{
                    const el = result.snapshotItem(i);
                    const rect = el.getBoundingClientRect ? el.getBoundingClientRect() : {{}};
                    elements.push({{
                        tag: el.tagName?.toLowerCase(),
                        id: el.id || null,
                        className: el.className || null,
                        text: el.textContent?.trim().substring(0, 200),
                        rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
                        visible: rect.width > 0 && rect.height > 0
                    }});
                }}
                return {{ count: elements.length, elements }};
            }})()"#,
            sel = selector.replace('"', r#"\""#)
        ),
        "text" => format!(
            r#"(() => {{
                const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
                const elements = [];
                while (walker.nextNode()) {{
                    if (walker.currentNode.textContent.includes("{text}")) {{
                        const el = walker.currentNode.parentElement;
                        if (!el) continue;
                        const rect = el.getBoundingClientRect();
                        elements.push({{
                            tag: el.tagName.toLowerCase(),
                            id: el.id || null,
                            className: el.className || null,
                            text: el.textContent.trim().substring(0, 200),
                            rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
                            visible: rect.width > 0 && rect.height > 0
                        }});
                    }}
                }}
                return {{ count: elements.length, elements }};
            }})()"#,
            text = selector.replace('"', r#"\""#)
        ),
        _ => format!(
            r#"(() => {{
                const els = document.querySelectorAll("{sel}");
                const elements = [];
                els.forEach(el => {{
                    const rect = el.getBoundingClientRect();
                    elements.push({{
                        tag: el.tagName.toLowerCase(),
                        id: el.id || null,
                        className: el.className || null,
                        text: el.textContent?.trim().substring(0, 200),
                        rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
                        visible: rect.width > 0 && rect.height > 0
                    }});
                }});
                return {{ count: elements.length, elements }};
            }})()"#,
            sel = selector.replace('"', r#"\""#)
        ),
    };

    match bridge.execute_js(&script, 10_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Find element failed: {e}")),
    }
}

pub async fn get_styles(
    id: &str,
    selector: &str,
    properties: Option<&[String]>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let props_filter = match properties {
        Some(props) => {
            let list: Vec<String> = props.iter().map(|p| format!("'{p}'")).collect();
            format!("const filter = [{}];", list.join(","))
        }
        None => "const filter = null;".to_string(),
    };

    let script = format!(
        r#"(() => {{
            const el = document.querySelector("{sel}");
            if (!el) return {{ error: "Element not found: {sel}" }};
            const computed = getComputedStyle(el);
            {props_filter}
            const styles = {{}};
            if (filter) {{
                filter.forEach(p => {{ styles[p] = computed.getPropertyValue(p); }});
            }} else {{
                for (let i = 0; i < computed.length; i++) {{
                    const prop = computed[i];
                    styles[prop] = computed.getPropertyValue(prop);
                }}
            }}
            return {{ selector: "{sel}", styles }};
        }})()"#,
        sel = selector.replace('"', r#"\""#),
        props_filter = props_filter,
    );

    match bridge.execute_js(&script, 5_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Get styles failed: {e}")),
    }
}

pub async fn get_pointed_element(
    id: &str,
    state: &PluginState,
) -> Response {
    match state.take_pointed_element().await {
        Some(element) => Response::success(id.to_string(), element),
        None => Response::error(
            id.to_string(),
            "No pointed element. User must Alt+Shift+Click an element first.",
        ),
    }
}

// ============ Interaction ============

pub async fn interact(
    id: &str,
    action: &str,
    selector: Option<&str>,
    strategy: &str,
    x: Option<f64>,
    y: Option<f64>,
    direction: Option<&str>,
    distance: Option<f64>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let target = match (selector, x, y) {
        (Some(sel), _, _) => {
            let query = match strategy {
                "xpath" => format!(
                    "document.evaluate(\"{}\", document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue",
                    sel.replace('"', r#"\""#)
                ),
                "text" => format!(
                    "Array.from(document.querySelectorAll('*')).find(el => el.textContent.includes(\"{}\"))",
                    sel.replace('"', r#"\""#)
                ),
                _ => format!("document.querySelector(\"{}\")", sel.replace('"', r#"\""#)),
            };
            format!("const el = {query}; if (!el) return {{ error: 'Element not found' }};")
        }
        (None, Some(px), Some(py)) => {
            format!("const el = document.elementFromPoint({px}, {py}); if (!el) return {{ error: 'No element at ({px}, {py})' }};")
        }
        _ => {
            return Response::error(id.to_string(), "Provide selector or x/y coordinates");
        }
    };

    let action_code = match action {
        "click" => "el.click();".to_string(),
        "double-click" | "dblclick" => {
            "el.dispatchEvent(new MouseEvent('dblclick', {bubbles: true}));".to_string()
        }
        "focus" => "el.focus();".to_string(),
        "scroll" => {
            let dir = direction.unwrap_or("down");
            let dist = distance.unwrap_or(300.0);
            match dir {
                "up" => format!("el.scrollBy(0, -{dist});"),
                "down" => format!("el.scrollBy(0, {dist});"),
                "left" => format!("el.scrollBy(-{dist}, 0);"),
                "right" => format!("el.scrollBy({dist}, 0);"),
                _ => format!("el.scrollBy(0, {dist});"),
            }
        }
        "hover" => {
            "el.dispatchEvent(new MouseEvent('mouseenter', {bubbles: true})); el.dispatchEvent(new MouseEvent('mouseover', {bubbles: true}));".to_string()
        }
        _ => {
            return Response::error(
                id.to_string(),
                format!("Unknown action: {action}. Use: click, double-click, focus, scroll, hover"),
            );
        }
    };

    let script = format!(
        r#"(() => {{
            {target}
            {action_code}
            const rect = el.getBoundingClientRect();
            return {{ action: "{action}", tag: el.tagName.toLowerCase(), rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }} }};
        }})()"#
    );

    match bridge.execute_js(&script, 5_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Interact failed: {e}")),
    }
}

pub async fn keyboard(
    id: &str,
    action: &str,
    text: Option<&str>,
    key: Option<&str>,
    modifiers: Option<&[String]>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let mod_opts = modifiers
        .map(|mods| {
            mods.iter()
                .map(|m| match m.to_lowercase().as_str() {
                    "ctrl" | "control" => "ctrlKey: true",
                    "shift" => "shiftKey: true",
                    "alt" => "altKey: true",
                    "meta" | "cmd" => "metaKey: true",
                    _ => "",
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let script = match action {
        "type" => {
            let t = text.unwrap_or("");
            format!(
                r#"(() => {{
                    const el = document.activeElement;
                    if (!el) return {{ error: 'No focused element' }};
                    const chars = "{text}";
                    for (const ch of chars) {{
                        el.dispatchEvent(new KeyboardEvent('keydown', {{ key: ch, {mods} }}));
                        el.dispatchEvent(new KeyboardEvent('keypress', {{ key: ch, {mods} }}));
                        if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                            el.value += ch;
                            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        }}
                        el.dispatchEvent(new KeyboardEvent('keyup', {{ key: ch, {mods} }}));
                    }}
                    return {{ typed: "{text}", target: el.tagName.toLowerCase() }};
                }})()"#,
                text = t.replace('"', r#"\""#),
                mods = mod_opts,
            )
        }
        "press" => {
            let k = key.unwrap_or("Enter");
            format!(
                r#"(() => {{
                    const el = document.activeElement || document.body;
                    el.dispatchEvent(new KeyboardEvent('keydown', {{ key: '{key}', {mods}, bubbles: true }}));
                    el.dispatchEvent(new KeyboardEvent('keyup', {{ key: '{key}', {mods}, bubbles: true }}));
                    return {{ pressed: '{key}', target: el.tagName.toLowerCase() }};
                }})()"#,
                key = k,
                mods = mod_opts,
            )
        }
        _ => {
            return Response::error(
                id.to_string(),
                format!("Unknown keyboard action: {action}. Use: type, press"),
            );
        }
    };

    match bridge.execute_js(&script, 5_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Keyboard failed: {e}")),
    }
}

pub async fn wait_for(
    id: &str,
    selector: Option<&str>,
    strategy: &str,
    text: Option<&str>,
    timeout: u64,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let condition = if let Some(sel) = selector {
        match strategy {
            "xpath" => format!(
                "document.evaluate(\"{}\", document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue",
                sel.replace('"', r#"\""#)
            ),
            "text" => format!(
                "Array.from(document.querySelectorAll('*')).find(el => el.textContent.includes(\"{}\"))",
                sel.replace('"', r#"\""#)
            ),
            _ => format!("document.querySelector(\"{}\")", sel.replace('"', r#"\""#)),
        }
    } else if let Some(t) = text {
        format!(
            "document.body.innerText.includes(\"{}\")",
            t.replace('"', r#"\""#)
        )
    } else {
        return Response::error(id.to_string(), "Provide selector or text to wait for");
    };

    let script = format!(
        r#"new Promise((resolve) => {{
            const start = Date.now();
            const check = () => {{
                const found = {condition};
                if (found) {{
                    resolve({{ found: true, elapsed_ms: Date.now() - start }});
                }} else if (Date.now() - start > {timeout}) {{
                    resolve({{ found: false, timeout: true, elapsed_ms: Date.now() - start }});
                }} else {{
                    setTimeout(check, 100);
                }}
            }};
            check();
        }})"#
    );

    match bridge.execute_js(&script, timeout + 2000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Wait failed: {e}")),
    }
}

// ============ Window Management ============

pub async fn window_list(id: &str, app: Option<&tauri::AppHandle>) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let windows: Vec<serde_json::Value> = app
        .webview_windows()
        .iter()
        .map(|(label, window)| {
            serde_json::json!({
                "label": label,
                "title": window.title().unwrap_or_default(),
                "visible": window.is_visible().unwrap_or(false),
                "focused": window.is_focused().unwrap_or(false),
                "url": window.url().map(|u| u.to_string()).unwrap_or_default(),
            })
        })
        .collect();

    Response::success(
        id.to_string(),
        serde_json::json!({
            "windows": windows,
            "totalCount": windows.len(),
        }),
    )
}

pub async fn window_info(
    id: &str,
    window_id: &str,
    app: Option<&tauri::AppHandle>,
) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let Some(window) = app.get_webview_window(window_id) else {
        return Response::error(id.to_string(), format!("Window '{window_id}' not found"));
    };

    let size = window.inner_size().map(|s| (s.width, s.height)).unwrap_or((0, 0));
    let position = window.inner_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));

    Response::success(
        id.to_string(),
        serde_json::json!({
            "label": window_id,
            "title": window.title().unwrap_or_default(),
            "width": size.0,
            "height": size.1,
            "x": position.0,
            "y": position.1,
            "visible": window.is_visible().unwrap_or(false),
            "focused": window.is_focused().unwrap_or(false),
            "minimized": window.is_minimized().unwrap_or(false),
            "maximized": window.is_maximized().unwrap_or(false),
            "fullscreen": window.is_fullscreen().unwrap_or(false),
        }),
    )
}

pub async fn window_resize(
    id: &str,
    window_id: &str,
    width: u32,
    height: u32,
    app: Option<&tauri::AppHandle>,
) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let Some(window) = app.get_webview_window(window_id) else {
        return Response::error(id.to_string(), format!("Window '{window_id}' not found"));
    };

    let size = tauri::LogicalSize::new(width, height);
    match window.set_size(size) {
        Ok(()) => Response::success(
            id.to_string(),
            serde_json::json!({ "resized": true, "width": width, "height": height }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Resize failed: {e}")),
    }
}

// ============ IPC Operations ============

pub async fn backend_state(id: &str, app: Option<&tauri::AppHandle>) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let config = app.config();
    let windows: Vec<WindowEntry> = app
        .webview_windows()
        .iter()
        .map(|(label, window)| WindowEntry {
            label: label.clone(),
            title: window.title().unwrap_or_default(),
            visible: window.is_visible().unwrap_or(false),
            focused: window.is_focused().unwrap_or(false),
        })
        .collect();

    let state = BackendState {
        app: AppInfo {
            name: config.product_name.clone().unwrap_or_default(),
            identifier: config.identifier.clone(),
            version: config.version.clone().unwrap_or_default(),
        },
        tauri: TauriInfo {
            version: tauri::VERSION.to_string(),
        },
        environment: EnvInfo {
            debug: cfg!(debug_assertions),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        windows,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    };

    match serde_json::to_value(state) {
        Ok(v) => Response::success(id.to_string(), v),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}

pub async fn ipc_execute_command(
    id: &str,
    command: &str,
    args: Option<&serde_json::Value>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    let args_json = match args {
        Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()),
        None => "{}".to_string(),
    };

    let script = format!(
        r#"(async () => {{
            if (window.__TAURI_INTERNALS__) {{
                return await window.__TAURI_INTERNALS__.invoke("{cmd}", {args});
            }} else if (window.__TAURI__) {{
                return await window.__TAURI__.core.invoke("{cmd}", {args});
            }} else {{
                return {{ error: "Tauri IPC not available" }};
            }}
        }})()"#,
        cmd = command.replace('"', r#"\""#),
        args = args_json,
    );

    match bridge.execute_js(&script, 15_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("IPC command failed: {e}")),
    }
}

pub async fn ipc_monitor(
    id: &str,
    action: &str,
    state: &PluginState,
) -> Response {
    match action {
        "start" => {
            state.set_ipc_monitoring(true).await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": true }))
        }
        "stop" => {
            state.set_ipc_monitoring(false).await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": false }))
        }
        _ => Response::error(id.to_string(), format!("Unknown action: {action}. Use: start, stop")),
    }
}

pub async fn ipc_get_captured(
    id: &str,
    filter: Option<&str>,
    limit: usize,
    state: &PluginState,
) -> Response {
    let events = state.get_ipc_events(filter, limit).await;
    match serde_json::to_value(&events) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": events.len(), "events": v }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}

pub async fn ipc_emit_event(
    id: &str,
    event_name: &str,
    payload: Option<&serde_json::Value>,
    app: Option<&tauri::AppHandle>,
) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let payload_value = payload.cloned().unwrap_or(serde_json::Value::Null);
    match app.emit(event_name, payload_value) {
        Ok(()) => Response::success(
            id.to_string(),
            serde_json::json!({ "emitted": event_name }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Emit failed: {e}")),
    }
}

// ============ Console Logs ============

pub async fn console_logs(
    id: &str,
    lines: usize,
    filter: Option<&str>,
    _window_id: &str,
    state: &PluginState,
) -> Response {
    let logs = state.get_logs(lines, filter).await;
    match serde_json::to_value(&logs) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": logs.len(), "logs": v }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}

// ============ Screenshot (placeholder - needs platform impl) ============

pub async fn screenshot(
    id: &str,
    format: &str,
    quality: u8,
    _max_width: Option<u32>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
    // Use canvas-based screenshot via JS bridge
    let mime = if format == "png" { "image/png" } else { "image/jpeg" };
    let quality_f = f64::from(quality) / 100.0;

    let script = format!(
        r#"(async () => {{
            const canvas = document.createElement('canvas');
            const rect = document.documentElement.getBoundingClientRect();
            canvas.width = window.innerWidth;
            canvas.height = window.innerHeight;
            // Note: html2canvas or similar library needed for full rendering
            // This is a basic fallback
            return {{
                width: window.innerWidth,
                height: window.innerHeight,
                note: "Canvas screenshot requires html2canvas. Use platform screenshot API for full fidelity."
            }};
        }})()"#
    );
    let _ = (mime, quality_f);

    match bridge.execute_js(&script, 10_000).await {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Screenshot failed: {e}")),
    }
}
