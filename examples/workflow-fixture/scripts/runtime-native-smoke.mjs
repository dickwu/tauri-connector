// Native runtime/bridge lifecycle smoke. Does not synthesize or claim user input.
import assert from 'node:assert/strict';
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {randomBytes,randomUUID} from 'node:crypto';
import {mkdirSync,mkdtempSync,openSync,closeSync,writeFileSync,readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';
import net from 'node:net';
const root=fileURLToPath(new URL('../../../',import.meta.url));
const output=process.env.CONNECTOR_FIXTURE_OUTPUT||mkdtempSync(resolve(tmpdir(),'connector-runtime-native-'));
mkdirSync(output,{recursive:true,mode:0o700});
const token=randomBytes(32).toString('hex');
const binary=process.env.CONNECTOR_FIXTURE_BINARY||resolve(root,`target/debug/connector-workflow-fixture${process.platform==='win32'?'.exe':''}`);
const results={platform:process.platform,layer:'native_webview',input:'none',tests:[],passed:false};
let child,socket;const pending=new Map();
const occupied=port=>new Promise(resolve=>{const s=net.createConnection({host:'127.0.0.1',port});s.once('connect',()=>{s.destroy();resolve(true);});s.once('error',()=>resolve(false));});
function request(command){return new Promise((resolve,reject)=>{const id=randomUUID();const timer=setTimeout(()=>{pending.delete(id);reject(Error(`fixture RPC deadline: ${command.type}/${command.operation||''}`));},10000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,...command}));});}
async function value(command){const reply=await request(command);if(reply.error)throw Error(JSON.stringify(reply.error));return reply.result;}
const inspect=(operation,args={})=>value({type:'inspection',operation,args:{...args,authToken:token}});
try {
  for(const port of [19555,19556])assert.equal(await occupied(port),false,'refusing occupied fixture port');
  const activateHelper=resolve(output,'native-input');
  if(process.platform==='darwin')await promisify(execFile)('swiftc',[resolve(root,'examples/workflow-fixture/scripts/native-input.swift'),'-o',activateHelper]);
  const log=openSync(resolve(output,'private-app.log'),'w',0o600);
  child=spawn(binary,[],{cwd:root,env:{...process.env,TAURI_CONNECTOR_WORKFLOW_TOKEN:token,CONNECTOR_FIXTURE_ID:`dev.connector.workflow-fixture.runtime.${randomUUID()}`,CONNECTOR_FIXTURE_EVIDENCE:resolve(output,'independent-native-store.json')},stdio:['ignore',log,log]});closeSync(log);
  for(let attempt=0;attempt<100;attempt++){
    const candidate=new WebSocket('ws://127.0.0.1:19555');
    try{await new Promise((ok,no)=>{candidate.addEventListener('open',ok,{once:true});candidate.addEventListener('error',no,{once:true});});socket=candidate;break;}catch{candidate.close();await delay(100);}
  }
  assert.ok(socket,'native fixture endpoint unavailable');
  socket.addEventListener('message',event=>{const reply=JSON.parse(String(event.data));const waiter=pending.get(reply.id);if(waiter){pending.delete(reply.id);clearTimeout(waiter.timer);waiter.resolve(reply);}});
  for(let attempt=0;attempt<60;attempt++){
    const status=await value({type:'bridge_status'});
    if(status.clients.some(client=>client.windowId==='main'))break;
    await delay(100);
  }
  // Explicit fixture preparation keeps the native test surface foreground;
  // production picker APIs never activate, focus or restore an application.
  if(process.platform==='darwin') {
    await promisify(execFile)(activateHelper,['activate',String(child.pid)]);
    results.preparation='Explicit activation of the owned native fixture PID; no OS input events posted';
  }
  const anonymous=await request({type:'inspection',operation:'runtime_health',args:{windowId:'main'}});
  assert.ok(anonymous.error);results.tests.push({id:'UP-T036',name:'anonymous rich health rejected',passed:true});
  const cold=await inspect('runtime_health',{windowId:'main'});
  assert.equal(cold.checks.bridge.connected,true);assert.equal(cold.checks.bridge.status,'responsive');
  assert.equal(cold.checks.runtime.status,'unavailable');assert.equal(cold.diagnostics.installAttempts,0);
  results.tests.push({id:'UP-T033',name:'native read-only probe does not install missing runtime',passed:true});
  let picker=await inspect('webview_select_element',{action:'start',requestKey:randomUUID(),captureScreenshot:false});
  const until=Date.now()+6000;
  while(['created','installing'].includes(picker.status)&&Date.now()<until)picker=await inspect('webview_select_element',{action:'get',pickerId:picker.pickerId,waitMs:1000});
  assert.equal(picker.status,'awaiting_selection',JSON.stringify(picker));
  const health=await inspect('runtime_health',{windowId:'main'});
  assert.equal(health.checks.runtime.status,'responsive');assert.ok(health.context.runtimeId);assert.ok(health.context.pageEpoch);assert.ok(health.context.windowInstanceId);assert.equal(health.diagnostics.installSuccesses,1);
  results.context=health.context;results.diagnostics=health.diagnostics;
  results.tests.push({id:'UP-T009',name:'native cold runtime handshake confirms complete context',passed:true});
  picker=await inspect('webview_select_element',{action:'cancel',pickerId:picker.pickerId});
  const cleanupDeadline=Date.now()+5000;
  while(!picker.resultComplete&&Date.now()<cleanupDeadline)picker=await inspect('webview_select_element',{action:'get',pickerId:picker.pickerId,waitMs:1000});
  assert.equal(picker.status,'cancelled');assert.equal(picker.cleanup.status,'confirmed');
  results.tests.push({id:'UP-PK029',name:'native explicit cancel confirms guard cleanup',passed:true});
  if(process.env.CONNECTOR_NATIVE_PICKER_CYCLES==='100') {
    const script=()=>value({type:'execute_js',window_id:'main',script:"({owned:document.querySelectorAll('[data-connector-picker-ui]').length,guard:window.__CONNECTOR_INPUT_GUARD__.state()})"});
    const baseline=await script();assert.equal(baseline.owned,0);assert.equal(baseline.guard.active,false);
    for(let cycle=0;cycle<100;cycle++) {
      results.cycle=cycle;
      let current=await inspect('webview_select_element',{action:'start',requestKey:randomUUID(),captureScreenshot:false});
      const until=Date.now()+5000;while(['created','installing'].includes(current.status)&&Date.now()<until)current=await inspect('webview_select_element',{action:'get',pickerId:current.pickerId,waitMs:1000});
      assert.equal(current.status,'awaiting_selection');assert.equal(current.context.runtimeId,health.context.runtimeId);
      current=await inspect('webview_select_element',{action:'cancel',pickerId:current.pickerId});
      const cleanupUntil=Date.now()+5000;while(!current.resultComplete&&Date.now()<cleanupUntil)current=await inspect('webview_select_element',{action:'get',pickerId:current.pickerId,waitMs:1000});
      assert.equal(current.status,'cancelled');
      if(current.cleanup.status!=='confirmed')results.cleanupFailure={cycle,picker:current,health:await inspect('runtime_health',{windowId:'main'})};
      assert.equal(current.cleanup.status,'confirmed');assert.equal(current.cleanup.leaseReleased,true);
      let bridge=await value({type:'bridge_status'});const idleUntil=Date.now()+1000;while(bridge.pending!==0&&Date.now()<idleUntil){await delay(10);bridge=await value({type:'bridge_status'});}assert.equal(bridge.pending,0);
      assert.deepEqual(await script(),baseline);
    }
    const end=await inspect('runtime_health',{windowId:'main'});assert.equal(end.diagnostics.installSuccesses,1);assert.equal(end.diagnostics.activeObservers,0);
    results.tests.push({id:'UP-PK029',name:'100 native WebView start/cancel cycles release DOM, guard, leases and bridge waiters',passed:true,cycles:100});
    let expired=await inspect('webview_select_element',{action:'start',requestKey:randomUUID(),captureScreenshot:false,timeoutMs:5000});
    const expiryUntil=Date.now()+8000;
    while(!expired.resultComplete&&Date.now()<expiryUntil)expired=await inspect('webview_select_element',{action:'get',pickerId:expired.pickerId,waitMs:1000});
    assert.equal(expired.status,'expired');assert.equal(expired.resultComplete,true);
    assert.equal(expired.cleanup.status,'confirmed');assert.equal(expired.cleanup.leaseReleased,true);
    assert.deepEqual(await script(),baseline);
    results.tests.push({id:'UP-PK029',name:'Native deadline expiry restores the same DOM and guard baseline without client cancel',passed:true,timeoutMs:5000});
  }
  if(process.env.CONNECTOR_NATIVE_BENCHMARK==='1') {
    const {runTransportBenchmark}=await import('./transport-benchmark.mjs');
    const rpc={inspect,js:script=>value({type:'execute_js',window_id:'main',script}),workflow:(operation,args)=>value({type:'workflow',operation:'workflow_'+operation,args:{...args,authToken:token}})};
    const store=()=>JSON.parse(readFileSync(resolve(output,'independent-native-store.json'),'utf8'));
    const benchmark=await runTransportBenchmark({rpc,root,output,store});
    results.benchmark=benchmark.groups;results.cold=benchmark.cold;
    results.tests.push({id:'UP-T105',name:'native cold service and matched warm transport measurements',passed:true});
  }
  results.passed=true;
}catch(error){results.error=String(error.stack||error);process.exitCode=1;console.error(results.error);}
finally{
  socket?.close();for(const waiter of pending.values()){clearTimeout(waiter.timer);waiter.reject(Error('fixture closed'));}pending.clear();
  if(child){child.kill('SIGTERM');await Promise.race([new Promise(resolve=>child.once('exit',resolve)),delay(3000)]);if(child.exitCode===null&&child.signalCode===null)child.kill('SIGKILL');}
  const destination=resolve(output,'runtime-native-results.json');writeFileSync(destination,JSON.stringify(results,null,2),{mode:0o600});console.log(`Evidence: ${destination}`);
}
