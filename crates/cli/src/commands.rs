//! CLI command implementations.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use connector_client::discovery::{ConnectorInstance, ResolvedConnection};
use connector_client::ConnectorClient;
use serde_json::{json, Value};

use crate::snapshot::{build_resolve_and_act_script, js_string, RefMap, SnapshotRefs};

static WINDOW_ID: OnceLock<String> = OnceLock::new();

pub fn set_window_id(window_id: String) {
    let _ = WINDOW_ID.set(window_id);
}

fn window_id() -> &'static str {
    WINDOW_ID.get().map(String::as_str).unwrap_or("main")
}

/// Wire command for `execute_js` with a page-side deadline. The default is
/// omitted so older plugins see an unchanged request.
fn execute_js_command(script: &str, page_timeout_ms: u64) -> Value {
    let mut command = json!({ "type": "execute_js", "script": script, "window_id": window_id() });
    if page_timeout_ms != connector_client::DEFAULT_EVAL_TIMEOUT_MS {
        command["timeout_ms"] = json!(page_timeout_ms);
    }
    command
}

/// Execute JS in the webview via the connector. `page_timeout_ms` is the
/// plugin's deadline; the client waits a little longer so the plugin's typed
/// timeout (with its requestId) arrives instead of a bare client timeout.
async fn exec_js(
    client: &ConnectorClient,
    script: &str,
    page_timeout_ms: u64,
) -> Result<Value, String> {
    client
        .send_with_timeout(
            execute_js_command(script, page_timeout_ms),
            connector_client::eval_client_timeout_ms(Some(page_timeout_ms)),
        )
        .await
}

/// Take a DOM snapshot and return the ref map.
#[allow(clippy::too_many_arguments)]
pub async fn snapshot(
    client: &ConnectorClient,
    interactive: bool,
    compact: bool,
    max_depth: usize,
    max_elements: usize,
    selector: Option<String>,
    mode: Option<String>,
    react_enrich: bool,
    follow_portals: bool,
    max_tokens: usize,
    no_split: bool,
) -> Result<SnapshotRefs, String> {
    let snapshot_mode = mode.unwrap_or_else(|| {
        if interactive {
            "ai".to_string()
        } else {
            "accessibility".to_string()
        }
    });
    let mut cmd = json!({
        "type": "dom_snapshot",
        "mode": snapshot_mode,
        "max_depth": max_depth as u64,
        "max_elements": max_elements as u64,
        "max_tokens": max_tokens as u64,
        "no_split": no_split,
        "react_enrich": react_enrich,
        "follow_portals": follow_portals,
        "window_id": window_id(),
    });
    if let Some(selector) = selector {
        cmd["selector"] = json!(selector);
    }
    let result = client.send_with_timeout(cmd, 30_000).await?;

    let mut snapshot_text = result
        .get("snapshot")
        .and_then(|v| v.as_str())
        .unwrap_or("(no snapshot)")
        .to_string();

    if compact {
        snapshot_text = snapshot_text
            .lines()
            .filter(|line| line.contains("ref=") || line.contains('"') || line.contains("subtree:"))
            .collect::<Vec<_>>()
            .join("\n");
    }

    println!("{snapshot_text}");

    // Print subtree info if split occurred
    if let Some(meta) = result.get("meta") {
        if meta.get("split").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(sid) = meta.get("snapshotId").and_then(|v| v.as_str()) {
                eprintln!("\nSnapshot {sid}");
            }
            if let Some(files) = meta.get("subtreeFiles").and_then(|v| v.as_array()) {
                for f in files {
                    let label = f.get("label").and_then(|v| v.as_str()).unwrap_or("?");
                    let tokens = f
                        .get("estimatedTokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    eprintln!("  {} ({} tokens) -> {}", label, tokens, path);
                }
            }
        }
    }

    let inline_refs: RefMap = result
        .get("refs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let inline_count = inline_refs.len();
    // A split snapshot only inlines the refs it shows; the full map is written
    // next to the subtree files. Cache all of them so refs read from a subtree
    // file work with click/fill too.
    let all_refs = result
        .pointer("/meta/allRefsPath")
        .and_then(|v| v.as_str())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|data| serde_json::from_str::<RefMap>(&data).ok());
    let refs = merge_snapshot_refs(inline_refs, all_refs);
    eprintln!("\n{}", ref_count_summary(inline_count, refs.len()));

    let snapshot_id = result
        .pointer("/meta/snapshotId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(SnapshotRefs {
        refs,
        snapshot_id,
        snapshot_mode,
    })
}

fn merge_snapshot_refs(inline: RefMap, all: Option<RefMap>) -> RefMap {
    match all {
        Some(mut all) => {
            all.extend(inline);
            all
        }
        None => inline,
    }
}

fn ref_count_summary(inline: usize, total: usize) -> String {
    if total > inline {
        format!(
            "{total} refs captured ({inline} inline, {} in subtree files)",
            total - inline
        )
    } else {
        format!("{total} refs captured")
    }
}

/// List recent snapshot sessions from the app snapshot directory.
pub fn snapshots_list(instance: Option<&ConnectorInstance>) -> Result<(), String> {
    let dir = snapshots_dir(instance);
    if !dir.exists() {
        println!("No snapshots found");
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));
    for entry in entries.iter().take(10) {
        let name = entry.file_name();
        let meta_path = entry.path().join("meta.json");
        if let Ok(meta_str) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                let files = meta
                    .get("files")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                println!("{} -- {} subtree files", name.to_string_lossy(), files);
            }
        }
    }
    Ok(())
}

/// Read a subtree file from a snapshot session.
pub fn snapshots_read(
    instance: Option<&ConnectorInstance>,
    uuid: &str,
    file: Option<&str>,
) -> Result<(), String> {
    let dir = snapshots_dir(instance).join(uuid);
    if !dir.exists() {
        return Err(format!("Snapshot {uuid} not found"));
    }
    let target = file.unwrap_or("layout.txt");
    let path = dir.join(target);
    // Canonical path verification (prevent traversal)
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("File not found: {e}"))?;
    let canonical_dir = dir.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&canonical_dir) {
        return Err("Invalid path".to_string());
    }
    let content = std::fs::read_to_string(canonical).map_err(|e| e.to_string())?;
    println!("{content}");
    Ok(())
}

fn snapshots_dir(instance: Option<&ConnectorInstance>) -> PathBuf {
    instance
        .map(ConnectorInstance::snapshots_dir)
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join(format!("tauri-connector-{}", std::process::id()))
                .join("snapshots")
        })
}

