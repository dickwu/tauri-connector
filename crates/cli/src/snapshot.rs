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

const INTERACTIVE_ROLES: &[&str] = &[
    "button", "link", "textbox", "checkbox", "radio", "combobox",
    "listbox", "menuitem", "menuitemcheckbox", "menuitemradio", "option",
    "searchbox", "slider", "spinbutton", "switch", "tab", "treeitem",
];

const CONTENT_ROLES: &[&str] = &[
    "heading", "cell", "gridcell", "columnheader", "rowheader",
    "listitem", "article", "region", "main", "navigation", "img",
];

/// Options for building a snapshot script.
pub struct SnapshotOptions {
    pub interactive: bool,
    pub compact: bool,
    pub max_depth: usize,
    pub selector: Option<String>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            interactive: false,
            compact: false,
            max_depth: 0,
            selector: None,
        }
    }
}

/// Build the JavaScript that runs in the webview to produce a ref-annotated snapshot.
/// Returns JS that evaluates to `{ snapshot: string, refs: Record<string, RefEntry> }`.
pub fn build_snapshot_script(opts: &SnapshotOptions) -> String {
    let interactive_json = serde_json::to_string(INTERACTIVE_ROLES).unwrap();
    let content_json = serde_json::to_string(CONTENT_ROLES).unwrap();

    let root_selector = match &opts.selector {
        Some(s) => format!(
            "document.querySelector(\"{}\")",
            s.replace('"', "\\\"")
        ),
        None => "document.body".to_string(),
    };

    let interactive_flag = if opts.interactive { "true" } else { "false" };
    let max_depth = opts.max_depth;

    let compact_block = if opts.compact {
        r#"
    const compactLines = snapshot.split('\n').filter(l => l.includes('ref=') || l.includes('"'));
    snapshot = compactLines.join('\n');
    "#
    } else {
        ""
    };

    format!(
        r#"(() => {{
    const INTERACTIVE = new Set({interactive_json});
    const CONTENT = new Set({content_json});
    const root = {root_selector};
    if (!root) return {{ snapshot: '(element not found)', refs: {{}} }};

    let nextRef = 1;
    const refs = {{}};
    const lines = [];

    const ROLE_MAP = {{
      'a': (el) => el.href ? 'link' : null,
      'button': () => 'button',
      'input': (el) => {{
        const t = String(el.type || 'text').toLowerCase();
        return {{ checkbox:'checkbox', radio:'radio', range:'slider',
                 search:'searchbox', number:'spinbutton' }}[t] || 'textbox';
      }},
      'select': () => 'combobox', 'textarea': () => 'textbox',
      'img': () => 'img', 'nav': () => 'navigation', 'main': () => 'main',
      'header': () => 'banner', 'footer': () => 'contentinfo',
      'aside': () => 'complementary', 'form': () => 'form',
      'table': () => 'table', 'ul': () => 'list', 'ol': () => 'list',
      'li': () => 'listitem',
      'h1': () => 'heading', 'h2': () => 'heading', 'h3': () => 'heading',
      'h4': () => 'heading', 'h5': () => 'heading', 'h6': () => 'heading',
    }};

    function getRole(el) {{
      const explicit = el.getAttribute('role');
      if (explicit) return explicit;
      const tag = el.tagName.toLowerCase();
      const fn_ = ROLE_MAP[tag];
      return fn_ ? fn_(el) : null;
    }}

    function getName(el) {{
      const label = el.getAttribute('aria-label');
      if (label) return label;
      const labelledBy = el.getAttribute('aria-labelledby');
      if (labelledBy) {{
        const ref_ = document.getElementById(labelledBy);
        if (ref_) return ref_.textContent.trim();
      }}
      if (el.tagName === 'IMG') return el.alt || '';
      if (['INPUT','SELECT','TEXTAREA'].includes(el.tagName)) {{
        if (el.id) {{
          const lbl = document.querySelector('label[for="' + el.id + '"]');
          if (lbl) return lbl.textContent.trim();
        }}
        return el.placeholder || '';
      }}
      if (['BUTTON','A','H1','H2','H3','H4','H5','H6'].includes(el.tagName)) {{
        return el.textContent.trim().substring(0, 100);
      }}
      return '';
    }}

    function getSelector(el) {{
      if (el.id) return '#' + el.id;
      const tag = el.tagName.toLowerCase();
      const cls = el.className && typeof el.className === 'string'
        ? el.className.trim().split(/\\s+/).filter(c => !c.includes('_') && c.length < 30).slice(0, 2)
        : [];
      if (cls.length > 0) return tag + '.' + cls.join('.');
      return tag;
    }}

    function shouldHaveRef(role, name) {{
      if (INTERACTIVE.has(role)) return true;
      if (CONTENT.has(role) && name) return true;
      return false;
    }}

    function walk(el, depth, maxD) {{
      if (maxD > 0 && depth > maxD) return;
      if (!el || !el.tagName) return;
      if (['SCRIPT','STYLE','NOSCRIPT','SVG','PATH','DEFS','CLIPPATH'].includes(el.tagName)) return;

      const tag = el.tagName.toLowerCase();
      const role = getRole(el);
      const name = getName(el);
      const indent = '  '.repeat(depth);

      let refId = null;
      const displayRole = role || tag;
      const wantsRef = shouldHaveRef(displayRole, name)
        || el.onclick || el.getAttribute('tabindex') !== null
        || (typeof getComputedStyle !== 'undefined' && getComputedStyle(el).cursor === 'pointer' && ['DIV','SPAN','LI','A'].includes(el.tagName));

      if (wantsRef) {{
        refId = 'e' + nextRef++;
        refs[refId] = {{
          tag: tag,
          role: role,
          name: name.substring(0, 100),
          selector: getSelector(el),
          nth: null,
        }};
      }}

      if ({interactive_flag} && !refId) {{
        for (const child of el.children) walk(child, depth, maxD);
        return;
      }}

      let line = indent + '- ' + displayRole;
      if (name) line += ' "' + name.substring(0, 100).replace(/"/g, '\\"') + '"';

      const props = [];
      if (tag.match(/^h[1-6]$/)) props.push('level=' + tag[1]);
      const checked = el.getAttribute('aria-checked') || (el.checked !== undefined ? String(el.checked) : null);
      if (checked) props.push('checked=' + checked);
      const expanded = el.getAttribute('aria-expanded');
      if (expanded) props.push('expanded=' + expanded);
      if (el.getAttribute('aria-selected') === 'true') props.push('selected');
      if (el.disabled) props.push('disabled');
      if (el.required) props.push('required');
      if (refId) props.push('ref=' + refId);
      if (props.length > 0) line += ' [' + props.join(', ') + ']';

      if (role === 'textbox' || role === 'searchbox' || role === 'combobox') {{
        const val = el.value || el.getAttribute('aria-valuenow') || '';
        if (val) line += ': ' + val.substring(0, 50);
      }}

      lines.push(line);

      for (const child of el.children) walk(child, depth + 1, maxD);
    }}

    walk(root, 0, {max_depth});

    let snapshot = lines.join('\n');
    {compact_block}

    return {{ snapshot, refs }};
  }})()"#
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
