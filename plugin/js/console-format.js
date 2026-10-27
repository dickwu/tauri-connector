// Formats console arguments the way devtools does: printf-style %s %d %i %f
// %o %O substitution, `%c` styling dropped together with its CSS argument,
// and Error objects serialised as "Name: message" plus stack instead of `{}`.
// Objects are walked by hand so the output stays small and safe: Map and Set
// show their entries, DOM nodes and React elements become one-line
// summaries, framework internals (`__react*`, `_reactInternals`) are never
// read, depth and entry counts are capped, and every message is bounded.
(function connectorFormatConsole(args) {
  'use strict';
  const MAX_DEPTH = 6;
  const MAX_ENTRIES = 50;
  const MAX_STRING = 2000;
  const MAX_MESSAGE = 16384;
  const tagOf = v => Object.prototype.toString.call(v);
  const isError = v => v instanceof Error || (v && typeof v === 'object' && typeof v.message === 'string' && typeof v.stack === 'string');
  const errorText = e => {
    const head = (e.name || 'Error') + ': ' + e.message;
    const stack = typeof e.stack === 'string' ? e.stack : '';
    if (!stack) return head;
    return stack.indexOf(head) === 0 ? stack : head + '\n' + stack;
  };
  const isNode = v => {
    try { return typeof v.nodeType === 'number' && typeof v.nodeName === 'string'; } catch (_) { return false; }
  };
  const isWindow = v => {
    try { return v != null && v.window === v && typeof v.document === 'object'; } catch (_) { return false; }
  };
  const clip = (text, max) => text.length > max ? text.slice(0, max) + '…' : text;
  const nodeSummary = n => {
    try {
      if (n.nodeType === 1) {
        let out = '<' + String(n.nodeName).toLowerCase();
        if (n.id) out += '#' + n.id;
        const cls = typeof n.className === 'string' ? n.className.trim().split(/\s+/).filter(Boolean).slice(0, 3) : [];
        if (cls.length) out += '.' + cls.join('.');
        return out + '>';
      }
      if (n.nodeType === 3) return '#text "' + clip(String(n.nodeValue || '').trim(), 60) + '"';
      if (n.nodeType === 8) return '<!--' + clip(String(n.nodeValue || ''), 60) + '-->';
      if (n.nodeType === 9) return '#document';
      if (n.nodeType === 11) return '#document-fragment';
      return String(n.nodeName);
    } catch (_) { return '[Node]'; }
  };
  const isReactElement = v => {
    try { return typeof v.$$typeof === 'symbol' && 'props' in v && 'type' in v; } catch (_) { return false; }
  };
  const reactSummary = v => {
    try {
      const t = v.type;
      const name = typeof t === 'string' ? t : (t && (t.displayName || t.name)) || 'Component';
      return '<' + name + ' />';
    } catch (_) { return '[ReactElement]'; }
  };
  // React attaches fibers and props to DOM nodes and class instances under
  // these keys; walking them dumps the whole tree.
  const isInternalKey = k => /^__react|^_react|^__zone_symbol__|^__vue|^__ngContext__$/.test(k);
  const quote = text => JSON.stringify(clip(text, MAX_STRING));
  const serialize = (v, depth, ancestors, budget) => {
    if (budget.left <= 0) return '…';
    if (v === null) return 'null';
    switch (typeof v) {
      case 'string': return quote(v);
      case 'number': return isFinite(v) ? String(v) : 'null';
      case 'boolean': return String(v);
      case 'bigint': return '"' + v + 'n"';
      case 'undefined':
      case 'function':
      case 'symbol': return undefined;
    }
    if (isNode(v)) return quote(nodeSummary(v));
    if (isWindow(v)) return '"[Window]"';
    if (isReactElement(v)) return quote(reactSummary(v));
    if (ancestors.indexOf(v) !== -1) return '"[Circular]"';
    if (isError(v)) v = { name: v.name, message: v.message, stack: v.stack };
    else {
      try {
        if (typeof v.toJSON === 'function') {
          const json = v.toJSON();
          if (json !== v) return serialize(json, depth, ancestors, budget);
        }
      } catch (_) { return '"[unserializable]"'; }
    }
    const tag = tagOf(v);
    const nested = depth >= MAX_DEPTH;
    const next = ancestors.concat([v]);
    const walk = (items, render) => {
      const out = [];
      let count = 0;
      for (const item of items) {
        if (count >= MAX_ENTRIES || budget.left <= 0) { out.push('"…"'); break; }
        const text = render(item);
        if (text === undefined) continue;
        out.push(text);
        budget.left -= text.length + 1;
        count++;
      }
      return out.join(',');
    };
    if (tag === '[object Map]') {
      let size = 0; try { size = v.size; } catch (_) {}
      if (nested) return '"Map(' + size + ')"';
      let entries; try { entries = Array.from(Map.prototype.entries.call(v)); } catch (_) { entries = []; }
      return 'Map(' + size + ') {' + walk(entries, ([k, val]) => {
        const key = serialize(k, depth + 1, next, budget);
        const value = serialize(val, depth + 1, next, budget);
        return (key === undefined ? 'undefined' : key) + ' => ' + (value === undefined ? 'undefined' : value);
      }) + '}';
    }
    if (tag === '[object Set]') {
      let size = 0; try { size = v.size; } catch (_) {}
      if (nested) return '"Set(' + size + ')"';
      let values; try { values = Array.from(Set.prototype.values.call(v)); } catch (_) { values = []; }
      return 'Set(' + size + ') {' + walk(values, val => {
        const value = serialize(val, depth + 1, next, budget);
        return value === undefined ? 'undefined' : value;
      }) + '}';
    }
    if (tag === '[object WeakMap]' || tag === '[object WeakSet]' || tag === '[object Promise]') return '"' + tag.slice(8, -1) + '"';
    if (Array.isArray(v) || ArrayBuffer.isView(v)) {
      if (nested) return '"[Array(' + v.length + ')]"';
      return '[' + walk(Array.from(v), val => {
        const value = serialize(val, depth + 1, next, budget);
        return value === undefined ? 'null' : value;
      }) + ']';
    }
    let keys;
    try { keys = Object.keys(v).filter(k => !isInternalKey(k)); } catch (_) { return '"[unserializable]"'; }
    if (nested) return keys.length ? '"[Object]"' : '{}';
    return '{' + walk(keys, k => {
      let val;
      try { val = v[k]; } catch (_) { return undefined; }
      const value = serialize(val, depth + 1, next, budget);
      return value === undefined ? undefined : JSON.stringify(k) + ':' + value;
    }) + '}';
  };
  const show = v => {
    if (typeof v === 'string') return v;
    if (v === undefined) return 'undefined';
    if (typeof v === 'bigint') return v + 'n';
    if (typeof v === 'symbol' || typeof v === 'function') return String(v);
    if (isError(v)) return errorText(v);
    if (v !== null && typeof v === 'object') {
      if (isNode(v)) return nodeSummary(v);
      if (isWindow(v)) return '[Window]';
      if (isReactElement(v)) return reactSummary(v);
    }
    try {
      const text = serialize(v, 0, [], { left: MAX_MESSAGE });
      return text === undefined ? String(v) : text;
    } catch (_) {
      try { return String(v); } catch (_) { return '[unserializable]'; }
    }
  };
  const list = Array.from(args);
  let index = 0;
  const parts = [];
  if (typeof list[0] === 'string' && list[0].indexOf('%') !== -1) {
    index = 1;
    parts.push(list[0].replace(/%([sdifoOc%])/g, (match, kind) => {
      if (kind === '%') return '%';
      if (index >= list.length) return match;
      const value = list[index++];
      switch (kind) {
        case 'c': return '';
        case 's': return show(value);
        case 'd':
        case 'i': return typeof value === 'symbol' ? 'NaN' : String(Math.trunc(Number(value)));
        case 'f': return typeof value === 'symbol' ? 'NaN' : String(Number(value));
        default: return show(value);
      }
    }));
  }
  for (; index < list.length; index++) parts.push(show(list[index]));
  const message = parts.join(' ');
  return message.length > MAX_MESSAGE ? message.slice(0, MAX_MESSAGE) + '… [truncated ' + (message.length - MAX_MESSAGE) + ' chars]' : message;
})
