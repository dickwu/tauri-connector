(async function connectorInput(el, options) {
  const interactionMode = 'synthetic';
  // Failures found before any event is dispatched report dispatched:false so
  // the host does not quarantine the lease. After dispatch, the page effect
  // monitor says whether a backend-capable call happened meanwhile.
  const fx = (typeof window !== 'undefined' && window && window.__CONNECTOR_EFFECTS__) || null;
  const mark = fx ? fx.mark() : 0;
  let dispatched = false;
  const fail = (code, error) => dispatched
    ? { code, error, interactionMode, dispatched, effect: fx && fx.unchangedSince(mark) ? 'none' : 'possible' }
    : { code, error, interactionMode, dispatched };
  if (!el) return fail('target_not_found', 'No unique input target');
  if (!el.isConnected) return fail('target_changed', 'Input target was detached');
  if (el.disabled || el.matches?.(':disabled') || el.getAttribute('aria-disabled') === 'true') {
    return fail('not_actionable', 'Input target is disabled');
  }
  const action = options.action;
  // contenteditable hosts and Monaco do not read a value property; the only
  // generic path is the browser's own editing pipeline. When it is refused the
  // honest answer is not_actionable, never a fake success.
  async function editorInput(target, monacoRoot) {
    const text = String(options.text ?? '');
    if (monacoRoot && action === 'fill') return fail('not_actionable', 'fill on a Monaco editor needs a component adapter; clear it through the app, then use type');
    if (typeof document.execCommand !== 'function') return fail('not_actionable', 'This editor needs a component adapter: document.execCommand is unavailable');
    const focusTarget = monacoRoot ? (monacoRoot.querySelector('textarea') || target) : target;
    dispatched = true;
    focusTarget.focus();
    const active = document.activeElement;
    if (active !== focusTarget && !(focusTarget.contains && focusTarget.contains(active))) {
      return fail('not_actionable', 'Editor could not receive focus');
    }
    if (action === 'fill') document.execCommand('selectAll', false);
    if (text && !document.execCommand('insertText', false, text)) {
      return fail('not_actionable', 'The editor refused execCommand insertText; it needs a component adapter');
    }
    await new Promise(resolve => setTimeout(resolve, 0));
    const result = { action, [action === 'type' ? 'typed' : 'filled']: text, target: target.tagName.toLowerCase(), targetId: target.id || null, interactionMode, editor: monacoRoot ? 'monaco' : 'contenteditable', method: 'execCommand' };
    if (monacoRoot) return { ...result, verified: false, note: 'Monaco moves input into its model asynchronously; confirm with wait/expect' };
    if (!target.isConnected) return fail('target_changed', 'Editor was replaced during input');
    if (!String(target.textContent).replace(/\u00a0/g, ' ').includes(text.replace(/\u00a0/g, ' '))) return fail('postcondition_failed', 'Editor text does not contain the typed text after event processing');
    return result;
  }
  const modifiers = options.modifiers || {};
  if (action === 'press') {
    dispatched = true;
    el.focus();
    if (document.activeElement !== el) return fail('not_actionable', 'Input target could not receive focus');
    const key = options.key || 'Enter';
    el.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...modifiers }));
    el.dispatchEvent(new KeyboardEvent('keyup', { key, bubbles: true, cancelable: true, ...modifiers }));
    return { pressed: key, target: el.tagName.toLowerCase(), targetId: el.id || null, interactionMode };
  }
  if (!['fill', 'type'].includes(action)) return fail('unsupported_feature', 'Unsupported input action');
  const monaco = typeof el.closest === 'function' ? el.closest('.monaco-editor') : null;
  if (monaco || el.isContentEditable) return editorInput(el, monaco);
  const isInput = el instanceof HTMLInputElement;
  const isTextarea = el instanceof HTMLTextAreaElement;
  if ((!isInput && !isTextarea) || (isInput && !['text', 'search', 'url', 'tel', 'email', 'password', 'number'].includes(el.type))) {
    return fail('not_actionable', 'This input requires a component adapter');
  }
  if (el.readOnly) return fail('not_actionable', 'Input target is readonly');
  const proto = isInput ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
  if (!setter) return fail('capability_unavailable', 'Native input setter is unavailable');
  dispatched = true;
  el.focus();
  if (document.activeElement !== el) return fail('not_actionable', 'Input target could not receive focus');
  const text = String(options.text ?? '');
  let expected = String(el.value);
  const write = (value, data, inputType) => {
    const before = new InputEvent('beforeinput', { data, inputType, bubbles: true, cancelable: true, composed: true });
    if (!el.dispatchEvent(before)) return false;
    setter.call(el, value);
    el.dispatchEvent(new InputEvent('input', { data, inputType, bubbles: true, composed: true }));
    return true;
  };
  if (action === 'fill') {
    expected = text;
    if (!write(expected, text, 'insertReplacementText')) return fail('not_actionable', 'Input was cancelled by beforeinput');
  } else {
    for (const ch of text) {
      const options = { key: ch, bubbles: true, cancelable: true, ...modifiers };
      const down = el.dispatchEvent(new KeyboardEvent('keydown', options));
      if (down && el.dispatchEvent(new KeyboardEvent('keypress', options))) {
        expected += ch;
        if (!write(expected, ch, 'insertText')) {
          el.dispatchEvent(new KeyboardEvent('keyup', options));
          return fail('not_actionable', 'Input was cancelled by beforeinput');
        }
      } else {
        el.dispatchEvent(new KeyboardEvent('keyup', options));
        return fail('not_actionable', 'Typing was cancelled by a keyboard handler');
      }
      el.dispatchEvent(new KeyboardEvent('keyup', options));
    }
  }
  el.dispatchEvent(new Event('change', { bubbles: true }));
  // Let framework event handlers and their pending render commit run. This
  // proves the retained DOM value; business persistence still needs an expect.
  await new Promise(resolve => setTimeout(resolve, 0));
  if (!el.isConnected) return fail('target_changed', 'Input target was replaced during input');
  if (String(el.value) !== expected) return fail('postcondition_failed', 'Input value did not persist after event processing');
  return { action, [action === 'type' ? 'typed' : 'filled']: text, value: expected, target: el.tagName.toLowerCase(), targetId: el.id || null, interactionMode };
})
