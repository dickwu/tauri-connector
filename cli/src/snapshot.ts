/**
 * Snapshot system with ref-based element addressing.
 *
 * Takes the raw DOM and builds an accessibility-tree style snapshot
 * with stable ref IDs (ref=e1, ref=e2, ...) that can be used for
 * subsequent interactions (click, hover, fill, etc.).
 *
 * Inspired by vercel-labs/agent-browser's snapshot format.
 */

export interface RefEntry {
  tag: string
  role: string | null
  name: string
  selector: string
  nth: number | null
}

export type RefMap = Map<string, RefEntry>

const INTERACTIVE_ROLES = new Set([
  'button', 'link', 'textbox', 'checkbox', 'radio', 'combobox',
  'listbox', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'option',
  'searchbox', 'slider', 'spinbutton', 'switch', 'tab', 'treeitem',
])

const CONTENT_ROLES = new Set([
  'heading', 'cell', 'gridcell', 'columnheader', 'rowheader',
  'listitem', 'article', 'region', 'main', 'navigation', 'img',
])

/**
 * JS to execute in the webview that builds a ref-annotated accessibility tree.
 * Returns { snapshot: string, refs: Record<string, RefEntry> }
 */
export function buildSnapshotScript(options: {
  interactive?: boolean
  compact?: boolean
  maxDepth?: number
  selector?: string
}): string {
  const { interactive = false, compact = false, maxDepth = 0, selector } = options
  const rootSelector = selector ? `document.querySelector("${selector.replace(/"/g, '\\"')}")` : 'document.body'

  return `(() => {
    const INTERACTIVE = new Set(${JSON.stringify([...INTERACTIVE_ROLES])});
    const CONTENT = new Set(${JSON.stringify([...CONTENT_ROLES])});
    const root = ${rootSelector};
    if (!root) return { snapshot: '(element not found)', refs: {} };

    let nextRef = 1;
    const refs = {};
    const lines = [];

    const ROLE_MAP = {
      'a': (el) => el.href ? 'link' : null,
      'button': () => 'button',
      'input': (el) => {
        const t = String(el.type || 'text').toLowerCase();
        return { checkbox:'checkbox', radio:'radio', range:'slider',
                 search:'searchbox', number:'spinbutton' }[t] || 'textbox';
      },
      'select': () => 'combobox', 'textarea': () => 'textbox',
      'img': () => 'img', 'nav': () => 'navigation', 'main': () => 'main',
      'header': () => 'banner', 'footer': () => 'contentinfo',
      'aside': () => 'complementary', 'form': () => 'form',
      'table': () => 'table', 'ul': () => 'list', 'ol': () => 'list',
      'li': () => 'listitem',
      'h1': () => 'heading', 'h2': () => 'heading', 'h3': () => 'heading',
      'h4': () => 'heading', 'h5': () => 'heading', 'h6': () => 'heading',
    };

    function getRole(el) {
      const explicit = el.getAttribute('role');
      if (explicit) return explicit;
      const tag = el.tagName.toLowerCase();
      const fn = ROLE_MAP[tag];
      return fn ? fn(el) : null;
    }

    function getName(el) {
      const label = el.getAttribute('aria-label');
      if (label) return label;
      const labelledBy = el.getAttribute('aria-labelledby');
      if (labelledBy) {
        const ref = document.getElementById(labelledBy);
        if (ref) return ref.textContent.trim();
      }
      if (el.tagName === 'IMG') return el.alt || '';
      if (['INPUT','SELECT','TEXTAREA'].includes(el.tagName)) {
        if (el.id) {
          const lbl = document.querySelector('label[for="' + el.id + '"]');
          if (lbl) return lbl.textContent.trim();
        }
        return el.placeholder || '';
      }
      if (['BUTTON','A','H1','H2','H3','H4','H5','H6'].includes(el.tagName)) {
        return el.textContent.trim().substring(0, 100);
      }
      return '';
    }

    function getSelector(el) {
      if (el.id) return '#' + el.id;
      const tag = el.tagName.toLowerCase();
      const cls = el.className && typeof el.className === 'string'
        ? el.className.trim().split(/\\s+/).filter(c => !c.includes('_') && c.length < 30).slice(0, 2)
        : [];
      if (cls.length > 0) return tag + '.' + cls.join('.');
      return tag;
    }

    function shouldHaveRef(role, name) {
      if (INTERACTIVE.has(role)) return true;
      if (CONTENT.has(role) && name) return true;
      return false;
    }

    function walk(el, depth, maxD) {
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

      if (wantsRef) {
        refId = 'e' + nextRef++;
        refs[refId] = {
          tag: tag,
          role: role,
          name: name.substring(0, 100),
          selector: getSelector(el),
          nth: null,
        };
      }

      if (${interactive} && !refId) {
        // In interactive mode, skip non-interactive nodes but recurse children
        for (const child of el.children) walk(child, depth, maxD);
        return;
      }

      // Build line
      let line = indent + '- ' + displayRole;
      if (name) line += ' "' + name.substring(0, 100).replace(/"/g, '\\\\"') + '"';

      // Properties
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

      // Value
      if (role === 'textbox' || role === 'searchbox' || role === 'combobox') {
        const val = el.value || el.getAttribute('aria-valuenow') || '';
        if (val) line += ': ' + val.substring(0, 50);
      }

      lines.push(line);

      for (const child of el.children) walk(child, depth + 1, maxD);
    }

    walk(root, 0, ${maxDepth});

    let snapshot = lines.join('\\n');
    ${compact ? `
    // Compact mode: remove structural lines with no ref
    const compactLines = snapshot.split('\\n').filter(l => l.includes('ref=') || l.includes('"'));
    snapshot = compactLines.join('\\n');
    ` : ''}

    return { snapshot, refs };
  })()`
}

/**
 * Parse a ref string (@e1, ref=e1, e1) into a canonical ref ID.
 */
export function parseRef(input: string): string | null {
  const trimmed = input.trim()
  if (trimmed.startsWith('@')) return trimmed.slice(1)
  if (trimmed.startsWith('ref=')) return trimmed.slice(4)
  if (/^e\d+$/.test(trimmed)) return trimmed
  return null
}

/**
 * Build JS to resolve an element by ref using the stored ref data.
 */
export function resolveRefScript(ref: RefEntry): string {
  const { tag, role, name, selector } = ref

  // Try multiple strategies to find the element
  return `(() => {
    // Strategy 1: CSS selector
    let el = document.querySelector("${selector.replace(/"/g, '\\"')}");
    if (el) return el;

    // Strategy 2: role + name matching
    const allEls = document.querySelectorAll("${tag}");
    for (const candidate of allEls) {
      const r = candidate.getAttribute('role') || null;
      const n = candidate.getAttribute('aria-label') || candidate.textContent?.trim().substring(0, 100) || '';
      if ((r === "${role}" || ${!role ? 'true' : 'false'}) && n.includes("${name.replace(/"/g, '\\"').substring(0, 50)}")) {
        return candidate;
      }
    }
    return null;
  })()`
}
