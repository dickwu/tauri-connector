// Windows inspection stays usable even though legacy mutating automation and
// the durable workflow journal fail closed. No execute_js bypass is introduced.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
export async function verifyWindowsNative({rpc,token,root,output,fixtureId,child,input,exec,store,test,results}) {
  const geometry=async()=>JSON.parse((await exec('powershell',['-NoProfile','-ExecutionPolicy','Bypass','-File',resolve(root,'examples/workflow-fixture/scripts/native-input.ps1'),'window',String(child.pid),'0'])).stdout);
  const wait=async(report,ready=false)=>{const until=Date.now()+12000;while((ready?['created','installing'].includes(report.status):!report.resultComplete)&&Date.now()<until){report=await rpc.pick({action:'get',pickerId:report.pickerId,waitMs:1000,includeImage:true});}return report;};
  await test([], 'Windows strict identity and original workflow storage boundary',async()=>{
    const identity=await rpc.inspect('app_identity');assert.equal(identity.appId,fixtureId);
    const spec=JSON.parse(readFileSync(resolve(root,'examples/workflow/create-task.json'),'utf8'));spec.runKey=randomUUID();
    const before=store().saveCalls;const reply=await rpc.request({type:'workflow',operation:'workflow_run',args:{spec,authToken:token}});
    assert.ok(JSON.stringify(reply).includes('persistence_unavailable'));assert.equal(store().saveCalls,before);
    results.identity={appId:identity.appId,appInstanceId:identity.appInstanceId};
  });
  // Fixture CSS puts the first Save button at x30/y66. The tested point lies
  // within it across fixture fonts; selection metadata independently checks it.
  const clickSave=async()=>{const g=await geometry();assert.ok(g.width>100&&g.height>120);await input('click',g.x+50*g.scale,g.y+85*g.scale);};
  await test([], 'Windows native input sanity and immutable-invoke observation',async()=>{
    const capture=await rpc.inspect('ipc_capture',{action:'start',options:{resultPolicy:'preview',argumentPolicy:'preview',commands:['fixture_create_task']}});
    const before=store().saveCalls;await clickSave();for(let n=0;n<30&&store().saveCalls===before;n++)await delay(100);assert.equal(store().saveCalls,before+1);
    await delay(150);const query=await rpc.inspect('ipc_query',{captureSessionId:capture.captureSessionId});
    const starts=query.events.filter(e=>e.phase==='started'),ends=query.events.filter(e=>e.phase==='succeeded');
    assert.equal(starts.length,1);assert.equal(ends.length,1);assert.equal(starts[0].invocationId,ends[0].invocationId);
    await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:capture.captureSessionId});
    return{nativeSaveDelta:1};
  });
  await test(['UP-PK001','UP-PK014','UP-PK026'], 'Windows picker owns native input and captures selected WebView content',async()=>{
    const before=store();const key=randomUUID();let report=await rpc.pick({requestKey:key,waitMs:0,screenshotSource:'webview_native'});
    report=await wait(report,true);assert.equal(report.status,'awaiting_selection');await clickSave();report=await wait(report);
    assert.equal(report.status,'selected',JSON.stringify(report));assert.equal(report.cleanup.status,'confirmed');assert.equal(report.selection.name,'Save');
    assert.ok(report.selection.locatorCandidates.some(c=>c.target?.value==='pick-save'||c.target?.value==='#pick-save'));
    assert.equal(report.screenshot.status,'captured');assert.equal(report.screenshot.captureSource,'webview_native');assert.ok(report.screenshot.widthPx>0&&report.screenshot.heightPx>0);assert.deepEqual(store(),before);
    results.selection={...report,screenshot:{...report.screenshot,image:undefined}};
  });
  await test(['UP-T069','UP-PK019'], 'Windows native Escape cancels without workflow storage',async()=>{
    await geometry();let report=await rpc.pick({action:'start',requestKey:randomUUID(),captureScreenshot:false});report=await wait(report,true);assert.equal(report.status,'awaiting_selection');
    await input('escape');report=await wait(report);assert.equal(report.status,'cancelled');assert.equal(report.cleanup.status,'confirmed');
  });
  await test(['UP-T101','UP-PK004'], 'Windows four-entry state remains shared under memory-only inspection',async()=>{
    const {verifyEntryPoints}=await import('./entry-points.mjs');return verifyEntryPoints({rpc,token,root,output,selectedPickerId:results.selection?.pickerId});
  });
  results.windowsBoundary='Durable workflow rejected with no dispatch; protected in-memory inspection/native input/IPC tested independently.';
}
