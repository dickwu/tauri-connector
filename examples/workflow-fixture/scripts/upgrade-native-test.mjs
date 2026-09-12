// True native WebView + OS input. Never substitutes DOM dispatchEvent or mock images.
import assert from 'node:assert/strict';
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {randomBytes,randomUUID} from 'node:crypto';
import {mkdtempSync,mkdirSync,readFileSync,writeFileSync,openSync,closeSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';
import net from 'node:net';
const exec=promisify(execFile);
const root=fileURLToPath(new URL('../../../',import.meta.url));
const output=process.env.CONNECTOR_FIXTURE_OUTPUT||mkdtempSync(resolve(tmpdir(),'connector-upgrade-native-'));
mkdirSync(output,{recursive:true,mode:0o700});
const token=randomBytes(32).toString('hex');
const fixtureId=`dev.connector.workflow-fixture.${randomUUID()}`;
const storePath=resolve(output,'independent-native-store.json');
const binary=process.env.CONNECTOR_FIXTURE_BINARY||resolve(root,`target/debug/connector-workflow-fixture${process.platform==='win32'?'.exe':''}`);
const inputBinary=resolve(output,'native-input');
const tests=[];const sockets=[];let child;
const results={platform:process.platform,layer:'native_webview',inputPreparation:'Explicit fixture activation by native PID before OS input',input:process.platform==='darwin'?'Quartz':process.platform==='win32'?'user32':'XTest',display:process.env.DISPLAY||'desktop',tests};
class Rpc {
  constructor(ws){this.ws=ws;this.pending=new Map();ws.addEventListener('message',e=>{const r=JSON.parse(String(e.data));const w=this.pending.get(r.id);if(w){this.pending.delete(r.id);clearTimeout(w.timer);w.resolve(r);}});}
  static async connect(){const ws=new WebSocket('ws://127.0.0.1:19555');await new Promise((ok,no)=>{ws.addEventListener('open',ok,{once:true});ws.addEventListener('error',no,{once:true});});sockets.push(ws);return new Rpc(ws);}
  request(command,timeoutMs=15000){return new Promise((resolve,reject)=>{const id=randomUUID();const timer=setTimeout(()=>{this.pending.delete(id);reject(new Error('native fixture RPC deadline'));},timeoutMs);this.pending.set(id,{resolve,reject,timer});this.ws.send(JSON.stringify({id,...command}));});}
  async value(command){const r=await this.request(command);if(r.error!==undefined)throw new Error(JSON.stringify(r.error));return r.result;}
  inspect(operation,args={}){return this.value({type:'inspection',operation,args:{...args,authToken:token}});}
  js(script){return this.value({type:'execute_js',window_id:'main',script});}
  pick(args){return this.inspect('webview_select_element',args);}
  workflow(operation,args){return this.value({type:'workflow',operation:'workflow_'+operation,args:{...args,authToken:token}});}
}
async function test(ids,name,body){const started=performance.now();try{const details=await body();tests.push({ids,name,passed:true,durationMs:performance.now()-started,...details});console.log('PASS '+name);}catch(e){tests.push({ids,name,passed:false,error:String(e)});throw e;}}
async function occupied(port){return new Promise(resolve=>{const s=net.createConnection({host:'127.0.0.1',port});s.once('connect',()=>{s.destroy();resolve(true);});s.once('error',()=>resolve(false));});}
async function input(action,x=0,y=0){
  if(process.platform==='darwin')await exec(inputBinary,[action,String(x),String(y)]);
  else if(process.platform==='win32')await exec('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File',resolve(root,'examples/workflow-fixture/scripts/native-input.ps1'),action,String(Math.round(x)),String(Math.round(y))]);
  else if(action==='escape')await exec('xdotool',['key','Escape']);
  else {await exec('xdotool',['mousemove','--sync',String(Math.round(x)),String(Math.round(y))]);await exec('xdotool',action==='double'?['click','--repeat','2','--delay','100','1']:['click','1']);}
}
async function settled(rpc,report){const until=Date.now()+12000;while(!report.resultComplete&&Date.now()<until){report=await rpc.pick({action:'get',pickerId:report.pickerId,waitMs:1000,includeImage:true});}return report;}
async function awaiting(rpc,report){const until=Date.now()+6000;while(['created','installing'].includes(report.status)&&Date.now()<until){report=await rpc.pick({action:'get',pickerId:report.pickerId,waitMs:1000});}assert.equal(report.status,'awaiting_selection',JSON.stringify(report));return report;}
function store(){return JSON.parse(readFileSync(storePath,'utf8'));}
async function point(rpc,selector){
  const p=await rpc.js(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,viewportHeight:innerHeight}})()`);
  const w=await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_window')");
  if(process.platform==='darwin') {
    await exec(inputBinary,['activate',String(child.pid)]);
    let bounds;
    for(let n=0;n<30;n++) {const {stdout}=await exec(inputBinary,['window',String(child.pid)]);bounds=JSON.parse(stdout);if(bounds.Frontmost&&await rpc.js('document.hasFocus()'))break;await delay(50);}
    assert.equal(bounds.Frontmost,true,'Fixture must be the frontmost native app before posting input');
    assert.equal(await rpc.js('document.hasFocus()'),true,'WebView must actually receive input');
    assert.ok(p.x>=0&&p.x<bounds.Width&&p.y>=0&&p.y<p.viewportHeight,'Refusing input outside fixture viewport');
    return {x:bounds.X+p.x,y:bounds.Y+bounds.Height-p.viewportHeight+p.y};
  }
  return {x:w.x+p.x*w.scale,y:w.y+p.y*w.scale};
}
try {
  for(const port of [19555,19556])assert.equal(await occupied(port),false,'refusing occupied fixture ports');
  if(process.platform==='darwin'){await exec('swiftc',[resolve(root,'examples/workflow-fixture/scripts/native-input.swift'),'-o',inputBinary]);await exec(inputBinary,['probe']);}
  else if(process.platform==='linux')await exec('xdotool',['version']);
  const log=openSync(resolve(output,'private-app.log'),'w',0o600);
  child=spawn(binary,[],{cwd:root,env:{...process.env,TAURI_CONNECTOR_WORKFLOW_TOKEN:token,CONNECTOR_FIXTURE_ID:fixtureId,CONNECTOR_FIXTURE_EVIDENCE:storePath,TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS:'fixture_create_task,fixture_slow_write,fixture_fail_after_write,fixture_binary_result,fixture_pending',TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS:'id,name'},stdio:['ignore',log,log]});closeSync(log);
  let rpc;for(let n=0;n<100;n++){try{rpc=await Rpc.connect();break;}catch{await delay(100);}}
  assert.ok(rpc,'fixture bridge unavailable');
  for(let n=0;n<60;n++){
    try {
      if(process.platform==='win32') {const health=await rpc.inspect('runtime_health',{depth:'bridge',timeoutMs:1000});if(health.checks.bridge.connected)break;}
      else if(await rpc.js("!!document.querySelector('#pick-save')&&!!window.__WORKFLOW_FIXTURE__"))break;
    }catch{}
    await delay(100);
  }
  if(process.platform==='win32') {
    const {verifyWindowsNative}=await import('./windows-native.mjs');
    await verifyWindowsNative({rpc,token,root,output,fixtureId,child,input,exec,store,test,results});
  } else {
  await test([], 'OS input driver reaches independent native fixture counter', async()=>{
    const p=await point(rpc,'#pick-save');results.inputPoint=p;const before=store().saveCalls;
    await input('click',p.x,p.y);for(let n=0;n<30&&store().saveCalls===before;n++)await delay(100);
    assert.equal(store().saveCalls,before+1,JSON.stringify({point:p,store:store()}));
    await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_reset')");
    return {nativeWriteDelta:1,reset:true};
  });
  await test(['UP-T052','UP-T060'], 'legacy WS cannot discard requested native source or required masks', async()=>{
    for(const args of [{source:'webview_native'}, {redaction:'required'}, {source:null}]) {
      const response=await rpc.request({type:'screenshot',...args});
      assert.ok(response.error?.includes('inspection'));
      assert.equal(response.result,undefined);
    }
    return {rejectedBeforeCapture:true};
  });
  await test(['UP-T036','UP-T102','UP-PK012'],'anonymous rich request fails before picker creation',async()=>{const r=await rpc.request({type:'inspection',operation:'webview_select_element',args:{action:'start'}});assert.ok(r.error);assert.ok(JSON.stringify(r).includes('unauthorized'));});
  await test(['UP-T033'],'native identity and layered health expose cold state',async()=>{const r=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:2000});results.health=r;const identity=await rpc.inspect('app_identity');assert.ok(identity.appInstanceId);assert.equal(identity.appId,fixtureId);assert.equal(r.checks.transport.status,'responsive');assert.equal(r.checks.runtime.status,'unavailable');assert.equal(r.conclusion,'runtime_missing');assert.equal(r.diagnostics.installAttempts,0);return{appInstanceId:identity.appInstanceId};});
  await test(['UP-PK001','UP-PK004','UP-PK007','UP-PK014','UP-PK026','UP-PK028','UP-PK029','UP-T067','UP-T068','UP-T074'],'native save selection deduplicates, captures and never invokes save',async()=>{
    const p=await point(rpc,'#pick-save');const before=store();const key=randomUUID();
    const second=await Rpc.connect();const [a,b]=await Promise.all([rpc.pick({action:'start',requestKey:key,screenshotSource:'webview_native'}),second.pick({action:'start',requestKey:key,screenshotSource:'webview_native'})]);assert.equal(a.pickerId,b.pickerId);
    await awaiting(rpc,a);await input('click',p.x,p.y);const r=await settled(second,a);assert.equal(r.status,'selected',JSON.stringify(r));assert.equal(r.cleanup.status,'confirmed');assert.equal(r.selection.tag,'button');assert.equal(r.screenshot.status,'captured',JSON.stringify(r));assert.equal(r.screenshot.captureSource,'webview_native');assert.ok(r.screenshot.widthPx>0&&r.screenshot.heightPx>0);assert.equal(r.screenshot.redaction.status,'applied');
    assert.equal(store().saveCalls,before.saveCalls);assert.deepEqual(store().inputEffects,before.inputEffects);
    const again=await rpc.pick({action:'get',pickerId:r.pickerId});assert.deepEqual(again.selection,r.selection);assert.equal(again.screenshot.artifactId,r.screenshot.artifactId);
    assert.equal(await rpc.js("document.querySelectorAll('[data-connector-picker-ui]').length"),0);
    results.selection={...r,screenshot:{...r.screenshot,image:undefined}};return{nativeSaveDelta:0,screenshot:r.screenshot.captureSource,dimensions:[r.screenshot.widthPx,r.screenshot.heightPx]};
  });
  await test(['UP-PK001'],'omitted-action call waits for a real native user selection',async()=>{
    const p=await point(rpc,'#pick-save');const key=randomUUID();const before=store();
    const response=rpc.pick({requestKey:key,timeoutMs:5000,waitMs:2000,captureScreenshot:false});
    const peer=await Rpc.connect();const handle=await peer.pick({action:'start',requestKey:key,timeoutMs:5000,waitMs:0,captureScreenshot:false});await awaiting(peer,handle);
    await input('click',p.x,p.y);const selected=await response;assert.equal(selected.status,'selected',JSON.stringify(selected));assert.ok(selected.selection);
    const complete=await settled(peer,selected);assert.equal(complete.cleanup.status,'confirmed');assert.deepEqual(store(),before);
  });
  await test(['UP-PK027','UP-T074'],'explicit screenshot backend failure keeps the native selection',async()=>{
    const p=await point(rpc,'#pick-save');const before=store();let selected=await rpc.pick({action:'start',requestKey:randomUUID(),screenshotSource:'dom_rendering'});await awaiting(rpc,selected);
    await input('click',p.x,p.y);selected=await settled(rpc,selected);assert.equal(selected.status,'selected');assert.ok(selected.selection);assert.equal(selected.screenshot.status,'failed');assert.deepEqual(store(),before);results.partialPickerId=selected.pickerId;
  });
  for(const [selector,label,action] of [['#pick-checkbox','checkbox','click'],['#pick-link','link','click'],['#pick-submit','form','double']])await test(['UP-PK014','UP-PK018'],`native ${label} gesture has no business default effect`,async()=>{
    const p=await point(rpc,selector),before=store();let r=await rpc.pick({action:'start',requestKey:randomUUID(),captureScreenshot:false});await awaiting(rpc,r);await input(action,p.x,p.y);r=await settled(rpc,r);assert.equal(r.status,'selected',JSON.stringify(r));assert.equal(r.screenshot.status,'not_requested');assert.deepEqual(store(),before);
  });
  await test(['UP-PK019','UP-PK020','UP-T069'],'native Escape cancels and releases guard',async()=>{
    await point(rpc,'#pick-save');let r=await rpc.pick({action:'start',requestKey:randomUUID(),captureScreenshot:false});await awaiting(rpc,r);await input('escape');r=await settled(rpc,r);assert.equal(r.status,'cancelled');assert.equal(r.cleanup.status,'confirmed');
  });
  await test(['UP-PK030','UP-PK031','UP-T076'],'reload invalidates original picker identity',async()=>{
    // Navigation is scheduled by the isolated fixture before acquiring the lease.
    await rpc.js('(()=>{setTimeout(()=>location.reload(),600);return true;})()');let r=await rpc.pick({action:'start',requestKey:randomUUID(),captureScreenshot:false});r=await settled(rpc,r);assert.equal(r.status,'target_changed',JSON.stringify(r));assert.equal(r.cleanup.status,'context_destroyed');
  });
  await delay(700);
  if(process.env.CONNECTOR_NATIVE_BENCHMARK==='1') {
    const {runCaptureBenchmark}=await import('./capture-benchmark.mjs');
    await test(['UP-T105'],'same GUI workflow load with capture off and on',async()=>{
      const benchmark=await runCaptureBenchmark({rpc,output,store});
      results.captureBenchmark=benchmark.groups;
      return {groups:benchmark.groups};
    });
  }
  await test([], 'legacy native data callbacks remain available without exposing protected capture',async()=>{
    await rpc.js("window.__TAURI_INTERNALS__.invoke('plugin:connector|push_logs',{payload:{entries:[{level:'info',message:'legacy-permission-fixture',timestamp:Date.now(),windowId:'main'}]}})");
    await rpc.js("window.__TAURI_INTERNALS__.invoke('plugin:connector|set_pointed_element',{payload:{element:{tag:'fixture-legacy-only'}}})");
    const pointed=await rpc.value({type:'get_pointed_element'});assert.ok(JSON.stringify(pointed).includes('fixture-legacy-only'));
  });
  await test(['UP-T081','UP-T084','UP-T088','UP-T091','UP-T095'],'native capture returns bounded invoke evidence without legacy leak',async()=>{
    const session=await rpc.inspect('ipc_capture',{action:'start',options:{resultPolicy:'preview',argumentPolicy:'preview',commands:['fixture_create_task']}});results.captureStart=session;
    const value=await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_create_task',{name:'capture-only-fixture'})");assert.ok(value.id);
    await delay(150);const report=await rpc.inspect('ipc_query',{captureSessionId:session.captureSessionId});assert.ok(JSON.stringify(report).includes('fixture_create_task'));assert.ok(!JSON.stringify(report).includes(token));
    await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:session.captureSessionId});return{eventCount:report.events?.length,coverage:report.coverage};
  });
  await test(['UP-T084','UP-T087','UP-T088','UP-T093'],'native capture preserves rejection, raw binary and pending business work',async()=>{
    const capture=await rpc.inspect('ipc_capture',{action:'start',options:{resultPolicy:'preview',argumentPolicy:'preview',commands:['fixture_fail_after_write','fixture_binary_result','fixture_pending']}});
    const before=store().saveCalls;
    const rejected=await rpc.js("(async()=>{try{await window.__TAURI_INTERNALS__.invoke('fixture_fail_after_write');return {rejected:false}}catch(error){return {rejected:true,reason:String(error)}}})()");
    assert.equal(rejected.rejected,true);assert.equal(rejected.reason,'fixture rejection password=must-not-appear');assert.equal(store().saveCalls,before+1);
    const binary=await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_binary_result').then(value=>({isBuffer:value instanceof ArrayBuffer,bytes:Array.from(new Uint8Array(value))}))");assert.equal(binary.isBuffer,true);assert.deepEqual(binary.bytes,[1,2,3,4]);
    await rpc.js("(()=>{window.__FIXTURE_PENDING_RESOLVED__=false;window.__TAURI_INTERNALS__.invoke('fixture_pending').then(()=>window.__FIXTURE_PENDING_RESOLVED__=true);return true;})()");
    await delay(150);const query=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});
    assert.ok(query.events.some(e=>e.command==='fixture_fail_after_write'&&e.phase==='failed'));assert.ok(query.events.some(e=>e.command==='fixture_binary_result'&&e.phase==='succeeded'));
    assert.ok(!JSON.stringify(query).includes('must-not-appear'));assert.ok(query.pending.some(e=>e.command==='fixture_pending'&&e.observationStatus==='pending'));
    const active=store().pendingInvocations;assert.ok(active>0);await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:capture.captureSessionId});
    const stopped=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});assert.ok(stopped.pending.some(e=>e.command==='fixture_pending'&&e.observationStatus==='observation_interrupted'));assert.equal(store().pendingInvocations,active);assert.equal(await rpc.js('window.__FIXTURE_PENDING_RESOLVED__'),false);
    return{nativeExpectedWriteDelta:1,binaryBytes:binary.bytes.length,pendingBusinessStillActive:active,redactedError:true};
  });
  if(process.platform==='win32') {
    await test([], 'Windows durable workflow remains fail closed',async()=>{
      const spec=JSON.parse(readFileSync(resolve(root,'examples/workflow/create-task.json'),'utf8'));spec.runKey=randomUUID();
      const before=store().saveCalls;const response=await rpc.request({type:'workflow',operation:'workflow_run',args:{spec,authToken:token}});
      assert.ok(JSON.stringify(response).includes('persistence_unavailable'));assert.equal(store().saveCalls,before);
    });
  } else {
    await test(['UP-T099'],'strict instance, capture, GUI workflow, query and native image compose',async()=>{
      const identity=await rpc.inspect('app_identity');assert.equal(identity.appId,fixtureId);
      const capture=await rpc.inspect('ipc_capture',{action:'start',options:{resultPolicy:'preview',argumentPolicy:'preview',commands:['fixture_create_task']}});
      const spec=JSON.parse(readFileSync(resolve(root,'examples/workflow/create-task.json'),'utf8'));spec.runKey=randomUUID();spec.inputs.taskName='upgrade-gui-'+randomUUID();
      const before=store().saveCalls;let run=await rpc.workflow('run',{spec,waitMs:10000});
      const until=Date.now()+30000;while(['queued','running','cancelling'].includes(run.status)&&Date.now()<until){run=await rpc.workflow('get',{runId:run.runId});await delay(30);}
      assert.equal(run.status,'completed',JSON.stringify(run));assert.equal(run.goalStatus,'passed');assert.equal(run.dispatchContext.runtimeId,capture.context.runtimeId);assert.equal(run.dispatchContext.windowInstanceId,capture.context.windowInstanceId);assert.equal(store().saveCalls,before+1);assert.equal(store().tasks.at(-1).name,spec.inputs.taskName);
      await delay(150);const events=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});
      const started=events.events.filter(event=>event.command==='fixture_create_task'&&event.phase==='started');const completed=events.events.filter(event=>event.command==='fixture_create_task'&&event.phase==='succeeded');
      assert.equal(started.length,1);assert.equal(completed.length,1);assert.equal(started[0].invocationId,completed[0].invocationId);
      const shot=await rpc.inspect('webview_screenshot',{source:'webview_native',includeImage:false});assert.equal(shot.captureSource,'webview_native');assert.equal(shot.redaction.status,'applied');
      await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:capture.captureSessionId});
      return{runId:run.runId,nativeSaveDelta:1,invocationId:started[0].invocationId,screenshotSource:shot.captureSource};
    });
  }
  await test(['UP-T101','UP-PK003','UP-PK004','UP-PK005','UP-PK012'],'four real entry points share lifecycle, authority and exit semantics',async()=>{
    const {verifyEntryPoints}=await import('./entry-points.mjs');
    return await verifyEntryPoints({rpc,token,root,output,selectedPickerId:results.selection?.pickerId,partialPickerId:results.partialPickerId});
  });
  if(process.env.CONNECTOR_NATIVE_BENCHMARK==='1') {
    const {runTransportBenchmark}=await import('./transport-benchmark.mjs');
    await test(['UP-T105'],'measured warm full-source vs short-packet transport',async()=>{
      const benchmark=await runTransportBenchmark({rpc,root,output,store});results.benchmark=benchmark.groups;return{samplesPerConfiguration:benchmark.samplesPerConfiguration};
    });
  }
  await test(['UP-T100'],'unknown result keeps original effect and permits captured diagnostics without replay',async()=>{
    const capture=await rpc.inspect('ipc_capture',{action:'start',options:{resultPolicy:'preview',commands:['fixture_create_task']}});
    const before=store().saveCalls;
    const original=await rpc.request({type:'execute_js',window_id:'main',script:"(async()=>{await window.__TAURI_INTERNALS__.invoke('fixture_create_task',{name:'upgrade-lost-result'});return new Promise(()=>{})})()"},45000);
    assert.equal(original.outcome.execution,'outcome_unknown',JSON.stringify(original));assert.equal(store().saveCalls,before+1);
    const first=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});assert.ok(first.events.some(e=>e.phase==='succeeded'&&e.command==='fixture_create_task'));
    const health=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:2000});assert.equal(health.executionReplayed,false);
    const second=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});assert.deepEqual(second.events,first.events);
    const blocked=await rpc.request({type:'execute_js',window_id:'main',script:'1+1'});assert.equal(blocked.outcome.error.code,'resource_busy');
    assert.equal(store().saveCalls,before+1);assert.equal(original.outcome.execution,'outcome_unknown');
    return{nativeSaveDelta:1,originalExecution:original.outcome.execution,quarantinePreserved:true,diagnosticEvents:first.events.length};
  });
  }
  results.passed=true;
} catch(error){results.passed=false;results.error=String(error.stack||error);console.error(results.error);process.exitCode=1;}
finally {
  for(const ws of sockets)ws.close();
  if(child){child.kill('SIGTERM');await Promise.race([new Promise(r=>child.once('exit',r)),delay(3000)]);if(child.exitCode===null&&child.signalCode===null)child.kill('SIGKILL');}
  writeFileSync(resolve(output,'upgrade-native-results.json'),JSON.stringify(results,null,2),{mode:0o600});console.log(`Evidence: ${resolve(output,'upgrade-native-results.json')}`);
}
