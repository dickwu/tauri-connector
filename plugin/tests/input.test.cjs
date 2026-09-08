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
