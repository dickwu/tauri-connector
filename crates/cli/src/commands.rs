//! CLI command implementations.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use connector_client::discovery::ConnectorInstance;
use connector_client::ConnectorClient;
use serde_json::{json, Value};

use crate::snapshot::{build_resolve_and_act_script, RefMap};

/// Execute JS in the webview via the connector.
async fn exec_js(client: &ConnectorClient, script: &str, timeout_ms: u64) -> Result<Value, String> {
    client
        .send_with_timeout(
            json!({ "type": "execute_js", "script": script, "window_id": "main" }),
            timeout_ms,
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
) -> Result<RefMap, String> {
    let mut cmd = json!({
        "type": "dom_snapshot",
        "mode": mode.unwrap_or_else(|| if interactive { "ai".to_string() } else { "accessibility".to_string() }),
        "max_depth": max_depth as u64,
        "max_elements": max_elements as u64,
        "max_tokens": max_tokens as u64,
        "no_split": no_split,
        "react_enrich": react_enrich,
        "follow_portals": follow_portals,
        "window_id": "main",
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

    let refs: RefMap = result
        .get("refs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let count = refs.len();
    eprintln!("\n{count} refs captured");

    Ok(refs)
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

/// Drag an element to a target.
pub async fn drag(
    client: &ConnectorClient,
    refs: &RefMap,
    source: &str,
    target: &str,
    steps: u32,
    duration_ms: u32,
    drag_strategy: &str,
) -> Result<(), String> {
    let source_selector = resolve_to_selector(source, refs)?;
    let target_resolve = resolve_target_js(target, refs)?;

    let escaped_src = source_selector.replace('"', "\\\"");
    let script = format!(
        r#"new Promise((resolve) => {{
            const el = document.querySelector("{escaped_src}");
            if (!el) {{ resolve({{ error: 'Source element not found: {escaped_src}' }}); return; }}
            {target_resolve}
            const srcRect = el.getBoundingClientRect();
            const startX = srcRect.x + srcRect.width / 2;
            const startY = srcRect.y + srcRect.height / 2;
            const dragStrat = '{drag_strategy}';
            const useHtml5 = dragStrat === 'html5dnd' || (dragStrat === 'auto' && (el.draggable === true || el.getAttribute('draggable') === 'true'));
            const totalSteps = {steps};
            const stepDelay = {duration_ms} / totalSteps;
            const opts = {{ bubbles: true, cancelable: true, view: window }};
            if (useHtml5) {{
                const dt = new DataTransfer();
                el.dispatchEvent(new DragEvent('dragstart', {{ ...opts, dataTransfer: dt, clientX: startX, clientY: startY }}));
                let step = 0;
                const doStep = () => {{
                    step++;
                    const t = step / totalSteps;
                    const mx = startX + (endX - startX) * t;
                    const my = startY + (endY - startY) * t;
                    const cur = document.elementFromPoint(mx, my) || document.body;
                    cur.dispatchEvent(new DragEvent('dragenter', {{ ...opts, dataTransfer: dt, clientX: mx, clientY: my }}));
                    cur.dispatchEvent(new DragEvent('dragover', {{ ...opts, dataTransfer: dt, clientX: mx, clientY: my }}));
                    if (step < totalSteps) {{
                        setTimeout(doStep, stepDelay);
                    }} else {{
                        const dropEl = document.elementFromPoint(endX, endY) || document.body;
                        dropEl.dispatchEvent(new DragEvent('drop', {{ ...opts, dataTransfer: dt, clientX: endX, clientY: endY }}));
                        el.dispatchEvent(new DragEvent('dragend', {{ ...opts, dataTransfer: dt, clientX: endX, clientY: endY }}));
                        resolve({{ action: 'drag', strategy: 'html5dnd', from: {{ x: startX, y: startY }}, to: {{ x: endX, y: endY }}, steps: totalSteps, sourceTag: el.tagName.toLowerCase(), targetTag: dropEl.tagName.toLowerCase() }});
                    }}
                }};
                setTimeout(doStep, stepDelay);
            }} else {{
                el.dispatchEvent(new PointerEvent('pointerdown', {{ ...opts, clientX: startX, clientY: startY, button: 0, pointerId: 1 }}));
                el.dispatchEvent(new MouseEvent('mousedown', {{ ...opts, clientX: startX, clientY: startY, button: 0 }}));
                let step = 0;
                const doStep = () => {{
                    step++;
                    const t = step / totalSteps;
                    const mx = startX + (endX - startX) * t;
                    const my = startY + (endY - startY) * t;
                    const cur = document.elementFromPoint(mx, my) || document.body;
                    cur.dispatchEvent(new PointerEvent('pointermove', {{ ...opts, clientX: mx, clientY: my, pointerId: 1 }}));
                    cur.dispatchEvent(new MouseEvent('mousemove', {{ ...opts, clientX: mx, clientY: my }}));
                    if (step < totalSteps) {{
                        setTimeout(doStep, stepDelay);
                    }} else {{
                        const dropEl = document.elementFromPoint(endX, endY) || document.body;
                        dropEl.dispatchEvent(new PointerEvent('pointerup', {{ ...opts, clientX: endX, clientY: endY, button: 0, pointerId: 1 }}));
                        dropEl.dispatchEvent(new MouseEvent('mouseup', {{ ...opts, clientX: endX, clientY: endY, button: 0 }}));
                        resolve({{ action: 'drag', strategy: 'pointer', from: {{ x: startX, y: startY }}, to: {{ x: endX, y: endY }}, steps: totalSteps, sourceTag: el.tagName.toLowerCase(), targetTag: dropEl.tagName.toLowerCase() }});
                    }}
                }};
                setTimeout(doStep, stepDelay);
            }}
        }})"#
    );

    let timeout = duration_ms as u64 + 10_000;
    let result = exec_js(client, &script, timeout).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
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

/// Build JS that resolves a drag target to endX/endY variables.
/// Accepts @ref, CSS selector, or "x,y" coordinates.
fn resolve_target_js(target: &str, refs: &RefMap) -> Result<String, String> {
    // Check for coordinate pair like "300,400"
    if let Some((x_str, y_str)) = target.split_once(',') {
        if let (Ok(x), Ok(y)) = (x_str.trim().parse::<f64>(), y_str.trim().parse::<f64>()) {
            return Ok(format!("const endX = {x}; const endY = {y};"));
        }
    }

    let selector = resolve_to_selector(target, refs)?;
    let escaped = selector.replace('"', "\\\"");
    Ok(format!(
        "const tgt = document.querySelector(\"{escaped}\"); \
         if (!tgt) {{ resolve({{ error: 'Target not found: {escaped}' }}); return; }} \
         const tRect = tgt.getBoundingClientRect(); \
         const endX = tRect.x + tRect.width/2; const endY = tRect.y + tRect.height/2;"
    ))
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

/// Clear and fill an input element.
pub async fn fill(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
    value: &str,
) -> Result<(), String> {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    let action = format!(
        r#"
      el.focus();
      if (el.select) el.select();
      el.value = "";
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      el.value = "{escaped}";
      el.dispatchEvent(new Event('input', {{ bubbles: true }}));
      el.dispatchEvent(new Event('change', {{ bubbles: true }}));
      return {{ action: 'fill', tag: el.tagName.toLowerCase(), value: "{escaped}" }};
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

/// Type text character by character.
pub async fn type_text(
    client: &ConnectorClient,
    refs: &RefMap,
    target: &str,
    text: &str,
) -> Result<(), String> {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let action = format!(
        r#"
      el.focus();
      const chars = "{escaped}";
      for (const ch of chars) {{
        el.dispatchEvent(new KeyboardEvent('keydown', {{ key: ch, bubbles: true }}));
        el.dispatchEvent(new KeyboardEvent('keypress', {{ key: ch, bubbles: true }}));
        if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
          el.value += ch;
          el.dispatchEvent(new Event('input', {{ bubbles: true }}));
        }}
        el.dispatchEvent(new KeyboardEvent('keyup', {{ key: ch, bubbles: true }}));
      }}
      return {{ action: 'type', tag: el.tagName.toLowerCase(), text: "{escaped}" }};
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

/// Press a key on the focused element.
pub async fn press(client: &ConnectorClient, key: &str) -> Result<(), String> {
    let script = format!(
        r#"(() => {{
      const el = document.activeElement || document.body;
      el.dispatchEvent(new KeyboardEvent('keydown', {{ key: '{key}', bubbles: true }}));
      el.dispatchEvent(new KeyboardEvent('keyup', {{ key: '{key}', bubbles: true }}));
      return {{ action: 'press', key: '{key}', target: el.tagName.toLowerCase() }};
    }})()"#
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
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
            let escaped = t.replace('"', "\\\"");
            let result = exec_js(
                client,
                &format!(r#"document.querySelectorAll("{escaped}").length"#),
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
            let escaped = attr.replace('"', "\\\"");
            format!(r#"return el.getAttribute("{escaped}");"#)
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
pub async fn wait(
    client: &ConnectorClient,
    selector: Option<&str>,
    text: Option<&str>,
    timeout_ms: u64,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "wait_for",
        "timeout": timeout_ms,
        "window_id": "main",
    });
    if let Some(s) = selector {
        cmd["selector"] = json!(s);
    }
    if let Some(t) = text {
        cmd["text"] = json!(t);
    }
    let result = client.send_with_timeout(cmd, timeout_ms + 5000).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

/// Evaluate arbitrary JavaScript.
pub async fn eval_js(client: &ConnectorClient, script: &str) -> Result<(), String> {
    let result = exec_js(client, script, 30_000).await?;
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
    let mut cmd = json!({ "type": "console_logs", "lines": lines, "window_id": "main" });
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
            let secs = ts / 1000;
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            println!("{h:02}:{m:02}:{s:02} {:<5} {msg}", lvl.to_uppercase());
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    }
    Ok(())
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
    format: &str,
    quality: u8,
    max_width: Option<u32>,
    overwrite: bool,
    output_dir: Option<&Path>,
    name_hint: Option<&str>,
    instance: Option<&ConnectorInstance>,
) -> Result<(), String> {
    let mut cmd = json!({
        "type": "screenshot",
        "format": format,
        "quality": quality,
        "window_id": "main",
    });
    if let Some(w) = max_width {
        cmd["max_width"] = json!(w);
    }
    let result = client.send_with_timeout(cmd, 60_000).await?;

    let base64_data = result
        .get("base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No base64 data in screenshot response".to_string())?;

    let bytes = b64_decode(base64_data)?;
    let target = resolve_screenshot_path(output, output_dir, format, name_hint, instance)?;
    let artifact = write_screenshot_artifact(&target, &bytes, overwrite)?;

    let width = result.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
    let height = result.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
    let size_kb = bytes.len() / 1024;
    if artifact.resolved_from_collision {
        eprintln!(
            "Requested path existed; saved unique screenshot to {}",
            artifact.path.display()
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "path": artifact.path,
            "requestedPath": artifact.requested_path,
            "resolvedFromCollision": artifact.resolved_from_collision,
            "overwrote": artifact.overwrote,
            "sha256": sha256_hex(&bytes),
            "width": width,
            "height": height,
            "sizeKb": size_kb,
        }))
        .unwrap_or_default()
    );
    Ok(())
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
    instance: Option<&ConnectorInstance>,
) -> Result<PathBuf, String> {
    if let Some(output) = output {
        let path = PathBuf::from(output);
        if path.extension().is_some() {
            return Ok(path);
        }
        return Ok(path.join(default_screenshot_name(format, name_hint, instance)));
    }

    let dir = output_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        instance
            .and_then(|i| i.log_dir.clone())
            .unwrap_or_else(|| std::env::temp_dir().join("tauri-connector"))
            .join("artifacts")
            .join("screenshots")
    });
    Ok(dir.join(default_screenshot_name(format, name_hint, instance)))
}

fn default_screenshot_name(
    format: &str,
    name_hint: Option<&str>,
    instance: Option<&ConnectorInstance>,
) -> String {
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let app = instance
        .and_then(|i| i.app_name.as_deref().or(i.app_id.as_deref()))
        .map(slug)
        .unwrap_or_else(|| "app".to_string());
    let hint = name_hint.map(slug).unwrap_or_else(|| "full".to_string());
    let ext = match format {
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        _ => "png",
    };
    format!("{created}-{app}-main-{hint}-{}.{}", uuid8(), ext)
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

fn sha256_hex(bytes: &[u8]) -> String {
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

    for chunk in msg.chunks_exact(64) {
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
            "window_id": "main",
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
        .send(json!({ "type": "ipc_monitor", "action": action }))
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
        "all" => "all",
        _ => {
            return Err(format!(
                "Unknown target: {target}. Use: logs, ipc, events, all"
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

fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
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

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
