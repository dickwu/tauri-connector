// Scroll the container a user would scroll. Apps usually scroll an inner
// panel, not the document, so `window.scrollBy` alone often moves nothing.
// With a target, scroll it or its nearest scrollable ancestor. Without one,
// try the element under the viewport centre, then the focused element, then
// the hovered element (each with its ancestors), then the document. A
// container that can still move in the requested direction wins over one
// already at its edge. The result reports which container moved and by how
// much, so `scrolledBy: {x: 0, y: 0}` means everything was at its edge.
// Canonical copy; crates/cli/src/scroll.js must stay byte-identical.
(function connectorScroll(target, dx, dy) {
  'use strict';
  const doc = document.scrollingElement || document.documentElement;
  const isDocument = n => n === doc || n === document.body || n === document.documentElement;
  const describe = n => {
    if (!n) return null;
    if (isDocument(n)) return { tag: 'document', id: null, className: null };
    return { tag: n.tagName.toLowerCase(), id: n.id || null, className: typeof n.className === 'string' ? n.className || null : null };
  };
  const axis = dy ? 'y' : 'x';
  const delta = dy || dx;
  const overflows = n => {
    const style = getComputedStyle(n);
    const mode = axis === 'y' ? style.overflowY : style.overflowX;
    if (!/^(auto|scroll|overlay)$/.test(mode)) return false;
    return axis === 'y' ? n.scrollHeight > n.clientHeight + 1 : n.scrollWidth > n.clientWidth + 1;
  };
  const canMove = n => {
    if (!delta) return false;
    const pos = axis === 'y' ? n.scrollTop : n.scrollLeft;
    const max = axis === 'y' ? n.scrollHeight - n.clientHeight : n.scrollWidth - n.clientWidth;
    return delta < 0 ? pos > 0.5 : pos < max - 0.5;
  };
  const parentOf = n => n.parentElement || (n.parentNode && n.parentNode.host) || null;
  // Scrollable containers from `start` outwards, the document excluded.
  const chain = start => {
    const out = [];
    for (let n = start; n && n.nodeType === 1 && !isDocument(n); n = parentOf(n)) {
      if (overflows(n)) out.push(n);
    }
    return out;
  };
  const deepest = n => {
    while (n && n.shadowRoot && n.shadowRoot.activeElement) n = n.shadowRoot.activeElement;
    return n;
  };
  const sources = target
    ? [['selector', target]]
    : [
        ['viewport-center', document.elementFromPoint(innerWidth / 2, innerHeight / 2)],
        ['focus', deepest(document.activeElement)],
        ['hover', Array.from(document.querySelectorAll(':hover')).pop() || null],
      ];
  let chosen = null;
  let fallback = null;
  for (const [source, start] of sources) {
    if (!start || isDocument(start)) continue;
    for (const n of chain(start)) {
      if (!fallback) fallback = { el: n, source };
      if (canMove(n)) { chosen = { el: n, source }; break; }
    }
    if (chosen) break;
  }
  if (!chosen && (!fallback || canMove(doc))) chosen = { el: doc, source: 'document' };
  if (!chosen) chosen = fallback;
  const el = chosen.el;
  const read = () => el === doc ? [window.scrollX, window.scrollY] : [el.scrollLeft, el.scrollTop];
  const [beforeX, beforeY] = read();
  // `instant` bypasses CSS `scroll-behavior: smooth`, so the result is final.
  const opts = { left: dx, top: dy, behavior: 'instant' };
  if (el === doc) window.scrollBy(opts); else el.scrollBy(opts);
  const [afterX, afterY] = read();
  return {
    container: describe(el),
    source: chosen.source,
    scrolledBy: { x: Math.round(afterX - beforeX), y: Math.round(afterY - beforeY) },
    position: { x: Math.round(afterX), y: Math.round(afterY) },
  };
})
