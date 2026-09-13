import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';
import { assertFrontendBoot } from './native-evidence.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = process.env.CONNECTOR_FIXTURE_OUTPUT || mkdtempSync(resolve(tmpdir(), 'connector-feature-off-'));
mkdirSync(output, { recursive: true, mode: 0o700 });
const binary = process.env.CONNECTOR_FIXTURE_BINARY || resolve(root, `target/debug/connector-workflow-fixture${process.platform === 'win32' ? '.exe' : ''}`);
async function listening(port) {
  return new Promise(resolveResult => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    const finish = value => { socket.destroy(); resolveResult(value); };
    socket.once('connect', () => finish(true));
    socket.once('error', () => finish(false));
    socket.setTimeout(200, () => finish(false));
  });
}
for (const port of [19555, 19556]) assert.equal(await listening(port), false, `Fixture port ${port} already occupied`);
const bootPath = resolve(output, `frontend-boot-${randomUUID()}.json`);
const env = { ...process.env, CONNECTOR_FIXTURE_ID: `dev.connector.workflow-fixture.${randomUUID()}`,
  CONNECTOR_FIXTURE_BOOT_EVIDENCE: bootPath };
delete env.TAURI_CONNECTOR_WORKFLOW_TOKEN;
let log = '';
const child = spawn(binary, [], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
let exited = false;
let spawnError;
child.once('exit', () => { exited = true; });
child.once('error', error => { spawnError = error; });
function assertRunning() {
  assert.equal(spawnError, undefined, 'Feature-off Wry app must start successfully');
  assert.equal(child.exitCode, null, 'Feature-off Wry app must remain running');
  // A process terminated by a signal also has exitCode === null.
  assert.equal(child.signalCode, null, 'Feature-off Wry app must remain running without a terminating signal');
  assert.equal(exited, false, 'Feature-off Wry app must remain running');
  assert.equal(child.killed, false, 'Feature-off Wry app must not already be stopping');
  assert.ok(Number.isInteger(child.pid) && child.pid > 0, 'Feature-off Wry app must have a process identity');
  assert.doesNotThrow(() => process.kill(child.pid, 0), 'Feature-off Wry app is no longer alive');
}
child.stdout.on('data', data => { log += data; });
child.stderr.on('data', data => { log += data; });
try {
  await delay(2000);
  assertRunning();
  const deadline = Date.now() + 8000;
  while (!existsSync(bootPath) && Date.now() < deadline) {
    assertRunning();
    await delay(100);
  }
  assertRunning();
  assert.ok(existsSync(bootPath), 'Feature-off frontend boot evidence missing: live process alone does not prove WebView readiness');
  const frontendBoot = JSON.parse(readFileSync(bootPath, 'utf8'));
  assertFrontendBoot(frontendBoot, child.pid);
  for (const port of [19555, 19556]) assert.equal(await listening(port), false, `Feature-off app opened connector port ${port}`);
  assert.ok(!log.includes('[connector]'), 'Feature-off app must not initialize connector');
  assertRunning();
  writeFileSync(resolve(output, 'result.json'), JSON.stringify({ platform: process.platform, layer: 'native-webview',
    display: process.platform === 'linux' ? (process.env.DISPLAY || 'unavailable') : 'desktop-session', nativeAppRunning: true, connectorFeature: false,
    frontendBoot, listeningPorts: [], checkedPorts: [19555, 19556], passed: true }, null, 2));
  console.log(`PASS native feature-off frontend is ready with no connector injection or listeners: ${output}/result.json`);
} finally {
  if (!exited && child.pid && child.exitCode === null && child.signalCode === null) {
    child.kill('SIGTERM');
    await new Promise(resolveExit => {
      const timeout = setTimeout(resolveExit, 2000);
      child.once('exit', () => { clearTimeout(timeout); resolveExit(); });
    });
    if (!exited && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  }
}
