// This expression returns the page-local workflow dispatcher. All arguments are
// supplied as serialized JSON by dom.rs; no user data is interpolated as code.
(() => {
  'use strict';
  const slot = '__TAURI_CONNECTOR_WORKFLOW_V1__';
  if (window[slot]) return window[slot];

  const semantic = window.__CONNECTOR_SEMANTIC__;
  if (!semantic) throw new Error("Trusted semantic runtime is missing");
  const semanticVersion = semantic.version;
  const pageEpoch = window.__CONNECTOR_BOOTSTRAP__?.pageEpoch || crypto.randomUUID();
  const observations = new Map();
  const normalize = value => String(value || '').replace(/\s+/gu, ' ').trim();
  const fail = (code, message, stage = 'preparing') => {
    const error = new Error(message);
    error.code = code;
    error.stage = stage;
    throw error;
  };
  const literal = value => value && typeof value === 'object' && Object.prototype.hasOwnProperty.call(value, 'literal') ? value.literal : value;
  const string = (value, field, nonempty = false) => {
    value = literal(value);
    if (typeof value !== 'string') fail('binding_type_mismatch', `${field} must resolve to a string`);
    if (nonempty && !value.trim()) fail('invalid_spec', `${field} must not be empty`);
    return value;
  };
  const coverage = {
    correlation: 'state_only', businessPersistence: 'unobserved',
    observation: 'document_dom_and_input_events', shadowDom: 'unsupported',
    iframe: 'unsupported', nativeInput: 'unsupported',
    accessibleName: semantic.coverage.name, semanticVersion
  };
  const description = element => element ? {
    tag: element.tagName.toLowerCase(), id: element.id || undefined,
    role: role(element), windowId: undefined, pageEpoch
  } : null;
  const text = element => normalize(element.textContent);
  const name = semantic.getAccessibleName;
  const role = semantic.getRole;
  function resolve(locator, allowMissing = false) {
    // Strict workflow policy wraps the same semantic resolver used by inspection.
    const candidates = semantic.resolveCandidates(locator);
    if (candidates.length > 1) fail('ambiguous_target', `Strict locator matched ${candidates.length} elements`, 'locating');
    if (!candidates.length) {
      if (allowMissing) return null;
      fail('target_not_found', 'Strict locator matched no element', 'locating');
    }
    return candidates[0];
  }
  const visible = semantic.isVisible;
  const enabled = semantic.isEnabled;
  const editable = semantic.isEditable;
  function actionable(element, edit = false) {
    if (!semantic.checkActionability(element, edit ? 'fill' : 'click').actionable)
      fail('not_actionable', 'Target must be visible, enabled, unobstructed and editable for input', 'locating');
  }
  const valueOf = element => 'value' in element ? element.value : element.isContentEditable ? element.textContent : null;
  const sensitive = semantic.isSensitive;
  function pointer(data, path) {
    if (path === '') return { exists: true, value: data };
    if (typeof path !== 'string' || !path.startsWith('/') || /~(?![01])/u.test(path)) fail('invalid_spec', 'Invalid JSON pointer');
    for (const encoded of path.slice(1).split('/')) {
      const key = encoded.replace(/~1/gu, '/').replace(/~0/gu, '~');
      if (data === null || typeof data !== 'object' || !Object.prototype.hasOwnProperty.call(data, key)) return { exists: false };
      data = data[key];
    }
    return { exists: true, value: data };
  }
  function sameValue(a, b) {
    if (a === b) return true;
    if (!a || !b || typeof a !== 'object' || typeof b !== 'object' || Array.isArray(a) !== Array.isArray(b)) return false;
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every(key => Object.prototype.hasOwnProperty.call(b, key) && sameValue(a[key], b[key]));
  }
  function evaluate(condition, results = {}, depth = 0, budget = { nodes: 0 }) {
    if (!condition || typeof condition !== 'object') fail('invalid_spec', 'A condition is required', 'verifying');
    if (++budget.nodes > 100 || depth > 8) fail('invalid_spec', 'Condition tree limit exceeded', 'verifying');
    if ((condition.semantics && condition.semantics !== 'current_state') || (condition.observation && condition.observation !== 'current_state')) fail('unsupported_condition', 'Only current_state DOM conditions are supported', 'verifying');
    if (['all', 'any'].includes(condition.kind)) {
      if (!Array.isArray(condition.conditions) || !condition.conditions.length) fail('invalid_spec', 'Condition groups must be nonempty', 'verifying');
      const children = condition.conditions.map(child => evaluate(child, results, depth + 1, budget));
      const matchedBranches = children.flatMap((child, index) => child.satisfied ? [index] : []);
      return { satisfied: condition.kind === 'all' ? matchedBranches.length === children.length : matchedBranches.length > 0,
        lastObserved: { kind: condition.kind, matchedBranches, children: children.map(child => child.lastObserved) } };
    }
    if (condition.kind === 'result') {
      const found = Object.prototype.hasOwnProperty.call(results, condition.stepId) ? pointer(results[condition.stepId], condition.pointer) : { exists: false };
      let satisfied;
      if (condition.operator === 'exists') satisfied = found.exists;
      else if (condition.operator === 'nonEmptyString') satisfied = found.exists && typeof found.value === 'string' && found.value.length > 0;
      else if (condition.operator === 'eq') satisfied = found.exists && sameValue(found.value, literal(condition.expected));
      else fail('unsupported_condition', 'Unsupported result operator', 'verifying');
      return { satisfied, lastObserved: { kind: 'result', exists: found.exists, valueType: found.exists ? typeof found.value : 'missing', satisfied } };
    }
    if (!['element', 'valueEquals', 'textContains', 'attributeEquals'].includes(condition.kind)) fail('unsupported_condition', `Unsupported condition kind: ${condition.kind}`, 'verifying');
    const element = resolve(condition.target, true);
    let satisfied = false;
    if (condition.kind === 'element') {
      if (!['visible', 'hidden', 'attached', 'detached', 'enabled', 'editable'].includes(condition.state)) fail('unsupported_condition', 'Unsupported element state', 'verifying');
      satisfied = { visible: () => visible(element), hidden: () => !visible(element), attached: () => Boolean(element?.isConnected), detached: () => !element?.isConnected,
        enabled: () => enabled(element), editable: () => editable(element) }[condition.state]();
    } else if (condition.kind === 'valueEquals') {
      const expected = string(condition.expected, 'condition.expected');
      satisfied = Boolean(element) && valueOf(element) === expected;
    } else if (condition.kind === 'textContains') {
      const expected = string(condition.expected, 'condition.expected');
      satisfied = Boolean(element) && (element.textContent || '').includes(expected);
    } else {
      const attribute = string(condition.name, 'condition.name', true);
      const expected = literal(condition.expected);
      if (expected !== null && typeof expected !== 'string') fail('binding_type_mismatch', 'Attribute expected must be a string or null', 'verifying');
      satisfied = Boolean(element) && element.getAttribute(attribute) === expected;
    }
    return { satisfied, lastObserved: { kind: condition.kind, state: condition.state, matched: Boolean(element), satisfied, target: description(element) } };
  }
  function cleanup(id) {
    const observation = observations.get(id);
    if (!observation) return false;
    observations.delete(id);
    observation.observer?.disconnect();
    clearTimeout(observation.expiry);
    clearInterval(observation.poller);
    document.removeEventListener('input', observation.sample, true);
    document.removeEventListener('change', observation.sample, true);
    return true;
  }
  function checkContext(args, observation) {
    const expected = args.context || {};
    if (window.__CONNECTOR_SEMANTIC__ !== semantic || (expected.semanticVersion !== undefined && expected.semanticVersion !== semanticVersion)) fail('semantic_version_changed', 'Semantic runtime changed during workflow observation', 'preparing');
    if ((args.expectedPageEpoch !== undefined && args.expectedPageEpoch !== pageEpoch) || (expected.pageEpoch !== undefined && expected.pageEpoch !== pageEpoch)) fail('stale_ref', 'Page context has changed', 'preparing');
    if (expected.origin !== undefined && expected.origin !== location.origin) fail('target_changed', 'Page origin has changed', 'preparing');
    if (observation && ((expected.appInstanceId !== undefined && expected.appInstanceId !== observation.context.appInstanceId) || (expected.windowId !== undefined && expected.windowId !== observation.context.windowId))) fail('target_changed', 'Application or window identity has changed', 'preparing');
  }
  function observe(args) {
    checkContext(args);
    const id = string(args.observationId, 'observationId', true);
    if (observations.has(id)) fail('observation_failed', 'Observation identifier is already active');
    if (observations.size >= 128) fail('resource_busy', 'Page observation capacity reached');
    const timeoutMs = args.timeoutMs ?? 10000;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 300000) fail('invalid_spec', 'Observation budget must be between 1 and 300000 ms');
    const observation = {
      context: { appInstanceId: string(args.context?.appInstanceId, 'context.appInstanceId', true), windowId: string(args.context?.windowId, 'context.windowId', true), pageEpoch, semanticVersion, origin: location.origin },
      deadline: performance.now() + timeoutMs, condition: args.condition || null, results: args.results || {},
      mutationCount: 0, sampleCount: 0, last: null, sampleError: null, trueSince: null, dispatched: new Map()
    };
    observation.sample = () => {
      observation.sampleCount++;
      if (!observation.condition) return;
      try {
        observation.last = evaluate(observation.condition, observation.results);
        observation.sampleError = null;
        if (observation.last.satisfied) observation.trueSince ??= performance.now();
        else observation.trueSince = null;
      } catch (error) { observation.sampleError = error; observation.trueSince = null; }
    };
    try {
      observation.observer = new MutationObserver(records => {
        observation.mutationCount = Math.min(Number.MAX_SAFE_INTEGER, observation.mutationCount + records.length);
        observation.sample();
      });
      observation.observer.observe(document.documentElement, { childList: true, attributes: true, characterData: true, subtree: true });
      document.addEventListener('input', observation.sample, true);
      document.addEventListener('change', observation.sample, true);
      observations.set(id, observation);
      observation.expiry = setTimeout(() => cleanup(id), timeoutMs);
      observation.poller = setInterval(observation.sample, Math.min(100, timeoutMs));
      observation.sample();
      if (observation.sampleError) throw observation.sampleError;
      return { ok: true, ready: true, observationId: id, context: observation.context, correlation: 'state_only', coverage, baseline: observation.last?.lastObserved || null };
    } catch (error) {
      if (!cleanup(id)) observation.observer?.disconnect();
      if (error.code) throw error;
      fail('observation_failed', 'Could not register page observation');
    }
  }
  function active(args) {
    const observation = observations.get(args.observationId);
    if (!observation || performance.now() >= observation.deadline) {
      cleanup(args.observationId);
      fail('observation_failed', 'Observation is missing, expired or cancelled');
    }
    checkContext(args, observation);
    if (observation.sampleError) throw observation.sampleError;
    return observation;
  }
  function fill(element, value, append, revalidate) {
    const next = append ? valueOf(element) + value : value;
    const inputType = append ? 'insertText' : 'insertReplacementText';
    const event = new InputEvent('beforeinput', { bubbles: true, cancelable: true, composed: true, inputType, data: value });
    if (!element.dispatchEvent(event)) fail('execution_failed', 'Application prevented input', 'executing');
    revalidate();
    if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
      const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, next);
    } else {
      // Plain contenteditable is supported; complex editors may reject the
      // observable value and require their own explicitly registered adapter.
      element.textContent = next;
    }
    element.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType, data: value }));
    element.dispatchEvent(new Event('change', { bubbles: true }));
    if (valueOf(element) !== next) fail('execution_failed', 'Application did not retain the requested DOM value', 'executing');
    return next;
  }
  function keyOptions(key) {
    const parts = key.split('+');
    const keyName = parts.pop();
    const modifiers = new Set(parts);
    if (!keyName || parts.some(part => !['Control', 'Alt', 'Shift', 'Meta'].includes(part))) fail('unsupported_feature', 'Unsupported key chord', 'preparing');
    return { key: keyName === 'Space' ? ' ' : keyName, bubbles: true, cancelable: true, composed: true,
      ctrlKey: modifiers.has('Control'), altKey: modifiers.has('Alt'), shiftKey: modifiers.has('Shift'), metaKey: modifiers.has('Meta') };
  }
  function keyEvents(element, options) {
    const accepted = element.dispatchEvent(new KeyboardEvent('keydown', options));
    element.dispatchEvent(new KeyboardEvent('keyup', options));
    return accepted;
  }
  async function execute(args, state) {
    const observation = active(args);
    const step = args.step || args.resolvedStep;
    if (!step || !['click', 'fill', 'type', 'press', 'query'].includes(step.op)) fail('unsupported_feature', 'DOM executor supports click, fill, type, press and query');
    const signature = JSON.stringify(step);
    if (step.id && observation.dispatched.has(step.id)) {
      const previous = observation.dispatched.get(step.id);
      if (previous.signature !== signature) fail('run_key_conflict', 'Step identifier already dispatched with different input');
      return previous.result;
    }
    if (observation.dispatched.size >= 100) fail('resource_busy', 'Observation dispatch cache capacity reached');
    let input;
    if (step.op === 'fill' || step.op === 'type') input = string(step.value, 'step.value');
    if (step.op === 'press') input = keyOptions(string(step.key, 'step.key', true));
    const element = step.target ? resolve(step.target) : step.op === 'press' ? document.activeElement : null;
    if (!element) fail('target_not_found', 'No target or actual focus is available', 'locating');
    if (step.op !== 'query') actionable(element, step.op === 'fill' || step.op === 'type');
    const target = { ...description(element), windowId: observation.context.windowId };
    if (step.op === 'query') {
      if (sensitive(element, step.query?.kind === 'attribute' ? step.query.name : null)) fail('capability_unavailable', 'Sensitive field extraction requires an explicit host adapter', 'querying');
      let value;
      if (step.query?.kind === 'value') value = valueOf(element);
      else if (step.query?.kind === 'text') value = element.textContent || '';
      else if (step.query?.kind === 'attribute') value = element.getAttribute(string(step.query.name, 'query.name', true));
      else fail('unsupported_feature', 'Unsupported query kind');
      return { ok: true, dispatched: false, data: { value, target }, coverage };
    }
    // Reserve before the first side effect, including synchronous reentrancy
    // from application handlers. A duplicate can observe uncertainty but must
    // never dispatch again while the original request is completing.
    if (step.id) observation.dispatched.set(step.id, { signature, result: {
      ok: false, dispatched: true, error: { code: 'outcome_unknown', stage: 'executing',
        message: 'The original step is still completing', retryableBeforeDispatch: false }, coverage
    } });
    state.dispatched = true;
    let result;
    try {
      element.focus({ preventScroll: true });
      // Focus handlers may replace or relabel a node. Recheck all constraints
      // synchronously before sending the requested interaction to it.
      if (step.target && resolve(step.target) !== element) fail('target_changed', 'Target changed while acquiring focus', 'executing');
      actionable(element, step.op === 'fill' || step.op === 'type');
      if (['fill', 'type', 'press'].includes(step.op) && document.activeElement !== element) fail('not_actionable', 'Target did not acquire focus', 'executing');
      let data = { target, interactionMode: 'synthetic' };
      let effectScope = 'ui_event_dispatch';
      if (step.op === 'click') element.click();
      else if (step.op === 'fill' || step.op === 'type') {
        data.value = fill(element, input, step.op === 'type', () => {
          if (resolve(step.target) !== element) fail('target_changed', 'Target changed during beforeinput', 'executing');
          actionable(element, true);
        });
        effectScope = 'dom_value';
      } else {
        data.defaultPrevented = !keyEvents(element, input);
      }
      // Give controlled components a microtask to accept/reject the event. This
      // confirms only the observable DOM value, never application persistence.
      await Promise.resolve();
      if (data.value !== undefined) {
        const current = resolve(step.target);
        if (valueOf(current) !== data.value) fail('execution_failed', 'Controlled input reverted the requested DOM value', 'executing');
        data.target = { ...description(current), windowId: observation.context.windowId };
      }
      observation.sample();
      result = { ok: true, dispatched: true, data, effectScope, interactionMode: 'synthetic', coverage };
    } catch (error) {
      result = errorResult(error, true);
    }
    if (step.id) observation.dispatched.set(step.id, { signature, result });
    return result;
  }
  function condition(args) {
    const observation = active(args);
    if (args.condition !== undefined && JSON.stringify(args.condition) !== JSON.stringify(observation.condition)) {
      observation.condition = args.condition;
      observation.trueSince = null;
    }
    if (args.results !== undefined) observation.results = args.results;
    observation.sample();
    if (observation.sampleError) throw observation.sampleError;
    if (!observation.last) fail('invalid_spec', 'No condition was prepared', 'verifying');
    const stabilityMs = observation.condition.stabilityMs ?? 0;
    if (!Number.isFinite(stabilityMs) || stabilityMs < 0 || stabilityMs > 300000) fail('invalid_spec', 'Invalid stabilityMs', 'verifying');
    const stable = observation.trueSince !== null && performance.now() - observation.trueSince >= stabilityMs;
    return { ok: true, satisfied: observation.last.satisfied && stable, lastObserved: observation.last.lastObserved, correlation: 'state_only', coverage,
      observation: { sampleCount: observation.sampleCount, mutationCount: observation.mutationCount, stableForMs: observation.trueSince === null ? 0 : performance.now() - observation.trueSince } };
  }
  function evidence(args) {
    const observation = observations.get(args.observationId);
    const budget = Math.max(256, Math.min(16384, Number(args.maxBytes) || 4096));
    const elements = [];
    let root = document;
    let scope = 'document_fallback';
    if (args.target) {
      try {
        if (args.target.scope) { root = resolve(args.target.scope); scope = 'locator_scope'; }
        else { root = resolve(args.target); scope = 'target'; }
      } catch (_) { /* Preserve failure evidence even when strict resolution failed. */ }
    }
    const result = { ok: true, context: observation?.context || { pageEpoch }, coverage, scope, elements, truncated: false,
      observation: { active: Boolean(observation), mutationCount: observation?.mutationCount || 0, sampleCount: observation?.sampleCount || 0 } };
    const bytes = () => new TextEncoder().encode(JSON.stringify(result)).length;
    const selector = 'button,input,textarea,select,[role="dialog"],[role="alert"]';
    const candidates = [...(root.nodeType === 1 ? [root] : []), ...root.querySelectorAll(selector)];
    for (const element of candidates) {
      // Deliberately omit values, text, names, IDs and arbitrary attributes.
      const tag = element.tagName.toLowerCase();
      const safeTag = /^(?:a|article|aside|button|div|dialog|form|h[1-6]|img|input|label|li|main|nav|ol|option|output|p|section|select|span|table|tbody|td|textarea|th|thead|tr|ul)$/u.test(tag) ? tag : 'element';
      const elementRole = role(element);
      const safeRole = /^(?:alert|alertdialog|article|banner|button|cell|checkbox|columnheader|combobox|complementary|contentinfo|dialog|form|grid|gridcell|group|heading|img|link|list|listbox|listitem|main|menu|menuitem|navigation|none|option|presentation|progressbar|radio|region|row|rowgroup|rowheader|scrollbar|search|searchbox|separator|slider|spinbutton|status|switch|tab|table|tablist|tabpanel|textbox|timer|toolbar|tooltip|tree|treeitem)$/u.test(elementRole || '') ? elementRole : null;
      elements.push({ tag: safeTag, role: safeRole, visible: visible(element), enabled: enabled(element), editable: editable(element) });
      if (elements.length > 40 || bytes() > budget) { elements.pop(); result.truncated = true; break; }
    }
    if (bytes() > budget) return { ok: true, truncated: true, elements: [], coverage: { capture: 'budget_exhausted' } };
    return result;
  }
  function errorResult(error, dispatched) {
    return { ok: false, dispatched, error: { code: error.code || 'execution_failed', stage: error.stage || (dispatched ? 'executing' : 'preparing'),
      message: error.code ? error.message : 'Page operation failed', retryableBeforeDispatch: !dispatched && error.code === 'target_not_found' }, coverage };
  }
  const dispatcher = async args => {
    const state = { dispatched: false };
    try {
      if (!args || typeof args !== 'object') fail('invalid_spec', 'A command object is required');
      if (args.cmd === 'prepare') return observe(args);
      if (args.cmd === 'execute') return await execute(args, state);
      if (args.cmd === 'condition') return condition(args);
      if (args.cmd === 'cleanup') return { ok: true, cleaned: cleanup(args.observationId) };
      if (args.cmd === 'evidence') return evidence(args);
      fail('unsupported_feature', 'Unknown DOM workflow command');
    } catch (error) { return errorResult(error, state.dispatched); }
  };
  const dispose = () => {
    for (const id of observations.keys()) cleanup(id);
    window.removeEventListener('pagehide', dispose);
    if (window[slot] === dispatcher) delete window[slot];
  };
  dispatcher.dispose = dispose;
  dispatcher.activeObservers = () => observations.size;
  window.addEventListener('pagehide', dispose);
  Object.defineProperty(window, slot, { value: dispatcher, configurable: true, writable: false });
  return dispatcher;
})()
