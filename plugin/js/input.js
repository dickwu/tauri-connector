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
  // Named keys with the legacy keyCode/which and code values that many
  // handlers still read (Monaco keybindings, rc-dialog's Escape, rc-util).
  const NAMED_KEYS = {
    Enter: ['Enter', 13], Tab: ['Tab', 9], Escape: ['Escape', 27], Backspace: ['Backspace', 8],
    Delete: ['Delete', 46], Insert: ['Insert', 45], Home: ['Home', 36], End: ['End', 35],
    PageUp: ['PageUp', 33], PageDown: ['PageDown', 34], ArrowLeft: ['ArrowLeft', 37],
    ArrowUp: ['ArrowUp', 38], ArrowRight: ['ArrowRight', 39], ArrowDown: ['ArrowDown', 40],
    ' ': ['Space', 32], Shift: ['ShiftLeft', 16], Control: ['ControlLeft', 17], Alt: ['AltLeft', 18],
    Meta: ['MetaLeft', 91], CapsLock: ['CapsLock', 20], ContextMenu: ['ContextMenu', 93],
    Pause: ['Pause', 19], PrintScreen: ['PrintScreen', 44], ScrollLock: ['ScrollLock', 145], NumLock: ['NumLock', 144],
  };
  for (let n = 1; n <= 24; n++) NAMED_KEYS['F' + n] = ['F' + n, 111 + n];
  const KEY_ALIASES = { esc: 'Escape', return: 'Enter', space: ' ', spacebar: ' ', del: 'Delete', up: 'ArrowUp', down: 'ArrowDown', left: 'ArrowLeft', right: 'ArrowRight', pgup: 'PageUp', pgdn: 'PageDown' };
  const MODIFIER_NAMES = { ctrl: 'ctrlKey', control: 'ctrlKey', shift: 'shiftKey', alt: 'altKey', option: 'altKey', meta: 'metaKey', cmd: 'metaKey', command: 'metaKey' };
  const PUNCTUATION_CODES = { ';': ['Semicolon', 186], '=': ['Equal', 187], ',': ['Comma', 188], '-': ['Minus', 189], '.': ['Period', 190], '/': ['Slash', 191], '`': ['Backquote', 192], '[': ['BracketLeft', 219], '\\': ['Backslash', 220], ']': ['BracketRight', 221], "'": ['Quote', 222] };
  // Resolves "Enter", "a", "Meta+z" or "Control+Shift+Z" to a key, its
  // modifiers and legacy codes; null for anything a keyboard cannot produce.
  function parseKey(spec) {
    const raw = String(spec ?? '');
    let parts = raw.length > 1 && raw.includes('+') ? raw.split('+') : [raw];
    if (parts.length > 1 && parts[parts.length - 1] === '' && parts[parts.length - 2] === '') parts = parts.slice(0, -2).concat('+');
    const mods = {};
    for (const part of parts.slice(0, -1)) {
      const name = MODIFIER_NAMES[part.trim().toLowerCase()];
      if (!name) return null;
      mods[name] = true;
    }
    let key = parts[parts.length - 1];
    if (key.length > 1) key = key.trim();
    if (!key) return null;
    const named = Object.keys(NAMED_KEYS).find(name => name.toLowerCase() === key.toLowerCase()) || KEY_ALIASES[key.toLowerCase()];
    if (named) return { key: named, code: NAMED_KEYS[named][0], keyCode: NAMED_KEYS[named][1], mods };
    if ([...key].length !== 1) return null;
    if (mods.shiftKey && /^[a-z]$/.test(key)) key = key.toUpperCase();
    const upper = key.toUpperCase();
    if (/^[A-Z]$/.test(upper)) return { key, code: 'Key' + upper, keyCode: upper.charCodeAt(0), mods };
    if (/^[0-9]$/.test(key)) return { key, code: 'Digit' + key, keyCode: key.charCodeAt(0), mods };
    const punctuation = PUNCTUATION_CODES[key];
    return { key, code: punctuation ? punctuation[0] : '', keyCode: punctuation ? punctuation[1] : 0, mods };
  }
  function keyEvent(type, parsed, extra) {
    const init = { key: parsed.key, code: parsed.code, keyCode: parsed.keyCode, which: parsed.keyCode, bubbles: true, cancelable: true, composed: true, ...extra, ...parsed.mods };
    const event = new KeyboardEvent(type, init);
    // Engines that ignore the legacy init members still expose them to handlers.
    for (const name of ['keyCode', 'which']) {
      if (event[name] !== parsed.keyCode) {
        try { Object.defineProperty(event, name, { get: () => parsed.keyCode }); } catch (_) {}
      }
    }
    return event;
  }
  const frame = () => new Promise(resolve => typeof requestAnimationFrame === 'function' ? requestAnimationFrame(() => resolve()) : setTimeout(resolve, 16));
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  // contenteditable hosts and Monaco do not read a value property; the only
  // generic path is the browser's own editing pipeline. When it is refused the
  // honest answer is not_actionable, never a fake success.
  async function editorInput(target, monacoRoot) {
    const text = String(options.text ?? '');
    if (monacoRoot && action === 'fill') return fail('not_actionable', 'fill on a Monaco editor needs a component adapter; clear it through the app, then use type');
    if (typeof document.execCommand !== 'function') return fail('not_actionable', 'This editor needs a component adapter: document.execCommand is unavailable');
    const inside = node => !!node && (node === focusTarget || (focusTarget.contains && focusTarget.contains(node)) || (monacoRoot && monacoRoot.contains && monacoRoot.contains(node)));
    // Monaco's input surface is a hidden textarea (or an EditContext div);
    // when it already has focus keep it, otherwise focus the surface.
    let focusTarget = target;
    if (monacoRoot) {
      const active = document.activeElement;
      focusTarget = (active && monacoRoot.contains && monacoRoot.contains(active) && active !== monacoRoot ? active : null)
        || monacoRoot.querySelector('textarea.inputarea, .native-edit-context, textarea') || target;
    }
    dispatched = true;
    const hadFocus = document.activeElement === focusTarget;
    if (!hadFocus) focusTarget.focus();
    if (!inside(document.activeElement)) return fail('not_actionable', 'Editor could not receive focus');
    if (monacoRoot && !hadFocus) {
      // Monaco syncs its textarea state on focus over the next frames; text
      // inserted before then is diffed against stale state and garbles.
      const started = Date.now();
      while (Date.now() - started < 300 && !(monacoRoot.classList && monacoRoot.classList.contains('focused'))) await frame();
      await frame();
      if (!inside(document.activeElement)) return fail('not_actionable', 'Editor lost focus while settling');
    }
    if (action === 'fill') document.execCommand('selectAll', false);
    // Monaco re-syncs its textarea after each input event, so long text goes
    // in line-sized chunks with a frame in between.
    const chunks = monacoRoot ? (text.match(/[^\n]{1,64}|\n/g) || []) : (text ? [text] : []);
    // Monaco on engines with the EditContext API (Chromium) reads text from
    // textupdate events instead of a textarea; execCommand cannot reach it.
    const surface = document.activeElement;
    const editContext = monacoRoot && surface && surface.editContext;
    const useEditContext = !!editContext && typeof TextUpdateEvent === 'function';
    for (let i = 0; i < chunks.length; i++) {
      if (i && monacoRoot) await frame();
      if (useEditContext) {
        const start = Math.min(editContext.selectionStart, editContext.selectionEnd);
        const end = Math.max(editContext.selectionStart, editContext.selectionEnd);
        const caret = start + chunks[i].length;
        editContext.updateText(start, end, chunks[i]);
        editContext.updateSelection(caret, caret);
        editContext.dispatchEvent(new TextUpdateEvent('textupdate', { text: chunks[i], updateRangeStart: start, updateRangeEnd: end, selectionStart: caret, selectionEnd: caret }));
      } else if (!document.execCommand('insertText', false, chunks[i])) {
        return fail('not_actionable', 'The editor refused execCommand insertText; it needs a component adapter');
      }
    }
    await new Promise(resolve => setTimeout(resolve, 0));
    const result = { action, [action === 'type' ? 'typed' : 'filled']: text, target: target.tagName.toLowerCase(), targetId: target.id || null, interactionMode, editor: monacoRoot ? 'monaco' : 'contenteditable', method: useEditContext ? 'editContext' : 'execCommand' };
    if (monacoRoot) {
      // When the page exposes the Monaco API, report what the model holds.
      const api = typeof window !== 'undefined' && window && window.monaco && window.monaco.editor;
      const editors = api && typeof api.getEditors === 'function' ? api.getEditors() : [];
      const owner = editors.find(e => { try { const node = e.getContainerDomNode(); return node === monacoRoot || (node.contains && node.contains(monacoRoot)) || (monacoRoot.contains && monacoRoot.contains(node)); } catch (_) { return false; } });
      if (owner) {
        const contains = String(owner.getValue()).includes(text);
        return { ...result, verified: contains, modelContainsText: contains, note: contains ? undefined : 'The Monaco model does not contain the typed text verbatim (auto-closing or formatting may change it); confirm with wait/expect' };
      }
      return { ...result, verified: false, note: 'Monaco moves input into its model asynchronously; confirm with wait/expect' };
    }
    if (!target.isConnected) return fail('target_changed', 'Editor was replaced during input');
    if (!String(target.textContent).replace(/\u00a0/g, ' ').includes(text.replace(/\u00a0/g, ' '))) return fail('postcondition_failed', 'Editor text does not contain the typed text after event processing');
    return result;
  }
  const modifiers = options.modifiers || {};
  if (action === 'press') {
    const parsed = parseKey(options.key || 'Enter');
    if (!parsed) return fail('invalid_key', 'Unknown key ' + JSON.stringify(String(options.key)) + '; use a single character or a named key such as Enter, Tab, Escape, Backspace, Delete, ArrowDown, Home, F5, optionally with modifiers like Meta+z or Control+Shift+Z');
    parsed.mods = { ...modifiers, ...parsed.mods };
    dispatched = true;
    if (document.activeElement !== el) el.focus();
    if (document.activeElement !== el && !(el.contains && el.contains(document.activeElement))) return fail('not_actionable', 'Input target could not receive focus');
    const receiver = document.activeElement || el;
    const down = receiver.dispatchEvent(keyEvent('keydown', parsed));
    // Synthetic key events never edit text. When no handler took the key,
    // perform the editing default for editable targets so Backspace, Delete
    // and undo/redo behave like a real keyboard.
    let defaultAction = null;
    const mod = parsed.mods.metaKey || parsed.mods.ctrlKey;
    const lower = parsed.key.toLowerCase();
    const editable = receiver.isContentEditable || ((receiver instanceof HTMLTextAreaElement || receiver instanceof HTMLInputElement) && !receiver.readOnly && !receiver.disabled);
    if (down && editable && typeof document.execCommand === 'function') {
      const command = parsed.key === 'Backspace' && !mod ? 'delete'
        : parsed.key === 'Delete' && !mod ? 'forwardDelete'
        : mod && lower === 'z' ? (parsed.mods.shiftKey ? 'redo' : 'undo')
        : mod && lower === 'y' && parsed.mods.ctrlKey ? 'redo'
        : mod && lower === 'a' ? 'selectAll'
        : null;
      if (command && document.execCommand(command, false)) defaultAction = command;
    }
    receiver.dispatchEvent(keyEvent('keyup', parsed));
    return { pressed: parsed.key, keyboardCode: parsed.code, modifiers: Object.keys(parsed.mods).filter(name => parsed.mods[name]), defaultPrevented: !down, defaultAction, target: receiver.tagName.toLowerCase(), targetId: receiver.id || null, interactionMode };
  }
  if (!['fill', 'type'].includes(action)) return fail('unsupported_feature', 'Unsupported input action');
  // A Monaco editor is addressed through its own node, any node inside it
  // (its hidden textarea) or a container wrapping exactly one editor.
  let monaco = typeof el.closest === 'function' ? el.closest('.monaco-editor') : null;
  if (!monaco && typeof el.querySelectorAll === 'function') {
    const nested = el.querySelectorAll('.monaco-editor');
    if (nested.length === 1) monaco = nested[0];
  }
  if (monaco || el.isContentEditable) return editorInput(el, monaco);
  // Component libraries (AntD Input's affix wrapper, MUI TextField) put the
  // real field inside a wrapper; a wrapper with exactly one field means it.
  let retargeted = null;
  const fillable = node => node instanceof HTMLTextAreaElement || (node instanceof HTMLInputElement && ['text', 'search', 'url', 'tel', 'email', 'password', 'number'].includes(node.type));
  if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLTextAreaElement) && typeof el.querySelectorAll === 'function') {
    const fields = Array.from(el.querySelectorAll('input, textarea')).filter(fillable);
    if (fields.length === 1) { retargeted = { from: el.tagName.toLowerCase(), to: fields[0].tagName.toLowerCase() }; el = fields[0]; }
  }
  // What was actually written, so a caller can tell a wrong element apart
  // from a field the framework did not observe.
  const describe = node => {
    const semantic = typeof window !== 'undefined' && window ? window.__CONNECTOR_SEMANTIC__ : null;
    let name = '', role = null;
    try { name = semantic ? semantic.getAccessibleName(node) : (node.getAttribute('aria-label') || node.getAttribute('placeholder') || ''); role = semantic ? semantic.getRole(node) : null; } catch (_) {}
    const visible = !!(node.getClientRects && node.getClientRects().length);
    return { tag: node.tagName.toLowerCase(), role, name: String(name).slice(0, 100), visible };
  };
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
  const result = { action, [action === 'type' ? 'typed' : 'filled']: text, value: expected, target: el.tagName.toLowerCase(), targetId: el.id || null, interactionMode, element: describe(el) };
  if (options.resolvedVia) result.resolvedVia = options.resolvedVia;
  if (retargeted) result.retargeted = retargeted;
  return result;
})
