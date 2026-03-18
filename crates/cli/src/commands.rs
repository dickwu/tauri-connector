//! CLI command implementations.

use connector_client::ConnectorClient;
use serde_json::{json, Value};

use crate::snapshot::{build_resolve_and_act_script, build_snapshot_script, RefMap, SnapshotOptions};

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
pub async fn snapshot(
    client: &ConnectorClient,
    interactive: bool,
    compact: bool,
    max_depth: usize,
    selector: Option<String>,
) -> Result<RefMap, String> {
    let opts = SnapshotOptions {
        interactive,
        compact,
        max_depth,
        selector,
    };
    let script = build_snapshot_script(&opts);
    let result = exec_js(client, &script, 30_000).await?;

    let snapshot_text = result
        .get("snapshot")
        .and_then(|v| v.as_str())
        .unwrap_or("(no snapshot)");

    println!("{snapshot_text}");

    let refs: RefMap = result
        .get("refs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let count = refs.len();
    eprintln!("\n{count} refs captured");

    Ok(refs)
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}

/// Focus an element.
pub async fn focus(client: &ConnectorClient, refs: &RefMap, target: &str) -> Result<(), String> {
    let script = build_resolve_and_act_script(
        target,
        refs,
        "el.focus(); return { action: 'focus', tag: el.tagName.toLowerCase() };",
    );
    let result = exec_js(client, &script, 30_000).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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

    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
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
            "Usage: get <text|html|value|attr|box|styles> <@ref|selector> [attr-name]".to_string()
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
        _ => println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()),
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
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}

/// Evaluate arbitrary JavaScript.
pub async fn eval_js(client: &ConnectorClient, script: &str) -> Result<(), String> {
    let result = exec_js(client, script, 30_000).await?;
    match &result {
        Value::String(s) => println!("{s}"),
        _ => println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()),
    }
    Ok(())
}

/// Read console logs.
pub async fn logs(
    client: &ConnectorClient,
    lines: usize,
    filter: Option<&str>,
) -> Result<(), String> {
    let filter_clause = match filter {
        Some(f) => {
            let escaped = f.to_lowercase().replace('"', "\\\"");
            format!(
                r#".filter(l => l.message.toLowerCase().includes("{escaped}"))"#
            )
        }
        None => String::new(),
    };

    let script = format!(
        r#"(() => {{
      const logs = (window.__CONNECTOR_LOGS__ || []){filter_clause};
      return logs.slice(-{lines});
    }})()"#
    );

    let result = exec_js(client, &script, 30_000).await?;
    if let Some(entries) = result.as_array() {
        for entry in entries {
            let ts = entry.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
            let level = entry
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("LOG");
            let msg = entry
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let secs = ts / 1000;
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            println!("{h:02}:{m:02}:{s:02} {:<5} {msg}", level.to_uppercase());
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    }
    Ok(())
}

/// Get app backend state.
pub async fn state(client: &ConnectorClient) -> Result<(), String> {
    let result = client.send(json!({ "type": "backend_state" })).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}

/// List app windows.
pub async fn windows(client: &ConnectorClient) -> Result<(), String> {
    let result = client.send(json!({ "type": "window_list" })).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}
