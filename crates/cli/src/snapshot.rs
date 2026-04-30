//! Snapshot system with ref-based element addressing.
//!
//! Takes the raw DOM and builds an accessibility-tree style snapshot
//! with stable ref IDs (ref=e1, ref=e2, ...) for subsequent interactions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use connector_client::discovery::ResolvedConnection;

/// Metadata for a referenced DOM element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    pub tag: String,
    pub role: Option<String>,
    pub name: String,
    pub selector: String,
    pub nth: Option<usize>,
}

pub type RefMap = HashMap<String, RefEntry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefCache {
    pub schema_version: u8,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub ws_port: u16,
    pub window_id: String,
    pub snapshot_id: Option<String>,
    pub snapshot_mode: String,
    pub created_at_ms: u64,
    pub refs: RefMap,
}

#[derive(Debug, Clone)]
pub struct SnapshotRefs {
    pub refs: RefMap,
    pub snapshot_id: Option<String>,
    pub snapshot_mode: String,
}

pub fn ref_cache_path(resolved: &ResolvedConnection, window_id: &str) -> PathBuf {
    let app = resolved
        .instance
        .as_ref()
        .and_then(|i| i.app_id.as_deref())
        .map(slug)
        .or_else(|| resolved.instance.as_ref().map(|i| format!("pid{}", i.pid)))
        .unwrap_or_else(|| "explicit".to_string());
    let file = format!("{app}-{}-{}.json", resolved.port, slug(window_id));
    cache_root().join(file)
}

pub fn legacy_ref_cache_path() -> PathBuf {
    std::env::temp_dir().join("tauri-connector-refs.json")
}

pub fn load_ref_cache(resolved: &ResolvedConnection, window_id: &str) -> Result<RefMap, String> {
    Ok(load_ref_cache_full(resolved, window_id)?
        .map(|cache| cache.refs)
        .unwrap_or_default())
}

pub fn load_ref_cache_full(
    resolved: &ResolvedConnection,
    window_id: &str,
) -> Result<Option<RefCache>, String> {
    let path = ref_cache_path(resolved, window_id);
    if let Ok(data) = std::fs::read_to_string(&path) {
        let cache: RefCache = serde_json::from_str(&data)
            .map_err(|e| format!("Ref cache {} is invalid: {e}", path.display()))?;
        validate_ref_cache(&cache, resolved, window_id)?;
        return Ok(Some(cache));
    }

    let legacy = legacy_ref_cache_path();
    if let Ok(data) = std::fs::read_to_string(&legacy) {
        if let Ok(refs) = serde_json::from_str::<RefMap>(&data) {
            eprintln!(
                "Warning: using legacy global ref cache at {}. Run snapshot again to create scoped refs for this app/window.",
                legacy.display()
            );
            return Ok(Some(RefCache {
                schema_version: 0,
                app_id: None,
                app_name: None,
                pid: None,
                ws_port: resolved.port,
                window_id: window_id.to_string(),
                snapshot_id: None,
                snapshot_mode: "legacy".to_string(),
                created_at_ms: 0,
                refs,
            }));
        }
    }

    Ok(None)
}

pub fn save_ref_cache(
    resolved: &ResolvedConnection,
    window_id: &str,
    snapshot: SnapshotRefs,
) -> Result<(), String> {
    let path = ref_cache_path(resolved, window_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create ref cache dir {}: {e}", parent.display()))?;
    }
    let instance = resolved.instance.as_ref();
    let cache = RefCache {
        schema_version: 1,
        app_id: instance.and_then(|i| i.app_id.clone()),
        app_name: instance.and_then(|i| i.app_name.clone()),
        pid: instance.map(|i| i.pid),
        ws_port: resolved.port,
        window_id: window_id.to_string(),
        snapshot_id: snapshot.snapshot_id,
        snapshot_mode: snapshot.snapshot_mode,
        created_at_ms: now_ms(),
        refs: snapshot.refs,
    };
    let json = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write ref cache {}: {e}", path.display()))
}

fn validate_ref_cache(
    cache: &RefCache,
    resolved: &ResolvedConnection,
    window_id: &str,
) -> Result<(), String> {
    let current = resolved.instance.as_ref();
    let current_app = current.and_then(|i| i.app_id.as_deref());
    if cache.ws_port != resolved.port
        || cache.window_id != window_id
        || (cache.app_id.as_deref().is_some()
            && current_app.is_some()
            && cache.app_id.as_deref() != current_app)
    {
        return Err(format!(
            "Ref cache belongs to app {} window {}, ws port {}, but current target is app {} window {}, ws port {}. Re-run snapshot for this target.",
            cache.app_id.as_deref().unwrap_or(cache.app_name.as_deref().unwrap_or("?")),
            cache.window_id,
            cache.ws_port,
            current_app.unwrap_or(current.and_then(|i| i.app_name.as_deref()).unwrap_or("?")),
            window_id,
            resolved.port
        ));
    }
    Ok(())
}

