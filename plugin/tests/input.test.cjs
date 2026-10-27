const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const helperPath = path.join(__dirname, '../js/input.js');
function fixture() {
  const app = { submitted: null, value: 'old', extra: 0 };
  class TestEvent {
    constructor(type, options = {}) { this.type = type; Object.assign(this, options); this.defaultPrevented = false; }
    preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
  }
  class HTMLInputElement {
    constructor(id, value) { this.id = id; this._value = value; this.tagName = 'INPUT'; this.type = 'text'; this.events = []; this.disabled = false; this.readOnly = false; this.isConnected = true; }
    get value() { return this._value; }
    set value(value) { this._value = String(value); }
    focus() { document.activeElement = this; }
    getAttribute(name) { return name === 'id' ? this.id : null; }
    matches(selector) { return selector === ':disabled' && this.disabled; }
    dispatchEvent(event) {
      this.events.push(event);
      if (this.cancelInput && event.type === 'beforeinput') event.preventDefault();
      if (this.id === 'target' && event.type === 'input') app.value = this.value;
      return !event.defaultPrevented;
    }
  }
  const prior = new HTMLInputElement('prior', 'untouched');
  const target = new HTMLInputElement('target', 'old');
  // Controlled inputs may have a framework-installed own-property setter. An
  // ordinary assignment updates its tracker, suppressing framework change detection.
  Object.defineProperty(target, 'value', {
    get() { return this._value; },
    set(value) { this._value = String(value); this.tracked = String(value); },
  });
  const dispatch = target.dispatchEvent.bind(target);
  target.dispatchEvent = (event) => {
    if (event.type === 'input' && target.tracked === target.value) {
      target.events.push(event); return true;
    }
    return dispatch(event);
  };
  const document = {
    activeElement: prior,
    querySelectorAll(selector) { return selector === '#target' ? [target] : selector === '.many' ? [prior, target] : []; },
  };
  const context = { document, window: null, HTMLInputElement, HTMLTextAreaElement: class {}, Event: TestEvent, InputEvent: TestEvent, KeyboardEvent: TestEvent, setTimeout, clearTimeout, app };
  context.window = context;
  return { context, app, prior, target, submit: () => app.submitted = app.value };
}
async function run(f, operation, value, selector = '#target') {
  const fn = vm.runInNewContext(fs.readFileSync(helperPath, 'utf8'), f.context);
  const els = f.context.document.querySelectorAll(selector);
  return fn(els.length === 1 ? els[0] : null, { action: operation, text: value });
}

test('T04 fill addresses target even when a different field has focus', async () => {
  const f = fixture(); await run(f, 'fill', 'new');
  assert.equal(f.prior.value, 'untouched'); assert.equal(f.target.value, 'new');
});
test('T05 fill replaces rather than appends and type appends', async () => {
  const f = fixture(); f.context.document.activeElement = f.target;
  await run(f, 'fill', 'new'); assert.equal(f.target.value, 'new');
  await run(f, 'type', '!'); assert.equal(f.target.value, 'new!');
});
test('T06 input events update controlled application state before submission', async () => {
  const f = fixture(); await run(f, 'fill', 'submitted value'); f.submit();
  assert.equal(f.app.submitted, 'submitted value');
});
test('T07 quotes, newlines, backslash, unicode and interpolation remain data', async () => {
  const f = fixture(); const value = '"\'\\\n雪${app.extra++}';
  await run(f, 'fill', value); assert.equal(f.target.value, value); assert.equal(f.app.extra, 0);
});
test('readonly input is rejected before any value change', async () => {
  const f = fixture(); f.target.readOnly = true;
  const result = await run(f, 'fill', 'changed');
  assert.equal(result.code, 'not_actionable'); assert.equal(f.target.value, 'old');
});
test('cancelled beforeinput does not change the input value', async () => {
  const f = fixture(); f.target.cancelInput = true;
  const result = await run(f, 'fill', 'changed');
  assert.equal(result.code, 'not_actionable'); assert.equal(f.target.value, 'old');
});
test('pre-dispatch failures report dispatched:false so the lease is not quarantined', async () => {
  const f = fixture(); f.target.readOnly = true;
  const result = await run(f, 'fill', 'changed');
  assert.equal(result.dispatched, false); assert.equal(result.effect, undefined);
  assert.equal(f.target.events.length, 0);
  const missing = await run(f, 'fill', 'x', '#nothing');
  assert.equal(missing.code, 'target_not_found'); assert.equal(missing.dispatched, false);
});
test('failures after dispatch carry the page effect evidence', async () => {
  const f = fixture(); f.target.cancelInput = true;
  let dispatches = 0;
  f.context.window.__CONNECTOR_EFFECTS__ = { mark: () => dispatches, unchangedSince: m => m === dispatches };
  const quiet = await run(f, 'fill', 'changed');
  assert.equal(quiet.dispatched, true); assert.equal(quiet.effect, 'none');
  f.target.dispatchEvent = (event) => { if (event.type === 'beforeinput') { dispatches++; return false; } return true; };
  const noisy = await run(f, 'fill', 'changed');
  assert.equal(noisy.effect, 'possible');
});
function editorFixture({ monaco = false, accept = true } = {}) {
  const f = fixture();
  const commands = [];
  const editor = {
    tagName: 'DIV', id: 'editor', isConnected: true, isContentEditable: !monaco, textContent: 'hello',
    focus() { f.context.document.activeElement = this; },
    contains(node) { return node === this; },
    closest(selector) { return monaco && selector === '.monaco-editor' ? monacoRoot : null; },
    getAttribute() { return null; }, matches() { return false; }, dispatchEvent() { return true; },
  };
  const monacoRoot = { querySelector: () => editor };
  f.context.document.execCommand = (name, _ui, value) => {
    commands.push([name, value]);
    if (!accept) return false;
    if (name === 'selectAll') editor.selected = true;
    if (name === 'insertText') editor.textContent = editor.selected ? value : editor.textContent + value;
    return true;
  };
  return { f, editor, commands };
}
async function runEditor(f, el, action, text) {
  const fn = vm.runInNewContext(fs.readFileSync(helperPath, 'utf8'), f.context);
  return fn(el, { action, text });
}
test('contenteditable type uses execCommand insertText and verifies the text', async () => {
  const { f, editor, commands } = editorFixture();
  const result = await runEditor(f, editor, 'type', ' world');
  assert.equal(result.code, undefined, JSON.stringify(result));
  assert.equal(result.method, 'execCommand'); assert.equal(editor.textContent, 'hello world');
  assert.deepEqual(commands, [['insertText', ' world']]);
  const filled = await runEditor(f, editor, 'fill', 'new');
  assert.equal(filled.editor, 'contenteditable'); assert.equal(editor.textContent, 'new');
});
test('Monaco type goes through insertText; fill and refused editing are honest not_actionable', async () => {
  const { f, editor } = editorFixture({ monaco: true });
  const typed = await runEditor(f, editor, 'type', 'let x = 1;');
  assert.equal(typed.editor, 'monaco'); assert.equal(typed.verified, false);
  const fill = await runEditor(f, editor, 'fill', 'x');
  assert.equal(fill.code, 'not_actionable'); assert.equal(fill.dispatched, false);
  const refused = editorFixture({ accept: false });
  const result = await runEditor(refused.f, refused.editor, 'type', 'x');
  assert.equal(result.code, 'not_actionable'); assert.equal(result.dispatched, true);
});
