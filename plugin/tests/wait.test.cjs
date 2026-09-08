const assert = require('node:assert/strict');
const {test} = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
function setup() {
  const listeners = new Map(); const timers = new Set();
  const context = {Date, Promise,
    setTimeout(fn, ms) { const timer = setTimeout(()=>{timers.delete(timer); fn();}, ms); timers.add(timer); return timer; },
    clearTimeout(timer) { timers.delete(timer); clearTimeout(timer); },
    window:{addEventListener(name, fn) { listeners.set(name,fn); }, removeEventListener(name,fn) { if(listeners.get(name)===fn) listeners.delete(name); }} };
  function run(check, timeout) {
    const file = path.join(__dirname, '../js/wait.js');
    return vm.runInNewContext(fs.readFileSync(file,'utf8'),context)(check,timeout);
  }
  return {run,listeners,timers};
}
test('wait condition timeout records final observation and releases timers',async()=>{
  const f=setup(); const result=await f.run(()=>false,5);
  assert.equal(result.timeout,true); assert.equal(result.found,false); assert.equal(f.timers.size,0);
});
test('navigation stops polling and removes listeners',async()=>{
  const f=setup(); const pending=f.run(()=>false,10);
  f.listeners.get('pagehide')?.();
  const result=await pending;
  assert.equal(result.code,'observation_failed'); assert.equal(f.timers.size,0); assert.equal(f.listeners.size,0);
});
test('a hanging async predicate cannot outlive the wait deadline',async()=>{
  const f=setup(); const result=await Promise.race([f.run(()=>new Promise(()=>{}),5),new Promise(resolve=>setTimeout(()=>resolve({hung:true}),30))]);
  assert.equal(result.timeout,true); assert.equal(f.timers.size,0); assert.equal(f.listeners.size,0);
});
