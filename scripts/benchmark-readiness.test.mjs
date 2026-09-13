import assert from 'node:assert/strict';
import {test} from 'node:test';
import {waitForColdDocument} from '../examples/workflow-fixture/scripts/transport-benchmark.mjs';
import {inspectionTargetReady} from '../examples/workflow-fixture/scripts/fixture-readiness.mjs';

test('Native capture readiness requires a responsive document, not a connected socket',()=>{
  assert.equal(inspectionTargetReady({checks:{bridge:{connected:true,status:'unavailable'}},conclusion:'runtime_target_unavailable'}),false);
  assert.equal(inspectionTargetReady({checks:{bridge:{connected:true,status:'responsive'}},conclusion:'runtime_probe_not_completed'}),false);
  assert.equal(inspectionTargetReady({checks:{bridge:{status:'responsive'}},conclusion:'runtime_missing'}),true);
  assert.equal(inspectionTargetReady({checks:{bridge:{status:'responsive'}},conclusion:'runtime_responsive'}),true);
  assert.equal(inspectionTargetReady(null),false);
});

test('UP-T105 cold preparation rejects the old epoch and incomplete replacement documents',async()=>{
  const pages=[{epoch:'old',ready:true,fixture:true},{epoch:'new',ready:false,fixture:true},{epoch:'new',ready:true,fixture:false},{epoch:'new',ready:true,fixture:true}];
  let inspections=0,reads=0;
  const rpc={
    async inspect(operation){assert.equal(operation,'runtime_health');inspections++;return {checks:{bridge:{status:'responsive'}},conclusion:'runtime_missing',diagnostics:{installSuccesses:0}};},
    async js(){return pages[reads++];},
  };
  const result=await waitForColdDocument(rpc,'old');
  assert.equal(result.epoch,'new');assert.equal(reads,4);assert.equal(inspections,4);
  assert.equal(result.health.diagnostics.installSuccesses,0);
});

test('UP-T105 an unavailable bridge cannot authorize cold workflow dispatch',async()=>{
  let inspections=0,reads=0;
  const result=await waitForColdDocument({
    async inspect(){inspections++;return {checks:{bridge:{status:inspections===1?'unresponsive':'responsive'}},conclusion:'runtime_missing'};},
    async js(){reads++;return {epoch:'new',ready:true,fixture:true};},
  },'old');
  assert.equal(result.epoch,'new');assert.equal(inspections,2);assert.equal(reads,1);
});
