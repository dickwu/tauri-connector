const {test,after}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const connectorEval=eval(fs.readFileSync(path.join(__dirname,'../js/eval.js'),'utf8'));
const run=(source,ms=1000)=>connectorEval(source,ms,'connector_eval_timeout');

test('expressions keep their historical meaning',async()=>{
  assert.equal(await run('1 + 1'),2);
  assert.deepEqual(await run('{a: 1}'),{a:1});
  assert.equal(await run('(() => { return 3 })()'),3);
  assert.equal(await run('Promise.resolve(4)'),4);
  assert.equal(await run('1 // trailing comment'),1);
});

test('statement lists return the last completion value (R2 item 2)',async()=>{
  assert.equal(await run('const a = 1; a + 1'),2);
  assert.equal(await run('let s = 0\nfor (const n of [1, 2, 3]) s += n\ns'),6);
  assert.equal(await run('if (true) { "yes" } else { "no" }'),'yes');
  // Block scoping: running the same script twice does not redeclare.
  assert.equal(await run('const b = 5; b'),5);
  assert.equal(await run('const b = 6; b'),6);
});

test('async statement bodies with return/await still work',async()=>{
  assert.equal(await run('const v = await Promise.resolve(7); return v * 2'),14);
  assert.equal(await run('return 8'),8);
});

test('a runtime SyntaxError runs the script exactly once',async()=>{
  globalThis.__evalRuns=0;
  await assert.rejects(run('globalThis.__evalRuns++; JSON.parse("{")'),SyntaxError);
  assert.equal(globalThis.__evalRuns,1);
  globalThis.__evalRuns=0;
  await assert.rejects(run('globalThis.__evalRuns++; await 0; JSON.parse("{")'),SyntaxError);
  assert.equal(globalThis.__evalRuns,1);
});

test('genuine syntax errors are reported',async()=>{
  assert.throws(()=>run('const = 1;'),SyntaxError);
  assert.throws(()=>run('} foo(); {'),SyntaxError);
});

test('an effectful pending promise is left to the bridge deadline',async()=>{
  let changed=false;
  globalThis.window={__CONNECTOR_EFFECTS__:{mark:()=>0,unchangedSince:()=>!changed}};
  try{
    changed=true;
    const pending=run('new Promise(r => setTimeout(() => r(11), 120))',30);
    // Past the page deadline the effectful script still resolves normally.
    assert.equal(await pending,11);
    changed=false;
    await assert.rejects(run('new Promise(() => {})',30),/connector_eval_timeout/);
  }finally{globalThis.window=undefined;delete globalThis.window;}
});

test('without an effect monitor the page never claims a clean timeout',async()=>{
  const outcome=await Promise.race([run('new Promise(() => {})',20).then(()=>'settled',()=>'rejected'),new Promise(r=>setTimeout(()=>r('pending'),150))]);
  assert.equal(outcome,'pending');
});

test('a pending promise rejects with the timeout mark at the page deadline',async()=>{
  globalThis.window={__CONNECTOR_EFFECTS__:{mark:()=>0,unchangedSince:()=>true}};
  after(()=>{delete globalThis.window;});
  const started=Date.now();
  await assert.rejects(run('new Promise(() => {})',50),/^Error: connector_eval_timeout: script still pending after 50 ms/);
  assert.ok(Date.now()-started<1000);
  assert.equal(await run('new Promise(r => setTimeout(() => r(9), 10))',500),9);
});
