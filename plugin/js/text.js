// Text matching shared by the `text` locator strategy and `wait --text`.
// Normalises NBSP and other Unicode spaces so rendered text matches what a
// user typed, and resolves a text locator to the deepest matching element
// rather than the first ancestor (`<html>`) whose textContent contains it.
(function connectorText() {
  'use strict';
  const SKIP = /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE)$/;
  const norm = value => String(value == null ? '' : value)
    .replace(/[\s\u00a0\u1680\u2000-\u200b\u2028\u2029\u202f\u205f\u3000\ufeff]+/g, ' ')
    .trim();
  const ownText = el => {
    let out = '';
    const walk = node => {
      for (let child = node.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 3) out += child.nodeValue;
        else if (child.nodeType === 1 && !SKIP.test(child.tagName)) walk(child);
      }
    };
    walk(el);
    return out;
  };
  const documents = doc => {
    const out = [doc];
    let frames = [];
    try { frames = Array.from(doc.querySelectorAll('iframe,frame')); } catch (_) {}
    for (const frame of frames) {
      try {
        const inner = frame.contentDocument;
        if (inner && inner !== doc) out.push(...documents(inner));
      } catch (_) { /* cross-origin frame */ }
    }
    return out;
  };
  const fieldValues = doc => {
    const values = [];
    try {
      for (const field of doc.querySelectorAll('input,textarea,select')) {
        const type = String(field.type || '').toLowerCase();
        if (type === 'password' || type === 'hidden') continue;
        if (field.value) values.push(field.value);
      }
    } catch (_) {}
    return values;
  };
  const visible = el => {
    try { return el.getClientRects().length > 0; } catch (_) { return true; }
  };
  function has(needle, doc) {
    const want = norm(needle);
    if (!want) return true;
    for (const d of documents(doc || document)) {
      const root = d.body || d.documentElement;
      if (!root) continue;
      if (norm(root.innerText).includes(want)) return true;
      if (norm(ownText(root)).includes(want)) return true;
      if (fieldValues(d).some(value => norm(value).includes(want))) return true;
    }
    return false;
  }
  function find(needle, doc) {
    const want = norm(needle);
    if (!want) return null;
    const d = doc || document;
    const root = d.body || d.documentElement;
    if (!root) return null;
    const matches = new Set();
    const ordered = [];
    for (const el of [root, ...root.querySelectorAll('*')]) {
      if (SKIP.test(el.tagName)) continue;
      if (norm(ownText(el)).includes(want)) { matches.add(el); ordered.push(el); }
    }
    const deepest = ordered.filter(el => !Array.from(el.children).some(child => matches.has(child)));
    return deepest.find(visible) || deepest[0] || null;
  }
  return { norm, has, find };
})()
