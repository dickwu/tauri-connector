// Real Chromium DOM tests using the repository's locked Playwright and React.
// Run: node plugin/tests/workflow/page.test.cjs
const { readFile } = require('node:fs/promises');
const { createServer } = require('node:http');
const { join } = require('node:path');
const { chromium } = require('playwright');

async function browserTests(runtime) {
  const results = [];
  let sequence = 0;
  const context = { appInstanceId: 'isolated-test', windowId: 'main' };
  const target = (value, extra = {}) => ({ by: 'css', value, ...extra });
  const assert = (condition, message) => { if (!condition) throw new Error(message); };
  const equal = (a, b) => assert(JSON.stringify(a) === JSON.stringify(b), `${JSON.stringify(a)} != ${JSON.stringify(b)}`);
  const call = (cmd, args = {}) => runtime({ cmd, ...args });
  const setup = async (html, condition) => {
    document.getElementById('fixture').innerHTML = html;
    const observationId = `test-${++sequence}`;
    const prepared = await call('prepare', { observationId, context, condition, timeoutMs: 2000 });
    assert(prepared.ok && prepared.ready, JSON.stringify(prepared));
    return { observationId, context: prepared.context };
  };
  const test = async (name, fn) => {
    try { await fn(); results.push({ name, passed: true }); }
    catch (e) { results.push({ name, passed: false, error: e.stack || String(e) }); }
  };
  await test('scope, role, name and entity intersect, with normalized exact names', async () => {
    const h = await setup('<section role="dialog" aria-label="Create task"><button data-id="one"> Save </button><button data-id="two">Save</button></section><button data-id="one">Save</button>');
    const result = await call('execute', { ...h, step: { op: 'query', target: {
      scope: { by: 'role', value: { literal: 'dialog' }, name: { literal: 'Create task' } },
      by: 'role', value: 'button', name: 'Save', entity: { attribute: 'data-id', value: { literal: 'two' } }
    }, query: { kind: 'attribute', name: 'data-id' } } });
    equal(result.data.value, 'two');
    await call('cleanup', h);
  });
  await test('ambiguous targets and ambiguous scopes never choose first', async () => {
    const h = await setup('<section><button>A</button></section><section><button>B</button></section>');
    let r = await call('execute', { ...h, step: { op: 'click', target: target('button') } });
    equal(r.error.code, 'ambiguous_target'); equal(r.dispatched, false);
    r = await call('execute', { ...h, step: { op: 'click', target: target('button', { scope: target('section') }) } });
    equal(r.error.code, 'ambiguous_target');
    await call('cleanup', h);
  });
  await test('missing entity cannot degrade to the locator without entity', async () => {
    const h = await setup('<button data-id="one">Save</button>');
    const r = await call('execute', { ...h, step: { op: 'click', target: target('button', { entity: { attribute: 'data-id', value: 'missing' } }) } });
    equal(r.error.code, 'target_not_found'); equal(r.dispatched, false);
    await call('cleanup', h);
  });
  await test('entity is rechecked after virtual-list node reuse', async () => {
    const h = await setup('<button data-id="one">Save</button>');
    document.querySelector('button').setAttribute('data-id', 'two');
    const r = await call('execute', { ...h, step: { op: 'click', target: target('button', { entity: { attribute: 'data-id', value: 'one' } }) } });
    equal(r.error.code, 'target_not_found'); equal(r.dispatched, false);
    await call('cleanup', h);
  });
  await test('associated and nested labels locate inputs exactly', async () => {
    const h = await setup('<label for="title">Task name</label><input id="title"><label>Detail<textarea></textarea></label>');
    for (const name of ['Task name', 'Detail']) {
      const r = await call('execute', { ...h, step: { op: 'fill', target: { by: 'label', value: name }, value: { literal: name } } });
      assert(r.ok, JSON.stringify(r)); equal(r.data.value, name);
    }
    await call('cleanup', h);
  });
  await test('fill replaces and type appends through native setter, with controlled event state', async () => {
    const h = await setup('<textarea>old</textarea><button>Submit</button><output></output>');
    const input = document.querySelector('textarea');
    let applicationValue = 'old';
    let directSetterCalls = 0;
    const descriptor = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value');
    Object.defineProperty(input, 'value', { get() { return descriptor.get.call(this); }, set(v) { directSetterCalls++; descriptor.set.call(this, v); }, configurable: true });
    input.addEventListener('input', event => { applicationValue = event.target.value; });
    document.querySelector('button').onclick = () => { document.querySelector('output').textContent = applicationValue; };
    const value = 'quote\" slash\\ newline\n中文 ${notCode} `tick`';
    let r = await call('execute', { ...h, step: { op: 'fill', target: target('textarea'), value: { literal: value } } });
    assert(r.ok, JSON.stringify(r)); equal(applicationValue, value); equal(directSetterCalls, 0); equal(r.interactionMode, 'synthetic');
    r = await call('execute', { ...h, step: { op: 'type', target: target('textarea'), value: '+' } });
    assert(r.ok, JSON.stringify(r)); equal(applicationValue, value + '+');
    await call('execute', { ...h, step: { op: 'click', target: target('button') } });
    equal(document.querySelector('output').textContent, value + '+');
    await call('cleanup', h);
  });
  if (window.WorkflowReact) await test('React controlled textarea submits application state after fill and type', async () => {
    const h = await setup('<div id="react-root"></div>');
    const { React, ReactDOM } = window.WorkflowReact;
    const submitted = [];
    function App() {
      const [value, setValue] = React.useState('initial');
      return React.createElement('form', { onSubmit(event) { event.preventDefault(); submitted.push(value); } },
        React.createElement('label', { htmlFor: 'react-name' }, 'React name'),
        React.createElement('textarea', { id: 'react-name', value, onChange: event => setValue(event.target.value) }),
        React.createElement('button', { type: 'submit' }, 'Submit React value'),
        React.createElement('output', { 'data-testid': 'react-state' }, value));
    }
    const root = ReactDOM.createRoot(document.getElementById('react-root'));
    try {
      root.render(React.createElement(App));
      for (let n = 0; n < 30 && !document.getElementById('react-name'); n++) await new Promise(resolve => setTimeout(resolve, 10));
      const value = 'React "quoted" \\ text\n中文 ${literal}';
      let r = await call('execute', { ...h, step: { op: 'fill', target: { by: 'label', value: 'React name' }, value } });
      assert(r.ok, JSON.stringify(r));
      r = await call('execute', { ...h, step: { op: 'type', target: target('#react-name'), value: ' appended' } });
      assert(r.ok, JSON.stringify(r));
      r = await call('execute', { ...h, step: { op: 'click', target: { by: 'role', value: 'button', name: 'Submit React value' } } });
      assert(r.ok, JSON.stringify(r));
      equal(submitted, [value + ' appended']);
      equal(document.querySelector('output').textContent, value + ' appended');
    } finally { root.unmount(); await call('cleanup', h); }
  });
  await test('number in a string slot is rejected without mutation', async () => {
    const h = await setup('<input value="keep">');
    const r = await call('execute', { ...h, step: { op: 'fill', target: target('input'), value: { literal: 12 } } });
    equal(r.error.code, 'binding_type_mismatch'); equal(r.dispatched, false); equal(document.querySelector('input').value, 'keep');
    await call('cleanup', h);
  });
  await test('hidden, disabled, readonly, covered and inert controls fail before dispatch', async () => {
    for (const html of ['<input hidden>', '<input disabled>', '<input readonly>', '<input><div id="cover"></div>', '<div inert><input></div>']) {
      const h = await setup(html);
      const r = await call('execute', { ...h, step: { op: 'fill', target: target('input'), value: 'forbidden' } });
      equal(r.error.code, 'not_actionable'); equal(r.dispatched, false);
      await call('cleanup', h);
    }
  });
  await test('press targets and reports the actual focused object', async () => {
    const h = await setup('<input id="a"><input id="b">');
    document.querySelector('#a').focus();
    let received = null;
    document.querySelector('#b').addEventListener('keydown', e => { received = e.key; });
    let r = await call('execute', { ...h, step: { op: 'press', target: target('#b'), key: { literal: 'Enter' } } });
    assert(r.ok, JSON.stringify(r)); equal(received, 'Enter'); equal(r.data.target.id, 'b');
    r = await call('execute', { ...h, step: { op: 'press', key: 'Escape' } });
    equal(r.data.target.id, 'b');
    await call('cleanup', h);
  });
  await test('duplicate in-flight step identity dispatches a click only once', async () => {
    const h = await setup('<button>Save once</button>');
    let clicks = 0;
    document.querySelector('button').onclick = () => clicks++;
    const step = { id: 'save-once', op: 'click', target: target('button') };
    const [first] = await Promise.all([call('execute', { ...h, step }), call('execute', { ...h, step })]);
    assert(first.ok, JSON.stringify(first)); equal(clicks, 1);
    assert((await call('execute', { ...h, step })).ok, 'Completed result is replayable'); equal(clicks, 1);
    const conflict = await call('execute', { ...h, step: { ...step, target: target('button', { name: 'other' }) } });
    equal(conflict.error.code, 'run_key_conflict');
    await call('cleanup', h);
  });
  await test('unsupported key chords fail before focus or keyboard effects', async () => {
    const h = await setup('<input id="before"><input id="after">');
    document.querySelector('#before').focus();
    const r = await call('execute', { ...h, step: { op: 'press', target: target('#after'), key: 'Unknown+Enter' } });
    equal(r.error.code, 'unsupported_feature'); equal(r.dispatched, false); equal(document.activeElement.id, 'before');
    await call('cleanup', h);
  });
  await test('application prevention and focus-time replacement never become successful fills', async () => {
    const h = await setup('<input id="prevented" value="keep"><input id="replaced">');
    document.querySelector('#prevented').addEventListener('beforeinput', event => event.preventDefault());
    let r = await call('execute', { ...h, step: { op: 'fill', target: target('#prevented'), value: 'changed' } });
    equal(r.error.code, 'execution_failed'); equal(r.dispatched, true); equal(document.querySelector('#prevented').value, 'keep');
    document.querySelector('#replaced').onfocus = event => { event.target.outerHTML = '<input id="replaced">'; };
    r = await call('execute', { ...h, step: { op: 'fill', target: target('#replaced'), value: 'wrong node' } });
    equal(r.error.code, 'target_changed'); equal(document.querySelector('#replaced').value, '');
    await call('cleanup', h);
  });
  await test('input cannot succeed when focus is redirected or the rendered target rejects its value', async () => {
    let h = await setup('<input id="redirected"><input id="other">');
    document.querySelector('#redirected').onfocus = () => document.querySelector('#other').focus();
    let r = await call('execute', { ...h, step: { op: 'fill', target: target('#redirected'), value: 'wrong focus' } });
    equal(r.error.code, 'not_actionable'); equal(document.querySelector('#redirected').value, '');
    await call('cleanup', h);
    h = await setup('<input id="field">');
    document.querySelector('#field').oninput = event => { event.target.outerHTML = '<input id="field" value="rejected">'; };
    r = await call('execute', { ...h, step: { op: 'fill', target: target('#field'), value: 'requested' } });
    equal(r.error.code, 'execution_failed'); equal(r.dispatched, true);
    await call('cleanup', h);
  });
  await test('fresh current-state observation does not latch a vanished element', async () => {
    const condition = { kind: 'element', target: target('#flash'), state: 'visible' };
    const h = await setup('<button id="fire">Fire</button>', condition);
    const p = await call('condition', h); equal(p.satisfied, false);
    document.querySelector('#fire').onclick = () => {
      const flash = document.createElement('div'); flash.id = 'flash'; flash.textContent = 'seen'; document.getElementById('fixture').append(flash); queueMicrotask(() => flash.remove());
    };
    await call('execute', { ...h, step: { op: 'click', target: target('#fire') } });
    const r = await call('condition', h); equal(r.satisfied, false); equal(r.correlation, 'state_only');
    await call('cleanup', h);
  });
  await test('conditions use current values, nullable attributes and any branch evidence', async () => {
    const h = await setup('<input value="yes"><div id="text">Hello world</div>');
    const r = await call('condition', { ...h, condition: { kind: 'any', conditions: [
      { kind: 'valueEquals', target: target('input'), expected: { literal: 'no' } },
      { kind: 'all', conditions: [
        { kind: 'textContains', target: target('#text'), expected: 'world' },
        { kind: 'attributeEquals', target: target('#text'), name: 'missing', expected: { literal: null } }
      ] }
    ] } });
    equal(r.satisfied, true); equal(r.lastObserved.matchedBranches, [1]);
    await call('cleanup', h);
  });
  await test('result conditions preserve JSON pointer escapes and legitimate null', async () => {
    const h = await setup('<p>Read only</p>');
    const resultsMap = { read: { 'a/b': { '~x': null } } };
    const r = await call('condition', { ...h, results: resultsMap, condition: { kind: 'result', stepId: 'read', pointer: '/a~1b/~0x', operator: 'exists' } });
    equal(r.satisfied, true);
    await call('cleanup', h);
  });
  await test('negative conditions are state only and unsupported transition semantics fail closed', async () => {
    const h = await setup('<p>Nothing pending</p>');
    let r = await call('condition', { ...h, condition: { kind: 'element', target: target('#missing'), state: 'hidden' } });
    equal(r.satisfied, true); equal(r.correlation, 'state_only');
    r = await call('condition', { ...h, condition: { kind: 'element', target: target('#missing'), state: 'hidden', semantics: 'transition_since_step_start' } });
    equal(r.error.code, 'unsupported_condition');
    await call('cleanup', h);
  });
  await test('stale page identity and unsupported bare refs cannot dispatch', async () => {
    const h = await setup('<button>Save</button>');
    let r = await call('execute', { ...h, expectedPageEpoch: 'another-page', step: { op: 'click', target: target('button') } });
    equal(r.error.code, 'stale_ref'); equal(r.dispatched, false);
    r = await call('execute', { ...h, step: { op: 'click', target: { by: 'ref', value: '@e1' } } });
    equal(r.error.code, 'unsupported_feature');
    await call('cleanup', h);
  });
  await test('observer registration failure blocks subsequent dispatch', async () => {
    const original = window.MutationObserver;
    try {
      window.MutationObserver = class { observe() { throw new Error('fixture observation failure'); } disconnect() {} };
      const r = await call('prepare', { observationId: 'broken', context, timeoutMs: 100 });
      equal(r.error.code, 'observation_failed');
      const e = await call('execute', { observationId: 'broken', context, step: { op: 'click', target: target('button') } });
      equal(e.error.code, 'observation_failed'); equal(e.dispatched, false);
    } finally { window.MutationObserver = original; }
  });
  await test('UP-T020 semantic replacement cannot silently change active workflow meaning', async () => {
    const h = await setup('<button>Save</button>');
    const original = window.__CONNECTOR_SEMANTIC__;
    try {
      Object.defineProperty(window, '__CONNECTOR_SEMANTIC__', { value: { ...original, version: 'changed' }, configurable: true });
      const r = await call('execute', { ...h, step: { op: 'click', target: target('button') } });
      equal(r.error.code, 'semantic_version_changed'); equal(r.dispatched, false);
    } finally { Object.defineProperty(window, '__CONNECTOR_SEMANTIC__', { value: original, configurable: true }); await call('cleanup', h); }
  });
  await test('cleanup is idempotent and prevents action after cancellation', async () => {
    const h = await setup('<button>Save</button>');
    let clicks = 0; document.querySelector('button').onclick = () => clicks++;
    equal((await call('cleanup', h)).cleaned, true);
    equal((await call('cleanup', h)).cleaned, false);
    const r = await call('execute', { ...h, step: { op: 'click', target: target('button') } });
    equal(r.error.code, 'observation_failed'); equal(clicks, 0);
  });
  await test('expired observation cannot dispatch and releases resources', async () => {
    const h = await setup('<button>Save</button>');
    await call('cleanup', h);
    const p = await call('prepare', { observationId: 'expiring', context, timeoutMs: 10 });
    await new Promise(resolve => setTimeout(resolve, 30));
    const r = await call('execute', { observationId: 'expiring', context: p.context, step: { op: 'click', target: target('button') } });
    equal(r.error.code, 'observation_failed'); equal(r.dispatched, false);
  });
  await test('evidence excludes user values and arbitrary data attributes', async () => {
    const secret = 'do-not-log-secret';
    const h = await setup(`<input type="password" value="${secret}" data-token="${secret}"><div>${secret}</div>`);
    const r = await call('evidence', { ...h, maxBytes: 1024 });
    assert(r.ok, JSON.stringify(r)); assert(!JSON.stringify(r).includes(secret), 'Evidence leaked fixture secret');
    assert(new TextEncoder().encode(JSON.stringify(r)).length <= 1024, 'Evidence exceeded budget');
    await call('cleanup', h);
  });
  await test('sensitive field extraction fails closed before values enter result bindings', async () => {
    for (const html of ['<input type="password" value="secret">', '<input name="access_token" value="secret">', '<div data-sensitive="true"><span>secret</span></div>']) {
      const h = await setup(html);
      const r = await call('execute', { ...h, step: { op: 'query', target: target('input,span'), query: { kind: 'value' } } });
      equal(r.error.code, 'capability_unavailable'); equal(r.dispatched, false);
      assert(!JSON.stringify(r).includes('secret'), 'Query leaked a sensitive fixture value');
      await call('cleanup', h);
    }
  });
  await test('scoped evidence stays within the requested dialog', async () => {
    const h = await setup('<input id="outside"><section role="dialog" aria-label="Selected"><button>Inside</button></section>');
    const r = await call('evidence', { ...h, target: { scope: { by: 'role', value: 'dialog', name: 'Selected' }, by: 'role', value: 'button' }, maxBytes: 2048 });
    equal(r.scope, 'locator_scope'); assert(r.elements.every(element => element.tag !== 'input'), 'Evidence included an outside field');
    await call('cleanup', h);
  });
  await test('evidence does not echo unrecognized custom tag or role strings', async () => {
    const h = await setup('<private-secret role="private-secret"></private-secret>');
    const r = await call('evidence', { ...h, target: target('private-secret'), maxBytes: 2048 });
    assert(!JSON.stringify(r).includes('private-secret'), 'Evidence retained an arbitrary role or tag');
    await call('cleanup', h);
  });
  return { tests: results.length, failures: results.filter(r => !r.passed).length, reactCoverage: window.WorkflowReact ? 'verified' : 'not_run_set_WORKFLOW_REACT_MODULES', results };
}

(async () => {
  const runtime = await readFile(join(__dirname, '../../src/workflow/page.js'), 'utf8');
  const semantic = await readFile(join(__dirname, '../../src/semantic/core.js'), 'utf8');
  const html = await readFile(join(__dirname, 'fixture.html'));
  const reactBundle = require('./react-fixture.cjs')(process.env.WORKFLOW_REACT_MODULES || join(__dirname, '../../..'));
  const server = createServer((req, res) => {
    if (req.url === '/react-fixture.js' && reactBundle) { res.setHeader('Content-Type', 'text/javascript'); res.end(reactBundle); }
    else { res.setHeader('Content-Type', 'text/html'); res.end(Buffer.concat([html, Buffer.from(reactBundle ? '<script src="/react-fixture.js"></script>' : '')])); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    const result = await page.evaluate(`(async () => { ${semantic}; return (${browserTests.toString()})(${runtime}); })()`);
    console.log(JSON.stringify({ layer: 'browser', browser: browser.version(), ...result }, null, 2));
    if (result.failures) process.exitCode = 1;
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
