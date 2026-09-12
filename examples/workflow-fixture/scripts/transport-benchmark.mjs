// Measures the transport change with identical current runtime/semantic checks.
// This is deliberately not presented as an old-binary-vs-new-binary speed claim.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {createHash,randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';

export async function runTransportBenchmark({rpc,root,output,store}) {
  const source=readFileSync(resolve(root,'plugin/src/workflow/page.js'),'utf8');
  const rows=[];
  const samples=30;
  const baseline=store().saveCalls;
  const setupInput=()=>rpc.js("(()=>{document.querySelector('#upgrade-fixture').insertAdjacentHTML('beforeend','<label for=transport-bench>Transport benchmark</label><input id=transport-bench>');window.__BENCH_INPUT_EVENTS__=0;document.querySelector('#transport-bench').addEventListener('input',()=>window.__BENCH_INPUT_EVENTS__++);return true;})()");
  const cold=[];
  for(const steps of [2,10,20]) {
    const oldEpoch=(await rpc.inspect('runtime_health',{timeoutMs:2000})).context.pageEpoch;
    await rpc.js('(()=>{setTimeout(()=>location.reload(),50);return true;})()');
    let before;
    for(let attempt=0;attempt<100;attempt++) {
      before=await rpc.inspect('runtime_health',{timeoutMs:1000});
      if(before.checks.bridge.connected && before.conclusion==='runtime_missing')break;
      await delay(30);
    }
    assert.equal(before.conclusion,'runtime_missing','cold sample must start on a fresh document without runtime');
    await setupInput();
    const target={by:'css',value:'#transport-bench'};const plan=[];
    for(let step=0;step<steps;step+=2){const value=`cold-${steps}-${step}-中文`;plan.push({id:'fill'+step,op:'fill',target,value,expect:{kind:'valueEquals',target,expected:value}},{id:'read'+step,op:'query',target,query:{kind:'value'},expect:{kind:'result',stepId:'read'+step,pointer:'/value',operator:'eq',expected:value}});}
    const spec={schemaVersion:1,runKey:randomUUID(),windowId:'main',mode:'strict',schedule:'sequential',deadlineMs:30000,steps:plan};
    const started=performance.now();let run=await rpc.workflow('run',{spec,waitMs:10000});
    const until=Date.now()+30000;while(['queued','running'].includes(run.status)&&Date.now()<until){run=await rpc.workflow('get',{runId:run.runId});await delay(10);}
    assert.equal(run.status,'completed',JSON.stringify(run));
    const inputEvents=await rpc.js('window.__BENCH_INPUT_EVENTS__');assert.equal(inputEvents,steps/2);
    const elapsedMs=performance.now()-started;
    const after=await rpc.inspect('runtime_health',{timeoutMs:2000});
    assert.notEqual(after.context.pageEpoch,oldEpoch);assert.equal(after.diagnostics.installSuccesses-before.diagnostics.installSuccesses,1);assert.equal(store().saveCalls,baseline);
    cold.push({steps,samples:1,layer:'full_workflow_service',elapsedMs,inputEvents,installations:1,bundleBytes:after.diagnostics.bundleBytesSent-before.diagnostics.bundleBytesSent,commandBytes:after.diagnostics.commandBytesSent-before.diagnostics.commandBytesSent,nativeSaveDelta:0});
  }
  const health=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:2000});
  assert.equal(health.checks.runtime.status,'responsive');
  const context=health.context;
  assert.ok(context.runtimeId);
  const target={by:'css',value:'#transport-bench'};
  const call=async(mode,args,counters)=>{
    const packet={runtimeProtocolVersion:1,module:'workflow',requestId:randomUUID(),expectedContext:context,remainingMs:5000,args:{...args,context}};
    const short=`window.__CONNECTOR_INSPECTION_RUNTIME__.dispatch(${JSON.stringify(packet)})`;
    // Evaluating the identical cached factory has no second dispatch. Both
    // modes call the same runtime.dispatch and all of its identity checks.
    const script='/*__CONNECTOR_RUNTIME_COMMAND__*/'+(mode==='full_source'?`(\n${source}\n,${short})`:short);
    counters.calls++;counters.scriptBytes+=Buffer.byteLength(script);
    const result=await rpc.js(script);assert.equal(result.ok,true,JSON.stringify(result));return result;
  };
  for(const steps of [2,10,20]) {
    for(let sample=0;sample<samples;sample++) {
      // Alternate order to reduce bias from time/temperature/background load.
      for(const mode of sample%2?['short_packet','full_source']:['full_source','short_packet']) {
        const counts={calls:0,scriptBytes:0};
        const before=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:2000});
        const beforeInputEvents=await rpc.js('window.__BENCH_INPUT_EVENTS__');
        const observationId=randomUUID();const start=performance.now();
        await call(mode,{cmd:'prepare',observationId,timeoutMs:5000},counts);
        for(let step=0;step<steps;step+=2){
          const value=`sample-${sample}-pair-${step}-${mode==='full_source'?'A':'B'}-中文`;
          await call(mode,{cmd:'execute',observationId,step:{op:'fill',target,value}},counts);
          const verified=await call(mode,{cmd:'condition',observationId,condition:{kind:'valueEquals',target,expected:value},results:{}},counts);assert.equal(verified.satisfied,true);
          const result=await call(mode,{cmd:'execute',observationId,step:{op:'query',target,query:{kind:'value'}}},counts);
          assert.equal(result.data.value,value);
        }
        await call(mode,{cmd:'cleanup',observationId},counts);
        const inputEvents=(await rpc.js('window.__BENCH_INPUT_EVENTS__'))-beforeInputEvents;assert.equal(inputEvents,steps/2);
        const elapsedMs=performance.now()-start;
        const after=await rpc.inspect('runtime_health',{windowId:'main',timeoutMs:2000});
        assert.equal(after.diagnostics.installSuccesses,before.diagnostics.installSuccesses);
        assert.equal(store().saveCalls,baseline,'transport change must not dispatch a save');
        rows.push({steps,mode,sample,elapsedMs,inputEvents,...counts,
          bridgeBundleBytes:after.diagnostics.bundleBytesSent-before.diagnostics.bundleBytesSent,
          bridgeCommandBytes:after.diagnostics.commandBytesSent-before.diagnostics.commandBytesSent,
          nativeSaveDelta:store().saveCalls-baseline});
      }
    }
  }
  const groups=[];
  for(const steps of [2,10,20])for(const mode of ['full_source','short_packet']){
    const matching=rows.filter(row=>row.steps===steps&&row.mode===mode);const times=matching.map(row=>row.elapsedMs).sort((a,b)=>a-b);
    const quantile=q=>times[Math.min(times.length-1,Math.ceil(q*times.length)-1)];
    groups.push({steps,mode,samples:matching.length,medianMs:quantile(.5),p95Ms:quantile(.95),meanScriptBytes:matching.reduce((sum,r)=>sum+r.scriptBytes,0)/matching.length,meanBridgeCommandBytes:matching.reduce((sum,r)=>sum+r.bridgeCommandBytes,0)/matching.length,nativeSaveDelta:0});
  }
  const result={schemaVersion:1,platform:process.platform,layer:'native_webview',runtimeSourceSha256:createHash('sha256').update(source).digest('hex'),
    comparison:'Identical installed runtime.dispatch, current semantic algorithm, fill expectations and query assertions; only redundant source transport differs.',
    limitations:['Warm transport microbenchmark, not an old-binary overall workflow latency comparison. Cold full-service samples include journal/preparation and are reported separately without a speed ratio.','Main window only; multimonitor input tested separately.','Thirty samples per configuration do not establish production tail latency.','Both routes use the existing authorized isolated-fixture legacy JS envelope for transport measurement; full-service workflow regression is separate.'],
    samplesPerConfiguration:samples,cold,groups,rows};
  writeFileSync(resolve(output,'transport-benchmark.json'),JSON.stringify(result,null,2));return result;
}
