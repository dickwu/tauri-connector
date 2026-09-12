// Document-start bootstrap. No host token, page data, or arbitrary-code API.
(() => {
  'use strict';
  if (window.top !== window || window.__CONNECTOR_BOOTSTRAP__) return;
  let pageEpoch = crypto.randomUUID();
  let suspended = false;
  const lifecycle = new Set();
  let handler = null, drainTimer = null, hardTimer = null, draining = false, finishDrain = null;
  let quietMs = 250;
  const held = new Set();
  const pointerKinds = new Map();
  const hoverEvents = new Set(['pointermove','pointerover','pointerout','pointerenter','pointerleave',
    'mousemove','mouseover','mouseout','mouseenter','mouseleave']);
  const early = document.readyState === 'loading';
  const resetGuard = (confirmed = true) => {
    handler = null; draining = false;
    if (drainTimer !== null) clearTimeout(drainTimer);
    if (hardTimer !== null) clearTimeout(hardTimer);
    hardTimer = null;
    drainTimer = null;
    if (finishDrain) { const done = finishDrain; finishDrain = null; done({confirmed}); }
  };
  const guard = Object.freeze({
    get active() { return Boolean(handler) || draining; },
    activate(next) {
      if (suspended || typeof next !== 'function') throw new Error('picker_guard_unavailable');
      if (handler || draining) throw new Error('picker_guard_busy');
      if (held.size) throw new Error('picker_gesture_in_progress');
      handler = next;
      return { early };
    },
    deactivate(options = {}) {
      handler = null;
      if (draining) return new Promise(resolve => { const previous = finishDrain; finishDrain = result => { if (previous) previous(result); resolve(result); }; });
      quietMs = Math.min(2000, Math.max(0, Number(options.drainMs ?? 250) || 0));
      if (!quietMs && !held.size) { resetGuard(); return Promise.resolve({confirmed:true}); }
      draining = true;
      return new Promise(resolve => { finishDrain = resolve; hardTimer = setTimeout(()=>resetGuard(false),2000); scheduleQuiet(); });
    },
    state() { return { active: Boolean(handler), draining, early, heldInputs: held.size }; }
  });
  const scheduleQuiet = () => {
    if (drainTimer !== null) clearTimeout(drainTimer);
    drainTimer = null;
    if (draining && held.size === 0) drainTimer = setTimeout(()=>resetGuard(true),quietMs);
  };
  const releasePointer = pointer => { held.delete(pointer); pointerKinds.delete(pointer); };
  const releasePointerKind = kind => {
    for (const [pointer, actual] of pointerKinds) if (actual === kind) releasePointer(pointer);
  };
  const releaseMouse = () => { held.delete('mouse'); releasePointerKind('mouse'); };
  const suppress = event => {
    const heldBefore = held.size;
    const pointer = 'p:' + String(event.pointerId ?? 0);
    const key = 'k:' + String(event.code || event.key || 'unknown');
    const pointerEvent = event.type.startsWith('pointer') || ['gotpointercapture','lostpointercapture'].includes(event.type);
    const mouseEvent = event.type.startsWith('mouse') || ['click','dblclick','auxclick','contextmenu'].includes(event.type);
    const pressed = typeof event.buttons === 'number' && event.buttons > 0;
    if (pointerEvent) {
      const kind = event.pointerType || pointerKinds.get(pointer) || 'unknown';
      // A pointer can enter the WebView already pressed, without a local down.
      // Capture loss alone says nothing about whether its buttons were released.
      if (event.type === 'pointerdown' || event.type === 'gotpointercapture' || pressed) {
        held.add(pointer); pointerKinds.set(pointer, kind);
      } else if (['pointerup','pointercancel'].includes(event.type) || event.buttons === 0) {
        releasePointer(pointer);
        if (kind === 'mouse') releaseMouse();
      }
    }
    if (mouseEvent) {
      if (event.type === 'mousedown' || pressed) held.add('mouse');
      else if (event.type === 'mouseup' || event.buttons === 0) releaseMouse();
    }
    if (event.type === 'touchstart' || (event.type === 'touchmove' && event.touches?.length)) held.add('touch');
    if (['touchend','touchcancel'].includes(event.type) && !event.touches?.length) {
      held.delete('touch');
      if (event.touches?.length === 0) releasePointerKind('touch');
    }
    if (event.type === 'keydown') held.add(key);
    if (event.type === 'keyup') held.delete(key);

    if (!handler && !draining) return;
    // Suppression precedes picker code and application capture handlers. This
    // cannot undo handlers installed earlier, hence the explicit early flag.
    event.preventDefault();
    event.stopImmediatePropagation();
    if (handler) handler(event);
    // Passive hover has no click/key compatibility tail. Continuing to move the
    // mouse must not keep an otherwise quiescent guard alive until hard timeout.
    else if (draining && !(heldBefore === 0 && held.size === 0 && event.buttons === 0 && hoverEvents.has(event.type))) scheduleQuiet();
  };
  const events = ['pointerdown','pointerup','pointermove','pointercancel','pointerover','pointerout',
    'pointerenter','pointerleave','gotpointercapture','lostpointercapture','mousedown','mouseup','mousemove',
    'mouseover','mouseout','mouseenter','mouseleave','touchstart','touchmove','touchend','touchcancel',
    'click','dblclick','auxclick','contextmenu','keydown','keyup','keypress','beforeinput','input','wheel','dragstart'];
  for (const type of events) window.addEventListener(type, suppress, { capture: true, passive: false });
  window.addEventListener('pagehide', () => {
    suspended = true;
    held.clear(); pointerKinds.clear();
    resetGuard();
    for (const dispose of [...lifecycle]) { try { dispose('pagehide'); } catch (_) { /* next disposer must still run */ } }
    lifecycle.clear();
  }, true);
  window.addEventListener('pageshow', event => {
    if (event.persisted) { pageEpoch = crypto.randomUUID(); held.clear(); pointerKinds.clear(); resetGuard(); }
    suspended = false;
  }, true);
  window.__CONNECTOR_INPUT_GUARD__ = guard;
  window.__CONNECTOR_BOOTSTRAP__ = Object.freeze({
    get pageEpoch() { return pageEpoch; }, get suspended() { return suspended; }, guard,
    registerLifecycle(callback) { lifecycle.add(callback); return () => lifecycle.delete(callback); }
  });
})();
