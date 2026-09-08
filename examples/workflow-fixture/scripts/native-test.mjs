import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, openSync, closeSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = process.env.CONNECTOR_FIXTURE_OUTPUT || mkdtempSync(resolve(tmpdir(), 'connector-workflow-native-'));
mkdirSync(output, { recursive: true, mode: 0o700 });
const binary = process.env.CONNECTOR_FIXTURE_BINARY || resolve(root, 'target/debug/connector-workflow-fixture');
const evidencePath = resolve(output, 'native-store.json');
const token = randomBytes(32).toString('hex');
const instance = `dev.connector.workflow-fixture.${randomUUID()}`;
const isolatedInstances = [instance];
const tests = [];
const metrics = { clientRequests: 0, sentBytes: 0, receivedBytes: 0 };
const resultPath = resolve(output, 'native-results.json');
let processHandle;
const sockets = new Set();

async function portOccupied(port) {
  return new Promise(resolveResult => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    const done = value => { socket.destroy(); resolveResult(value); };
    socket.once('connect', () => done(true));
    socket.once('error', () => done(false));
    socket.setTimeout(200, () => done(false));
  });
}

class Rpc {
  constructor(socket) {
    this.socket = socket;
    this.pending = new Map();
    socket.addEventListener('message', event => {
      const text = String(event.data);
      metrics.receivedBytes += Buffer.byteLength(text);
      const response = JSON.parse(text);
      const waiter = this.pending.get(response.id);
      if (waiter) { this.pending.delete(response.id); clearTimeout(waiter.timer); waiter.resolve(response); }
    });
    socket.addEventListener('close', () => {
      for (const waiter of this.pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error('Fixture connection closed')); }
      this.pending.clear();
    });
  }
  static async connect() {
    const socket = new WebSocket('ws://127.0.0.1:19555');
    await new Promise((resolveReady, reject) => {
      socket.addEventListener('open', resolveReady, { once: true });
      socket.addEventListener('error', reject, { once: true });
    });
    sockets.add(socket);
    return new Rpc(socket);
  }
  request(command, timeoutMs = 45_000) {
    const id = randomUUID();
    const text = JSON.stringify({ id, ...command });
    metrics.clientRequests++;
    metrics.sentBytes += Buffer.byteLength(text);
    return new Promise((resolveReply, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Fixture request timeout: ${command.type}`)); }, timeoutMs);
      this.pending.set(id, { resolve: resolveReply, reject, timer });
      this.socket.send(text);
    });
  }
  async value(command) {
    const response = await this.request(command);
    if (response.error !== undefined) {
      throw new Error(`Fixture request failed: ${JSON.stringify({ error: response.error, outcome: response.outcome })}`);
    }
    return response.result;
  }
  workflow(operation, args) {
    return this.value({ type: 'workflow', operation: `workflow_${operation}`, args: { ...args, authToken: token } });
  }
  js(script, window_id = 'main') { return this.value({ type: 'execute_js', script, window_id }); }
  close() { this.socket.close(); sockets.delete(this.socket); }
}

function store() { return JSON.parse(readFileSync(evidencePath, 'utf8')); }
function spec(label) {
  const result = JSON.parse(readFileSync(resolve(root, 'examples/workflow/create-task.json'), 'utf8'));
  result.runKey = `native-${label}-${randomUUID()}`;
  result.inputs.taskName = 'native-' + label + '-' + randomUUID() + ' \"quote\" \\ newline\n中文 ${unintended} `';
  return result;
}
async function complete(rpc, report) {
  const until = Date.now() + 35_000;
  while (['queued', 'running', 'cancelling'].includes(report.status) && Date.now() < until) {
    await delay(30);
    report = await rpc.workflow('get', { runId: report.runId });
  }
  return report;
}
async function test(name, body) {
  const started = performance.now();
  try {
    const details = await body();
    tests.push({ name, passed: true, durationMs: Math.round(performance.now() - started), ...details });
    console.log(`PASS ${name}`);
  } catch (error) {
    tests.push({ name, passed: false, durationMs: Math.round(performance.now() - started), error: String(error.stack || error) });
    throw error;
  }
}
async function launchFixture(identifier, logName = 'native-app.log') {
  for (const port of [19555, 19556]) assert.equal(await portOccupied(port), false, `Refusing to use occupied fixture port ${port}`);
  const log = openSync(resolve(output, logName), 'w', 0o600);
  processHandle = spawn(binary, [], { cwd: root, env: { ...process.env,
    TAURI_CONNECTOR_WORKFLOW_TOKEN: token, CONNECTOR_FIXTURE_EVIDENCE: evidencePath, CONNECTOR_FIXTURE_ID: identifier },
    stdio: ['ignore', log, log] });
  closeSync(log);
  processHandle.on('error', error => console.error(`Fixture failed to launch: ${error.message}`));
  let rpc;
  const readyDeadline = Date.now() + 25_000;
  while (!rpc && Date.now() < readyDeadline) {
    if (processHandle.exitCode !== null) throw new Error(`Fixture exited: ${processHandle.exitCode}; see native-app.log`);
    try { rpc = await Rpc.connect(); } catch { await delay(100); }
  }
  assert.ok(rpc, 'Fixture WebSocket did not start');
  let ready;
  for (let attempt = 0; attempt < 40; attempt++) {
    // Plugin setup can expose its port before Tauri creates configured windows.
    try {
      ready = await rpc.js('({native:!!window.__TAURI_INTERNALS__,react:!!window.WorkflowReact,ready:!!window.__WORKFLOW_FIXTURE__,title:document.title})');
      if (ready.ready) break;
    } catch (error) {
      if (!String(error).includes("Window 'main' not found")) throw error;
    }
    await delay(100);
  }
  assert.equal(ready.native, true);
  assert.equal(ready.react, true);
  assert.equal(ready.ready, true);
  return rpc;
}
async function stopFixture() {
  if (processHandle && processHandle.exitCode === null) {
    processHandle.kill('SIGTERM');
    await Promise.race([new Promise(resolveExit => processHandle.once('exit', resolveExit)), delay(3000)]);
    if (processHandle.exitCode === null && processHandle.signalCode === null) {
      processHandle.kill('SIGKILL');
      await new Promise(resolveExit => processHandle.once('exit', resolveExit));
    }
  }
}
async function run() {
  let rpc = await launchFixture(instance);
  await test('native React controlled input reaches Rust save exactly once', async () => {
    const workflow = spec('combined');
    const before = store().saveCalls;
    const requests = metrics.clientRequests;
    const started = performance.now();
    const report = await complete(rpc, await rpc.workflow('run', { spec: workflow, waitMs: 30_000 }));
    assert.equal(report.status, 'completed', JSON.stringify(report));
    assert.equal(report.goalStatus, 'passed');
    assert.equal(store().saveCalls - before, 1);
    assert.equal(store().tasks.at(-1).name, workflow.inputs.taskName);
    const stats = await rpc.js('window.__WORKFLOW_FIXTURE__');
    assert.equal(stats.submissions.at(-1), workflow.inputs.taskName);
    assert.ok(stats.inputEvents > 0);
    writeFileSync(resolve(output, 'combined-report.json'), JSON.stringify(report, null, 2));
    return { runId: report.runId, saveEffects: 1, workflowClientRequests: metrics.clientRequests - requests - 1, elapsedMs: Math.round(performance.now() - started), reportBytes: Buffer.byteLength(JSON.stringify(report)) };
  });
  await test('same UI path via one caller request per step', async () => {
    const workflow = spec('per-step');
    const before = store().saveCalls;
    const requests = metrics.clientRequests;
    for (let index = 0; index < workflow.steps.length; index++) {
      const single = { ...workflow, runKey: `${workflow.runKey}-${index}`, steps: [workflow.steps[index]] };
      if (index < workflow.steps.length - 1) delete single.goal;
      const report = await complete(rpc, await rpc.workflow('run', { spec: single, waitMs: 30_000 }));
      assert.equal(report.status, 'completed', JSON.stringify(report));
      if (single.goal) assert.equal(report.goalStatus, 'passed');
    }
    assert.equal(store().saveCalls - before, 1);
    assert.equal(store().tasks.at(-1).name, workflow.inputs.taskName);
    return { saveEffects: 1, workflowClientRequests: metrics.clientRequests - requests };
  });
  await test('two clients share one runKey and cannot change its inputs', async () => {
    const second = await Rpc.connect();
    const workflow = spec('dedup');
    const before = store().saveCalls;
    const [first, duplicate] = await Promise.all([
      rpc.workflow('run', { spec: workflow, waitMs: 0 }), second.workflow('run', { spec: workflow, waitMs: 0 })
    ]);
    assert.equal(first.runId, duplicate.runId);
    const done = await complete(rpc, first);
    assert.equal(done.status, 'completed', JSON.stringify(done));
    assert.equal(store().saveCalls - before, 1);
    const conflict = await second.request({ type: 'workflow', operation: 'workflow_run', args: { authToken: token,
      spec: { ...workflow, inputs: { taskName: 'conflicting input' } } } });
    assert.match(JSON.stringify(conflict.error), /run_key_conflict/);
    second.close();
    return { runId: first.runId, saveEffects: 1 };
  });
  await test('disconnect and resubmit retain the existing logical run', async () => {
    await rpc.js("window.__TAURI__.core.invoke('fixture_set_delay',{delayMs:1200})");
    const transient = await Rpc.connect();
    const workflow = spec('reconnect');
    const before = store().saveCalls;
    const first = await transient.workflow('run', { spec: workflow, waitMs: 0 });
    transient.close();
    const replacement = await Rpc.connect();
    const duplicate = await replacement.workflow('run', { spec: workflow, waitMs: 0 });
    assert.equal(first.runId, duplicate.runId);
    const done = await complete(replacement, duplicate);
    assert.equal(done.status, 'completed', JSON.stringify(done));
    assert.equal(store().saveCalls - before, 1);
    await rpc.js("window.__TAURI__.core.invoke('fixture_set_delay',{delayMs:0})");
    replacement.close();
    return { runId: first.runId, saveEffects: 1 };
  });
  await test('cancel stops an outstanding wait and releases bridge waiters/resources', async () => {
    const workflow = spec('cancel');
    workflow.steps = [{ id: 'waiting', op: 'wait', condition: { kind: 'element', target: { by: 'testId', value: 'never-created' }, state: 'visible' } }, workflow.steps[0]];
    delete workflow.goal;
    const before = store().saveCalls;
    const first = await rpc.workflow('run', { spec: workflow, waitMs: 0 });
    await delay(100);
    const cancelled = await complete(rpc, await rpc.workflow('cancel', { runId: first.runId }));
    assert.ok(['cancelled', 'paused'].includes(cancelled.status), JSON.stringify(cancelled));
    assert.equal(store().saveCalls, before);
    const inspection = await rpc.js('({fixture:window.__WORKFLOW_FIXTURE__,page:window.__CONNECTOR_WORKFLOW__ ? Object.keys(window.__CONNECTOR_WORKFLOW__) : []})');
    assert.equal(inspection.fixture.errors.length, 0);
    const bridge = await rpc.value({ type: 'bridge_status' });
    assert.equal(bridge.pending, 0);
    return { runId: first.runId, status: cancelled.status, bridgePending: bridge.pending, saveEffects: 0 };
  });
  await test('WS operation beyond two seconds produces one native write', async () => {
    const before = store().saveCalls;
    await rpc.js("window.__TAURI__.core.invoke('fixture_set_delay',{delayMs:2200})");
    const result = await rpc.js("window.__TAURI__.core.invoke('fixture_create_task',{name:'slow-transport-fixture'})");
    assert.equal(result.name, 'slow-transport-fixture');
    assert.equal(store().saveCalls - before, 1);
    await rpc.js("window.__TAURI__.core.invoke('fixture_set_delay',{delayMs:0})");
    return { saveEffects: 1 };
  });
  await test('JS exception after native write does not replay through eval', async () => {
    const before = store().saveCalls;
    const response = await rpc.request({ type: 'execute_js', window_id: 'main', script: "(async()=>{await window.__TAURI__.core.invoke('fixture_create_task',{name:'throw-after-write'});throw new Error('fixture exception after write')})()" });
    assert.match(JSON.stringify(response.error), /fixture exception after write/);
    assert.equal(response.outcome.execution, 'failed');
    assert.equal(store().saveCalls - before, 1);
    const conflict = await rpc.request({ type: 'execute_js', window_id: 'main', script: '1+1' });
    assert.equal(conflict.outcome?.error?.code, 'resource_busy', JSON.stringify(conflict));
    return { saveEffects: 1, outcome: response.outcome, conflictingWriteBlocked: true };
  });
  if (!process.argv.includes('--skip-unknown')) {
    // A partial-effect exception now correctly quarantines the fixture. The
    // independent lost-response scenario needs its own process and journal;
    // clearing or bypassing that quarantine would invalidate the safety test.
    writeFileSync(resolve(output, 'native-store-before-isolated-scenario.json'), readFileSync(evidencePath));
    rpc.close();
    await stopFixture();
    const unknownInstance = `dev.connector.workflow-fixture.${randomUUID()}`;
    isolatedInstances.push(unknownInstance);
    rpc = await launchFixture(unknownInstance, 'native-unknown-app.log');
    await test('unknown write response quarantines resource without replay', async () => {
      const before = store().saveCalls;
      const response = await rpc.request({ type: 'execute_js', window_id: 'main', script: "(async()=>{await window.__TAURI__.core.invoke('fixture_create_task',{name:'lost-response-fixture'});return new Promise(()=>{})})()" });
      assert.equal(response.outcome.execution, 'outcome_unknown', JSON.stringify(response));
      assert.equal(store().saveCalls - before, 1);
      const conflict = await rpc.request({ type: 'execute_js', window_id: 'main', script: '1+1' });
      assert.equal(conflict.outcome?.error?.code, 'resource_busy', JSON.stringify(conflict));
      const bridge = await rpc.value({ type: 'bridge_status' });
      assert.equal(bridge.pending, 0);
      assert.equal(store().saveCalls - before, 1);
      return { saveEffects: 1, outcome: response.outcome, conflictingWriteBlocked: true, bridgePending: 0, isolatedInstance: unknownInstance };
    });
  }
  rpc.close();
}

try {
  await run();
} catch (error) {
  console.error(String(error.stack || error));
  process.exitCode = 1;
} finally {
  for (const socket of sockets) socket.close();
  await stopFixture();
  writeFileSync(resultPath, JSON.stringify({ platform: process.platform, runtime: 'actual Tauri Wry native WebView',
    instance, isolatedInstances, tests, metrics, passed: tests.length > 0 && tests.every(test => test.passed) && !process.exitCode,
    untestedPlatforms: ['win32', 'linux'].filter(platform => platform !== process.platform),
    caveats: ['UI state and isolated native fixture store only; no production service or business persistence provider', 'Synthetic DOM input and event dispatch'],
  }, null, 2));
  console.log(`Native fixture evidence: ${resultPath}`);
}
