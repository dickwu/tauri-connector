// Capture enabled/disabled comparison through the same full workflow service.
// The GUI actions run in a native WebView; they are not OS-level input events.
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';

const SAMPLES=30;
const WARMUPS=1;
const MODES=['capture_off','capture_on'];
const LIVE_STATUSES=new Set(['queued','running','cancelling']);

function counters(value) {
  return {
    saveCalls:value.saveCalls,
    taskCount:value.tasks.length,
    pendingInvocations:value.pendingInvocations,
    delayMs:value.delayMs,
    inputEffects:structuredClone(value.inputEffects),
  };
}
function assertEmptyStore(value,phase) {
  assert.equal(value.saveCalls,0,`${phase}: native save counter must be zero`);
  assert.deepEqual(value.tasks,[],`${phase}: native task list must be empty`);
  assert.equal(value.pendingInvocations,0,`${phase}: pending fixture invocations must be zero`);
  assert.equal(value.delayMs,0,`${phase}: native delay must be zero`);
  assert.deepEqual(value.inputEffects,{},`${phase}: native input counters must be reset`);
}
function quantile(values,q) {
  const sorted=[...values].sort((a,b)=>a-b);
  assert.ok(sorted.length>0);
  if(q===.5) return (sorted[Math.floor((sorted.length-1)/2)]+sorted[Math.ceil((sorted.length-1)/2)])/2;
  return sorted[Math.min(sorted.length-1,Math.ceil(q*sorted.length)-1)];
}
async function pageState(rpc,taskName=null) {
  return rpc.js(`(()=>{
    const stats=window.__WORKFLOW_FIXTURE__;
    const capture=window.__CONNECTOR_CAPTURE__?.status();
    const taskName=${JSON.stringify(taskName)};
    return {
      pageEpoch:window.__CONNECTOR_BOOTSTRAP__?.pageEpoch,
      ready:!!stats&&!!document.querySelector('#fixture main'),
      opens:stats?.opens,inputEvents:stats?.inputEvents,saves:stats?.saves,
      errorCount:stats?.errors.length,submissionCount:stats?.submissions.length,
      taskCount:stats?.tasks.length,lastSubmission:stats?.submissions.at(-1),
      dialogs:document.querySelectorAll('[role="dialog"]').length,
      rows:[...document.querySelectorAll('[data-testid="task-row"]')].filter(row=>taskName!==null&&row.getAttribute('data-task-name')===taskName).map(row=>({id:row.getAttribute('data-task-id'),name:row.getAttribute('data-task-name')})),
      pickerActive:window.__CONNECTOR_INPUT_GUARD__?.active===true,
      legacyMonitoring:window.__CONNECTOR_IPC_MONITOR__===true,
      capture:capture?{applied:capture.applied,status:capture.status,sourceId:capture.sourceId,queuedEvents:capture.queuedEvents,queuedBytes:capture.queuedBytes,droppedEvents:capture.droppedEvents,activeSessions:capture.activeSessions}:null,
    };
  })()`);
}
function assertCaptureOff(page,phase) {
  assert.notEqual(page.capture?.applied,true,`${phase}: an unrelated capture is active`);
  assert.equal(page.legacyMonitoring,false,`${phase}: legacy monitoring would contaminate the comparison`);
  assert.equal(page.capture?.queuedEvents??0,0,`${phase}: capture queue is not empty`);
}
async function waitForFreshPage(rpc,oldEpoch) {
  const until=performance.now()+12000;
  while(performance.now()<until) {
    // Health is the protected read-only probe. Do not retry failed mutations.
    const health=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:1000});
    if(health.checks.bridge.connected&&health.checks.bridge.status==='responsive') {
      const page=await pageState(rpc);
      if(page.ready&&page.pageEpoch&&page.pageEpoch!==oldEpoch) return page;
    }
    await delay(30);
  }
  throw new Error('Capture benchmark fresh-document deadline exceeded');
}
async function resetFixture({rpc,store,identity,phase}) {
  const before=store();
  assert.equal(before.pendingInvocations,0,`${phase}: refusing to reset a pending business fixture`);
  const page=await pageState(rpc);
  assert.equal(page.ready,true,`${phase}: isolated fixture UI is missing`);
  assert.equal(page.pickerActive,false,`${phase}: refusing to reset an active picker`);
  assertCaptureOff(page,phase);
  const current=await rpc.inspect('app_identity');
  assert.equal(current.appId,identity.appId,`${phase}: fixture app changed`);
  assert.equal(current.appInstanceId,identity.appInstanceId,`${phase}: fixture process changed`);
  await rpc.js("window.__TAURI_INTERNALS__.invoke('fixture_reset')");
  assertEmptyStore(store(),phase);
  await rpc.js('(()=>{setTimeout(()=>location.reload(),50);return true;})()');
  const fresh=await waitForFreshPage(rpc,page.pageEpoch);
  assertCaptureOff(fresh,phase);
  assert.equal(fresh.opens,0,`${phase}: frontend opens must reset`);
  assert.equal(fresh.inputEvents,0,`${phase}: frontend input count must reset`);
  assert.equal(fresh.saves,0,`${phase}: frontend save count must reset`);
  assert.equal(fresh.errorCount,0,`${phase}: frontend errors must reset`);
  assert.equal(fresh.taskCount,0,`${phase}: frontend task list must reset`);
  assert.equal(fresh.dialogs,0,`${phase}: no dialog may remain open`);
  assertEmptyStore(store(),phase);
  return {phase,before:counters(before),after:counters(store()),oldPageEpoch:page.pageEpoch,pageEpoch:fresh.pageEpoch};
}
async function readCaptureSample(rpc,captureSessionId,cursor,expectedCalls) {
  const until=performance.now()+3000;
  const observations=[];
  let query,status;
  do {
    status=await rpc.inspect('ipc_capture',{action:'status',captureSessionId});
    assert.equal(status.desired,true,'capture unexpectedly stopped during a measured configuration');
    assert.equal(status.applied,true,'capture hook must remain acknowledged');
    query=await rpc.inspect('ipc_query',{
      captureSessionId,...(cursor===null?{}:{cursor}),limit:500,maxBytes:65536,
    });
    assert.equal(query.status,'ok','capture pagination cannot hide a gap');
    assert.equal(query.truncated,false,'capture sample evidence must fit its declared page budget');
    assert.equal(query.droppedEvents,0,'capture dropped evidence during benchmark');
    assert.ok(Number.isInteger(status.hookStatus?.queuedEvents),'page queue count is unavailable');
    assert.ok(Number.isInteger(status.hookStatus?.queuedBytes),'page queue byte count is unavailable');
    assert.ok(Number.isInteger(status.hookStatus?.droppedEvents),'page drop count is unavailable');
    assert.equal(status.hookStatus.droppedEvents,0,'page capture dropped evidence during benchmark');
    assert.ok(Array.isArray(query.pending),'capture pending coverage is unavailable');
    const pending=query.pending.filter(event=>event.command==='fixture_create_task');
    observations.push({
      queuedEvents:status.hookStatus?.queuedEvents??null,
      queuedBytes:status.hookStatus?.queuedBytes??null,
      droppedEvents:query.droppedEvents,
      pageDroppedEvents:status.hookStatus?.droppedEvents??null,
      pendingCount:pending.length,
    });
    const events=query.events.filter(event=>event.command==='fixture_create_task');
    const started=events.filter(event=>event.phase==='started');
    const completed=events.filter(event=>event.phase==='succeeded');
    assert.equal(events.some(event=>event.phase==='failed'),false,'captured GUI save failed');
    assert.ok(started.length<=expectedCalls&&completed.length<=expectedCalls,'duplicate business invoke evidence');
    if(started.length===expectedCalls&&completed.length===expectedCalls&&pending.length===0) {
      const startIds=started.map(event=>event.invocationId).sort();
      assert.equal(new Set(startIds).size,expectedCalls,'invocation IDs must be unique');
      assert.deepEqual(startIds,completed.map(event=>event.invocationId).sort());
      assert.equal(typeof query.nextCursor,'string','capture cursor must stay opaque');
      return {
        cursor:query.nextCursor,started:started.length,succeeded:completed.length,
        failed:0,pendingCount:0,droppedEvents:query.droppedEvents,
        sourceSequences:events.map(event=>event.sourceSequence),
        invocationIds:startIds,coverage:query.coverage,observations,
      };
    }
    await delay(20);
  } while(performance.now()<until);
  throw new Error(`Capture evidence did not settle: ${JSON.stringify({query,status,observations})}`);
}

