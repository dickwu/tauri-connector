//! Command handlers for all supported operations.

use base64::Engine;
use regex::Regex;
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;
use tauri::{Emitter, Manager};

#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;

use crate::bridge::Bridge;
use crate::protocol::{AppInfo, BackendState, EnvInfo, Response, TauriInfo, WindowEntry};
#[allow(unused_imports)]
use crate::state::{DomEntry, EventEntry, PluginState, RefEntry, RefMap, SnapshotMeta};

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn parse_ref(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix('@') {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ref=") {
        return Some(rest.to_string());
    }
    if trimmed.starts_with('e')
        && trimmed.len() > 1
        && trimmed[1..].chars().all(|c| c.is_ascii_digit())
    {
        return Some(trimmed.to_string());
    }
    None
}

async fn cached_ref(
    state: &PluginState,
    window_id: &str,
    selector_or_ref: &str,
) -> Result<Option<(String, RefEntry, Option<String>, u64)>, String> {
    let Some(ref_id) = parse_ref(selector_or_ref) else {
        return Ok(None);
    };
    let Some(entry) = state.get_dom(window_id).await else {
        return Err(format!(
            "No snapshot refs cached for window '{window_id}'. Run webview_dom_snapshot(mode:'ai') again."
        ));
    };
    let Some(ref_entry) = entry.refs.get(&ref_id).cloned() else {
        return Err(format!(
            "Could not resolve ref={ref_id} in window {window_id}; run snapshot again."
        ));
    };
    let now = now_ms();
    let age = now.saturating_sub(entry.timestamp_ms());
    Ok(Some((ref_id, ref_entry, entry.snapshot_id, age)))
}

fn ref_resolver_block(var_name: &str, ref_id: &str, entry: &RefEntry) -> String {
    let ref_id_js = js_string(ref_id);
    let selector_js = js_string(&entry.selector);
    let name_js = js_string(&entry.name);
    let tag_js = js_string(&entry.tag);
    let role_js = js_string(entry.role.as_deref().unwrap_or(""));
    format!(
        r#"
        let {var_name} = null;
        {{
        const __ref = {ref_id_js};
        const __selector = {selector_js};
        const __name = {name_js};
        const __tag = {tag_js};
        const __role = {role_js};
        try {{ {var_name} = document.querySelector('[data-connector-ref="' + CSS.escape(__ref) + '"]'); }} catch (_) {{}}
        if (!{var_name} && __selector) {{
            try {{ {var_name} = document.querySelector(__selector); }} catch (_) {{}}
        }}
        if (!{var_name} && __tag) {{
            const candidates = document.querySelectorAll(__tag);
            for (const c of candidates) {{
                const al = c.getAttribute('aria-label') || '';
                const placeholder = c.getAttribute('placeholder') || '';
                const text = (c.textContent || '').trim().substring(0, 120);
                if (__name && (al.includes(__name) || placeholder.includes(__name) || text.includes(__name))) {{
                    {var_name} = c; break;
                }}
            }}
        }}
        if (!{var_name} && __role) {{
            const candidates = document.querySelectorAll('[role="' + CSS.escape(__role) + '"]');
            for (const c of candidates) {{
                const al = c.getAttribute('aria-label') || '';
                const text = (c.textContent || '').trim().substring(0, 120);
                if (__name && (al.includes(__name) || text.includes(__name))) {{
                    {var_name} = c; break;
                }}
            }}
        }}
        }}
        "#
    )
}

async fn element_resolver_block(
    state: &PluginState,
    window_id: &str,
    selector: &str,
    strategy: &str,
    var_name: &str,
) -> Result<String, String> {
    if let Some((ref_id, entry, snapshot_id, snapshot_age_ms)) =
        cached_ref(state, window_id, selector).await?
    {
        let block = ref_resolver_block(var_name, &ref_id, &entry);
        let error = serde_json::json!({
            "error": format!("Could not resolve ref={ref_id} in window {window_id}"),
            "staleRef": true,
            "snapshotId": snapshot_id,
            "snapshotAgeMs": snapshot_age_ms,
            "suggestion": "Run webview_dom_snapshot(mode:'ai') again, then retry with the new ref."
        });
        return Ok(format!(
            r#"{block}
            if (!{var_name}) return {error};"#
        ));
    }

    let selector_js = js_string(selector);
    let query = match strategy {
        "xpath" => format!(
            "document.evaluate({selector_js}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue"
        ),
        "text" => format!(
            "Array.from(document.querySelectorAll('*')).find(el => el.textContent && el.textContent.includes({selector_js}))"
        ),
        _ => format!("document.querySelector({selector_js})"),
    };
    Ok(format!(
        "const {var_name} = {query}; if (!{var_name}) return {{ error: 'Element not found', selector: {selector_js} }};"
    ))
}

async fn promise_element_resolver_block(
    state: &PluginState,
    window_id: &str,
    selector: &str,
    strategy: &str,
    var_name: &str,
    label: &str,
) -> Result<String, String> {
    if let Some((ref_id, entry, snapshot_id, snapshot_age_ms)) =
        cached_ref(state, window_id, selector).await?
    {
        let block = ref_resolver_block(var_name, &ref_id, &entry);
        let error = serde_json::json!({
            "error": format!("Could not resolve ref={ref_id} in window {window_id}"),
            "staleRef": true,
            "snapshotId": snapshot_id,
            "snapshotAgeMs": snapshot_age_ms,
            "suggestion": "Run webview_dom_snapshot(mode:'ai') again, then retry with the new ref."
        });
        return Ok(format!(
            r#"{block}
            if (!{var_name}) {{ resolve({error}); return; }}"#
        ));
    }

    let selector_js = js_string(selector);
    let query = match strategy {
        "xpath" => format!(
            "document.evaluate({selector_js}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue"
        ),
        "text" => format!(
            "Array.from(document.querySelectorAll('*')).find(el => el.textContent && el.textContent.includes({selector_js}))"
        ),
        _ => format!("document.querySelector({selector_js})"),
    };
    let label_js = js_string(label);
    Ok(format!(
        "const {var_name} = {query}; if (!{var_name}) {{ resolve({{ error: {label_js}, selector: {selector_js} }}); return; }}"
    ))
}