/// Click an element by ref or selector.
pub async fn click(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        r#"
      const rect = el.getBoundingClientRect();
      el.click();
      return { action: 'click', tag: el.tagName.toLowerCase(), x: rect.x + rect.width/2, y: rect.y + rect.height/2 };
    "#,
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Double-click an element.
pub async fn dblclick(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        r#"
      el.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
      return { action: 'dblclick', tag: el.tagName.toLowerCase() };
    "#,
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Hover over an element.
pub async fn hover(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        r#"
      el.dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
      el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
      const rect = el.getBoundingClientRect();
      return { action: 'hover', tag: el.tagName.toLowerCase(), x: rect.x + rect.width/2, y: rect.y + rect.height/2 };
    "#,
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Drag an element to a target (`@ref`, CSS selector or "x,y"). Runs the
/// plugin's drag so the CLI and MCP dispatch the same event sequence.
pub async fn drag(
    client: &ConnectorClient,
    refs: &RefMap,
    source: &str,
    target: &str,
    steps: u32,
    duration_ms: u32,
    drag_strategy: &str,
) -> Result<(), String> {
    let point = target
        .split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?)));
    let build = |resolve: &dyn Fn(&str) -> Result<String, String>| -> Result<Value, String> {
        let mut cmd = json!({
            "type": "interact",
            "action": "drag",
            "selector": resolve(source)?,
            "steps": steps.max(1),
            "duration_ms": duration_ms,
            "drag_strategy": drag_strategy,
            "window_id": window_id(),
        });
        match point {
            Some((x, y)) => {
                cmd["target_x"] = json!(x);
                cmd["target_y"] = json!(y);
            }
            None => cmd["target_selector"] = json!(resolve(target)?),
        }
        Ok(cmd)
    };
    let timeout = duration_ms as u64 + 15_000;
    print_json_result(send_with_ref_fallback(client, refs, build, timeout).await)
}

/// Sends a command built from the raw targets, so the plugin resolves
/// `@refs` from its own snapshot cache. When the plugin has no matching ref
/// (a refusal before anything is dispatched), retries once with the
/// selectors from the CLI's local ref cache.
async fn send_with_ref_fallback(
    client: &ConnectorClient,
    refs: &RefMap,
    build: impl Fn(&dyn Fn(&str) -> Result<String, String>) -> Result<Value, String>,
    timeout_ms: u64,
) -> Result<Value, String> {
    let raw = build(&|target: &str| Ok(target.to_string()))?;
    let local = build(&|target: &str| resolve_to_selector(target, refs)).ok();
    let result = client.send_with_timeout(raw.clone(), timeout_ms).await;
    match (&result, local) {
        (Err(error), Some(local)) if local != raw && is_pre_dispatch_ref_miss(error) => {
            client.send_with_timeout(local, timeout_ms).await
        }
        _ => result,
    }
}

/// True for the plugin's ref-resolution refusals, which happen before any
/// event is dispatched and are therefore safe to retry.
fn is_pre_dispatch_ref_miss(error: &str) -> bool {
    if error.contains("No snapshot refs cached") || error.contains("Could not resolve ref=") {
        return true;
    }
    serde_json::from_str::<Value>(error)
        .ok()
        .and_then(|value| {
            let outcome = value.get("outcome")?;
            let code = outcome.pointer("/error/code")?.as_str()?;
            let execution = outcome.get("execution")?.as_str()?;
            Some(code == "stale_ref" && execution == "not_dispatched")
        })
        .unwrap_or(false)
}

/// Fill, type or press through the plugin's input helper: it focuses the
/// target, waits for focus-driven editors such as Monaco to settle and uses
/// the native value setter plus input/change events for form fields.
async fn keyboard_input(
    client: &ConnectorClient,
    refs: &RefMap,
    action: &str,
    target: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
) -> Result<(), String> {
    let build = |resolve: &dyn Fn(&str) -> Result<String, String>| -> Result<Value, String> {
        let mut cmd = json!({ "type": "keyboard", "action": action, "window_id": window_id() });
        if let Some(target) = target {
            cmd["selector"] = json!(resolve(target)?);
        }
        if let Some(text) = text {
            cmd["text"] = json!(text);
        }
        if let Some(key) = key {
            cmd["key"] = json!(key);
        }
        Ok(cmd)
    };
    print_json_result(send_with_ref_fallback(client, refs, build, 30_000).await)
}

/// Resolve a @ref or CSS selector to a CSS selector string.
fn resolve_to_selector(target: &str, refs: &RefMap) -> Result<String, String> {
    match crate::snapshot::parse_ref(target) {
        Some(ref_id) => {
            let entry = refs
                .get(&ref_id)
                .ok_or_else(|| format!("Unknown ref: {ref_id}. Run snapshot first."))?;
            Ok(entry.selector.clone())
        }
        None => Ok(target.to_string()),
    }
}

