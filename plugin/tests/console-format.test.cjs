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
