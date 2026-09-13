import {test} from 'node:test';
import assert from 'node:assert/strict';
import {sendLinuxInput} from './native-input-linux.mjs';

function harness(positions,{clickError}={}) {
  const calls=[];let tick=0,positionIndex=0;
  const exec=async(file,args,options)=>{
    calls.push({file,args,options});tick+=1;
    if(args[0]==='getmouselocation')return {stdout:positions[Math.min(positionIndex++,positions.length-1)]};
    if(args[0]==='click'&&clickError)throw clickError;
    return {stdout:''};
  };
  return {calls,options:{exec,now:()=>tick,sleep:async ms=>{tick+=ms;}}};
}
const position=(x,y)=>`X=${x}\nY=${y}\nSCREEN=0\nWINDOW=1234\n`;
test('Linux same-point click never enters old xdotool --sync wait',async()=>{
  const h=harness([position(64,86)]);
  const result=await sendLinuxInput({action:'click',x:63.5,y:85.5,...h.options});
  assert.equal(result.alreadyAtTarget,true);
  assert.deepEqual(h.calls.map(call=>call.args),[['getmouselocation','--shell'],['click','1']]);
  assert.ok(h.calls.every(call=>call.options.timeout>0&&call.options.timeout<=1500));
});
test('Linux move confirms both coordinates before dispatching exactly one click',async()=>{
  const h=harness([position(0,0),position(64,0),position(64,86)]);
  const result=await sendLinuxInput({action:'click',x:64,y:86,...h.options});
  assert.equal(result.alreadyAtTarget,false);assert.deepEqual(result.confirmedPosition,{x:64,y:86});
  assert.deepEqual(h.calls.map(call=>call.args),[['getmouselocation','--shell'],['mousemove','64','86'],['getmouselocation','--shell'],['getmouselocation','--shell'],['click','1']]);
  assert.equal(h.calls.filter(call=>call.args.includes('--sync')).length,0);
});
test('Linux cursor failure exhausts bounded preparation without clicking or replaying movement',async()=>{
  const h=harness([position(0,0)]);
  await assert.rejects(sendLinuxInput({action:'click',x:64,y:86,...h.options}),/position.*deadline/i);
  assert.equal(h.calls.filter(call=>call.args[0]==='click').length,0);
  assert.equal(h.calls.filter(call=>call.args[0]==='mousemove').length,1);
  assert.ok(h.calls.length<100);
});
test('Linux uncertain click failure is returned without replay',async()=>{
  const failure=new Error('XTest click response lost');const h=harness([position(64,86)],{clickError:failure});
  await assert.rejects(sendLinuxInput({action:'click',x:64,y:86,...h.options}),error=>error===failure);
  assert.equal(h.calls.filter(call=>call.args[0]==='click').length,1);
});
test('Linux double click retains the explicit native event count and delay',async()=>{
  const h=harness([position(64,86)]);
  await sendLinuxInput({action:'double',x:64,y:86,...h.options});
  assert.deepEqual(h.calls.at(-1).args,['click','--repeat','2','--delay','100','1']);
});
test('Linux malformed native coordinates fail before any input',async()=>{
  const h=harness(['X=64\nY=not-a-number\n']);
  await assert.rejects(sendLinuxInput({action:'click',x:64,y:86,...h.options}),/coordinates/i);
  assert.deepEqual(h.calls.map(call=>call.args),[['getmouselocation','--shell']]);
});
test('Linux Escape dispatches one native key command without moving the pointer',async()=>{
  const h=harness([]);
  await sendLinuxInput({action:'escape',...h.options});
  assert.deepEqual(h.calls.map(call=>call.args),[['key','Escape']]);
});
