const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const format = vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../js/console-format.js'), 'utf8'), {});
test('printf-style substitutions are applied', () => {
  assert.equal(format(['%s has %d items (%i) at %f%%', 'cart', 3.7, '4', '1.5']), 'cart has 3 items (4) at 1.5%');
  assert.equal(format(['obj %o', { a: 1 }, 'tail']), 'obj {"a":1} tail');
  assert.equal(format(['missing %s']), 'missing %s');
});
test('%c styling and its CSS argument are dropped', () => {
  assert.equal(format(['%c[app]%c ready', 'color: red', 'color: inherit', 42]), '[app] ready 42');
});
test('Error objects keep name, message and stack', () => {
  const error = new TypeError('bad thing');
  const text = format(['failed:', error]);
  assert.match(text, /^failed: TypeError: bad thing\n/);
  assert.ok(text.includes('console-format.test.cjs') || text.includes('at '), text);
  const webkit = { name: 'Error', message: 'boom', stack: 'fn@app.js:1:2' };
  assert.equal(format([webkit]), 'Error: boom\nfn@app.js:1:2');
  assert.match(format([{ cause: new Error('inner') }]), /"message":"inner"/);
});
test('circular, undefined and bigint values do not throw', () => {
  const a = {}; a.self = a;
  assert.equal(format([a, undefined, 10n]), '{"self":"[Circular]"} undefined 10n');
});
test('R2-4 Map and Set print their entries instead of {}', () => {
  const map = new Map([['a', 1], [{ k: 2 }, new Set([3, 'x'])]]);
  assert.equal(format([map]), 'Map(2) {"a" => 1,{"k":2} => Set(2) {3,"x"}}');
  assert.equal(format(['state', { tags: new Set(['p', 'q']) }]), 'state {"tags":Set(2) {"p","q"}}');
});
test('R2-4 DOM nodes are summarised and React internals are never read', () => {
  let touched = false;
  const fiber = {};
  Object.defineProperty(fiber, 'stateNode', { enumerable: true, get() { touched = true; return {}; } });
  const body = { nodeType: 1, nodeName: 'BODY', id: 'app', className: 'dark  ready', __reactFiber$abc: fiber, __reactProps$abc: fiber };
  assert.equal(format([body]), '<body#app.dark.ready>');
  assert.equal(format([{ root: body, text: { nodeType: 3, nodeName: '#text', nodeValue: '  hi  ' } }]), '{"root":"<body#app.dark.ready>","text":"#text \\"hi\\""}');
  const component = { state: 1, _reactInternals: fiber, __reactContainer$x: fiber };
  assert.equal(format([component]), '{"state":1}');
  assert.equal(touched, false);
  const element = { $$typeof: Symbol.for('react.element'), type: function Card() {}, props: {}, _owner: fiber };
  assert.equal(format([element]), '<Card />');
  assert.equal(touched, false);
});
test('R2-4 depth, entry count and total size are capped', () => {
  let deep = { leaf: true };
  for (let i = 0; i < 20; i++) deep = { child: deep };
  const text = format([deep]);
  assert.ok(text.includes('"[Object]"'), text);
  assert.ok(text.length < 200, text);
  const wide = Array.from({ length: 1000 }, (_, i) => i);
  assert.match(format([wide]), /,49,"…"\]$/);
  const huge = Array.from({ length: 40 }, () => ({ blob: 'x'.repeat(5000) }));
  const capped = format([huge, huge, huge, huge, huge]);
  assert.ok(capped.length <= 16384 + 64, String(capped.length));
});
