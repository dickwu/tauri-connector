import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = mkdtempSync(resolve(tmpdir(), 'connector-feature-off-'));
const binary = process.env.CONNECTOR_FIXTURE_BINARY || resolve(root, 'target/debug/connector-workflow-fixture');
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
const env = { ...process.env, CONNECTOR_FIXTURE_ID: `dev.connector.workflow-fixture.${randomUUID()}` };
delete env.TAURI_CONNECTOR_WORKFLOW_TOKEN;
let log = '';
const child = spawn(binary, [], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
child.stdout.on('data', data => { log += data; });
child.stderr.on('data', data => { log += data; });
try {
  await delay(2000);
  assert.equal(child.exitCode, null, 'Feature-off Wry app must remain running');
  for (const port of [19555, 19556]) assert.equal(await listening(port), false, `Feature-off app opened connector port ${port}`);
  assert.ok(!log.includes('[connector]'), 'Feature-off app must not initialize connector');
  writeFileSync(resolve(output, 'result.json'), JSON.stringify({ platform: process.platform, nativeAppRunning: true, connectorFeature: false,
    listeningPorts: [], checkedPorts: [19555, 19556], passed: true }, null, 2));
  console.log(`PASS native feature-off app opens no connector listeners: ${output}/result.json`);
} finally {
  child.kill('SIGTERM');
  await Promise.race([new Promise(resolveExit => child.once('exit', resolveExit)), delay(2000)]);
  if (child.exitCode === null) child.kill('SIGKILL');
}
