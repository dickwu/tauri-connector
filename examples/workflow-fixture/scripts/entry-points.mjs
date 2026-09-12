import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID, createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

// This helper controls only the already-running isolated fixture. It does not
// launch a second app, synthesize a selection, or keep a picker state machine.
export async function verifyEntryPoints({ rpc, token, root, output, selectedPickerId, partialPickerId }) {
  const host = '127.0.0.1';
  const port = '19555';
  const mcpUrl = `http://${host}:19556/mcp`;
  const suffix = process.platform === 'win32' ? '.exe' : '';
  const cliBinary = process.env.CONNECTOR_FIXTURE_CLI_BINARY || resolve(root, `target/debug/tauri-connector${suffix}`);
  const mcpBinary = process.env.CONNECTOR_FIXTURE_MCP_BINARY || resolve(root, `target/debug/tauri-connector-mcp${suffix}`);
  assert.ok(existsSync(cliBinary), 'Build connector-cli before entry-point verification');
  assert.ok(existsSync(mcpBinary), 'Build connector-mcp-server before entry-point verification');
  const identity = await rpc.inspect('app_identity');
  const cache = resolve(output, 'entry-point-private-cache');
  mkdirSync(cache, { recursive: true, mode: 0o700 });
  const env = { ...process.env, TAURI_CONNECTOR_HOST: host, TAURI_CONNECTOR_PORT: port,
    TAURI_CONNECTOR_APP_INSTANCE_ID: identity.appInstanceId, TAURI_CONNECTOR_APP_ID: identity.appId,
    TAURI_CONNECTOR_WORKFLOW_TOKEN: token, XDG_CACHE_HOME: cache };
  delete env.TAURI_CONNECTOR_PID_FILE;
  const cases = [];
  const handles = new Set();
  const sessions = [];
  let embeddedSession;
  const redact = value => String(value).split(token).join('[redacted]');
  const countersPath = resolve(output, 'independent-native-store.json');
  const beforeCounters = existsSync(countersPath) ? JSON.parse(readFileSync(countersPath, 'utf8')) : undefined;

  const parseReport = result => {
    const text = result?.content?.find(item => item.type === 'text')?.text;
    const parsed = text === undefined ? undefined : JSON.parse(text);
    if (result?.structuredContent !== undefined && parsed !== undefined) assert.deepEqual(result.structuredContent, parsed);
    return result?.structuredContent ?? parsed;
  };

  async function runCli(args, overrides = {}) {
    const child = spawn(cliBinary, ['--host', host, '--port', port, ...args], {
      cwd: root, env: { ...env, ...overrides }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '', stderr = '';
    const read = target => data => {
      if (target === 'stdout') stdout += data; else stderr += data;
      if (stdout.length + stderr.length > 2 * 1024 * 1024) child.kill('SIGKILL');
    };
    child.stdout.on('data', read('stdout')); child.stderr.on('data', read('stderr'));
    const { code, signal } = await new Promise((resolveExit, reject) => {
      const timeout = setTimeout(() => { child.kill('SIGKILL'); reject(new Error('CLI entry-point deadline exceeded')); }, 25000);
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('close', (code, signal) => { clearTimeout(timeout); resolveExit({ code, signal }); });
    });
    assert.equal(stdout.includes(token) || stderr.includes(token), false, 'CLI must not expose its credential');
    assert.equal(signal, null, redact(stderr));
    let report;
    try { report = JSON.parse(stdout); } catch { throw new Error(`CLI stdout was not one JSON result: ${redact(stdout)}; stderr: ${redact(stderr)}`); }
    return { code, report, stderr: redact(stderr) };
  }

  async function embedded(method, params, version = '2025-06-18') {
    const response = await fetch(mcpUrl, { method: 'POST', signal: AbortSignal.timeout(20000),
      headers: { 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream',
        'MCP-Protocol-Version': version, ...(embeddedSession ? { 'Mcp-Session-Id': embeddedSession } : {}) },
      body: JSON.stringify({ jsonrpc: '2.0', id: randomUUID(), method, params }),
    });
    assert.equal(response.ok, true, `Embedded MCP HTTP ${response.status}`);
    if (method === 'initialize') embeddedSession = response.headers.get('mcp-session-id');
    const body = await response.json();
    assert.equal(body.error, undefined, redact(JSON.stringify(body.error)));
    return body.result;
  }

  function startStdio(authorized) {
    const childEnv = { ...env };
    if (!authorized) { delete childEnv.TAURI_CONNECTOR_WORKFLOW_TOKEN; delete childEnv.TAURI_CONNECTOR_APP_INSTANCE_ID; delete childEnv.TAURI_CONNECTOR_APP_ID; }
    const child = spawn(mcpBinary, [], { cwd: root, env: childEnv, stdio: ['pipe', 'pipe', 'pipe'] });
    const pending = new Map();
    let buffer = '', stderr = '';
    const rejectAll = message => { for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error(redact(message))); } pending.clear(); };
    child.stderr.on('data', data => { stderr += data; if (stderr.length > 2 * 1024 * 1024) child.kill('SIGKILL'); });
    child.stdout.on('data', data => {
      buffer += data;
      if (buffer.length > 2 * 1024 * 1024) { rejectAll('MCP output budget exceeded'); child.kill('SIGKILL'); return; }
      for (;;) {
        const end = buffer.indexOf('\n'); if (end < 0) break;
        const line = buffer.slice(0, end); buffer = buffer.slice(end + 1); if (!line.trim()) continue;
        let response;
        try { response = JSON.parse(line); } catch { rejectAll('MCP stdout contained non-JSON output'); continue; }
        const item = pending.get(response.id); if (!item) continue;
        pending.delete(response.id); clearTimeout(item.timer);
        if (response.error) item.reject(new Error(redact(JSON.stringify(response.error)))); else item.resolve(response.result);
      }
    });
    child.on('error', error => rejectAll(error.message));
    child.on('close', () => rejectAll(`MCP subprocess closed: ${stderr}`));
    const session = {
      request(method, params) { return new Promise((resolveResult, reject) => {
        if (child.exitCode !== null) { reject(new Error('MCP subprocess already exited')); return; }
        const id = randomUUID();
        const timer = setTimeout(() => { pending.delete(id); reject(new Error('MCP response deadline exceeded')); }, 20000);
        pending.set(id, { resolve: resolveResult, reject, timer });
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
      }); },
      async close() {
        assert.equal(stderr.includes(token), false, 'MCP must not expose its environment credential');
        child.stdin.end();
        if (child.exitCode !== null) return;
        await new Promise(resolveExit => {
          const timer = setTimeout(() => { child.kill('SIGKILL'); resolveExit(); }, 2000);
          child.once('close', () => { clearTimeout(timer); resolveExit(); });
        });
      },
    };
    sessions.push(session); return session;
  }

  async function awaiting(report) {
    const deadline = Date.now() + 7000;
    while (['created', 'installing'].includes(report.status) && Date.now() < deadline) {
      report = await rpc.pick({ action: 'get', pickerId: report.pickerId, waitMs: 1000 });
    }
    assert.equal(report.status, 'awaiting_selection', JSON.stringify(report));
    assert.equal(report.context.appInstanceId, identity.appInstanceId);
    return report;
  }
  async function cleanup(id) {
    let report = await rpc.pick({ action: 'cancel', pickerId: id });
    const deadline = Date.now() + 7000;
    while (!report.resultComplete && Date.now() < deadline) report = await rpc.pick({ action: 'get', pickerId: id, waitMs: 1000 });
    assert.equal(report.resultComplete, true, 'Picker cleanup must complete before the next entry-point case');
    assert.ok(['confirmed', 'context_destroyed'].includes(report.cleanup.status), JSON.stringify(report.cleanup));
    handles.delete(id); return report;
  }

  try {
    const initialized = await embedded('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'isolated-entry-point-test', version: '1' } });
    assert.equal(initialized.protocolVersion, '2025-06-18'); assert.ok(embeddedSession);
    const stdio = startStdio(true);
    assert.equal((await stdio.request('initialize', { protocolVersion: '2025-06-18' })).protocolVersion, '2025-06-18');
    const capture = await rpc.inspect('ipc_capture',{action:'start',options:{commands:['fixture_binary_result']}});
    try {
      await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_binary_result').then(value=>value.byteLength)");
      let observed;
      const until=Date.now()+5000;
      do {observed=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId,limit:2});if(observed.events.length===2)break;await new Promise(resolve=>setTimeout(resolve,30));} while(Date.now()<until);
      assert.equal(observed.events.length,2);
      const first=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId,limit:1});
      assert.equal(first.events.length,1);assert.equal(typeof first.nextCursor,'string');
      const second=await runCli(['ipc','query',capture.captureSessionId,'--cursor',first.nextCursor,'--limit','1']);
      assert.equal(second.code,0);assert.equal(second.report.events.length,1);
      assert.notEqual(second.report.events[0].phase,first.events[0].phase);
      for(const call of [params=>embedded('tools/call',params),params=>stdio.request('tools/call',params)]) {
        const result=parseReport(await call({name:'ipc_query',arguments:{captureSessionId:capture.captureSessionId,cursor:first.nextCursor,limit:1,authToken:token}}));
        assert.deepEqual(result.events,second.report.events);
      }
      cases.push({name:'Opaque IPC cursor round-trips unchanged through WS, CLI and both MCP servers',passed:true});
    } finally {await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:capture.captureSessionId});}
    if (selectedPickerId) {
      const saved=await rpc.pick({action:'get',pickerId:selectedPickerId,includeImage:true});
      const expected=saved.screenshot.image.base64;
      for(const [name,call] of [['embedded',params=>embedded('tools/call',params)],['stdio',params=>stdio.request('tools/call',params)]]) {
        const result=await call({name:'webview_select_element',arguments:{action:'get',pickerId:selectedPickerId,includeImage:true,authToken:token}});
        assert.notEqual(result.isError,true);const report=parseReport(result);assert.deepEqual(report.selection,saved.selection);
        const images=result.content.filter(item=>item.type==='image');assert.equal(images.length,1);assert.equal(images[0].mimeType,'image/png');assert.equal(images[0].data,expected);
        assert.equal(report.screenshot.widthPx,saved.screenshot.widthPx);
        cases.push({name:name+' returns cached masked PNG and complete metadata',passed:true,pickerId:selectedPickerId,pngSha256:createHash('sha256').update(Buffer.from(expected,'base64')).digest('hex')});
      }
    }
    if (partialPickerId) {
      for(const call of [params=>embedded('tools/call',params),params=>stdio.request('tools/call',params)]) {
        const result=await call({name:'webview_select_element',arguments:{action:'get',pickerId:partialPickerId,includeImage:true,authToken:token}});
        assert.notEqual(result.isError,true);const report=parseReport(result);assert.equal(report.status,'selected');assert.ok(report.selection);assert.equal(report.screenshot.status,'failed');assert.ok(!result.content.some(item=>item.type==='image'));
      }
      cases.push({name:'Both MCPs preserve selected metadata when screenshot failed',passed:true,pickerId:partialPickerId});
    }
    const key = randomUUID();
    const started = await runCli(['picker', 'start', '--window', 'main', '--request-key', key, '--timeout-ms', '20000', '--no-screenshot']);
    assert.equal(started.code, 2); assert.ok(started.report.pickerId);
    assert.ok(started.stderr.includes(key)); assert.ok(started.stderr.includes(started.report.pickerId));
    handles.add(started.report.pickerId);
    const ready = await awaiting(started.report);
    const cliGet=await runCli(['picker','get',ready.pickerId]);
    assert.equal(cliGet.code,2);assert.equal(cliGet.report.pickerId,ready.pickerId);assert.deepEqual(cliGet.report.context,ready.context);
    const embeddedResult = await embedded('tools/call', { name: 'webview_select_element', arguments: { action: 'get', pickerId: ready.pickerId, authToken: token } });
    assert.notEqual(embeddedResult.isError, true);
    const embeddedReport = parseReport(embeddedResult);
    assert.equal(embeddedReport.pickerId, ready.pickerId); assert.deepEqual(embeddedReport.context, ready.context);
    const cancelledResult = await stdio.request('tools/call', { name: 'webview_select_element', arguments: { action: 'cancel', pickerId: ready.pickerId } });
    assert.notEqual(cancelledResult.isError, true);
    const cancelled = parseReport(cancelledResult);
    assert.equal(cancelled.pickerId, ready.pickerId); assert.equal(cancelled.status, 'cancelled');
    const final = await cleanup(ready.pickerId);
    assert.equal(final.status, 'cancelled'); assert.deepEqual(final.context, ready.context); assert.ok(final.revision >= embeddedReport.revision);
    const repeatedCancel = await runCli(['picker', 'cancel', ready.pickerId]);
    assert.equal(repeatedCancel.code, 0); assert.equal(repeatedCancel.report.status, 'cancelled');
    const missing = await runCli(['picker', 'get', `${identity.appInstanceId}:picker:${randomUUID()}`]);
    assert.equal(missing.code, 1); assert.equal(missing.report.code, 'picker_not_found');
    cases.push({ name: 'CLI start -> embedded MCP get -> standalone MCP cancel -> WS retained result', passed: true,
      pickerId: final.pickerId, appInstanceId: identity.appInstanceId, cliStartExit: started.code, cliCancelExit: repeatedCancel.code, cliMissingHandleExit: missing.code,
      status: final.status, cleanup: final.cleanup.status, requestKeyPresentOnStderr: true, metadataContextEqual: true });

    const denied = await embedded('tools/call', { name: 'webview_select_element', arguments: { action: 'get', pickerId: final.pickerId } });
    assert.equal(denied.isError, true); assert.equal(parseReport(denied).code, 'unauthorized');
    const deniedWs = await rpc.request({ type: 'inspection', operation: 'webview_select_element', args: { action: 'get', pickerId: final.pickerId } });
    assert.equal(JSON.parse(deniedWs.error).code, 'unauthorized');
    const anonymous = startStdio(false);
    await anonymous.request('initialize', { protocolVersion: '2025-03-26' });
    const deniedStdio = await anonymous.request('tools/call', { name: 'webview_select_element', arguments: { action: 'get', pickerId: final.pickerId } });
    assert.equal(deniedStdio.isError, true); assert.equal(deniedStdio.structuredContent, undefined);
    assert.equal(parseReport(deniedStdio).code, 'unauthorized');
    cases.push({ name: 'Anonymous retained picker denied across WS and both MCP paths', passed: true, oldProtocolTextRetained: true });

    const convenient = await runCli(['select-element', '--window', 'main', '--timeout-ms', '20000', '--no-screenshot', '--wait-ms', '0']);
    assert.equal(convenient.code, 2); assert.notEqual(convenient.report.pickerId, final.pickerId);
    handles.add(convenient.report.pickerId);
    const convenientReady = await awaiting(convenient.report);
    const completed = await cleanup(convenientReady.pickerId);
    cases.push({ name: 'CLI omitted-action convenience activates native page picker and cleans up', passed: true,
      pickerId: completed.pickerId, beforeCancel: convenientReady.status, afterCancel: completed.status, cleanup: completed.cleanup.status, cliExit: convenient.code });
    const example=name=>JSON.parse(readFileSync(resolve(root,'examples/inspection/'+name),'utf8'));
    const startExample=example('picker-start.json');startExample.requestKey=randomUUID();
    const sample=await awaiting(await rpc.pick(startExample));handles.add(sample.pickerId);
    const getExample={...example('picker-get.json'),pickerId:sample.pickerId};
    const reading=rpc.pick(getExample);
    const cancelExample={...example('picker-cancel.json'),pickerId:sample.pickerId};
    await embedded('tools/call',{name:'webview_select_element',arguments:{...cancelExample,authToken:token}});
    const readResult=await reading;assert.equal(readResult.pickerId,sample.pickerId);assert.equal(readResult.status,'cancelled');await cleanup(sample.pickerId);
    const convenienceExample=example('picker-convenience.json');convenienceExample.requestKey=randomUUID();
    const convenienceReply=rpc.pick(convenienceExample);
    let exampleReady=parseReport(await embedded('tools/call',{name:'webview_select_element',arguments:{...convenienceExample,action:'start',waitMs:0,authToken:token}}));handles.add(exampleReady.pickerId);
    const exampleDeadline=Date.now()+5000;
    while(['created','installing'].includes(exampleReady.status)&&Date.now()<exampleDeadline)exampleReady=parseReport(await embedded('tools/call',{name:'webview_select_element',arguments:{action:'get',pickerId:exampleReady.pickerId,waitMs:1000,authToken:token}}));
    assert.equal(exampleReady.status,'awaiting_selection');
    await embedded('tools/call',{name:'webview_select_element',arguments:{action:'cancel',pickerId:exampleReady.pickerId,authToken:token}});
    assert.equal((await convenienceReply).status,'cancelled');await cleanup(exampleReady.pickerId);
    cases.push({name:'All four shipped inspection JSON examples execute with fresh keys and returned handles',passed:true,examples:['picker-start.json','picker-get.json','picker-cancel.json','picker-convenience.json']});
    if (beforeCounters !== undefined) assert.deepEqual(JSON.parse(readFileSync(countersPath, 'utf8')), beforeCounters, 'Entry-point tests must not dispatch fixture business writes');
    const result = { layer: 'running-native-app-entry-points', syntheticSelectionUsed: false, nativeUserSelectionPerformed: false, cases };
    const serialized = JSON.stringify(result, null, 2);
    assert.equal(serialized.includes(token), false);
    writeFileSync(resolve(output, 'entry-points-results.json'), serialized, { mode: 0o600 });
    return result;
  } finally {
    for (const id of handles) await cleanup(id).catch(() => {});
    for (const session of sessions) await session.close();
    if (embeddedSession) await fetch(mcpUrl, { method: 'DELETE', signal: AbortSignal.timeout(3000), headers: { 'Mcp-Session-Id': embeddedSession, 'MCP-Protocol-Version': '2025-06-18' } }).catch(() => {});
  }
}
