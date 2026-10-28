// git-strata #8 remainder: a navigation requested by the page marks the
// document navigating before beforeunload, so the dispatch gate refuses the
// next command instead of letting the old page answer it.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const early = fs.readFileSync(path.join(__dirname, '../js/early.js'), 'utf8').trim().replace(/;$/, '');
const format = fs.readFileSync(path.join(__dirname, '../js/console-format.js'), 'utf8').trim().replace(/;$/, '');

function page({ navigation } = {}) {
  const listeners = {};
  class HTMLFormElement { constructor(target) { this.target = target; this.submitted = 0; }
    getAttribute(name) { return name === 'target' ? this.target : null; }
    submit() { this.submitted++; } }
  const window = {
    console: { log() {}, warn() {}, error() {}, info() {}, debug() {} },
    addEventListener(type, fn) { (listeners[type] = listeners[type] || []).push(fn); },
    HTMLFormElement,
    navigation,
  };
  window.top = window;
  vm.runInNewContext(`(${early})(${format})`, { window, Promise, Date, Math, Object });
  const fire = (type, event) => (listeners[type] || []).forEach((fn) => fn(event));
  return { window, state: window.__CONNECTOR_PAGE_STATE__, fire, HTMLFormElement };
}

test('page state starts with no navigation request', () => {
  assert.equal(page().state.navRequestedAt, 0);
});

test('a form submit nobody prevented marks the document; a prevented or _blank one does not', () => {
  const p = page();
  p.fire('submit', { defaultPrevented: true, target: new p.HTMLFormElement('') });
  assert.equal(p.state.navRequestedAt, 0);
  p.fire('submit', { defaultPrevented: false, target: new p.HTMLFormElement('_blank') });
  assert.equal(p.state.navRequestedAt, 0);
  p.fire('submit', { defaultPrevented: false, target: new p.HTMLFormElement('') });
  assert.ok(p.state.navRequestedAt > 0);
});

test('form.submit() (no submit event) marks the document and still submits', () => {
  const p = page();
  const form = new p.HTMLFormElement('');
  form.submit();
  assert.equal(form.submitted, 1);
  assert.ok(p.state.navRequestedAt > 0);
});

test('Navigation API: cross-document navigations mark, same-document or intercepted ones do not', async () => {
  const handlers = [];
  const navigation = { transition: null, addEventListener(type, fn) { if (type === 'navigate') handlers.push(fn); } };
  const p = page({ navigation });
  handlers.forEach((fn) => fn({ destination: { sameDocument: true }, defaultPrevented: false }));
  await Promise.resolve(); await Promise.resolve();
  assert.equal(p.state.navRequestedAt, 0);
  navigation.transition = {};
  handlers.forEach((fn) => fn({ destination: { sameDocument: false }, defaultPrevented: false }));
  await Promise.resolve(); await Promise.resolve();
  assert.equal(p.state.navRequestedAt, 0, 'an app that intercepted keeps the document');
  navigation.transition = null;
  handlers.forEach((fn) => fn({ destination: { sameDocument: false }, defaultPrevented: false }));
  await Promise.resolve(); await Promise.resolve();
  assert.ok(p.state.navRequestedAt > 0);
});

test('a restored (bfcache) page clears both marks', () => {
  const p = page();
  p.state.unloadingAt = 1; p.state.navRequestedAt = 1;
  p.fire('pageshow', { persisted: true });
  assert.equal(p.state.unloadingAt, 0);
  assert.equal(p.state.navRequestedAt, 0);
});