export async function runCaptureBenchmark({rpc,output,store}) {
  assert.equal(typeof rpc?.workflow,'function');
  assert.equal(typeof rpc?.inspect,'function');
  assert.equal(typeof rpc?.js,'function');
  assert.equal(typeof store,'function');
  assert.equal(typeof output,'string');
  const evidencePath=resolve(output,'capture-benchmark.json');
  const template=JSON.parse(readFileSync(new URL('../../workflow/create-task.json',import.meta.url),'utf8'));
  const workloadId=randomUUID();
  const result={
    schemaVersion:1,platform:process.platform,layer:'native_webview_workflow_gui_dispatch',
    input:'Existing workflow click/fill/click/query GUI dispatch inside the native WebView; not OS input',
    status:'running',passed:false,samplesPerConfiguration:SAMPLES,warmupsPerConfiguration:WARMUPS,
    capturePolicy:{resultPolicy:'metadata',argumentPolicy:'metadata',commands:['fixture_create_task'],followPages:false},
    measuredDimensions:['fullWorkflowElapsedMs','validatedSampleElapsedMs','frontendInputEvents','frontendOpens','frontendSaves','nativeSaveDelta','nativeTaskDelta','nativePendingInvocations','captureQueue','captureDrops','capturePending'],
    configurationOrder:MODES,rows:[],warmups:[],groups:[],resets:[],cleanup:{performed:false},
    limitations:[
      'Measures full workflow service, local transport, journal, semantic checks and GUI postconditions. It is not an isolated per-invoke CPU or allocation benchmark.',
      'Both modes use the same plugin build; capture-off still includes its installed but dormant interception code.',
      'One window and metadata capture policy only; result preview and multi-window overhead are not measured.',
      'Modes run in two blocks because completed capture sessions retain evidence and cannot be resumed. Reset/reload and matching warmups equalize DOM/backend sizes but cannot eliminate temporal, thermal or journal-history bias.',
      'Queue and pending metrics are sampled before/after workflows and during evidence drainage, outside the timed workflow. They do not establish continuous queue high-water marks.',
      'Thirty samples per configuration do not establish production tail latency. Median and nearest-rank p95 retain all successful samples; no outlier removal is performed.',
      'Rust counters come from the independently published fixture state file. pendingInvocations counts the dedicated never-completing fixture command, not arbitrary Rust tasks.',
      'This harness does not bypass the Windows durable-workflow private-storage boundary; run only where the normal workflow service is supported.',
    ],
  };
  let captureSessionId=null,identity=null,unsafeToReset=false,ownershipVerified=false,primaryError=null;
  const save=()=>writeFileSync(evidencePath,JSON.stringify(result,null,2),{mode:0o600});
  async function stopOwnedCapture() {
    if(captureSessionId===null)return;
    const id=captureSessionId;
    const stopped=await rpc.inspect('ipc_capture',{action:'stop',captureSessionId:id});
    assert.equal(stopped.desired,false,'owned capture must stop before baseline reset');
    assert.equal(stopped.applied,false,'owned capture remains applied after stop');
    assert.equal(stopped.hookCleanupAcknowledged,true,'capture stop drainage was not confirmed');
    captureSessionId=null;
    assertCaptureOff(await pageState(rpc),'after owned capture stop');
  }
  async function runOne(mode,phase,index,cursor) {
    const name=`capture-benchmark-${workloadId}-${phase}-${String(index).padStart(3,'0')}-中文`;
    const spec=structuredClone(template);spec.runKey=`capture-benchmark-${randomUUID()}`;spec.inputs.taskName=name;
    const before=store(),beforePage=await pageState(rpc);
    assert.ok(beforePage.capture,'native capture controller is unavailable');
    assert.equal(before.pendingInvocations,0,'a background pending fixture would invalidate the comparison');
    assert.equal(before.delayMs,0,'both modes require the same zero backend delay');
    assert.equal(beforePage.dialogs,0,'each workflow must open its own dialog');
    if(mode==='capture_off')assertCaptureOff(beforePage,'before capture-off sample');
    else assert.equal(beforePage.capture?.applied,true,'capture-on sample requires an installed hook');
    const row={mode,phase,index,taskName:name,runKey:spec.runKey,beforeNative:counters(before),beforeCapture:beforePage.capture,status:'running'};
    (phase==='warmup'?result.warmups:result.rows).push(row);
    unsafeToReset=true;
    const start=performance.now();
    let run=await rpc.workflow('run',{spec,waitMs:10000});
    const until=start+spec.deadlineMs+5000;
    while(LIVE_STATUSES.has(run.status)&&performance.now()<until) {
      await delay(20);run=await rpc.workflow('get',{runId:run.runId});
    }
    row.fullWorkflowElapsedMs=performance.now()-start;
    row.runId=run.runId;row.workflowStatus=run.status;row.goalStatus=run.goalStatus;
    row.originalTestVerdict=run.originalTestVerdict;row.context=run.dispatchContext;
    for(const key of ['appInstanceId','windowInstanceId','runtimeId','pageEpoch','runtimeVersion','semanticVersion','bundleHash']) assert.equal(typeof row.context?.[key],'string',`workflow context ${key} is unavailable`);
    assert.equal(run.status,'completed',JSON.stringify(run));
    unsafeToReset=false;
    assert.equal(run.goalStatus,'passed',JSON.stringify(run));
    assert.equal(run.recoveryOccurred,false,'benchmark must not compare recovered runs');
    assert.equal(run.summary.completedSteps,spec.steps.length);
    assert.equal(run.summary.remainingSteps,0);
    assert.deepEqual(run.steps.map(step=>step.id),spec.steps.map(step=>step.id));
    for(const step of run.steps) {
      assert.equal(step.status,'succeeded',JSON.stringify(step));
      assert.equal(step.outcome.execution,'completed',JSON.stringify(step));
      assert.equal(step.outcome.verification,'passed',JSON.stringify(step));
    }
    const after=store(),afterPage=await pageState(rpc,name);
    row.afterNative=counters(after);
    row.nativeSaveDelta=after.saveCalls-before.saveCalls;row.nativeTaskDelta=after.tasks.length-before.tasks.length;
    row.frontendInputEvents=afterPage.inputEvents-beforePage.inputEvents;
    row.frontendOpens=afterPage.opens-beforePage.opens;row.frontendSaves=afterPage.saves-beforePage.saves;
    row.nativePendingInvocations=after.pendingInvocations;row.nativeInputEffectsUnchanged=JSON.stringify(after.inputEffects)===JSON.stringify(before.inputEffects);
    assert.equal(row.nativeSaveDelta,1,'one GUI save must produce exactly one independently observed Rust write');
    assert.equal(row.nativeTaskDelta,1);assert.equal(after.tasks.at(-1).name,name);
    assert.equal(row.frontendInputEvents,1);assert.equal(row.frontendOpens,1);assert.equal(row.frontendSaves,1);
    assert.equal(afterPage.errorCount,beforePage.errorCount);assert.equal(afterPage.submissionCount,beforePage.submissionCount+1);
    assert.equal(afterPage.lastSubmission,name);assert.equal(afterPage.taskCount,beforePage.taskCount+1);assert.equal(afterPage.dialogs,0);
    assert.equal(afterPage.rows.length,1,'saved task must have one visible fixture entity');
    assert.equal(afterPage.rows[0].id,after.tasks.at(-1).id,'GUI result must match the independently recorded Rust task ID');
    assert.equal(row.nativePendingInvocations,0);assert.deepEqual(after.inputEffects,before.inputEffects);
    row.validatedSampleElapsedMs=performance.now()-start;
    if(mode==='capture_on')row.capture=await readCaptureSample(rpc,captureSessionId,cursor,1);
    else {
      assertCaptureOff(afterPage,'after capture-off sample');
      row.capture={status:'disabled',queuedEvents:afterPage.capture?.queuedEvents??0,queuedBytes:afterPage.capture?.queuedBytes??0,droppedEvents:afterPage.capture?.droppedEvents??0,pendingCount:null,pendingStatus:'not_observed_capture_disabled'};
    }
    row.status='passed';
    return row.capture.cursor??cursor;
  }
  try {
    assert.notEqual(process.platform,'win32','Windows GUI workflows must retain their fail-closed private-storage boundary');
    identity=await rpc.inspect('app_identity');
    assert.ok(identity.appId?.startsWith('dev.connector.workflow-fixture.'),'refusing reset outside the isolated workflow fixture');
    assert.ok(identity.appInstanceId);result.identity={appId:identity.appId,appInstanceId:identity.appInstanceId};
    const initial=store();result.initialNative=counters(initial);
    assert.equal(initial.pendingInvocations,0,'Run capture benchmark before any fixture_pending test; reset does not cancel business invocations');
    const initialPage=await pageState(rpc);assertCaptureOff(initialPage,'benchmark entry');assert.equal(initialPage.pickerActive,false);
    ownershipVerified=true;
    for(const mode of MODES) {
      result.resets.push(await resetFixture({rpc,store,identity,phase:`before_${mode}`}));
      let cursor=null;
      if(mode==='capture_on') {
        const capture=await rpc.inspect('ipc_capture',{action:'start',windowId:'main',options:result.capturePolicy});
        assert.equal(capture.desired,true);assert.equal(capture.applied,true);captureSessionId=capture.captureSessionId;
        result.captureStart={captureSessionId,context:capture.context,remainingMs:capture.remainingMs,coverage:capture.coverage};
      }
      for(let index=0;index<WARMUPS;index++)cursor=await runOne(mode,'warmup',index,cursor);
      for(let index=0;index<SAMPLES;index++)cursor=await runOne(mode,'sample',index,cursor);
      const rows=result.rows.filter(row=>row.mode===mode);
      assert.equal(rows.length,SAMPLES);assert.ok(rows.every(row=>row.status==='passed'));
      assert.equal(store().saveCalls,SAMPLES+WARMUPS,'native counter must include every measured sample and its warmup');
      const group={
        mode,samples:rows.length,warmups:WARMUPS,workflowSteps:template.steps.length,
        medianMs:quantile(rows.map(row=>row.fullWorkflowElapsedMs),.5),p95Ms:quantile(rows.map(row=>row.fullWorkflowElapsedMs),.95),
        medianValidatedMs:quantile(rows.map(row=>row.validatedSampleElapsedMs),.5),p95ValidatedMs:quantile(rows.map(row=>row.validatedSampleElapsedMs),.95),
        nativeSaveDelta:rows.reduce((sum,row)=>sum+row.nativeSaveDelta,0),nativeTaskDelta:rows.reduce((sum,row)=>sum+row.nativeTaskDelta,0),
        frontendInputEvents:rows.reduce((sum,row)=>sum+row.frontendInputEvents,0),frontendOpens:rows.reduce((sum,row)=>sum+row.frontendOpens,0),frontendSaves:rows.reduce((sum,row)=>sum+row.frontendSaves,0),
        nativeSaveCallsIncludingWarmup:store().saveCalls,nativePendingInvocations:store().pendingInvocations,
        capturePending:mode==='capture_on'?{status:'observed',final:rows.at(-1).capture.pendingCount}:{status:'not_enabled',final:null},
        maxObservedQueuedEvents:mode==='capture_on'?Math.max(...rows.flatMap(row=>row.capture.observations.map(o=>o.queuedEvents??0))):Math.max(...rows.map(row=>row.capture.queuedEvents)),
        maxObservedQueuedBytes:mode==='capture_on'?Math.max(...rows.flatMap(row=>row.capture.observations.map(o=>o.queuedBytes??0))):Math.max(...rows.map(row=>row.capture.queuedBytes)),
        droppedEvents:mode==='capture_on'?rows.at(-1).capture.droppedEvents:Math.max(...rows.map(row=>row.capture.droppedEvents)),
        context:rows[0].context,
      };
      result.groups.push(group);await stopOwnedCapture();save();
    }
    for(const group of result.groups) {
      assert.equal(group.nativeSaveDelta,SAMPLES);assert.equal(group.frontendInputEvents,SAMPLES);assert.equal(group.droppedEvents,0);
      assert.equal(group.context.appInstanceId,result.groups[0].context.appInstanceId);
      assert.equal(group.context.windowInstanceId,result.groups[0].context.windowInstanceId);
      assert.equal(group.context.runtimeVersion,result.groups[0].context.runtimeVersion);
      assert.equal(group.context.semanticVersion,result.groups[0].context.semanticVersion);
      assert.equal(group.context.bundleHash,result.groups[0].context.bundleHash);
    }
    const [off,on]=result.groups;
    result.comparison={medianDeltaMs:on.medianMs-off.medianMs,p95DeltaMs:on.p95Ms-off.p95Ms,medianRatio:on.medianMs/off.medianMs,p95Ratio:on.p95Ms/off.p95Ms};
    result.status='completed';result.passed=true;
  } catch(error) {
    result.status='failed';result.passed=false;result.error={message:String(error),stack:error.stack};
    primaryError=error;
    for(const row of [...result.warmups,...result.rows])if(row.status==='running'){row.status='failed';row.error=String(error);}
  } finally {
    try {
      await stopOwnedCapture();
      if(ownershipVerified&&!unsafeToReset) {
        result.cleanup=await resetFixture({rpc,store,identity,phase:'after_benchmark'});result.cleanup.performed=true;
      } else result.cleanup={performed:false,reason:unsafeToReset?'workflow_may_be_in_flight_or_unknown':'safe_fixture_ownership_not_established'};
    } catch(error) {
      result.cleanup={performed:false,error:String(error)};result.passed=false;result.status='failed';
      primaryError??=error;
    } finally {save();}
  }
  if(primaryError)throw primaryError;
  return result;
}
