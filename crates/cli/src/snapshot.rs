//! Snapshot system with ref-based element addressing.
//!
//! Takes the raw DOM and builds an accessibility-tree style snapshot
//! with stable ref IDs (ref=e1, ref=e2, ...) for subsequent interactions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Options for building a snapshot script.
#[derive(Default)]
pub struct SnapshotOptions {
    pub interactive: bool,
    pub compact: bool,
    pub max_depth: usize,
    pub max_elements: usize,
    pub selector: Option<String>,
    pub mode: Option<String>,
    pub react_enrich: bool,
    pub follow_portals: bool,
    pub max_tokens: usize,
    pub no_split: bool,
}


/// Build the JavaScript that runs in the webview to produce a ref-annotated snapshot.
/// Returns JS that evaluates to `{ snapshot: string, refs: Record<string, RefEntry> }`.
/// Delegates to the bridge's `window.__CONNECTOR_SNAPSHOT__` injected by the plugin.
pub fn build_snapshot_script(opts: &SnapshotOptions) -> String {
    let mode = match &opts.mode {
        Some(m) => m.as_str(),
        None => if opts.interactive { "ai" } else { "accessibility" },
    };

    let root_selector = match &opts.selector {
        Some(s) => format!("'{}'", s.replace('\'', "\\'")),
        None => "null".to_string(),
    };

    let max_tokens_val = if opts.no_split { 0 } else { opts.max_tokens };

    let compact_block = if opts.compact {
        r#"
    const compactLines = snapshot.split('\n').filter(l => l.includes('ref=') || l.includes('"') || l.includes('subtree:'));
    snapshot = compactLines.join('\n');
    "#
    } else {
        ""
    };

    format!(
        r#"(() => {{
    const result = window.__CONNECTOR_SNAPSHOT__({{
        mode: '{mode}',
        selector: {root_selector},
        maxDepth: {max_depth},
        maxElements: {max_elements},
        maxTokens: {max_tokens_val},
        reactEnrich: {react_enrich},
        followPortals: {follow_portals},
        shadowDom: false,
    }});

    let snapshot = result.snapshot;
    {compact_block}

    return {{ snapshot: snapshot, refs: result.refs, subtrees: result.subtrees || [], meta: result.meta || {{}} }};
}})()"#,
        mode = mode,
        root_selector = root_selector,
        max_depth = opts.max_depth,
        max_elements = opts.max_elements,
        max_tokens_val = max_tokens_val,
        react_enrich = opts.react_enrich,
        follow_portals = opts.follow_portals,
        compact_block = compact_block,
    )
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
    if trimmed.starts_with('e') && trimmed[1..].chars().all(|c| c.is_ascii_digit()) && trimmed.len() > 1 {
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