fn cache_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        Path::new(&home)
            .join(".cache")
            .join("tauri-connector")
            .join("refs")
    } else {
        std::env::temp_dir().join("tauri-connector").join("refs")
    }
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
        "main".to_string()
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Parse a ref string (@e1, ref=e1, e1) into a canonical ref ID.
pub fn parse_ref(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix('@') {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("ref=") {
        return Some(rest.to_string());
    }
    if trimmed.starts_with('e')
        && trimmed[1..].chars().all(|c| c.is_ascii_digit())
        && trimmed.len() > 1
    {
        return Some(trimmed.to_string());
    }
    None
}

/// Build JS that resolves an element by ref, then runs `action_js` on it.
/// The generated script defines `el` and expects `action_js` to use it.
pub fn build_resolve_and_act_script(
    selector_or_ref: &str,
    ref_map: &RefMap,
    action_js: &str,
) -> String {
    match parse_ref(selector_or_ref) {
        None => {
            // CSS selector path
            let escaped = selector_or_ref.replace('"', "\\\"").replace('\'', "\\'");
            format!(
                r#"(() => {{
      const el = document.querySelector("{escaped}");
      if (!el) return {{ error: 'Element not found: {escaped}' }};
      {action_js}
    }})()"#
            )
        }
        Some(ref_id) => {
            let Some(entry) = ref_map.get(&ref_id) else {
                return format!(
                    r#"(() => {{ return {{ error: 'Unknown ref: {ref_id}. Run snapshot first.' }}; }})()"#
                );
            };

            let escaped_selector = entry.selector.replace('"', "\\\"");
            let escaped_name = entry.name.replace('"', "\\\"");
            let escaped_name = if escaped_name.len() > 50 {
                &escaped_name[..50]
            } else {
                &escaped_name
            };
            let tag = &entry.tag;
            let role = entry.role.as_deref().unwrap_or("");

            format!(
                r#"(() => {{
      let el = null;

      // Strategy 1: CSS selector
      el = document.querySelector("{escaped_selector}");

      // Strategy 2: role + accessible name matching
      if (!el) {{
        const candidates = document.querySelectorAll("{tag}");
        for (const c of candidates) {{
          const al = c.getAttribute('aria-label') || '';
          const t = c.textContent?.trim().substring(0, 100) || '';
          if ((al.includes("{escaped_name}") || t.includes("{escaped_name}"))) {{
            el = c; break;
          }}
        }}
      }}

      // Strategy 3: all elements with matching role
      if (!el && "{role}") {{
        const byRole = document.querySelectorAll('[role="{role}"]');
        for (const c of byRole) {{
          const al = c.getAttribute('aria-label') || '';
          const t = c.textContent?.trim().substring(0, 100) || '';
          if (al.includes("{escaped_name}") || t.includes("{escaped_name}")) {{
            el = c; break;
          }}
        }}
      }}

      if (!el) return {{ error: 'Could not resolve ref={ref_id} ({tag} "{escaped_name}")' }};
      {action_js}
    }})()"#
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_client::discovery::{ConnectionSource, ConnectorInstance};

    fn resolved(app_id: Option<&str>, pid: u32, port: u16) -> ResolvedConnection {
        ResolvedConnection {
            host: "127.0.0.1".to_string(),
            port,
            source: ConnectionSource::PidFile,
            instance: Some(ConnectorInstance {
                pid,
                ws_port: port,
                mcp_port: Some(port + 1),
                bridge_port: Some(port - 1),
                app_name: Some("Example App".to_string()),
                app_id: app_id.map(str::to_string),
                log_dir: None,
                exe: None,
                started_at: None,
                pid_file: PathBuf::from(".connector.json"),
            }),
        }
    }

    #[test]
    fn ref_cache_key_includes_window_id() {
        let target = resolved(Some("com.example.app"), 123, 9555);
        let main = ref_cache_path(&target, "main");
        let settings = ref_cache_path(&target, "settings");
        assert_ne!(main.file_name(), settings.file_name());
        assert!(settings.to_string_lossy().contains("settings"));
    }

    #[test]
    fn ref_cache_key_includes_app_id_or_pid() {
        let app_a = resolved(Some("com.example.alpha"), 123, 9555);
        let app_b = resolved(Some("com.example.beta"), 123, 9555);
        let pid_only = resolved(None, 456, 9555);
        assert_ne!(
            ref_cache_path(&app_a, "main").file_name(),
            ref_cache_path(&app_b, "main").file_name()
        );
        assert!(ref_cache_path(&pid_only, "main")
            .to_string_lossy()
            .contains("pid456"));
    }
}
