(function () {
  'use strict';
  const guard = window.__CONNECTOR_INPUT_GUARD__;
  const semantics = () => window.__CONNECTOR_SEMANTIC__;
  let current = null;
  const fail = code => ({ error: { code }, businessActionDispatched: false });
  const rect = element => {
    const r = element.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  };
  const bounded = text => {
    let out = '';
    for (const c of String(text || '')) { if (new TextEncoder().encode(out + c).length > 256) break; out += c; }
    return out;
  };
  // Stored only within the page. This fingerprint is never exposed as metadata.
  const identity = element => {
    // Bounded private identity inputs. Reject oversized targets rather than
    // traversing unbounded text on every pointer frame.
    const attrs = [...element.attributes];
    if (attrs.length > 128 || attrs.some(a => a.value.length > 4096)) return null;
    const parts = [element.tagName, location.href];
    let identityUnits = location.href.length;
    let current = element;
    for (let depth=0; current && depth<5; depth++,current=current.parentElement) {
      const identityAttrs = [...current.attributes].filter(a => /^(id|name|href|aria-label|data-)/u.test(a.name));
      if (identityAttrs.length>64 || identityAttrs.some(a=>a.value.length>4096)) return null;
      identityUnits += identityAttrs.reduce((n,a)=>n+a.name.length+a.value.length,0);
      if (identityUnits > 4096) return null;
      parts.push(identityAttrs.map(a=>[a.name,a.value]));
    }
    const name = semantics().describe(element);
    if (name.truncated || name.sources?.name === 'unavailable') return null;
    parts.push(name.role,name.name);
    return JSON.stringify(parts);
  };
  const sameContext = (a, b) => ['appInstanceId','windowInstanceId','pageEpoch','runtimeId'].every(k => a?.[k] === b?.[k]);
  const isCurrent = p => current === p && !p.disposed;
  const snapshot = p => ({
    pickerId: p.id, nonce: p.nonce, sequence: p.sequence, context: p.context,
    status: p.status, selection: p.selection, cleanup: { status: p.cleanup },
    error: p.error, businessActionDispatched: false,
    limitations: ['No isolation guarantee against earlier page-global listeners',
      'Light DOM only; iframe, shadow host and canvas internals are not selected',
      'Selection does not authorize workflow spec mutation']
  });
  const validTarget = (element, fingerprint) => fingerprint !== null && element?.isConnected && identity(element) === fingerprint;
  const hide = p => { if (p.root) p.root.style.visibility = 'hidden'; };
  function releaseSelected(p) {
    if (p.referenceWatchdog !== null && p.referenceWatchdog !== undefined) clearTimeout(p.referenceWatchdog);
    p.referenceWatchdog = null;
    p.referenceUnsubscribe?.(); p.referenceUnsubscribe = null;
    p.selectedNode = null; p.selectedIdentity = null; p.selectedRect = null;
  }
  function retainSelectedUntilDeadline(p) {
    // This is the original page-start deadline, never a new full timeout after
    // selection. The host remains authoritative over its earlier total budget.
    const expire = () => {
      if (!p.selectedNode) return;
      const remaining = p.deadline - performance.now();
      if (remaining <= 0) { releaseSelected(p); return; }
      p.referenceWatchdog = setTimeout(expire, Math.ceil(remaining));
    };
    p.referenceUnsubscribe = window.__CONNECTOR_BOOTSTRAP__.registerLifecycle(() => releaseSelected(p));
    expire();
  }
  async function teardown(p, reason) {
    if (p.teardown) return p.teardown;
    p.teardown = (async () => {
      hide(p);
      clearTimeout(p.watchdog);
      if (p.frame) cancelAnimationFrame(p.frame);
      p.frame = null;
      window.removeEventListener('resize', p.redraw);
      window.removeEventListener('scroll', p.redraw, true);
      p.unsubscribe?.();
      p.gesture = null;
      const drained = await guard.deactivate({ drainMs: 450 });
      p.root?.remove(); p.root = null;
      p.toolbar = null; p.cancel = null; p.label = null; p.outline = null;
      p.redraw = null; p.unsubscribe = null;
      p.hover = null; p.pointer = null;
      p.cleanup = drained?.confirmed === false ? 'unconfirmed' : 'confirmed';
      p.sequence++;
      // A selected node is retained only for the bounded host screenshot phase.
      if (reason !== 'selected') releaseSelected(p);
    })();
    return p.teardown;
  }
  function finish(p, status, selection) {
    if (!isCurrent(p) || p.status !== 'awaiting_selection') return;
    p.status = status; p.sequence++;
    if (selection) p.selection = selection;
    if (status === 'selected') retainSelectedUntilDeadline(p);
    void teardown(p, status);
  }
  function hit(p, x, y) {
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return null;
    const previous = p.root.style.visibility;
    try {
      p.root.style.visibility = 'hidden';
      return document.elementsFromPoint(x, y).find(el => el !== document.documentElement && el !== document.body && !p.root.contains(el)) || null;
    } finally { p.root.style.visibility = previous; }
  }
  function describeTarget(element) {
    const core = semantics();
    const summary = core.describe(element);
    const sensitive = summary.sensitive;
    const surface = current?.root;
    const visibility = surface?.style.visibility;
    let actionable = false;
    try {
      if (surface) surface.style.visibility = 'hidden';
      actionable = core.checkActionability(element).actionable;
    } finally { if (surface) surface.style.visibility = visibility; }
    return {
      tag: element.tagName.toLowerCase(),
      role: bounded(summary.role),
      name: sensitive ? '[redacted]' : bounded(summary.name),
      description: sensitive ? '[redacted]' : bounded(summary.description),
      states: core.getAriaStates(element), rect: rect(element),
      coordinateSpace: 'layout_viewport_css',
      visible: core.isVisible(element),
      actionable,
      redaction: {status: 'applied', omittedFields: sensitive ? ['name','description','locators'] : ['value','outerHTML','attributes']}
    };
  }
  function draw(p) {
    p.frame = null;
    if (!isCurrent(p) || p.status !== 'awaiting_selection' || !p.pointer) return;
    const element = hit(p, p.pointer.x, p.pointer.y);
    p.hover = element ? {element, fingerprint: identity(element)} : null;
    if (!element) { p.outline.style.display = 'none'; p.label.textContent = 'No supported target — move over a visible element'; return; }
    const r = rect(element), desc = describeTarget(element);
    if (p.hover.fingerprint === null) {p.label.textContent='Target identity exceeds the safe observation budget';p.outline.style.display='none';return;}
    Object.assign(p.outline.style, {display:'block',left:`${r.x}px`,top:`${r.y}px`,width:`${r.width}px`,height:`${r.height}px`});
    p.label.textContent = bounded(`${desc.tag}${desc.role ? ` · ${desc.role}` : ''} ${desc.name}`);
    if (element.shadowRoot || ['IFRAME','CANVAS'].includes(element.tagName)) p.label.textContent += ' — element boundary only';
  }
  function schedule(p) { if (!p.frame) p.frame = requestAnimationFrame(() => draw(p)); }
  function confirm(p, candidate) {
    if (!candidate || !validTarget(candidate.element, candidate.fingerprint) || !semantics().isVisible(candidate.element) || (p.pointer && hit(p,p.pointer.x,p.pointer.y) !== candidate.element)) {
      p.hover = null; p.gesture = null; p.label.textContent = 'Target changed — move to select again'; return;
    }
    const element = candidate.element;
    const core = semantics();
    let candidates = [];
    if (!core.isSensitive?.(element)) {
      try {
        candidates = core.locatorCandidates(element).slice(0,5).map(item => ({
          ...item, target: item.target || item.locator,
          validatedInPageEpoch: p.context.pageEpoch, validity:'observed_context_only',
          semanticVersion: core.version, validatedAtMs: Date.now()
        })).filter(item => item.verified && item.matchCount === 1);
      } catch (_) { /* Lack of a safe candidate never fabricates a match. */ }
    }
    const selection = {
      selectionId: crypto.randomUUID(), capturedAtMs: Date.now(), context: p.context,
      semanticVersion: core.version, ...describeTarget(element), locatorCandidates: candidates,
      warnings: candidates.length ? [] : ['No safe unique locator candidate; do not reuse as a workflow target']
    };
    if (new TextEncoder().encode(JSON.stringify(selection)).length > 16384) {
      selection.locatorCandidates = [];
      selection.warnings = ['Candidate metadata omitted: selection byte budget'];
    }
    p.selectedNode = element; p.selectedIdentity = candidate.fingerprint;
    p.selectedRect = rect(element);
    finish(p, 'selected', selection);
  }
  function input(p, event) {
    if (!isCurrent(p) || p.status !== 'awaiting_selection') return;
    const path = event.composedPath();
    const cancelTarget = path.includes(p.cancel);
    const toolbarTarget = path.includes(p.toolbar);
    if (event.type === 'keydown') {
      if (event.key === 'Escape') { finish(p, 'cancelled'); return; }
      if (event.key === 'Enter' && !event.repeat && !event.isComposing && !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) confirm(p,p.hover);
      return;
    }
    if (event.type === 'pointermove') {
      if (toolbarTarget) {p.hover=null;p.pointer=null;return;}
      p.pointer = {x:event.clientX,y:event.clientY};
      if (p.gesture && Math.hypot(event.clientX-p.gesture.x,event.clientY-p.gesture.y) > 5) p.gesture.cancelled = true;
      schedule(p); return;
    }
    if (event.type === 'pointerdown') {
      if (toolbarTarget && !cancelTarget) {p.gesture=null;return;}
      p.pointer={x:event.clientX,y:event.clientY};
      if (event.button !== 0 || event.isPrimary === false || p.gesture) { if (p.gesture) p.gesture.cancelled = true; return; }
      const element = cancelTarget ? null : hit(p,event.clientX,event.clientY);
      p.gesture = {id:event.pointerId,element,fingerprint:element ? identity(element) : '',x:event.clientX,y:event.clientY,cancel:cancelTarget,cancelled:false};
      return;
    }
    if (['pointercancel','lostpointercapture'].includes(event.type)) { p.gesture = null; return; }
    if (event.type === 'pointerup') {
      const gesture = p.gesture; p.gesture = null;
      if (!gesture || gesture.id !== event.pointerId || gesture.cancelled || event.button !== 0) return;
      if (gesture.cancel && cancelTarget) { finish(p,'cancelled'); return; }
      if (gesture.element !== hit(p,event.clientX,event.clientY) || Math.hypot(event.clientX-gesture.x,event.clientY-gesture.y)>5) return;
      confirm(p,gesture);
    }
  }
  function start(args) {
    if (current && current.id === args.pickerId && current.nonce === args.nonce) return snapshot(current);
    if (current && (current.status === 'awaiting_selection' || current.cleanup === 'pending')) return fail('resource_busy');
    if (!guard || !semantics() || !window.__CONNECTOR_BOOTSTRAP__ || !guard.state().early) return {...fail('input_isolation_unavailable'),cleanup:{status:'confirmed'},pickerId:args.pickerId,nonce:args.nonce,context:args.context,sequence:1};
    if (current) { releaseSelected(current); current.disposed = true; }
    const timeout = Math.min(120000, Math.max(0, Number(args.timeoutMs) || 0));
    const p = {id:args.pickerId,nonce:args.nonce,context:args.context,sequence:1,status:'awaiting_selection',cleanup:'pending',selection:null,deadline:performance.now()+timeout,referenceWatchdog:null,referenceUnsubscribe:null};
    const root = document.createElement('div');
    root.setAttribute('data-connector-picker-ui',''); root.setAttribute('data-connector-owned','picker');
    root.setAttribute('popover','manual');
    root.style.cssText = 'all:initial;position:fixed;inset:0;margin:0;border:0;padding:0;width:100vw;height:100vh;max-width:none;max-height:none;background:transparent;color:#fff;z-index:2147483647;pointer-events:auto;overflow:visible;';
    const toolbar = document.createElement('div');
    toolbar.style.cssText = 'position:fixed;top:12px;left:12px;max-width:calc(100vw - 48px);padding:12px 16px;background:#14212e;color:white;border:1px solid #63bdff;border-radius:8px;font:14px/1.4 system-ui;display:flex;gap:16px;align-items:center;';
    const label = document.createElement('span'); label.textContent = 'Select an element · Enter confirms · Escape cancels';
    const cancel = document.createElement('button'); cancel.type = 'button'; cancel.textContent = 'Cancel selection';
    cancel.style.cssText = 'font:inherit;padding:6px 10px;background:#fff;color:#14212e;border:0;border-radius:4px;';
    toolbar.append(label,cancel);
    const outline = document.createElement('div');
    outline.style.cssText = 'display:none;position:fixed;box-sizing:border-box;border:2px solid #008cf0;box-shadow:0 0 0 1px white;pointer-events:none;';
    root.append(outline,toolbar); p.root=root;p.label=label;p.cancel=cancel;p.outline=outline;p.toolbar=toolbar;
    try {
      if (typeof root.showPopover !== 'function') throw new Error('top_layer_unavailable');
      // Preserve modal inertness. The connector popover belongs to the active modal.
      const modals = [...document.querySelectorAll('dialog:modal')];
      (modals.at(-1) || document.body).append(root);
      root.showPopover();
      if (!root.matches(':popover-open')) throw new Error('top_layer_unavailable');
      guard.activate(event => input(p,event));
    } catch (_) { root.remove(); return {...fail('input_isolation_unavailable'),cleanup:{status:'confirmed'},pickerId:p.id,nonce:p.nonce,context:p.context,sequence:1}; }
    current = p;
    p.redraw = () => schedule(p);
    window.addEventListener('resize',p.redraw);
    window.addEventListener('scroll',p.redraw,true);
    p.unsubscribe = window.__CONNECTOR_BOOTSTRAP__.registerLifecycle(() => {
      finish(p,'target_changed'); releaseSelected(p);
    });
    p.watchdog = setTimeout(() => { finish(p,'expired'); }, Math.ceil(Math.max(0,p.deadline-performance.now())));
    return snapshot(p);
  }
  function dispatch(args) {
    if (args.cmd === 'start') return start(args);
    const p = current;
    if (!p || p.id !== args.pickerId || p.nonce !== args.nonce) return fail('picker_not_found');
    if (args.context && !sameContext(args.context,p.context)) return fail('target_changed');
    if (args.cmd === 'status') {
      const state = guard.state();
      if (p.cleanup === 'unconfirmed' && !state.active && !state.draining && !state.heldInputs) {p.cleanup='confirmed';p.sequence++;}
      return snapshot(p);
    }
    if (args.cmd === 'cancel') { finish(p,'cancelled'); return snapshot(p); }
    if (args.cmd === 'release') { releaseSelected(p); return snapshot(p); }
    if (args.cmd === 'geometry') {
      if (!validTarget(p.selectedNode,p.selectedIdentity) || JSON.stringify(rect(p.selectedNode)) !== JSON.stringify(p.selectedRect)) return fail('capture_context_changed');
      return {context:p.context,rect:p.selectedRect,selectionId:p.selection.selectionId,ready:true};
    }
    return fail('invalid_arguments');
  }
  dispatch.dispose = () => { if (current) { finish(current,'target_changed');releaseSelected(current); } };
  dispatch.activeObservers = () => (current?.cleanup === 'pending' ? 1 : 0) + (current?.referenceWatchdog != null ? 1 : 0);
  return dispatch;
})()
