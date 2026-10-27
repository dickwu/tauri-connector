const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const install = vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/effects.js'), 'utf8'), {});

function page(options = {}) {
  const sent = [];
  class Socket { send(data) { sent.push(data); } }
  class Request { open() {} send() {} }
  const bridge = new Socket();
  const root = {
    fetch: options.fetch || (() => Promise.resolve('ok')),
    XMLHttpRequest: Request,
    navigator: { sendBeacon: () => true, userAgent: options.userAgent || 'Macintosh' },
    ipc: options.ipc === undefined ? { postMessage() {} } : options.ipc,
  };
  if (options.prepare) options.prepare(root);
  const effects = install(root, Socket, socket => socket === bridge);
  return { root, effects, bridge, app: new Socket(), Request };
}

function changedBy(s, dispatch) {
  const mark = s.effects.mark();
  dispatch();
  return !s.effects.unchangedSince(mark);
}

test('a script that dispatches nothing leaves the page unchanged', () => {
  const s = page();
  assert.equal(changedBy(s, () => { const r = undefined; try { r.querySelectorAll('x'); } catch (_) {} }), false);
});

test('application IPC counts, connector IPC does not', () => {
  const s = page();
  assert.equal(changedBy(s, () => s.root.fetch('ipc://localhost/save_workout', { method: 'POST' })), true);
  assert.equal(changedBy(s, () => s.root.fetch('http://ipc.localhost/save_workout', { method: 'POST' })), true);
  assert.equal(changedBy(s, () => s.root.fetch('ipc://localhost/plugin%3Aconnector%7Cpush_logs', { method: 'POST' })), false);
  assert.equal(changedBy(s, () => s.root.ipc.postMessage(JSON.stringify({ cmd: 'save_workout' }))), true);
  assert.equal(changedBy(s, () => s.root.ipc.postMessage(JSON.stringify({ cmd: 'plugin:connector|push_logs' }))), false);
});

test('safe HTTP reads do not count, unsafe requests do', () => {
  const s = page();
  assert.equal(changedBy(s, () => s.root.fetch('https://api.example/items')), false);
  assert.equal(changedBy(s, () => s.root.fetch({ url: 'https://api.example/items', method: 'HEAD' })), false);
  assert.equal(changedBy(s, () => s.root.fetch('https://api.example/items', { method: 'DELETE' })), true);
  const read = new s.Request(); read.open('GET', '/items');
  assert.equal(changedBy(s, () => read.send()), false);
  const write = new s.Request(); write.open('post', '/items');
  assert.equal(changedBy(s, () => write.send('{}')), true);
  assert.equal(changedBy(s, () => s.root.navigator.sendBeacon('/log', 'x')), true);
});

test('application sockets count, the bridge socket does not', () => {
  const s = page();
  assert.equal(changedBy(s, () => s.bridge.send('result')), false);
  assert.equal(changedBy(s, () => s.app.send('write')), true);
});

test('an unobservable channel never reports no effect', () => {
  const s = page({
    prepare(root) {
      Object.defineProperty(root, 'fetch', { value: () => Promise.resolve(), writable: false });
    },
  });
  assert.equal(changedBy(s, () => {}), true);
});

test('failed IPC fetch makes an unhooked postMessage fallback unobservable', async () => {
  const frozen = Object.freeze({ postMessage() {} });
  const s = page({ ipc: frozen, fetch: () => Promise.reject(new Error('custom protocol blocked')) });
  assert.equal(changedBy(s, () => {}), false);
  await s.root.fetch('ipc://localhost/plugin%3Aconnector%7Cpush_logs').catch(() => {});
  assert.equal(changedBy(s, () => {}), true);
});

test('Android requires an observable postMessage', () => {
  const frozen = Object.freeze({ postMessage() {} });
  assert.equal(changedBy(page({ ipc: frozen, userAgent: 'Linux; Android 14' }), () => {}), true);
  assert.equal(changedBy(page({ userAgent: 'Linux; Android 14' }), () => {}), false);
});