fn snapshot_meta_from_value(value: Option<&serde_json::Value>) -> SnapshotMeta {
    let Some(value) = value else {
        return SnapshotMeta::default();
    };
    SnapshotMeta {
        element_count: value
            .get("elementCount")
            .or_else(|| value.get("element_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        truncated: value
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        portal_count: value
            .get("portalCount")
            .or_else(|| value.get("portal_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        virtual_scroll_containers: value
            .get("virtualScrollContainers")
            .or_else(|| value.get("virtual_scroll_containers"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============ JavaScript Execution ============

pub async fn execute_js(id: &str, script: &str, window_id: &str, bridge: &Bridge) -> Response {
    match bridge
        .execute_js_for_window(script, 30_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), e),
    }
}

// ============ DOM Snapshot (via bridge JS) ============

#[allow(clippy::too_many_arguments)]
pub async fn dom_snapshot(
    id: &str,
    mode: &str,
    selector: Option<&str>,
    max_depth: Option<u64>,
    max_elements: Option<u64>,
    max_tokens: Option<u64>,
    react_enrich: bool,
    follow_portals: bool,
    shadow_dom: bool,
    window_id: &str,
    bridge: &Bridge,
    state: &PluginState,
) -> Response {
    // Validate mode to prevent JS injection
    let safe_mode = match mode {
        "ai" | "accessibility" | "structure" => mode,
        _ => return Response::error(id.to_string(), format!("Unknown snapshot mode: {mode}")),
    };

    let selector_arg = match selector {
        Some(s) => format!("'{}'", s.replace('\'', "\\'")),
        None => "null".to_string(),
    };

    let script = format!(
        "window.__CONNECTOR_SNAPSHOT__({{ mode: '{}', selector: {}, maxDepth: {}, maxElements: {}, maxTokens: {}, reactEnrich: {}, followPortals: {}, shadowDom: {} }})",
        safe_mode,
        selector_arg,
        max_depth.unwrap_or(0),
        max_elements.unwrap_or(0),
        max_tokens.unwrap_or(0),
        react_enrich,
        follow_portals,
        shadow_dom,
    );

    let result = match bridge
        .execute_js_for_window(&script, 15_000, window_id)
        .await
    {
        Ok(r) => r,
        Err(e) => return Response::error(id.to_string(), format!("DOM snapshot failed: {e}")),
    };

    let refs: RefMap = result
        .get("allRefs")
        .or_else(|| result.get("refs"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let snapshot = result
        .get("snapshot")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let meta = snapshot_meta_from_value(result.get("meta"));
    state
        .push_dom(DomEntry {
            window_id: window_id.to_string(),
            html: String::new(),
            text_content: String::new(),
            snapshot: snapshot.clone(),
            snapshot_mode: safe_mode.to_string(),
            refs,
            meta,
            timestamp: now_ms(),
            search_text: String::new(),
            snapshot_id: None,
        })
        .await;

    // If the JS engine did not split into subtrees, return as-is (backward compat).
    let subtrees = match result.get("subtrees").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr.clone(),
        _ => return Response::success(id.to_string(), result),
    };

    // --- Subtree file writing ---
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    let snapshots_dir = state.log_dir.join("snapshots");
    let session_dir = snapshots_dir.join(&snapshot_id);

    // Create secure directory (0700 on unix).
    let dir_ok = {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&session_dir).is_ok()
    };

    if !dir_ok {
        // Bounded fallback: return inline skeleton with splitFailed flag, NOT full content.
        let mut fallback = result.clone();
        if let Some(meta) = fallback.get_mut("meta").and_then(|m| m.as_object_mut()) {
            meta.insert("splitFailed".to_string(), serde_json::json!(true));
        }
        return Response::success(id.to_string(), fallback);
    }

    let skeleton = result
        .get("snapshot")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut subtree_files: Vec<serde_json::Value> = Vec::with_capacity(subtrees.len());
    let mut merged_search_text = String::from(skeleton);
    let mut write_failed = false;

    for (i, subtree) in subtrees.iter().enumerate() {
        let label = subtree
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let content = subtree
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let estimated_tokens = subtree
            .get("estimatedTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let filename = format!("subtree-{i}.txt");
        let final_path = session_dir.join(&filename);
        let tmp_path = session_dir.join(format!("subtree-{i}.txt.tmp"));

        // Atomic write: write to .tmp, then rename.
        match fs::write(&tmp_path, content) {
            Ok(()) => {
                if fs::rename(&tmp_path, &final_path).is_err() {
                    write_failed = true;
                    break;
                }
            }
            Err(_) => {
                write_failed = true;
                break;
            }
        }

        subtree_files.push(serde_json::json!({
            "name": filename,
            "label": label,
            "path": final_path.to_string_lossy(),
            "estimatedTokens": estimated_tokens,
        }));

        merged_search_text.push('\n');
        merged_search_text.push_str(content);
    }

    if write_failed {
        // Clean up partial writes and return bounded fallback.
        let _ = fs::remove_dir_all(&session_dir);
        let mut fallback = result.clone();
        if let Some(meta) = fallback.get_mut("meta").and_then(|m| m.as_object_mut()) {
            meta.insert("splitFailed".to_string(), serde_json::json!(true));
        }
        return Response::success(id.to_string(), fallback);
    }

    // Write layout.txt (copy of inline skeleton).
    let layout_path = session_dir.join("layout.txt");
    let layout_tmp = session_dir.join("layout.txt.tmp");
    if fs::write(&layout_tmp, skeleton)
        .and_then(|_| fs::rename(&layout_tmp, &layout_path))
        .is_err()
    {
        eprintln!("[connector] Failed to write layout.txt for snapshot {snapshot_id}");
    }

    // Write meta.json with snapshot metadata.
    let all_refs = result
        .get("allRefs")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let all_refs_path = session_dir.join("refs.json");
    let refs_tmp = session_dir.join("refs.json.tmp");
    if fs::write(
        &refs_tmp,
        serde_json::to_string_pretty(&all_refs).unwrap_or_default(),
    )
    .and_then(|_| fs::rename(&refs_tmp, &all_refs_path))
    .is_err()
    {
        eprintln!("[connector] Failed to write refs.json for snapshot {snapshot_id}");
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let session_meta = serde_json::json!({
        "snapshotId": snapshot_id,
        "timestamp": timestamp,
        "windowId": window_id,
        "refs": all_refs_path.to_string_lossy(),
        "files": subtree_files,
    });
    let meta_path = session_dir.join("meta.json");
    let meta_tmp = session_dir.join("meta.json.tmp");
    if fs::write(
        &meta_tmp,
        serde_json::to_string_pretty(&session_meta).unwrap_or_default(),
    )
    .and_then(|_| fs::rename(&meta_tmp, &meta_path))
    .is_err()
    {
        eprintln!("[connector] Failed to write meta.json for snapshot {snapshot_id}");
    }

    // Update search cache with merged full-text.
    {
        let mut cache = state.dom_cache.lock().await;
        if let Some(entry) = cache.get_mut(window_id) {
            entry.search_text = merged_search_text;
            entry.snapshot_id = Some(snapshot_id.clone());
        }
    }

    // Prune old snapshot sessions (keep newest 5).
    prune_old_snapshots(&snapshots_dir, &state.snapshot_prune_lock);

    // Enrich result: add file metadata, remove inline allRefs.
    let mut enriched = result.clone();
    if let Some(meta) = enriched.get_mut("meta").and_then(|m| m.as_object_mut()) {
        meta.insert("snapshotId".to_string(), serde_json::json!(snapshot_id));
        meta.insert("subtreeFiles".to_string(), serde_json::json!(subtree_files));
        meta.insert(
            "allRefsPath".to_string(),
            serde_json::json!(all_refs_path.to_string_lossy()),
        );
    }
    if let Some(obj) = enriched.as_object_mut() {
        obj.remove("allRefs");
        obj.remove("subtrees");
    }

    Response::success(id.to_string(), enriched)
}

/// Prune old snapshot sessions, keeping the newest `MAX_SESSIONS`.
/// Uses a std::sync::Mutex to serialize pruning across concurrent calls.
fn prune_old_snapshots(snapshots_dir: &std::path::Path, lock: &std::sync::Mutex<()>) {
    const MAX_SESSIONS: usize = 5;

    let _guard = match lock.lock() {
        Ok(g) => g,
        Err(_) => return, // poisoned lock, skip pruning
    };

    let entries = match fs::read_dir(snapshots_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    // Collect dirs with mtime for reliable ordering (UUID v4 is random)
    let mut dirs: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (t, e.path()))
        })
        .collect();

    // Sort newest first by mtime
    dirs.sort_unstable_by_key(|(t, _)| std::cmp::Reverse(*t));
    let dirs: Vec<std::path::PathBuf> = dirs.into_iter().map(|(_, p)| p).collect();

    if dirs.len() <= MAX_SESSIONS {
        return;
    }

    // Canonicalize the snapshots_dir once for symlink protection.
    let canonical_parent = match fs::canonicalize(snapshots_dir) {
        Ok(p) => p,
        Err(_) => return,
    };

    for dir in dirs.into_iter().skip(MAX_SESSIONS) {
        // Symlink protection: verify the resolved path lives under snapshots_dir.
        if let Ok(canonical) = fs::canonicalize(&dir)
            && canonical.starts_with(&canonical_parent)
        {
            let _ = fs::remove_dir_all(&canonical);
        }
    }
}

// ============ Cached DOM (pushed from frontend via invoke) ============

pub async fn get_cached_dom(id: &str, window_id: &str, state: &PluginState) -> Response {
    match state.get_dom(window_id).await {
        Some(entry) => Response::success(
            id.to_string(),
            serde_json::json!({
                "window_id": entry.window_id,
                "html": entry.html,
                "text_content": entry.text_content,
                "snapshot": entry.snapshot,
                "snapshot_mode": entry.snapshot_mode,
                "refs": entry.refs,
                "meta": entry.meta,
                "timestamp": entry.timestamp,
            }),
        ),
        None => Response::error(
            id.to_string(),
            format!(
                "No cached DOM for window '{window_id}'. The app may still be loading — wait a few seconds and retry."
            ),
        ),
    }
}

// ============ Element Operations ============

pub async fn find_element(
    id: &str,
    selector: &str,
    strategy: &str,
    target: Option<&str>,
    window_id: &str,
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
        "regex" => {
            let tgt = target.unwrap_or("text");
            let match_expr = match tgt {
                "class" => "el.className || ''",
                "id" => "el.id || ''",
                "attr" => "Array.from(el.attributes).map(a => a.name + '=' + a.value).join(' ')",
                "all" => "el.outerHTML",
                _ => "(el.textContent || '').trim()",
            };
            format!(
                r#"(() => {{
                    const re = new RegExp("{pat}", "i");
                    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
                    const elements = [];
                    while (walker.nextNode()) {{
                        const el = walker.currentNode;
                        const val = {match_expr};
                        if (re.test(val)) {{
                            const rect = el.getBoundingClientRect();
                            elements.push({{
                                tag: el.tagName.toLowerCase(),
                                id: el.id || null,
                                className: el.className || null,
                                text: (el.textContent || '').trim().substring(0, 200),
                                rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
                                visible: rect.width > 0 && rect.height > 0
                            }});
                        }}
                    }}
                    return {{ count: elements.length, elements }};
                }})()"#,
                pat = selector.replace('"', r#"\""#),
                match_expr = match_expr,
            )
        }
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

    match bridge
        .execute_js_for_window(&script, 10_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Find element failed: {e}")),
    }
}

pub async fn get_styles(
    id: &str,
    selector: &str,
    properties: Option<&[String]>,
    window_id: &str,
    bridge: &Bridge,
    state: &PluginState,
) -> Response {
    let props_filter = match properties {
        Some(props) => {
            let list: Vec<String> = props.iter().map(|p| format!("'{p}'")).collect();
            format!("const filter = [{}];", list.join(","))
        }
        None => "const filter = null;".to_string(),
    };

    let target = match element_resolver_block(state, window_id, selector, "css", "el").await {
        Ok(js) => js,
        Err(e) => return Response::error(id.to_string(), e),
    };
    let script = format!(
        r#"(() => {{
            {target}
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
        target = target,
    );

    match bridge
        .execute_js_for_window(&script, 5_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Get styles failed: {e}")),
    }
}

pub async fn get_pointed_element(id: &str, state: &PluginState) -> Response {
    match state.take_pointed_element().await {
        Some(element) => Response::success(id.to_string(), element),
        None => Response::error(
            id.to_string(),
            "No pointed element. User must Alt+Shift+Click an element first.",
        ),
    }
}

// ============ Interaction ============

#[allow(clippy::too_many_arguments)]
pub async fn interact(
    id: &str,
    action: &str,
    selector: Option<&str>,
    strategy: &str,
    x: Option<f64>,
    y: Option<f64>,
    direction: Option<&str>,
    distance: Option<f64>,
    window_id: &str,
    bridge: &Bridge,
    state: &PluginState,
) -> Response {
    let target = match (selector, x, y) {
        (Some(sel), _, _) => {
            match element_resolver_block(state, window_id, sel, strategy, "el").await {
                Ok(js) => js,
                Err(e) => return Response::error(id.to_string(), e),
            }
        }
        (None, Some(px), Some(py)) => {
            format!(
                "const el = document.elementFromPoint({px}, {py}); if (!el) return {{ error: 'No element at ({px}, {py})' }};"
            )
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
            // Full hover sequence: pointer events + mouse events + CSS :hover workaround.
            // Fires the same event sequence as a real cursor move (Playwright/CDP approach).
            r#"
            const hoverRect = el.getBoundingClientRect();
            const cx = hoverRect.x + hoverRect.width / 2;
            const cy = hoverRect.y + hoverRect.height / 2;
            const opts = { bubbles: true, cancelable: true, view: window, clientX: cx, clientY: cy };
            el.dispatchEvent(new PointerEvent('pointerover', opts));
            el.dispatchEvent(new PointerEvent('pointerenter', { ...opts, bubbles: false }));
            el.dispatchEvent(new MouseEvent('mouseover', opts));
            el.dispatchEvent(new MouseEvent('mouseenter', { ...opts, bubbles: false }));
            el.dispatchEvent(new PointerEvent('pointermove', opts));
            el.dispatchEvent(new MouseEvent('mousemove', opts));
            "#.to_string()
        }
        "hover-off" => {
            // Reverse hover: fire pointer/mouse leave events to dismiss dropdowns/tooltips.
            r#"
            const hoverRect = el.getBoundingClientRect();
            const opts = { bubbles: true, cancelable: true, view: window, clientX: 0, clientY: 0 };
            el.dispatchEvent(new PointerEvent('pointerout', opts));
            el.dispatchEvent(new PointerEvent('pointerleave', { ...opts, bubbles: false }));
            el.dispatchEvent(new MouseEvent('mouseout', opts));
            el.dispatchEvent(new MouseEvent('mouseleave', { ...opts, bubbles: false }));
            "#
            .to_string()
        }
        _ => {
            return Response::error(
                id.to_string(),
                format!(
                    "Unknown action: {action}. Use: click, double-click, focus, scroll, hover, hover-off, drag"
                ),
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

    match bridge
        .execute_js_for_window(&script, 5_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Interact failed: {e}")),
    }
}

// ============ Drag and Drop ============

#[allow(clippy::too_many_arguments)]
pub async fn drag(
    id: &str,
    selector: Option<&str>,
    strategy: &str,
    x: Option<f64>,
    y: Option<f64>,
    target_selector: Option<&str>,
    target_x: Option<f64>,
    target_y: Option<f64>,
    steps: u32,
    duration_ms: u32,
    drag_strategy: &str,
    window_id: &str,
    bridge: &Bridge,
    state: &PluginState,
) -> Response {
    // Resolve source element
    let source_js = match (selector, x, y) {
        (Some(sel), _, _) => {
            match promise_element_resolver_block(
                state,
                window_id,
                sel,
                strategy,
                "el",
                "Source element not found",
            )
            .await
            {
                Ok(js) => js,
                Err(e) => return Response::error(id.to_string(), e),
            }
        }
        (None, Some(px), Some(py)) => {
            format!(
                "const el = document.elementFromPoint({px}, {py}); if (!el) {{ resolve({{ error: 'No element at ({px}, {py})' }}); return; }}"
            )
        }
        _ => {
            return Response::error(id.to_string(), "Provide source selector or x/y coordinates");
        }
    };

    // Resolve target
    let target_js = match (target_selector, target_x, target_y) {
        (Some(sel), _, _) => {
            let target = match promise_element_resolver_block(
                state,
                window_id,
                sel,
                strategy,
                "tgt",
                "Target element not found",
            )
            .await
            {
                Ok(js) => js,
                Err(e) => return Response::error(id.to_string(), e),
            };
            format!(
                "{target} \
                 const tRect = tgt.getBoundingClientRect(); \
                 const endX = tRect.x + tRect.width/2; const endY = tRect.y + tRect.height/2;"
            )
        }
        (_, Some(tx), Some(ty)) => {
            format!("const endX = {tx}; const endY = {ty};")
        }
        _ => {
            return Response::error(
                id.to_string(),
                "Provide target_selector or target_x/target_y coordinates",
            );
        }
    };

    let script = format!(
        r#"new Promise((resolve) => {{
            {source_js}
            {target_js}
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

    let timeout = duration_ms as u64 + 5000;
    match bridge
        .execute_js_for_window(&script, timeout, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Drag failed: {e}")),
    }
}

pub async fn keyboard(
    id: &str,
    action: &str,
    text: Option<&str>,
    key: Option<&str>,
    modifiers: Option<&[String]>,
    window_id: &str,
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
            let mods_str = if mod_opts.is_empty() {
                String::new()
            } else {
                format!(", {}", mod_opts)
            };
            format!(
                r#"(() => {{
                    const el = document.activeElement;
                    if (!el) return {{ error: 'No focused element' }};
                    const chars = "{text}";
                    for (const ch of chars) {{
                        el.dispatchEvent(new KeyboardEvent('keydown', {{ key: ch{mods} }}));
                        el.dispatchEvent(new KeyboardEvent('keypress', {{ key: ch{mods} }}));
                        if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                            el.value += ch;
                            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        }}
                        el.dispatchEvent(new KeyboardEvent('keyup', {{ key: ch{mods} }}));
                    }}
                    return {{ typed: "{text}", target: el.tagName.toLowerCase() }};
                }})()"#,
                text = t.replace('"', r#"\""#),
                mods = mods_str,
            )
        }
        "press" => {
            let k = key.unwrap_or("Enter");
            let mods_str = if mod_opts.is_empty() {
                String::new()
            } else {
                format!(", {}", mod_opts)
            };
            format!(
                r#"(() => {{
                    const el = document.activeElement || document.body;
                    el.dispatchEvent(new KeyboardEvent('keydown', {{ key: '{key}', bubbles: true{mods} }}));
                    el.dispatchEvent(new KeyboardEvent('keyup', {{ key: '{key}', bubbles: true{mods} }}));
                    return {{ pressed: '{key}', target: el.tagName.toLowerCase() }};
                }})()"#,
                key = k,
                mods = mods_str,
            )
        }
        _ => {
            return Response::error(
                id.to_string(),
                format!("Unknown keyboard action: {action}. Use: type, press"),
            );
        }
    };

    match bridge
        .execute_js_for_window(&script, 5_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("Keyboard failed: {e}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn wait_for(
    id: &str,
    selector: Option<&str>,
    strategy: &str,
    text: Option<&str>,
    timeout: u64,
    window_id: &str,
    bridge: &Bridge,
    state: &PluginState,
) -> Response {
    let condition = if let Some(sel) = selector {
        match cached_ref(state, window_id, sel).await {
            Ok(Some((ref_id, entry, _, _))) => {
                let block = ref_resolver_block("__waitEl", &ref_id, &entry);
                format!("(() => {{ {block} return !!__waitEl; }})()")
            }
            Ok(None) => {
                let selector_js = js_string(sel);
                match strategy {
                    "xpath" => format!(
                        "document.evaluate({selector_js}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue"
                    ),
                    "text" => format!(
                        "Array.from(document.querySelectorAll('*')).find(el => el.textContent && el.textContent.includes({selector_js}))"
                    ),
                    _ => format!("document.querySelector({selector_js})"),
                }
            }
            Err(e) => return Response::error(id.to_string(), e),
        }
    } else if let Some(t) = text {
        format!("document.body.innerText.includes({})", js_string(t))
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

    match bridge
        .execute_js_for_window(&script, timeout + 2000, window_id)
        .await
    {
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

pub async fn window_info(id: &str, window_id: &str, app: Option<&tauri::AppHandle>) -> Response {
    let Some(app) = app else {
        return Response::error(id.to_string(), "App not initialized");
    };

    let Some(window) = app.get_webview_window(window_id) else {
        return Response::error(id.to_string(), format!("Window '{window_id}' not found"));
    };

    let size = window
        .inner_size()
        .map(|s| (s.width, s.height))
        .unwrap_or((0, 0));
    let position = window
        .inner_position()
        .map(|p| (p.x, p.y))
        .unwrap_or((0, 0));

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
    window_id: &str,
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

    match bridge
        .execute_js_for_window(&script, 15_000, window_id)
        .await
    {
        Ok(result) => Response::success(id.to_string(), result),
        Err(e) => Response::error(id.to_string(), format!("IPC command failed: {e}")),
    }
}

pub async fn ipc_monitor(
    id: &str,
    action: &str,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    match action {
        "start" => {
            state.set_ipc_monitoring(true).await;
            let _ = bridge
                .execute_js("window.__CONNECTOR_IPC_MONITOR__ = true", 2_000)
                .await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": true }))
        }
        "stop" => {
            state.set_ipc_monitoring(false).await;
            let _ = bridge
                .execute_js("window.__CONNECTOR_IPC_MONITOR__ = false", 2_000)
                .await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": false }))
        }
        _ => Response::error(
            id.to_string(),
            format!("Unknown action: {action}. Use: start, stop"),
        ),
    }
}

pub async fn ipc_get_captured(
    id: &str,
    filter: Option<&str>,
    pattern: Option<&str>,
    limit: usize,
    since: Option<u64>,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("ipc.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let filter_lower = filter.map(|f| f.to_lowercase());

    let _writer = state.ipc_writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<crate::state::IpcEvent>(
        &path,
        |line| {
            if let Some(ts) = since
                && let Some(pos) = line.find("\"timestamp\":")
            {
                let rest = &line[pos + 12..];
                if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next()
                    && let Ok(t) = val.parse::<u64>()
                    && t < ts
                {
                    return false;
                }
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            if let Some(ref f) = filter_lower {
                return line.to_lowercase().contains(f);
            }
            true
        },
        limit,
    );
    drop(_writer);

    match serde_json::to_value(&entries) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": entries.len(), "events": v }),
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
        Ok(()) => Response::success(id.to_string(), serde_json::json!({ "emitted": event_name })),
        Err(e) => Response::error(id.to_string(), format!("Emit failed: {e}")),
    }
}

// ============ Console Logs ============

#[allow(clippy::too_many_arguments)]
pub async fn console_logs(
    id: &str,
    lines: usize,
    filter: Option<&str>,
    pattern: Option<&str>,
    level: Option<&str>,
    window_id: &str,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("console.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let levels: Option<Vec<String>> =
        level.map(|l| l.split(',').map(|s| s.trim().to_lowercase()).collect());
    let filter_lower = filter.map(|f| f.to_lowercase());
    let wid = window_id.to_string();

    let _writer = state.console_writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<crate::state::LogEntry>(
        &path,
        |line| {
            if let Some(ref lvls) = levels {
                let has_level = lvls
                    .iter()
                    .any(|l| line.contains(&format!("\"level\":\"{}\"", l)));
                if !has_level {
                    return false;
                }
            }
            if !line.contains(&format!("\"window_id\":\"{}\"", wid)) {
                return false;
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            if let Some(ref f) = filter_lower {
                return line.to_lowercase().contains(f);
            }
            true
        },
        lines,
    );
    drop(_writer);

    match serde_json::to_value(&entries) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": entries.len(), "logs": v }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}

// ============ Screenshot ============

#[allow(clippy::too_many_arguments)]
pub async fn screenshot(
    id: &str,
    format: &str,
    quality: u8,
    max_width: Option<u32>,
    window_id: &str,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
    save: bool,
    output_dir: Option<&str>,
    name_hint: Option<&str>,
    overwrite: bool,
    selector: Option<&str>,
) -> Response {
    // Tier 1: xcap native capture (cross-platform, pixel-accurate)
    #[cfg(feature = "xcap")]
    if let Some(app) = app {
        match xcap_screenshot(app, window_id, format, quality, max_width).await {
            Ok(result) => {
                return finish_screenshot_response(
                    id, result, save, output_dir, name_hint, overwrite, selector, window_id, state,
                );
            }
            Err(e) => {
                eprintln!("[connector][screenshot] xcap failed, falling back to snapdom: {e}");
            }
        }
    }

    // suppress unused warnings when xcap feature is disabled
    let _ = app;

    // Tier 2: snapdom JS capture (requires @zumer/snapdom in frontend)
    match snapdom_screenshot(format, quality, max_width, window_id, bridge).await {
        Ok(result) => finish_screenshot_response(
            id, result, save, output_dir, name_hint, overwrite, selector, window_id, state,
        ),
        Err(e) => Response::error(id.to_string(), format!("Screenshot failed: {e}")),
    }
}

#[cfg(feature = "xcap")]
/// Cross-platform screenshot using xcap.
/// Captures the actual rendered window pixels — matches what the real web engine shows.
async fn xcap_screenshot(
    app: &tauri::AppHandle,
    window_id: &str,
    format: &str,
    quality: u8,
    max_width: Option<u32>,
) -> Result<serde_json::Value, String> {
    let tauri_window = app
        .get_webview_window(window_id)
        .ok_or_else(|| format!("Window '{window_id}' not found"))?;

    let title = tauri_window
        .title()
        .map_err(|e| format!("Failed to get window title: {e}"))?;

    // xcap uses blocking OS APIs — run on a blocking thread
    let captured = tokio::task::spawn_blocking(move || -> Result<image::RgbaImage, String> {
        let windows =
            xcap::Window::all().map_err(|e| format!("Failed to enumerate windows: {e}"))?;

        let target = windows
            .into_iter()
            .find(|w| {
                let is_minimized = w.is_minimized().unwrap_or(true);
                let w_title = w.title().unwrap_or_default();
                !is_minimized && w_title.contains(&title)
            })
            .ok_or_else(|| format!("No visible window matching title '{title}'"))?;

        target
            .capture_image()
            .map_err(|e| format!("xcap capture failed: {e}"))
    })
    .await
    .map_err(|e| format!("Screenshot task panicked: {e}"))??;

    let width = captured.width();
    let height = captured.height();

    let encoded = encode_image(captured, format, quality, max_width)?;

    let mime = match format {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&encoded);

    Ok(serde_json::json!({
        "base64": base64_data,
        "mimeType": mime,
        "width": width,
        "height": height,
        "method": "xcap",
    }))
}

/// Fallback screenshot using snapdom (@zumer/snapdom).
/// Requires the frontend project to have snapdom installed.
/// Captures the DOM as the web engine renders it — no re-rendering artifacts.
async fn snapdom_screenshot(
    format: &str,
    quality: u8,
    max_width: Option<u32>,
    window_id: &str,
    bridge: &Bridge,
) -> Result<serde_json::Value, String> {
    let mime = match format {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    let quality_f = f64::from(quality) / 100.0;
    let max_w = max_width.unwrap_or(0);

    let script = format!(
        r#"(async () => {{
            let snapdomFn;
            try {{
                const mod = await import('@zumer/snapdom');
                snapdomFn = mod.snapdom || mod.default;
            }} catch (_) {{
                if (typeof window.snapdom === 'function') {{
                    snapdomFn = window.snapdom;
                }} else if (typeof window.snapdom === 'object' && typeof window.snapdom.snapdom === 'function') {{
                    snapdomFn = window.snapdom.snapdom;
                }}
            }}
            if (!snapdomFn) {{
                throw new Error('snapdom not available — install @zumer/snapdom in your frontend project');
            }}
            const result = await snapdomFn(document.documentElement);
            const canvas = await result.toCanvas();
            let finalCanvas = canvas;
            const maxW = {max_w};
            if (maxW > 0 && canvas.width > maxW) {{
                const ratio = maxW / canvas.width;
                const newH = Math.round(canvas.height * ratio);
                finalCanvas = document.createElement('canvas');
                finalCanvas.width = maxW;
                finalCanvas.height = newH;
                const ctx = finalCanvas.getContext('2d');
                ctx.drawImage(canvas, 0, 0, maxW, newH);
            }}
            const dataUrl = finalCanvas.toDataURL('{mime}', {quality_f});
            const base64 = dataUrl.split(',')[1] || '';
            return {{
                base64: base64,
                mimeType: '{mime}',
                width: finalCanvas.width,
                height: finalCanvas.height,
                method: 'snapdom',
            }};
        }})()"#
    );

    bridge
        .execute_js_for_window(&script, 30_000, window_id)
        .await
}

#[allow(clippy::too_many_arguments)]
fn finish_screenshot_response(
    id: &str,
    mut result: serde_json::Value,
    save: bool,
    output_dir: Option<&str>,
    name_hint: Option<&str>,
    overwrite: bool,
    selector: Option<&str>,
    window_id: &str,
    state: &PluginState,
) -> Response {
    if save {
        match save_screenshot_artifact(
            &result, output_dir, name_hint, overwrite, selector, window_id, state,
        ) {
            Ok(artifact) => {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("artifact".to_string(), artifact);
                }
            }
            Err(e) => return Response::error(id.to_string(), e),
        }
    }
    Response::success(id.to_string(), result)
}

fn save_screenshot_artifact(
    result: &serde_json::Value,
    output_dir: Option<&str>,
    name_hint: Option<&str>,
    overwrite: bool,
    selector: Option<&str>,
    window_id: &str,
    state: &PluginState,
) -> Result<serde_json::Value, String> {
    let base64_data = result
        .get("base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No base64 data in screenshot response".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| format!("Invalid screenshot base64: {e}"))?;
    let mime = result
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("image/png");
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    let dir = output_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.log_dir.join("artifacts").join("screenshots"));
    fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Failed to create screenshot artifact dir {}: {e}",
            dir.display()
        )
    })?;
    let hint = name_hint
        .or(selector)
        .map(slug)
        .unwrap_or_else(|| "full".to_string());
    let created_at = now_ms();
    let requested = dir.join(format!(
        "{created_at}-app-{window_id}-{}-{hint}-{}.{}",
        if selector.is_some() {
            "element"
        } else {
            "full"
        },
        uuid8(),
        ext
    ));
    let (path, resolved_from_collision, overwrote) =
        write_unique_artifact(&requested, &bytes, overwrite)?;
    Ok(serde_json::json!({
        "id": path.file_stem().and_then(|s| s.to_str()).unwrap_or("screenshot"),
        "path": path.to_string_lossy(),
        "sha256": sha256_hex(&bytes),
        "createdAt": created_at,
        "overwrote": overwrote,
        "requestedPath": requested.to_string_lossy(),
        "resolvedFromCollision": resolved_from_collision,
        "source": if selector.is_some() { "selector" } else { "full_window" },
        "selector": selector,
        "windowId": window_id,
    }))
}

fn write_unique_artifact(
    requested: &std::path::Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<(std::path::PathBuf, bool, bool), String> {
    if overwrite {
        fs::write(requested, bytes)
            .map_err(|e| format!("Failed to write {}: {e}", requested.display()))?;
        return Ok((requested.to_path_buf(), false, true));
    }
    for attempt in 0..50 {
        let candidate = if attempt == 0 {
            requested.to_path_buf()
        } else {
            let stem = requested
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("screenshot");
            let ext = requested
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("png");
            requested.with_file_name(format!("{stem}-{attempt:04}-{}.{ext}", uuid8()))
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(bytes)
                    .map_err(|e| format!("Failed to write {}: {e}", candidate.display()))?;
                let _ = file.sync_all();
                return Ok((candidate, attempt > 0, false));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Failed to write {}: {e}", candidate.display())),
        }
    }
    Err("failed to allocate unique screenshot path after 50 attempts".to_string())
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
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "capture".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
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

#[cfg(feature = "xcap")]
/// Encode an RgbaImage to the requested format, optionally resizing.
fn encode_image(
    image: image::RgbaImage,
    format: &str,
    quality: u8,
    max_width: Option<u32>,
) -> Result<Vec<u8>, String> {
    let img = image::DynamicImage::ImageRgba8(image);

    let img = match max_width {
        Some(mw) if img.width() > mw => {
            img.resize(mw, u32::MAX, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    };

    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);

    match format {
        "jpeg" | "jpg" => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
            img.write_with_encoder(encoder)
                .map_err(|e| format!("JPEG encode error: {e}"))?;
        }
        "webp" => {
            img.write_to(&mut cursor, image::ImageFormat::WebP)
                .map_err(|e| format!("WebP encode error: {e}"))?;
        }
        _ => {
            img.write_to(&mut cursor, image::ImageFormat::Png)
                .map_err(|e| format!("PNG encode error: {e}"))?;
        }
    }

    Ok(buf)
}

// ============ Log Management ============

pub async fn clear_logs(id: &str, source: &str, state: &PluginState) -> Response {
    async fn clear(writer: &Arc<tokio::sync::Mutex<std::io::BufWriter<std::fs::File>>>) {
        let mut w = writer.lock().await;
        let _ = w.flush();
        let file = w.get_mut();
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
    }

    match source {
        "console" => clear(&state.console_writer).await,
        "ipc" => clear(&state.ipc_writer).await,
        "events" => clear(&state.event_writer).await,
        "all" => {
            clear(&state.console_writer).await;
            clear(&state.ipc_writer).await;
            clear(&state.event_writer).await;
        }
        _ => {
            return Response::error(
                id.to_string(),
                format!("Unknown source: {source}. Use: console, ipc, events, all"),
            );
        }
    }

    Response::success(
        id.to_string(),
        serde_json::json!({ "cleared": true, "source": source }),
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn read_log_file(
    id: &str,
    source: &str,
    lines: usize,
    level: Option<&str>,
    pattern: Option<&str>,
    since: Option<u64>,
    window_id: Option<&str>,
    state: &PluginState,
) -> Response {
    let (path, writer) = match source {
        "console" => (state.log_dir.join("console.log"), &state.console_writer),
        "ipc" => (state.log_dir.join("ipc.log"), &state.ipc_writer),
        "events" => (state.log_dir.join("events.log"), &state.event_writer),
        _ => return Response::error(id.to_string(), format!("Unknown source: {source}")),
    };

    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let levels: Option<Vec<String>> = if source == "console" {
        level.map(|l| l.split(',').map(|s| s.trim().to_lowercase()).collect())
    } else {
        None
    };
    let wid = window_id.map(|s| s.to_string());

    let _w = writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<serde_json::Value>(
        &path,
        |line| {
            if let Some(ts) = since
                && let Some(pos) = line.find("\"timestamp\":")
            {
                let rest = &line[pos + 12..];
                if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next()
                    && let Ok(t) = val.parse::<u64>()
                    && t < ts
                {
                    return false;
                }
            }
            if let Some(ref lvls) = levels {
                let has_level = lvls
                    .iter()
                    .any(|l| line.contains(&format!("\"level\":\"{}\"", l)));
                if !has_level {
                    return false;
                }
            }
            if let Some(ref wid) = wid
                && source != "ipc"
                && !line.contains(&format!("\"window_id\":\"{}\"", wid))
            {
                return false;
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            true
        },
        lines,
    );
    drop(_w);

    Response::success(
        id.to_string(),
        serde_json::json!({ "source": source, "count": entries.len(), "entries": entries }),
    )
}

// ============ Event Listeners ============

pub async fn ipc_listen(
    id: &str,
    action: &str,
    events: Option<&[String]>,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    match action {
        "start" => {
            let Some(event_names) = events else {
                return Response::error(id.to_string(), "events parameter required for start");
            };
            let listeners = state.event_listeners.lock().await;
            let new_events: Vec<&String> = event_names
                .iter()
                .filter(|e| !listeners.contains(e))
                .collect();

            if new_events.is_empty() {
                return Response::success(
                    id.to_string(),
                    serde_json::json!({
                        "listening": *listeners,
                        "added": 0,
                    }),
                );
            }

            let events_js: Vec<String> = new_events.iter().map(|e| {
                format!(
                    "window.__TAURI__.event.listen('{ev}', function(ev) {{\
                        var ipc = window.__CONNECTOR_ORIG_INVOKE__ || window.__TAURI_INTERNALS__.invoke;\
                        ipc('plugin:connector|push_event', {{\
                            payload: {{ event: '{ev}', payload: ev.payload, timestamp: Date.now(), windowId: ev.windowLabel || 'main' }}\
                        }}).catch(function(){{}});\
                    }}).then(function(unlisten) {{\
                        window.__CONNECTOR_EVENT_LISTENERS__ = window.__CONNECTOR_EVENT_LISTENERS__ || {{}};\
                        window.__CONNECTOR_EVENT_LISTENERS__['{ev}'] = unlisten;\
                    }});",
                    ev = e,
                )
            }).collect();

            let script = events_js.join("\n");
            drop(listeners); // release lock before bridge call
            match bridge.execute_js(&script, 5_000).await {
                Ok(_) => {
                    let mut listeners = state.event_listeners.lock().await;
                    for e in &new_events {
                        listeners.push((*e).clone());
                    }
                    Response::success(
                        id.to_string(),
                        serde_json::json!({
                            "listening": *listeners,
                            "added": new_events.len(),
                        }),
                    )
                }
                Err(e) => {
                    Response::error(id.to_string(), format!("Failed to register listeners: {e}"))
                }
            }
        }
        "stop" => {
            let script = r#"(function() {
                var listeners = window.__CONNECTOR_EVENT_LISTENERS__ || {};
                Object.values(listeners).forEach(function(unlisten) {
                    if (typeof unlisten === 'function') unlisten();
                });
                window.__CONNECTOR_EVENT_LISTENERS__ = {};
            })()"#;
            let _ = bridge.execute_js(script, 5_000).await;

            let mut listeners = state.event_listeners.lock().await;
            listeners.clear();
            Response::success(
                id.to_string(),
                serde_json::json!({ "listening": [], "stopped": true }),
            )
        }
        _ => Response::error(
            id.to_string(),
            format!("Unknown action: {action}. Use: start, stop"),
        ),
    }
}

pub async fn event_get_captured(
    id: &str,
    event: Option<&str>,
    pattern: Option<&str>,
    limit: usize,
    since: Option<u64>,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("events.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };

    let _w = state.event_writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<serde_json::Value>(
        &path,
        |line| {
            if let Some(ts) = since
                && let Some(pos) = line.find("\"timestamp\":")
            {
                let rest = &line[pos + 12..];
                if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next()
                    && let Ok(t) = val.parse::<u64>()
                    && t < ts
                {
                    return false;
                }
            }
            if let Some(ev) = event
                && !line.contains(&format!("\"event\":\"{}\"", ev))
            {
                return false;
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            true
        },
        limit,
    );
    drop(_w);

    Response::success(
        id.to_string(),
        serde_json::json!({ "count": entries.len(), "entries": entries }),
    )
}

// ============ Snapshot Search ============

pub async fn search_snapshot(
    id: &str,
    pattern: &str,
    context: usize,
    mode: &str,
    window_id: &str,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    let context = context.min(10);

    // Check cached snapshot first (< 10s old)
    let snapshot_text = {
        let cache = state.dom_cache.lock().await;
        if let Some(entry) = cache.get(window_id) {
            let now = now_ms();
            if entry.snapshot_mode == mode && now.saturating_sub(entry.timestamp_ms()) < 10_000 {
                let text = if entry.search_text.is_empty() {
                    entry.snapshot.clone()
                } else {
                    entry.search_text.clone()
                };
                Some(text)
            } else {
                None
            }
        } else {
            None
        }
    };

    let snapshot = match snapshot_text {
        Some(s) => s,
        None => {
            let script = format!(
                "JSON.stringify(window.__CONNECTOR_SNAPSHOT__({{ mode: '{}', maxTokens: 0 }}).snapshot)",
                mode
            );
            match bridge
                .execute_js_for_window(&script, 15_000, window_id)
                .await
            {
                Ok(val) => {
                    let s = val.as_str().unwrap_or("").to_string();
                    if s.is_empty() {
                        return Response::error(
                            id.to_string(),
                            "Snapshot returned empty — page may still be loading",
                        );
                    }
                    s
                }
                Err(e) => return Response::error(id.to_string(), format!("Snapshot failed: {e}")),
            }
        }
    };

    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
    };

    let all_lines: Vec<&str> = snapshot.lines().collect();
    let mut matches: Vec<serde_json::Value> = Vec::new();
    let mut last_end: usize = 0;

    for (i, line) in all_lines.iter().enumerate() {
        if re.is_match(line) {
            let ctx_start = i.saturating_sub(context);
            let ctx_end = (i + context + 1).min(all_lines.len());
            let actual_start = if ctx_start < last_end {
                last_end
            } else {
                ctx_start
            };
            last_end = ctx_end;

            if actual_start < ctx_end {
                let ctx_lines: Vec<&str> = all_lines[actual_start..ctx_end].to_vec();
                matches.push(serde_json::json!({
                    "line": i + 1,
                    "content": line,
                    "context": ctx_lines,
                }));
            } else {
                matches.push(serde_json::json!({
                    "line": i + 1,
                    "content": line,
                    "context": [],
                }));
            }
        }
    }

    Response::success(
        id.to_string(),
        serde_json::json!({
            "matches": matches,
            "total": matches.len(),
            "pattern": pattern,
        }),
    )
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