/// Focus an element.
pub async fn focus(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        "el.focus(); return { action: 'focus', tag: el.tagName.toLowerCase() };",
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Replace an input's value through the native setter with input/change
/// events, the path React-controlled fields observe.
pub async fn fill(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
    value: &str,
) -> Result<(), String> {
    keyboard_input(client, refs, "fill", Some(target), Some(value), None).await
}

/// Type text into a field, contenteditable or Monaco editor.
pub async fn type_text(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
    text: &str,
) -> Result<(), String> {
    keyboard_input(client, refs, "type", Some(target), Some(text), None).await
}

/// Check a checkbox.
pub async fn check(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        "if (!el.checked) el.click(); return { action: 'check', checked: el.checked };",
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Uncheck a checkbox.
pub async fn uncheck(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        "if (el.checked) el.click(); return { action: 'uncheck', checked: el.checked };",
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Select option(s) in a <select> element.
pub async fn select(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
    values: &[String],
) -> Result<(), String> {
    let vals_json = serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string());
    let action = format!(
        r#"
      const vals = {vals_json};
      const options = el.querySelectorAll('option');
      let matched = [];
      options.forEach(opt => {{
        if (vals.includes(opt.value) || vals.includes(opt.textContent.trim())) {{
          opt.selected = true;
          matched.push(opt.value);
        }}
      }});
      el.dispatchEvent(new Event('change', {{ bubbles: true }}));
      return {{ action: 'select', matched }};
    "#
    );
    let script = build_resolve_and_act_script(target, refs, &action);
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Scroll the page or an element.
pub async fn scroll(
    client: &ConnectorClient,
    refs: &RefMap,
    direction: &str,
    amount: i32,
    target: Option<&str>,
) -> Result<(), String> {
    let dx: i32 = match direction {
        "left" => -amount,
        "right" => amount,
        _ => 0,
    };
    let dy: i32 = match direction {
        "up" => -amount,
        "down" => amount,
        _ => 0,
    };

    let result = if let Some(t) = target {
        let action = format!(
            "el.scrollBy({dx}, {dy}); return {{ action: 'scroll', direction: '{direction}', amount: {amount} }};"
        );
        let script = build_resolve_and_act_script(t, refs, &action);
        exec_js(client, &script, 30_000).await?
    } else {
        let script = format!(
            "(() => {{ window.scrollBy({dx}, {dy}); return {{ action: 'scroll', direction: '{direction}', amount: {amount} }}; }})()"
        );
        exec_js(client, &script, 30_000).await?
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Scroll an element into view.
pub async fn scroll_into_view(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        "el.scrollIntoView({ behavior: 'smooth', block: 'center' }); return { action: 'scrollintoview', tag: el.tagName.toLowerCase() };",
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Press a key or combo (Enter, Escape, Meta+z) on the focused element.
/// Unknown keys are refused by the plugin before anything is dispatched.
pub async fn press(client: &ConnectorClient, key: &str) -> Result<(), String> {
    keyboard_input(client, &RefMap::new(), "press", None, None, Some(key)).await
}

/// Get a property from an element or the page.
pub async fn get_prop(
    client: &ConnectorClient,
    refs: &RefMap,
    prop: &str,
    target: Option<&str>,
    extra: Option<&str>,
) -> Result<(), String> {
    match prop {
        "title" => {
            let result = exec_js(client, "document.title", 30_000).await?;
            println!("{}", result.as_str().unwrap_or(&result.to_string()));
            return Ok(());
        }
        "url" => {
            let result = exec_js(client, "location.href", 30_000).await?;
            println!("{}", result.as_str().unwrap_or(&result.to_string()));
            return Ok(());
        }
        "count" => {
            let Some(t) = target else {
                return Err("Usage: get count <selector>".to_string());
            };
            let selector = js_string(t);
            let result = exec_js(
                client,
                &format!("document.querySelectorAll({selector}).length"),
                30_000,
            )
            .await?;
            println!("{result}");
            return Ok(());
        }
        _ => {}
    }

    let Some(t) = target else {
        return Err(
            "Usage: get <text|html|value|attr|box|styles> <@ref|selector> [attr-name]".to_string(),
        );
    };

    let action_js = match prop {
        "text" => "return el.textContent.trim();".to_string(),
        "html" => "return el.innerHTML;".to_string(),
        "value" => r#"return el.value || el.getAttribute("aria-valuenow") || "";"#.to_string(),
        "box" => "const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height };".to_string(),
        "styles" => "const cs = getComputedStyle(el); const s = {}; for (let i = 0; i < cs.length; i++) { s[cs[i]] = cs.getPropertyValue(cs[i]); } return s;".to_string(),
        "attr" => {
            let attr = extra.unwrap_or("");
            format!("return el.getAttribute({});", js_string(attr))
        }
        _ => return Err(format!("Unknown property: {prop}")),
    };

    let script = build_resolve_and_act_script(t, refs, &action_js);
    let result = exec_js(client, &script, 30_000).await?;

    match &result {
        Value::String(s) => println!("{s}"),
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    }
    Ok(())
}

/// Wait for an element or text.
#[allow(clippy::too_many_arguments)]
pub async fn wait(
    client: &ConnectorClient,
    selector: Option<&str>,
    text: Option<&str>,
    url: Option<&str>,
    load_state: Option<&str>,
    function: Option<&str>,
    selector_state: Option<&str>,
    timeout_ms: u64,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "wait_for",
        "timeout": timeout_ms,
        "window_id": window_id(),
    });
    if let Some(s) = selector {
        cmd["selector"] = json!(s);
    }
    if let Some(t) = text {
        cmd["text"] = json!(t);
    }
    if let Some(url) = url {
        cmd["url"] = json!(url);
    }
    if let Some(load_state) = load_state {
        cmd["load_state"] = json!(load_state);
    }
    if let Some(function) = function {
        cmd["function"] = json!(function);
    }
    if let Some(selector_state) = selector_state {
        cmd["state"] = json!(selector_state);
    }
    let result = client.send_with_timeout(cmd, timeout_ms + 5000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Find by semantic locator and optionally act on the match.
#[allow(clippy::too_many_arguments)]
pub async fn locator(
    client: &ConnectorClient,
    role: Option<&str>,
    text: Option<&str>,
    label: Option<&str>,
    placeholder: Option<&str>,
    alt: Option<&str>,
    title: Option<&str>,
    test_id: Option<&str>,
    name: Option<&str>,
    exact: bool,
    first: bool,
    last: bool,
    nth: Option<usize>,
    action: Option<&str>,
    value: Option<&str>,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "locator",
        "window_id": window_id(),
        "exact": exact,
        "first": first,
        "last": last,
    });
    for (key, value) in [
        ("role", role),
        ("text", text),
        ("label", label),
        ("placeholder", placeholder),
        ("alt", alt),
        ("title", title),
        ("test_id", test_id),
        ("name", name),
        ("action", action),
        ("value", value),
    ] {
        if let Some(value) = value {
            cmd[key] = json!(value);
        }
    }
    if let Some(nth) = nth {
        cmd["nth"] = json!(nth);
    }
    print_json_result(client.send_with_timeout(cmd, 30_000).await)
}

/// Evaluate arbitrary JavaScript.
pub async fn eval_js(
    client: &ConnectorClient,
    script: &str,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    if script.trim().is_empty() {
        return Err("eval needs a script, e.g. tauri-connector eval 'document.title'".into());
    }
    let page_timeout = timeout_ms.unwrap_or(connector_client::DEFAULT_EVAL_TIMEOUT_MS);
    let result = exec_js(client, script, page_timeout).await?;
    match &result {
        Value::String(s) => println!("{s}"),
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    }
    Ok(())
}

/// Read console logs.
pub async fn logs(
    client: &ConnectorClient,
    lines: usize,
    filter: Option<&str>,
    level: Option<&str>,
    pattern: Option<&str>,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "console_logs", "lines": lines, "window_id": window_id() });
    if let Some(f) = filter {
        cmd["filter"] = json!(f);
    }
    if let Some(l) = level {
        cmd["level"] = json!(l);
    }
    if let Some(p) = pattern {
        cmd["pattern"] = json!(p);
    }

    let result = client.send(cmd).await?;
    if let Some(logs) = result.get("logs").and_then(|v| v.as_array()) {
        for entry in logs {
            let ts = entry.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
            let lvl = entry.get("level").and_then(|v| v.as_str()).unwrap_or("LOG");
            let msg = entry.get("message").and_then(|v| v.as_str()).unwrap_or("");
            println!(
                "{} {:<5} {msg}",
                format_log_time(ts, &chrono::Local),
                lvl.to_uppercase()
            );
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    }
    Ok(())
}

/// Console timestamps are epoch milliseconds; print them in the caller's local
/// time with the UTC offset so they line up with app and system logs.
fn format_log_time<Tz: chrono::TimeZone>(ms: u64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match chrono::DateTime::from_timestamp_millis(ms as i64) {
        Some(utc) => utc
            .with_timezone(zone)
            .format("%H:%M:%S%.3f %:z")
            .to_string(),
        None => format!("{ms}ms"),
    }
}

/// Get app backend state.
pub async fn state(client: &ConnectorClient) -> Result<(), String> {
    let result = client.send(json!({ "type": "backend_state" })).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Show bridge connection status.
pub async fn bridge_status(client: &ConnectorClient) -> Result<(), String> {
    let result = client.send(json!({ "type": "bridge_status" })).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// List app windows.
pub async fn windows(client: &ConnectorClient) -> Result<(), String> {
    let result = client.send(json!({ "type": "window_list" })).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Take a screenshot and save to file.
#[allow(clippy::too_many_arguments)]
pub async fn screenshot(
    client: &ConnectorClient,
    output: Option<&str>,
    selector: Option<&str>,
    format: &str,
    quality: u8,
    max_width: Option<u32>,
    overwrite: bool,
    output_dir: Option<&Path>,
    name_hint: Option<&str>,
    annotate: bool,
    instance: Option<&ConnectorInstance>,
    resolved: &ResolvedConnection,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "screenshot",
        "format": format,
        "quality": quality,
        "window_id": window_id(),
    });
    if let Some(w) = max_width {
        cmd["max_width"] = json!(w);
    }
    if let Some(selector) = selector {
        cmd["selector"] = json!(selector);
    }
    if annotate {
        cmd["annotate"] = json!(true);
    }
    let result = client.send_with_timeout(cmd, 60_000).await?;

    let base64_data = result
        .get("base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No base64 data in screenshot response".to_string())?;

    let bytes = b64_decode(base64_data)?;
    let target =
        resolve_screenshot_path(output, output_dir, format, name_hint, selector, instance)?;
    let artifact = write_screenshot_artifact(&target, &bytes, overwrite)?;

    let width = result.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
    let height = result.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let capture_kind = if selector.is_some() {
        "element"
    } else {
        "full"
    };
    let created_at = now_ms();
    let artifact_id = format!(
        "shot_{}",
        artifact
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("screenshot")
    );
    let metadata = json!({
        "artifactId": artifact_id.clone(),
        "id": artifact_id,
        "kind": "screenshot",
        "captureKind": capture_kind,
        "path": artifact.path.to_string_lossy(),
        "requestedPath": artifact.requested_path.to_string_lossy(),
        "resolvedFromCollision": artifact.resolved_from_collision,
        "overwrote": artifact.overwrote,
        "sha256": sha256_hex(&bytes),
        "bytes": bytes.len(),
        "width": width,
        "height": height,
        "format": format,
        "createdAt": created_at,
        "createdAtMs": created_at,
        "source": if selector.is_some() { "selector" } else { "full_window" },
        "selector": selector,
        "windowId": window_id(),
        "annotated": result.get("annotated").cloned().unwrap_or(Value::Bool(false)),
        "annotations": result.get("annotations").cloned().unwrap_or(Value::Null),
        "snapshotId": result.get("snapshotId").cloned().or_else(|| latest_ref_cache_snapshot_id(resolved)).unwrap_or(Value::Null),
        "refsPath": result.get("refsPath").cloned().or_else(|| latest_ref_cache_path(resolved)).unwrap_or(Value::Null),
        "rectCssPx": result.get("rectCssPx").cloned().unwrap_or(Value::Null),
        "devicePixelRatio": result.get("devicePixelRatio").cloned().unwrap_or(Value::Null),
        "captureSource": result.get("captureSource").cloned().unwrap_or(Value::Null),
        "fallback": result.get("fallback").cloned().unwrap_or(Value::Null),
    });
    append_cli_artifact_manifest(instance, &metadata)?;
    let size_kb = bytes.len() / 1024;
    if artifact.resolved_from_collision {
        eprintln!(
            "Requested path existed; saved unique screenshot to {}",
            artifact.path.display()
        );
    }
    let warnings = result.get("warnings").cloned().unwrap_or(Value::Null);
    for message in screenshot_warning_messages(&warnings) {
        eprintln!("warning: {message}");
    }
    if let Some(note) = screenshot_fallback_note(result.get("fallback")) {
        eprintln!("note: {note}");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "path": artifact.path,
            "warnings": warnings,
            "requestedPath": artifact.requested_path,
            "resolvedFromCollision": artifact.resolved_from_collision,
            "overwrote": artifact.overwrote,
        "sha256": metadata.get("sha256").cloned().unwrap_or(Value::Null),
            "width": width,
            "height": height,
            "sizeKb": size_kb,
            "artifact": metadata,
        }))
        .unwrap_or_default()
    );
    Ok(())
}

/// Describe a plugin-side capture retake, e.g. a uniform window capture that
/// was replaced by the WebView's own snapshot.
fn screenshot_fallback_note(fallback: Option<&Value>) -> Option<String> {
    let fallback = fallback.filter(|v| v.is_object())?;
    let text = |key: &str| fallback.get(key).and_then(Value::as_str).unwrap_or("?");
    let reason = text("reason");
    Some(match text("outcome") {
        "used" => format!(
            "{} capture was {reason}; used {} instead",
            text("from"),
            text("to")
        ),
        "also_uniform" => format!(
            "{} capture was {reason}; {} retake was also uniform",
            text("from"),
            text("to")
        ),
        _ => format!(
            "{} capture was {reason}; {} retake failed: {}",
            text("from"),
            text("to"),
            text("error")
        ),
    })
}

/// One line per plugin screenshot warning, e.g. `page_hidden: ...`.
fn screenshot_warning_messages(warnings: &Value) -> Vec<String> {
    warnings
        .as_array()
        .into_iter()
        .flatten()
        .map(|warning| {
            format!(
                "{}: {}",
                warning
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("warning"),
                warning.get("message").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect()
}

struct ArtifactWrite {
    path: PathBuf,
    requested_path: PathBuf,
    resolved_from_collision: bool,
    overwrote: bool,
}

fn resolve_screenshot_path(
    output: Option<&str>,
    output_dir: Option<&Path>,
    format: &str,
    name_hint: Option<&str>,
    selector: Option<&str>,
    instance: Option<&ConnectorInstance>,
) -> Result<PathBuf, String> {
    if let Some(output) = output {
        let path = PathBuf::from(output);
        if path.extension().is_some() {
            return Ok(path);
        }
        return Ok(path.join(default_screenshot_name(
            format, name_hint, selector, instance,
        )));
    }

    let dir = output_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        instance
            .and_then(|i| i.log_dir.clone())
            .unwrap_or_else(|| std::env::temp_dir().join("tauri-connector"))
            .join("artifacts")
            .join("screenshots")
    });
    Ok(dir.join(default_screenshot_name(
        format, name_hint, selector, instance,
    )))
}

fn default_screenshot_name(
    format: &str,
    name_hint: Option<&str>,
    selector: Option<&str>,
    instance: Option<&ConnectorInstance>,
) -> String {
    let created = now_ms();
    let app = instance
        .and_then(|i| i.app_name.as_deref().or(i.app_id.as_deref()))
        .map(slug)
        .unwrap_or_else(|| "app".to_string());
    let kind = if selector.is_some() {
        "element"
    } else {
        "full"
    };
    let hint = name_hint
        .or(selector)
        .map(slug)
        .unwrap_or_else(|| kind.to_string());
    let ext = match format {
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        _ => "png",
    };
    format!(
        "{created}-{app}-{}-{kind}-{hint}-{}.{}",
        slug(window_id()),
        uuid8(),
        ext
    )
}

fn cli_artifact_manifest_path(instance: Option<&ConnectorInstance>) -> PathBuf {
    instance
        .and_then(|i| i.log_dir.clone())
        .unwrap_or_else(|| std::env::temp_dir().join("tauri-connector"))
        .join("artifacts")
        .join("manifest.jsonl")
}

fn latest_ref_cache_snapshot_id(resolved: &ResolvedConnection) -> Option<Value> {
    crate::snapshot::load_ref_cache_full(resolved, window_id())
        .ok()
        .flatten()
        .and_then(|cache| cache.snapshot_id)
        .map(Value::String)
}

fn latest_ref_cache_path(resolved: &ResolvedConnection) -> Option<Value> {
    let path = crate::snapshot::ref_cache_path(resolved, window_id());
    if path.exists() {
        Some(Value::String(path.to_string_lossy().to_string()))
    } else {
        None
    }
}

fn append_cli_artifact_manifest(
    instance: Option<&ConnectorInstance>,
    artifact: &Value,
) -> Result<(), String> {
    let path = cli_artifact_manifest_path(instance);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create artifact manifest dir {}: {e}",
                parent.display()
            )
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Failed to open artifact manifest {}: {e}", path.display()))?;
    let line = serde_json::to_string(artifact).map_err(|e| e.to_string())?;
    writeln!(file, "{line}")
        .map_err(|e| format!("Failed to write artifact manifest {}: {e}", path.display()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn write_screenshot_artifact(
    target: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<ArtifactWrite, String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }

    if overwrite {
        std::fs::write(target, bytes)
            .map_err(|e| format!("Failed to write {}: {e}", target.display()))?;
        return Ok(ArtifactWrite {
            path: target.to_path_buf(),
            requested_path: target.to_path_buf(),
            resolved_from_collision: false,
            overwrote: true,
        });
    }

    for attempt in 0..50 {
        let candidate = if attempt == 0 {
            target.to_path_buf()
        } else {
            collision_path(target, attempt)
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut f) => {
                f.write_all(bytes)
                    .map_err(|e| format!("Failed to write {}: {e}", candidate.display()))?;
                let _ = f.sync_all();
                return Ok(ArtifactWrite {
                    path: candidate,
                    requested_path: target.to_path_buf(),
                    resolved_from_collision: attempt > 0,
                    overwrote: false,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Failed to write {}: {e}", candidate.display())),
        }
    }
    Err("failed to allocate unique screenshot path after 50 attempts".to_string())
}

fn collision_path(path: &Path, attempt: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("screenshot");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    let suffix = uuid8();
    path.with_file_name(format!("{stem}-{attempt:04}-{suffix}.{ext}"))
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(48).collect()
}

fn uuid8() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:08x}", (nanos as u64) ^ u64::from(std::process::id()))[0..8].to_string()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut msg = bytes.to_vec();
    msg.push(0x80);
    while !(msg.len() + 8).is_multiple_of(64) {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let offset = i * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// Get cached DOM snapshot pushed from frontend.
pub async fn cached_dom(client: &ConnectorClient, window_id: &str) -> Result<(), String> {
    let result = client
        .send(json!({ "type": "get_cached_dom", "window_id": window_id }))
        .await?;
    match &result {
        Value::String(s) => println!("{s}"),
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    }
    Ok(())
}

/// Find elements by CSS selector, XPath, or text.
pub async fn find(client: &ConnectorClient, selector: &str, strategy: &str) -> Result<(), String> {
    let result = client
        .send(json!({
            "type": "find_element",
            "selector": selector,
            "strategy": strategy,
            "window_id": window_id(),
        }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Get element metadata from Alt+Shift+Click picker.
pub async fn pointed(client: &ConnectorClient) -> Result<(), String> {
    let result = client
        .send(json!({ "type": "get_pointed_element" }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Resize a window.
pub async fn resize(
    client: &ConnectorClient,
    window_id: &str,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let result = client
        .send(json!({
            "type": "window_resize",
            "window_id": window_id,
            "width": width,
            "height": height,
        }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Run a Tauri IPC command.
pub async fn ipc_exec(
    client: &ConnectorClient,
    command: &str,
    args_json: Option<&str>,
) -> Result<(), String> {
    let args: Option<Value> = args_json
        .map(|s| serde_json::from_str(s).map_err(|e| format!("Invalid JSON args: {e}")))
        .transpose()?;

    let mut cmd = json!({
        "type": "ipc_execute_command",
        "command": command,
    });
    if let Some(a) = args {
        cmd["args"] = a;
    }
    let result = client.send_with_timeout(cmd, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Start or stop IPC monitoring.
pub async fn ipc_monitor(client: &ConnectorClient, action: &str) -> Result<(), String> {
    let result = client
        .send(json!({ "type": "ipc_monitor", "action": action, "window_id": window_id() }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Get captured IPC traffic.
pub async fn ipc_captured(
    client: &ConnectorClient,
    filter: Option<&str>,
    pattern: Option<&str>,
    since: Option<u64>,
    limit: usize,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "ipc_get_captured", "limit": limit });
    if let Some(f) = filter {
        cmd["filter"] = json!(f);
    }
    if let Some(p) = pattern {
        cmd["pattern"] = json!(p);
    }
    if let Some(s) = since {
        cmd["since"] = json!(s);
    }
    let result = client.send(cmd).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Emit a custom Tauri event.
pub async fn ipc_emit(
    client: &ConnectorClient,
    event_name: &str,
    payload_json: Option<&str>,
) -> Result<(), String> {
    let payload: Option<Value> = payload_json
        .map(|s| serde_json::from_str(s).map_err(|e| format!("Invalid JSON payload: {e}")))
        .transpose()?;

    let mut cmd = json!({
        "type": "ipc_emit_event",
        "event_name": event_name,
    });
    if let Some(p) = payload {
        cmd["payload"] = p;
    }
    let result = client.send(cmd).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Start listening for Tauri events.
pub async fn event_listen(client: &ConnectorClient, events: &str) -> Result<(), String> {
    let event_list: Vec<String> = events.split(',').map(|s| s.trim().to_string()).collect();
    let result = client
        .send(json!({
            "type": "ipc_listen",
            "action": "start",
            "events": event_list,
        }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Get captured events.
pub async fn event_captured(
    client: &ConnectorClient,
    pattern: Option<&str>,
    since: Option<u64>,
    limit: usize,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "event_get_captured", "limit": limit });
    if let Some(p) = pattern {
        cmd["pattern"] = json!(p);
    }
    if let Some(s) = since {
        cmd["since"] = json!(s);
    }
    let result = client.send(cmd).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Stop listening for events.
pub async fn event_stop(client: &ConnectorClient) -> Result<(), String> {
    let result = client
        .send(json!({
            "type": "ipc_listen",
            "action": "stop",
        }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Clear logs, IPC traffic, events, or all.
pub async fn clear_logs(client: &ConnectorClient, target: &str) -> Result<(), String> {
    let source = match target {
        "logs" => "console",
        "ipc" => "ipc",
        "events" => "events",
        "runtime" => "runtime",
        "all" => "all",
        _ => {
            return Err(format!(
                "Unknown target: {target}. Use: logs, ipc, events, runtime, all"
            ))
        }
    };
    let result = client
        .send(json!({ "type": "clear_logs", "source": source }))
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Get captured frontend runtime events.
#[allow(clippy::too_many_arguments)]
pub async fn runtime(
    client: &ConnectorClient,
    limit: usize,
    kind: Option<&str>,
    level: Option<&str>,
    pattern: Option<&str>,
    since: Option<u64>,
    since_mark: Option<&str>,
    window_id: &str,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "runtime_get_captured",
        "limit": limit,
        "window_id": window_id,
    });
    if let Some(kind) = kind {
        cmd["kind"] = json!(kind);
    }
    if let Some(level) = level {
        cmd["level"] = json!(level);
    }
    if let Some(pattern) = pattern {
        cmd["pattern"] = json!(pattern);
    }
    if let Some(since) = since {
        cmd["since"] = json!(since);
    }
    if let Some(since_mark) = since_mark {
        cmd["since_mark"] = json!(since_mark);
    }
    let result = client.send(cmd).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

pub async fn artifacts_list(
    client: &ConnectorClient,
    kind: Option<&str>,
    limit: usize,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "artifact_list", "limit": limit });
    if let Some(kind) = kind {
        cmd["kind"] = json!(kind);
    }
    print_json_result(client.send(cmd).await)
}

pub async fn artifact_show(
    client: &ConnectorClient,
    artifact: &str,
    include_base64: bool,
) -> Result<(), String> {
    let args = json!({"artifactId":artifact,"includeImage":include_base64});
    let mut result =
        if connector_client::inspection::protected_artifact_request("artifact_read", &args) {
            client.inspect("artifact_read", &args).await?
        } else {
            client
                .send(json!({"type":"artifact_read","artifact":artifact}))
                .await?
        };
    if !include_base64 {
        if let Some(obj) = result.as_object_mut() {
            obj.remove("base64");
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

pub async fn artifact_prune(
    client: &ConnectorClient,
    keep: usize,
    kind: Option<&str>,
    delete_files: bool,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "artifact_prune",
        "keep": keep,
        "delete_files": delete_files,
    });
    if let Some(kind) = kind {
        cmd["kind"] = json!(kind);
    }
    print_json_result(client.send(cmd).await)
}

pub async fn artifact_compare(
    client: &ConnectorClient,
    before: &str,
    after: &str,
    threshold: f64,
) -> Result<(), String> {
    let args = json!({"baselineId":before,"currentId":after});
    if connector_client::inspection::protected_artifact_request("artifact_compare", &args) {
        if threshold != 0.0 {
            return Err("invalid_arguments: protected comparison reports contract compatibility and byte equality; nonzero threshold is unsupported".into());
        }
        return print_json_result(client.inspect("artifact_compare", &args).await);
    }
    print_json_result(
        client
            .send(json!({
                "type": "artifact_compare",
                "before": before,
                "after": after,
                "threshold": threshold,
            }))
            .await,
    )
}

pub async fn debug_mark(client: &ConnectorClient, label: Option<&str>) -> Result<(), String> {
    let mut cmd = json!({ "type": "debug_mark" });
    if let Some(label) = label {
        cmd["label"] = json!(label);
    }
    print_json_result(client.send(cmd).await)
}

#[allow(clippy::too_many_arguments)]
pub async fn debug_snapshot(
    client: &ConnectorClient,
    window_id: &str,
    include_dom: bool,
    include_screenshot: bool,
    include_logs: bool,
    include_ipc: bool,
    include_events: bool,
    include_runtime: bool,
    since: Option<u64>,
    since_mark: Option<&str>,
    max_tokens: Option<u64>,
    screenshot_name_hint: Option<&str>,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "debug_snapshot",
        "window_id": window_id,
        "include_dom": include_dom,
        "include_screenshot": include_screenshot,
        "include_logs": include_logs,
        "include_ipc": include_ipc,
        "include_events": include_events,
        "include_runtime": include_runtime,
    });
    if let Some(since) = since {
        cmd["since"] = json!(since);
    }
    if let Some(since_mark) = since_mark {
        cmd["since_mark"] = json!(since_mark);
    }
    if let Some(max_tokens) = max_tokens {
        cmd["max_tokens"] = json!(max_tokens);
    }
    if let Some(name_hint) = screenshot_name_hint {
        cmd["screenshot_name_hint"] = json!(name_hint);
    }
    print_json_result(client.send_with_timeout(cmd, 60_000).await)
}

#[allow(clippy::too_many_arguments)]
pub async fn act_and_verify(
    client: &ConnectorClient,
    action: &str,
    selector: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    target_selector: Option<&str>,
    wait_for_selector: Option<&str>,
    wait_for_text: Option<&str>,
    timeout: u64,
    verify_dom: bool,
    verify_screenshot: bool,
    include_logs: bool,
    include_ipc: bool,
    include_runtime: bool,
    window_id: &str,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "webview_act_and_verify",
        "action": action,
        "timeout": timeout,
        "verify_dom": verify_dom,
        "verify_screenshot": verify_screenshot,
        "include_logs": include_logs,
        "include_ipc": include_ipc,
        "include_runtime": include_runtime,
        "window_id": window_id,
    });
    if let Some(selector) = selector {
        cmd["selector"] = json!(selector);
    }
    if let Some(text) = text {
        cmd["text"] = json!(text);
    }
    if let Some(key) = key {
        cmd["key"] = json!(key);
    }
    if let Some(target_selector) = target_selector {
        cmd["target_selector"] = json!(target_selector);
    }
    if let Some(wait_for_selector) = wait_for_selector {
        cmd["wait_for_selector"] = json!(wait_for_selector);
    }
    if let Some(wait_for_text) = wait_for_text {
        cmd["wait_for_text"] = json!(wait_for_text);
    }
    print_json_result(client.send_with_timeout(cmd, timeout + 60_000).await)
}

/// Forward workflow lifecycle requests to the application-owned service.
pub async fn workflow(
    client: &ConnectorClient,
    host: &str,
    port: u16,
    command: crate::WorkflowCommand,
    selected_window: &str,
    app_instance_id: Option<&str>,
) -> Result<i32, String> {
    let (operation, args) = match command {
        crate::WorkflowCommand::Run { spec, wait_ms } => {
            let text = read_batch_spec_input(&spec)?;
            let spec: Value = serde_json::from_str(&text)
                .map_err(|error| format!("Invalid workflow JSON: {error}"))?;
            ("workflow_run", json!({ "spec": spec, "waitMs": wait_ms }))
        }
        crate::WorkflowCommand::Get {
            run_id,
            cursor,
            include,
            evidence_id,
            offset,
        } => {
            let mut args = json!({ "runId": run_id });
            if let Some(cursor) = cursor {
                args["cursor"] = json!(cursor);
            }
            if !include.is_empty() {
                args["include"] = json!(include);
            }
            if let Some(evidence_id) = evidence_id {
                args["evidenceId"] = json!(evidence_id);
            }
            if let Some(offset) = offset {
                args["offset"] = json!(offset);
            }
            ("workflow_get", args)
        }
        crate::WorkflowCommand::Cancel { run_id } => ("workflow_cancel", json!({"runId":run_id})),
        crate::WorkflowCommand::Resume {
            run_id,
            expected_revision,
            checkpoint_id,
            intent,
        } => (
            "workflow_resume",
            json!({"runId":run_id,"expectedRevision":expected_revision,"checkpointId":checkpoint_id,"intent":intent}),
        ),
        crate::WorkflowCommand::Capabilities => {
            ("workflow_capabilities", json!({"windowId":selected_window}))
        }
    };
    let report =
        connector_mcp_server::tools::dispatch_tool(client, host, port, operation, &args).await?;
    if let (Some(run_id), Some(instance)) =
        (report.get("runId").and_then(Value::as_str), app_instance_id)
    {
        if let Err(error) = crate::identity_binding::save(
            run_id,
            &crate::identity_binding::Binding {
                app_instance_id: instance.into(),
                host: host.into(),
                port,
            },
        ) {
            eprintln!("Warning: could not retain instance binding: {error}; recover with --app-instance-id {instance}");
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(if operation == "workflow_capabilities" {
        0
    } else {
        connector_mcp_server::tools::workflow_result_exit_code(&report)
    })
}

/// Run a JSON batch of MCP tool actions through the shared batch executor,
/// dispatching each action via the standalone MCP server's tool table so the
/// CLI speaks the exact same vocabulary as both MCP servers.
#[allow(clippy::too_many_arguments)]
pub async fn batch(
    client: &ConnectorClient,
    host: &str,
    port: u16,
    spec_input: &str,
    save: Option<&Path>,
    mode: Option<&str>,
    continue_on_error: bool,
    max_parallel: Option<usize>,
    window_id: &str,
) -> Result<(), String> {
    let text = read_batch_spec_input(spec_input)?;
    let value = prepare_batch_spec_value(
        &text,
        mode,
        continue_on_error,
        max_parallel,
        save,
        window_id,
    )?;

    let report = connector_client::batch::run_from_value(&value, |tool, targs| async move {
        if tool.starts_with("workflow_") {
            return Err("workflow lifecycle operations cannot be nested in batch_actions".into());
        }
        connector_mcp_server::tools::dispatch_tool(client, host, port, &tool, &targs).await
    })
    .await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    if report.ok {
        Ok(())
    } else {
        Err(format!(
            "batch finished with {} failed and {} skipped action(s)",
            report.failed, report.skipped
        ))
    }
}

/// Resolve the batch spec argument: '-' reads stdin, inline JSON is used
/// verbatim, anything else is treated as a file path.
fn read_batch_spec_input(spec: &str) -> Result<String, String> {
    let trimmed = spec.trim();
    if trimmed == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .map_err(|e| format!("Failed to read batch spec from stdin: {e}"))?;
        return Ok(buf);
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(trimmed.to_string());
    }
    std::fs::read_to_string(trimmed)
        .map_err(|e| format!("Failed to read batch spec file '{trimmed}': {e}"))
}

/// Parse the spec text and apply CLI overrides (mode, stopOnError,
/// maxParallel, save) plus the CLI-wide --window-id as the default windowId
/// for actions that don't set one themselves.
fn prepare_batch_spec_value(
    text: &str,
    mode: Option<&str>,
    continue_on_error: bool,
    max_parallel: Option<usize>,
    save: Option<&Path>,
    window_id: &str,
) -> Result<Value, String> {
    let mut value: Value =
        serde_json::from_str(text).map_err(|e| format!("Invalid batch spec JSON: {e}"))?;

    // Bare-array shorthand: wrap so overrides below have an object to land on.
    if value.is_array() {
        value = json!({ "actions": value });
    }
    if !value.is_object() {
        return Err("batch spec must be a JSON object or array".to_string());
    }
    if let Some(mode) = mode {
        if mode != "sequential" && mode != "parallel" {
            return Err(format!(
                "--mode must be 'sequential' or 'parallel', got '{mode}'"
            ));
        }
        value["mode"] = json!(mode);
    }
    if continue_on_error {
        value["stopOnError"] = json!(false);
    }
    if let Some(max) = max_parallel {
        value["maxParallel"] = json!(max);
    }
    if let Some(path) = save {
        value["save"] = json!(path.to_string_lossy());
    }
    if window_id != "main" {
        if let Some(actions) = value.get_mut("actions").and_then(|v| v.as_array_mut()) {
            for action in actions.iter_mut() {
                let Some(action_obj) = action.as_object_mut() else {
                    continue;
                };
                let args = action_obj.entry("args").or_insert_with(|| json!({}));
                if args.is_null() {
                    *args = json!({});
                }
                if let Some(obj) = args.as_object_mut() {
                    obj.entry("windowId").or_insert_with(|| json!(window_id));
                }
            }
        }
    }
    Ok(value)
}

/// List or release resource quarantines left by unknown outcomes.
pub async fn resources(
    client: &ConnectorClient,
    action: &str,
    request_id: Option<&str>,
    all: bool,
) -> Result<(), String> {
    let mut command = json!({ "type": "resources", "action": action });
    if let Some(request_id) = request_id {
        command["request_id"] = json!(request_id);
    }
    if all {
        command["all"] = json!(true);
    }
    let result = client.send(command).await?;
    print!("{}", format_resources(&result));
    Ok(())
}

fn format_resources(result: &Value) -> String {
    let mut out = String::new();
    if let Some(released) = result["released"].as_array() {
        if released.is_empty() {
            out.push_str("Nothing released\n");
        }
        for entry in released {
            out.push_str(&format!(
                "released {} ({})\n",
                entry["requestId"]
                    .as_str()
                    .or(entry["id"].as_str())
                    .unwrap_or("?"),
                entry["reason"].as_str().unwrap_or("unknown")
            ));
        }
    }
    let quarantined = result["quarantined"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if quarantined.is_empty() {
        out.push_str("No quarantined resources\n");
    }
    for entry in &quarantined {
        let scopes: Vec<&str> = entry["resources"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        out.push_str(&format!(
            "quarantined {} ({}) scopes: {}\n",
            entry["requestId"]
                .as_str()
                .or(entry["id"].as_str())
                .unwrap_or("?"),
            entry["reason"].as_str().unwrap_or("unknown"),
            scopes.join(", ")
        ));
    }
    for entry in result["lateSettlements"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "late result {}: {}{}\n",
            entry["requestId"].as_str().unwrap_or("?"),
            entry["outcome"].as_str().unwrap_or("?"),
            if entry["releasedQuarantine"].as_bool() == Some(true) {
                " (released quarantine)"
            } else {
                ""
            }
        ));
    }
    out
}

fn print_json_result(result: Result<Value, String>) -> Result<(), String> {
    let result = result?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

pub(crate) fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }

    let input = input.as_bytes();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &byte in input {
        if byte == b'=' || byte == b'\n' || byte == b'\r' || byte == b' ' {
            continue;
        }
        let val = lookup[byte as usize];
        if val == 255 {
            return Err(format!("Invalid base64 character: {}", byte as char));
        }
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;
    use super::{prepare_batch_spec_value, read_batch_spec_input};
    use serde_json::json;

    #[test]
    fn only_pre_dispatch_ref_misses_are_retried_with_local_selectors() {
        assert!(super::is_pre_dispatch_ref_miss(
            "No snapshot refs cached for window 'main'. Run webview_dom_snapshot(mode:'ai') again."
        ));
        let stale = json!({"error": "ref gone", "outcome": {"execution": "not_dispatched", "error": {"code": "stale_ref"}}});
        assert!(super::is_pre_dispatch_ref_miss(&stale.to_string()));
        let dispatched = json!({"error": "x", "outcome": {"execution": "failed", "error": {"code": "stale_ref"}}});
        assert!(!super::is_pre_dispatch_ref_miss(&dispatched.to_string()));
        let other = json!({"error": "x", "outcome": {"execution": "not_dispatched", "error": {"code": "not_actionable"}}});
        assert!(!super::is_pre_dispatch_ref_miss(&other.to_string()));
        assert!(!super::is_pre_dispatch_ref_miss(
            "outcome_unknown: Request timeout"
        ));
    }

    #[test]
    fn split_snapshots_report_and_cache_every_ref() {
        let entry = |selector: &str| -> crate::snapshot::RefEntry {
            serde_json::from_value(json!({"tag": "button", "name": "", "selector": selector}))
                .unwrap()
        };
        let inline: super::RefMap = [("e0".to_string(), entry("#a"))].into();
        let all: super::RefMap = [
            ("e0".to_string(), entry("#stale")),
            ("e1".to_string(), entry("#b")),
            ("e2".to_string(), entry("#c")),
        ]
        .into();
        let merged = super::merge_snapshot_refs(inline.clone(), Some(all));
        assert_eq!(merged.len(), 3);
        assert_eq!(merged["e0"].selector, "#a");
        assert_eq!(
            super::ref_count_summary(1, merged.len()),
            "3 refs captured (1 inline, 2 in subtree files)"
        );
        assert_eq!(super::merge_snapshot_refs(inline, None).len(), 1);
        assert_eq!(super::ref_count_summary(4, 4), "4 refs captured");
    }

    #[test]
    fn eval_sends_custom_page_deadlines_only() {
        let default = super::execute_js_command("1", 30_000);
        assert!(default.get("timeout_ms").is_none());
        let custom = super::execute_js_command("1", 120_000);
        assert_eq!(custom["timeout_ms"], 120_000);
    }

    #[tokio::test]
    async fn eval_rejects_empty_scripts_before_sending() {
        let client = connector_client::ConnectorClient::new();
        let error = super::eval_js(&client, "   ", None).await.unwrap_err();
        assert!(error.contains("needs a script"), "{error}");
    }

    #[test]
    fn screenshot_warnings_are_printed_with_their_code() {
        let warnings =
            json!([{"code":"page_hidden","message":"The page reports visibilityState=hidden"}]);
        assert_eq!(
            super::screenshot_warning_messages(&warnings),
            vec!["page_hidden: The page reports visibilityState=hidden".to_string()]
        );
        assert!(super::screenshot_warning_messages(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn screenshot_retakes_are_reported() {
        let used = json!({"from":"window_native","to":"webview_native","reason":"uniform_image","outcome":"used"});
        assert_eq!(
            super::screenshot_fallback_note(Some(&used)).unwrap(),
            "window_native capture was uniform_image; used webview_native instead"
        );
        let failed = json!({"from":"dom_rendering","to":"webview_native","reason":"uniform_image","outcome":"failed","error":"capture_timeout"});
        assert!(super::screenshot_fallback_note(Some(&failed))
            .unwrap()
            .ends_with("retake failed: capture_timeout"));
        assert!(super::screenshot_fallback_note(None).is_none());
        assert!(super::screenshot_fallback_note(Some(&serde_json::Value::Null)).is_none());
    }

    #[test]
    fn resources_output_names_request_ids_and_reasons() {
        let text = super::format_resources(&serde_json::json!({
            "released": [{"id": "q1", "requestId": "req-1", "reason": "outcome_unknown"}],
            "quarantined": [{"id": "q2", "requestId": null, "reason": "interrupted", "resources": ["backend", "ui"]}],
            "lateSettlements": [{"requestId": "req-0", "outcome": "succeeded", "releasedQuarantine": true}]
        }));
        assert!(text.contains("released req-1 (outcome_unknown)"), "{text}");
        assert!(
            text.contains("quarantined q2 (interrupted) scopes: backend, ui"),
            "{text}"
        );
        assert!(
            text.contains("late result req-0: succeeded (released quarantine)"),
            "{text}"
        );
    }

    #[test]
    fn log_times_are_local_with_an_offset_label() {
        let winnipeg = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        // 2026-10-27T16:18:05.250Z
        let ms = 1_793_117_885_250;
        assert_eq!(super::format_log_time(ms, &winnipeg), "11:18:05.250 -05:00");
        assert_eq!(
            super::format_log_time(ms, &chrono::Utc),
            "16:18:05.250 +00:00"
        );
    }

    #[test]
    fn batch_spec_inline_json_is_used_verbatim() {
        assert_eq!(
            read_batch_spec_input(r#"{"actions":[]}"#).unwrap(),
            r#"{"actions":[]}"#
        );
        assert_eq!(
            read_batch_spec_input(r#"[{"tool":"x"}]"#).unwrap(),
            r#"[{"tool":"x"}]"#
        );
        assert!(read_batch_spec_input("/no/such/spec-file.json").is_err());
    }

    #[test]
    fn batch_spec_overrides_apply() {
        let value = prepare_batch_spec_value(
            r#"{ "actions": [ { "tool": "bridge_status" } ] }"#,
            Some("parallel"),
            true,
            Some(3),
            Some(std::path::Path::new("/tmp/report.json")),
            "main",
        )
        .unwrap();
        assert_eq!(value["mode"], json!("parallel"));
        assert_eq!(value["stopOnError"], json!(false));
        assert_eq!(value["maxParallel"], json!(3));
        assert_eq!(value["save"], json!("/tmp/report.json"));
    }

    #[test]
    fn batch_spec_wraps_bare_array_and_injects_window_id() {
        let value = prepare_batch_spec_value(
            r#"[ { "tool": "bridge_status" }, { "tool": "read_logs", "args": { "windowId": "other" } } ]"#,
            None,
            false,
            None,
            None,
            "settings",
        )
        .unwrap();
        let actions = value["actions"].as_array().unwrap();
        // Missing windowId gets the CLI-wide default; explicit ones are kept.
        assert_eq!(actions[0]["args"]["windowId"], json!("settings"));
        assert_eq!(actions[1]["args"]["windowId"], json!("other"));
    }

    #[test]
    fn batch_spec_rejects_bad_mode_and_non_object() {
        assert!(prepare_batch_spec_value(
            r#"{ "actions": [] }"#,
            Some("sideways"),
            false,
            None,
            None,
            "main"
        )
        .is_err());
        assert!(prepare_batch_spec_value("42", None, false, None, None, "main").is_err());
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
