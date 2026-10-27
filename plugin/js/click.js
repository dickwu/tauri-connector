// Click at the element's visual centre the way a real cursor does: pointer and
// mouse down/up on the element actually hit there, then `click`. Frameworks
// that listen to pointerdown/mousedown (menus, custom selects, drag handles)
// ignore a bare `el.click()`. When something else covers the centre the events
// still go to the requested element and the result reports `mismatch` with the
// element a real click would have hit.
(function connectorClick(el) {
  'use strict';
  if (el.disabled || (el.matches && el.matches(':disabled'))) {
    return { code: 'not_actionable', error: 'Click target is disabled', dispatched: false };
  }
  const rect = el.getBoundingClientRect();
  const cx = rect.x + rect.width / 2;
  const cy = rect.y + rect.height / 2;
  const hit = document.elementFromPoint(cx, cy);
  const inside = !!hit && (hit === el || el.contains(hit));
  const receiver = inside ? hit : el;
  const describe = n => n ? { tag: n.tagName.toLowerCase(), id: n.id || null, className: typeof n.className === 'string' ? n.className || null : null } : null;
  const base = { bubbles: true, cancelable: true, composed: true, view: window, clientX: cx, clientY: cy, screenX: cx, screenY: cy, button: 0 };
  const ptr = { pointerId: 1, pointerType: 'mouse', isPrimary: true };
  const Pointer = typeof PointerEvent === 'function' ? PointerEvent : MouseEvent;
  const fire = (Type, type, extra) => receiver.dispatchEvent(new Type(type, { ...base, ...extra }));
  fire(Pointer, 'pointerover', { ...ptr, buttons: 0 });
  fire(MouseEvent, 'mouseover', { buttons: 0 });
  fire(Pointer, 'pointermove', { ...ptr, buttons: 0 });
  fire(MouseEvent, 'mousemove', { buttons: 0 });
  const pointerDown = fire(Pointer, 'pointerdown', { ...ptr, buttons: 1 });
  // A cancelled pointerdown suppresses the compatibility mousedown, as in browsers.
  const mouseDown = pointerDown ? fire(MouseEvent, 'mousedown', { buttons: 1, detail: 1 }) : false;
  if (mouseDown && receiver.closest) {
    const focusable = receiver.closest('button,a[href],input,select,textarea,summary,[tabindex],[contenteditable]:not([contenteditable="false"])');
    if (focusable && typeof focusable.focus === 'function' && document.activeElement !== focusable) focusable.focus({ preventScroll: true });
  }
  fire(Pointer, 'pointerup', { ...ptr, buttons: 0 });
  fire(MouseEvent, 'mouseup', { buttons: 0, detail: 1 });
  fire(MouseEvent, 'click', { buttons: 0, detail: 1 });
  const result = { dispatched: true, x: cx, y: cy, receiver: describe(receiver), mismatch: !inside };
  if (!inside) {
    result.hit = describe(hit);
    result.warning = 'Another element covers the centre of the target; events went to the target directly, but a real click would hit the reported element';
  }
  return result;
})
