const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const crypto=require('node:crypto');
const bootstrap=()=>fs.readFileSync(require('node:path').join(__dirname,'../src/runtime/bootstrap.js'),'utf8');
function page(subframe=false){
 const listeners=new Map(); const window={};window.top=subframe?{}:window;
 const context=vm.createContext({window,crypto,document:{readyState:'loading'},setTimeout,clearTimeout,performance,console});
 window.addEventListener=(type,fn)=>{const list=listeners.get(type)||[];list.push(fn);listeners.set(type,list)};
 window.removeEventListener=(type,fn)=>listeners.set(type,(listeners.get(type)||[]).filter(x=>x!==fn));
 const run=()=>vm.runInContext(bootstrap(),context); const fire=(type,extra={})=>{const event={type,defaultPrevented:false,preventDefault(){this.defaultPrevented=true},stopImmediatePropagation(){this.stopped=true},...extra}; for(const fn of listeners.get(type)||[]){fn(event); if(event.stopped)break}return event};
 return {window,run,fire,listeners,context};
}
test('UP-T021 bootstrap rejects child frames',()=>{const p=page(true);p.run();assert.equal(p.window.__CONNECTOR_BOOTSTRAP__,undefined)});
test('UP-T012 UP-T013 same document bootstrap once, new document new epoch',()=>{const p=page();p.run();const first=p.window.__CONNECTOR_BOOTSTRAP__;p.run();assert.equal(first,p.window.__CONNECTOR_BOOTSTRAP__);const q=page();q.run();assert.notEqual(first.pageEpoch,q.window.__CONNECTOR_BOOTSTRAP__.pageEpoch)});
test('UP-PK008 early idle guard has no effects; active guard suppresses before handlers',()=>{const p=page();p.run();assert.equal(p.fire('click').defaultPrevented,false);const guard=p.window.__CONNECTOR_INPUT_GUARD__;let seen=false;guard.activate(e=>{seen=e.defaultPrevented});const event=p.fire('pointerdown');assert.equal(seen,true);assert.equal(event.stopped,true);assert.equal(guard.state().early,true)});
test('UP-PK011 UP-PK022 cleanup drains delayed click and then returns to idle',async()=>{const p=page();p.run();const guard=p.window.__CONNECTOR_INPUT_GUARD__;guard.activate(()=>{});const cleanup=guard.deactivate({drainMs:30});assert.equal(p.fire('dblclick').defaultPrevented,true);await cleanup;assert.equal(p.fire('click').defaultPrevented,false);assert.equal(guard.state().active,false)});
test('UP-T022 pagehide invalidates modules and bfcache restores a fresh epoch without sessions',()=>{const p=page();p.run();const b=p.window.__CONNECTOR_BOOTSTRAP__;const first=b.pageEpoch;let disposed=0;b.registerLifecycle(()=>disposed++);b.guard.activate(()=>{});p.fire('pagehide',{persisted:true});assert.equal(disposed,1);assert.equal(b.suspended,true);p.fire('pageshow',{persisted:true});assert.notEqual(first,b.pageEpoch);assert.equal(b.guard.state().active,false);assert.equal(b.suspended,false)});
test('UP-PK018 start refuses a held gesture, drain waits for release and a quiet interval',async()=>{const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;p.fire('pointerdown',{pointerId:7});assert.throws(()=>g.activate(()=>{}),/gesture_in_progress/);p.fire('pointerup',{pointerId:7});g.activate(()=>{});p.fire('keydown',{key:'Escape'});let done=false;const cleanup=g.deactivate({drainMs:20}).then(value=>{done=true;return value});await new Promise(r=>setTimeout(r,30));assert.equal(done,false);p.fire('keyup',{key:'Escape'});await new Promise(r=>setTimeout(r,10));p.fire('dblclick');await new Promise(r=>setTimeout(r,12));assert.equal(done,false);assert.equal((await cleanup).confirmed,true)});
test('UP-PK029 hover without a held gesture cannot starve cleanup',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});
 let done=false;const cleanup=g.deactivate({drainMs:30}).then(result=>{done=true;return result});
 const hover=setInterval(()=>p.fire('pointermove',{buttons:0}),5);
 try {await new Promise(resolve=>setTimeout(resolve,80));assert.equal(done,true);assert.equal((await cleanup).confirmed,true);}
 finally {clearInterval(hover);await cleanup;}
});
const installer=()=>fs.readFileSync(require('node:path').join(__dirname,'../src/runtime/installer.js'),'utf8');
function install(p,modules={workflow:async args=>({ok:true,data:args})}) {p.run();const b=p.window.__CONNECTOR_BOOTSTRAP__;const context={appId:'fixture',appInstanceId:'app',windowId:'main',windowInstanceId:'window',pageEpoch:b.pageEpoch,runtimeId:'runtime',runtimeVersion:'1',semanticVersion:'1',bundleHash:'hash'};const config={context,runtimeVersion:'1',semanticVersion:'1',bundleHash:'hash'};vm.runInContext(installer(),p.context)(config,modules);return p.window.__CONNECTOR_INSPECTION_RUNTIME__;}
const packet=(runtime,args={})=>({runtimeProtocolVersion:1,requestId:'request',expectedContext:runtime.context,module:'workflow',remainingMs:100,args});
test('UP-T015 ready-to-dispatch context race is rejected before handler',async()=>{const p=page();let calls=0;const r=install(p,{workflow:()=>{calls++}});const request=packet(r);request.expectedContext={...request.expectedContext,pageEpoch:'reloaded'};const result=await r.dispatch(request);assert.equal(result.dispatched,false);assert.equal(result.error.code,'stale_context');assert.equal(calls,0)});
test('UP-T016 helper name in business exception does not replay',async()=>{const p=page();let calls=0;const r=install(p,{workflow:()=>{calls++;throw new Error('runtime_missing helper broken')}});await assert.rejects(r.dispatch(packet(r)),/runtime_missing/);assert.equal(calls,1)});
test('UP-T023 dangerous strings and prototype keys remain data',async()=>{const p=page();const r=install(p);const args=JSON.parse('{"value":"` ${danger} \\n 中文 \\u2028 \\u2029", "__proto__":{"polluted":true}}');const result=await r.dispatch(packet(r,args));assert.equal(result.data.value,args.value);assert.equal(Object.prototype.polluted,undefined);assert.deepEqual(result.data.__proto__,args.__proto__)});
test('UP-T019 UP-T020 disposal prevents further dispatch and clears module resources',async()=>{const p=page();let active=1;const workflow=()=>{};workflow.dispose=()=>{active=0};workflow.activeObservers=()=>active;const r=install(p,{workflow});assert.equal(r.status().activeObservers,1);r.dispose();assert.equal(r.status().activeObservers,0);assert.equal((await r.dispatch(packet(r))).error.code,'stale_context')});
test('UP-T024 expired deadline and unknown module never enter handler',async()=>{const p=page();let calls=0;const r=install(p,{workflow:()=>calls++});const expired=packet(r);expired.remainingMs=0;assert.equal((await r.dispatch(expired)).error.code,'deadline_expired');const unknown=packet(r);unknown.module='__proto__';assert.equal((await r.dispatch(unknown)).error.code,'unsupported_module');assert.equal(calls,0)});
test('UP-PK031 old operation context is not rebound to a new runtime',async()=>{const p=page();let calls=0;const r=install(p,{workflow:()=>calls++});const request=packet(r,{context:{...r.context,runtimeId:'old-runtime'}});assert.equal((await r.dispatch(request)).error.code,'stale_context');assert.equal(calls,0)});
test('UP-T001 legacy frontend runtime event array cannot collide with inspection dispatcher',()=>{const p=page();const events=[{kind:'fixture-existing-runtime-event'}];p.window.__CONNECTOR_RUNTIME__=events;const runtime=install(p);assert.equal(p.window.__CONNECTOR_RUNTIME__,events);assert.equal(runtime.status().ready,true);assert.notEqual(p.window.__CONNECTOR_INSPECTION_RUNTIME__,events)});
test('UP-PK018 pressed pointer entering drain remains held until actual release',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});
 let result;const cleanup=g.deactivate({drainMs:20}).then(value=>{result=value;return value});
 p.fire('pointermove',{pointerId:99,pointerType:'mouse',buttons:1});
 await new Promise(resolve=>setTimeout(resolve,35));
 try {assert.equal(result,undefined,'pressed buttons observed without pointerdown must prevent confirmation');}
 finally {p.fire('pointerup',{pointerId:99,pointerType:'mouse',buttons:0});await cleanup;}
 assert.equal(result.confirmed,true);
});
test('UP-PK018 losing pointer capture while pressed is not a release',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});
 p.fire('pointerdown',{pointerId:7,pointerType:'mouse',buttons:1});
 p.fire('gotpointercapture',{pointerId:7,pointerType:'mouse',buttons:1});
 let result;const cleanup=g.deactivate({drainMs:20}).then(value=>{result=value;return value});
 p.fire('lostpointercapture',{pointerId:7,pointerType:'mouse',buttons:1});
 await new Promise(resolve=>setTimeout(resolve,35));
 try {assert.equal(result,undefined,'capture loss cannot confirm while buttons remain pressed');}
 finally {p.fire('pointermove',{pointerId:7,pointerType:'mouse',buttons:0});await cleanup;}
 assert.equal(result.confirmed,true);
});
test('UP-PK008 idle pressed-buttons observations reject activation until released',async()=>{
 for(const type of ['pointermove','mousemove']) {
  const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;
  p.fire(type,{pointerId:3,pointerType:'mouse',buttons:1});
  assert.throws(()=>g.activate(()=>{}),/gesture_in_progress/);
  p.fire(type,{pointerId:3,pointerType:'mouse',buttons:0});
  g.activate(()=>{});assert.equal((await g.deactivate({drainMs:0})).confirmed,true);
 }
});
test('UP-PK018 mouse buttons zero cannot release touch contacts or held keys',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});
 p.fire('keydown',{code:'Escape',key:'Escape'});
 p.fire('pointermove',{pointerId:4,pointerType:'touch',buttons:1});
 let result;const cleanup=g.deactivate({drainMs:20}).then(value=>{result=value;return value});
 p.fire('mousemove',{buttons:0});p.fire('pointermove',{pointerId:5,pointerType:'mouse',buttons:0});
 await new Promise(resolve=>setTimeout(resolve,30));assert.equal(result,undefined);
 p.fire('keyup',{code:'Escape',key:'Escape'});
 await new Promise(resolve=>setTimeout(resolve,30));
 try {assert.equal(result,undefined,'mouse release must not erase touch state after the key is released');}
 finally {p.fire('pointerup',{pointerId:4,pointerType:'touch',buttons:0});await cleanup;}
 assert.equal(result.confirmed,true);
});
test('UP-PK029 only explicit zero-buttons hover skips quiet timer extension',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});
 let result;const cleanup=g.deactivate({drainMs:20}).then(value=>{result=value;return value});
 const hover=setInterval(()=>p.fire('pointermove',{pointerId:1}),5);
 await new Promise(resolve=>setTimeout(resolve,50));
 try {assert.equal(result,undefined,'missing buttons does not prove passive hover');}
 finally {clearInterval(hover);p.fire('pointermove',{pointerId:1,buttons:0});await cleanup;}
 assert.equal(result.confirmed,true);
});
test('UP-PK022 pressed buttons retain the hard cleanup deadline and unconfirmed result',async()=>{
 const p=page();const timers=new Map();let next=0;
 p.context.setTimeout=(fn,ms)=>{const id=++next;timers.set(id,{fn,ms});return id};p.context.clearTimeout=id=>timers.delete(id);
 p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});p.fire('pointermove',{pointerId:8,pointerType:'pen',buttons:1});
 const cleanup=g.deactivate({drainMs:20});const hard=[...timers.values()].find(timer=>timer.ms===2000);
 assert.ok(hard);hard.fn();assert.equal((await cleanup).confirmed,false);assert.equal(g.state().draining,false);
 assert.throws(()=>g.activate(()=>{}),/gesture_in_progress/);
 p.fire('pointerup',{pointerId:8,pointerType:'pen',buttons:0});g.activate(()=>{});await g.deactivate({drainMs:0});
});
test('UP-PK018 mouse zero does not clear legacy touch-event contacts',async()=>{
 const p=page();p.run();const g=p.window.__CONNECTOR_INPUT_GUARD__;g.activate(()=>{});p.fire('touchstart',{touches:[{identifier:2}]});
 let result;const cleanup=g.deactivate({drainMs:20}).then(value=>{result=value;return value});
 p.fire('mousemove',{buttons:0});await new Promise(resolve=>setTimeout(resolve,30));
 try {assert.equal(result,undefined);}
 finally {p.fire('touchend',{touches:[]});await cleanup;}
 assert.equal(result.confirmed,true);
});
