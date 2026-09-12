// Actual shipped code + browser input. These are Chromium integration tests, not native WebView tests.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const { chromium } = require('playwright');
const fixture = '<!doctype html><style>body{padding-top:100px}button,input,a{margin:30px;padding:20px}dialog{padding:80px}</style><button id="save" data-testid="save">Save</button><button id="other">Save</button><input id="check" type="checkbox"><a id="link" href="/changed">Go</a><form><button id="submit">Submit</button></form><input type="password" value="never-output" id="password"><script>window.counts={click:0,submit:0,change:0};document.addEventListener("click",()=>counts.click++);document.addEventListener("submit",e=>{e.preventDefault();counts.submit++});document.addEventListener("change",()=>counts.change++);</script>';

// Resource accounting is test-only and installed before the shipped bootstrap.
// Timers keep their real durations; no shortened drain or mocked picker is used.
function installResourceAccounting() {
  const listeners = new WeakMap();
  let globalListeners = 0;
  const add = EventTarget.prototype.addEventListener;
  const remove = EventTarget.prototype.removeEventListener;
  const capture = options => typeof options === 'boolean' ? options : Boolean(options?.capture);
  EventTarget.prototype.addEventListener = function(type, listener, options) {
    if (listener && (this === window || this === document)) {
      let entries = listeners.get(this);
      if (!entries) listeners.set(this, entries = []);
      if (!entries.some(entry => entry.type === type && entry.listener === listener && entry.capture === capture(options))) {
        entries.push({type, listener, capture:capture(options)}); globalListeners++;
      }
    }
    return add.call(this, type, listener, options);
  };
  EventTarget.prototype.removeEventListener = function(type, listener, options) {
    if (this === window || this === document) {
      const entries = listeners.get(this) || [];
      const index = entries.findIndex(entry => entry.type === type && entry.listener === listener && entry.capture === capture(options));
      if (index >= 0) { entries.splice(index, 1); globalListeners--; }
    }
    return remove.call(this, type, listener, options);
  };
  const timers = new Set(), intervals = new Set(), frames = new Set();
  const setTimer = window.setTimeout.bind(window), clearTimer = window.clearTimeout.bind(window);
  const setIntervalOriginal = window.setInterval.bind(window), clearIntervalOriginal = window.clearInterval.bind(window);
  const requestFrame = window.requestAnimationFrame.bind(window), cancelFrame = window.cancelAnimationFrame.bind(window);
  window.setTimeout = (callback, delay, ...args) => {
    if (typeof callback !== 'function') return setTimer(callback, delay, ...args);
    const id = setTimer(() => { timers.delete(id); callback(...args); }, delay);
    timers.add(id); return id;
  };
  window.clearTimeout = id => { timers.delete(id); intervals.delete(id); clearTimer(id); };
  window.setInterval = (callback, delay, ...args) => { const id = setIntervalOriginal(callback, delay, ...args); intervals.add(id); return id; };
  window.clearInterval = id => { intervals.delete(id); timers.delete(id); clearIntervalOriginal(id); };
  window.requestAnimationFrame = callback => { const id = requestFrame(time => { frames.delete(id); callback(time); }); frames.add(id); return id; };
  window.cancelAnimationFrame = id => { frames.delete(id); cancelFrame(id); };
  let lifecycle = 0;
  window.__pickerTestResources = {
    sleep: delay => new Promise(resolve => setTimer(resolve, delay)),
    instrumentLifecycle() {
      const original = window.__CONNECTOR_BOOTSTRAP__;
      const instrumented = Object.create(original);
      Object.defineProperty(instrumented, 'registerLifecycle', {value(callback) {
        lifecycle++;
        const unsubscribe = original.registerLifecycle(callback);
        let active = true;
        return () => { if (active) { active = false; lifecycle--; } unsubscribe(); };
      }});
      window.__CONNECTOR_BOOTSTRAP__ = instrumented;
    },
    snapshot() {
      return {globalListeners, timers:timers.size, intervals:intervals.size, frames:frames.size, lifecycle,
        ownedRoots:document.querySelectorAll('[data-connector-picker-ui]').length};
    }
  };
}

test('shipped picker input, lifecycle and privacy', async t => {
  const server = http.createServer((req,res)=>res.end(fixture));
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({headless:true});
  const context = await browser.newContext({hasTouch:true});
  await context.addInitScript({content:`(${installResourceAccounting.toString()})();\n${fs.readFileSync('plugin/src/runtime/bootstrap.js','utf8')}\nwindow.__pickerTestResources.instrumentLifecycle();`});
  const page = await context.newPage();
  let counter=0;
  const setup = async (html, options = {}) => {
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    if(html) await page.evaluate(html=>document.body.insertAdjacentHTML('beforeend',html),html);
    await page.evaluate(fs.readFileSync('plugin/src/semantic/core.js','utf8'));
    await page.evaluate(source=>{window.pick=eval(source);},fs.readFileSync('plugin/src/picker/page.js','utf8'));
    return page.evaluate(({id,options})=>{window.pickArgs={pickerId:id,nonce:'bounded-test',context:{pageEpoch:window.__CONNECTOR_BOOTSTRAP__.pageEpoch}};return pick({...pickArgs,cmd:'start',timeoutMs:5000,...options});},{id:`p${++counter}`,options});
  };
  const state=()=>page.evaluate(()=>pick({...pickArgs,cmd:'status'}));
  const center=async selector=>{const b=await page.locator(selector).boundingBox();return{x:b.x+b.width/2,y:b.y+b.height/2};};
  const click=async selector=>{const p=await center(selector);await page.mouse.click(p.x,p.y);};
  const cleanup = async () => {
    await page.evaluate(()=>pick({...pickArgs,cmd:'cancel'}));
    await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
  };
  const hover = async selector => {
    const point = await center(selector);
    await page.mouse.move(point.x,point.y);
    await page.waitForFunction(()=>document.querySelector('[data-connector-picker-ui] > div')?.style.display==='block',null,{timeout:3000});
    return point;
  };
  try {
    await t.test('UP-PK001 active UI and UP-PK014 click isolation',async()=>{
      assert.equal((await setup()).status,'awaiting_selection');
      assert.equal(await page.locator('[data-connector-picker-ui]').count(),1);
      await click('#save');
      const result=await state();assert.equal(result.status,'selected');assert.equal(result.selection.tag,'button');
      assert.ok(result.selection.locatorCandidates.some(c=>c.verified&&c.matchCount===1));
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
      await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
      assert.equal(await page.locator('[data-connector-picker-ui]').count(),0);
    });
    await t.test('UP-PK018 double-click tail is swallowed',async()=>{
      await setup();const p=await center('#save');await page.mouse.dblclick(p.x,p.y,{delay:80});
      assert.equal((await state()).status,'selected');assert.equal(await page.evaluate(()=>counts.click),0);
    });
    await t.test('UP-PK019 Escape and keyup consumed; no focus restoration',async()=>{
      await setup();const before=await page.evaluate(()=>document.activeElement.tagName);
      await page.keyboard.press('Escape');assert.equal((await state()).status,'cancelled');
      assert.equal(await page.evaluate(()=>document.activeElement.tagName),before);
    });
    await t.test('UP-PK017 drag gesture does not select',async()=>{
      await setup();const p=await center('#save');await page.mouse.move(p.x,p.y);await page.mouse.down();await page.mouse.move(p.x+30,p.y,{steps:3});await page.mouse.up();
      assert.equal((await state()).status,'awaiting_selection');
    });
    await t.test('UP-PK023 virtual node identity change rejects confirmation',async()=>{
      await setup();const p=await center('#save');await page.mouse.move(p.x,p.y);await page.mouse.down();
      await page.evaluate(()=>document.querySelector('#save').dataset.id='new-entity');await page.mouse.up();
      assert.equal((await state()).status,'awaiting_selection');
    });
    await t.test('UP-PK016 modal top layer and UP-PK021 actual span target',async()=>{
      await setup('<dialog id="dialog"><button id="inside"><span id="icon">Icon</span></button></dialog>');
      await page.evaluate(()=>pick({...pickArgs,cmd:'cancel'}));await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
      await page.evaluate(()=>{document.querySelector('dialog').showModal();pickArgs.pickerId+='modal';pick({...pickArgs,cmd:'start',timeoutMs:5000});});
      await click('#icon');assert.equal((await state()).selection.tag,'span');
    });
    await t.test('UP-PK025 sensitive element has no value or executable candidate',async()=>{
      await setup();await click('#password');const result=await state();assert.equal(result.status,'selected');
      assert.ok(!JSON.stringify(result).includes('never-output'));assert.deepEqual(result.selection.locatorCandidates,[]);
    });
    await t.test('UP-PK028 get freezes observation and UP-PK032 cancel preserves selected',async()=>{
      await setup();await click('#save');const first=await state();await page.evaluate(()=>pick({...pickArgs,cmd:'cancel'}));
      const second=await state();assert.equal(second.status,'selected');assert.equal(second.selection.capturedAtMs,first.selection.capturedAtMs);
      assert.deepEqual(second.selection,first.selection);
    });
    await t.test('UP-PK023 Enter revalidates a covered stale hover without pointer movement',async()=>{
      await setup();await hover('#save');
      await page.evaluate(()=>{
        const rect=document.querySelector('#save').getBoundingClientRect();
        const cover=document.createElement('div');cover.id='new-application-cover';
        Object.assign(cover.style,{position:'fixed',left:`${rect.x}px`,top:`${rect.y}px`,width:`${rect.width}px`,height:`${rect.height}px`,zIndex:'999999',background:'red'});
        document.body.append(cover);
      });
      await page.keyboard.press('Enter');
      assert.equal((await state()).status,'awaiting_selection');
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
    });
    await t.test('UP-PK020 visible cancel button cancels without business click or focus restoration',async()=>{
      await setup();await page.evaluate(()=>{window.focusBeforeCancel=document.activeElement;});
      const cancel=page.locator('[data-connector-picker-ui] button');
      assert.equal(await cancel.isVisible(),true);
      const box=await cancel.boundingBox();await page.mouse.click(box.x+box.width/2,box.y+box.height/2);
      const result=await state();assert.equal(result.status,'cancelled');assert.equal(result.selection,null);
      await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
      assert.equal(await page.evaluate(()=>document.activeElement===window.focusBeforeCancel),true);
      assert.equal(await page.locator('[data-connector-picker-ui]').count(),0);
    });
    await t.test('UP-PK020 toolbar text does not select the business element behind it',async()=>{
      await setup();
      const label=page.locator('[data-connector-picker-ui] span');const box=await label.boundingBox();
      await page.evaluate(box=>{
        const behind=document.createElement('button');behind.id='behind-toolbar';behind.textContent='Behind toolbar';
        Object.assign(behind.style,{position:'fixed',margin:'0',left:`${box.x}px`,top:`${box.y}px`,width:`${box.width}px`,height:`${box.height}px`});document.body.append(behind);
      },box);
      await page.mouse.click(box.x+box.width/2,box.y+box.height/2);
      assert.equal((await state()).status,'awaiting_selection');assert.equal(await page.evaluate(()=>counts.click),0);
      await page.keyboard.press('Enter');assert.equal((await state()).status,'awaiting_selection');
    });
    await t.test('UP-PK019 unsupported modifiers, repeat, IME and empty hover never confirm',async()=>{
      await setup();await page.keyboard.press('Enter');assert.equal((await state()).status,'awaiting_selection');
      await hover('#save');
      for(const key of ['Control+Enter','Alt+Enter','Shift+Enter','Meta+Enter']) {
        await page.keyboard.press(key);assert.equal((await state()).status,'awaiting_selection',key);
      }
      // Synthetic KeyboardEvents exercise flags unavailable through normal CDP typing.
      for(const flags of [{repeat:true},{isComposing:true}]) {
        await page.evaluate(flags=>{
          const init={key:'Enter',code:'Enter',bubbles:true,cancelable:true,composed:true,...flags};
          document.dispatchEvent(new KeyboardEvent('keydown',init));document.dispatchEvent(new KeyboardEvent('keyup',init));
        },flags);
        assert.equal((await state()).status,'awaiting_selection',JSON.stringify(flags));
      }
      assert.equal(await page.evaluate(()=>__CONNECTOR_INPUT_GUARD__.state().heldInputs),0);
      await page.keyboard.press('Enter');assert.equal((await state()).status,'selected');
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
    });
    await t.test('UP-PK021 disabled button, span, SVG and SVG child preserve exact hit targets',async()=>{
      const cases=[
        {html:'<button id="exact" disabled style="position:fixed;left:60px;top:350px">Disabled</button>',tag:'button',disabled:true},
        {html:'<button style="position:fixed;left:60px;top:350px"><span id="exact">Exact span</span></button>',tag:'span'},
        {html:'<svg id="exact" width="100" height="80" style="position:fixed;left:60px;top:350px;background:cyan"><title>Exact vector</title></svg>',tag:'svg'},
        {html:'<button style="position:fixed;left:60px;top:350px"><svg width="100" height="80"><rect id="exact" x="0" y="0" width="100" height="80" fill="cyan"/></svg></button>',tag:'rect'}
      ];
      for(const example of cases) {
        await setup(example.html);await click('#exact');const result=await state();
        assert.equal(result.status,'selected',example.tag);assert.equal(result.selection.tag,example.tag);
        if(example.disabled) {assert.equal(result.selection.states.disabled,true);assert.equal(result.selection.actionable,false);}
        const matches=await page.evaluate(selection=>selection.locatorCandidates.map(item=>{
          const found=__CONNECTOR_SEMANTIC__.resolveCandidates(item.target);return found.length===1&&found[0]===document.querySelector('#exact');
        }),result.selection);
        assert.ok(matches.every(Boolean),'candidate promoted the hit to an ancestor');
        assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
      }
    });
    await t.test('UP-PK017 synthetic pointer mismatch, multi-touch, cancel and lost capture do not select',async()=>{
      const scenarios=[
        [{type:'pointerdown',pointerId:1},{type:'pointerup',pointerId:2},{type:'pointercancel',pointerId:1}],
        [{type:'pointerdown',pointerId:1},{type:'pointerdown',pointerId:2,isPrimary:false},{type:'pointerup',pointerId:1},{type:'pointerup',pointerId:2,isPrimary:false}],
        [{type:'pointerdown',pointerId:1},{type:'pointercancel',pointerId:1},{type:'pointerup',pointerId:1}],
        [{type:'pointerdown',pointerId:1},{type:'lostpointercapture',pointerId:1},{type:'pointerup',pointerId:1}],
        [{type:'pointerdown',pointerId:1},{type:'pointerup',pointerId:1,otherTarget:true}]
      ];
      for(const sequence of scenarios) {
        await setup();const point=await center('#save'),other=await center('#other');
        await page.evaluate(({sequence,point,other})=>{
          const root=document.querySelector('[data-connector-picker-ui]');
          for(const {type,otherTarget,...fields} of sequence) {
            const pos=otherTarget?other:point;
            root.dispatchEvent(new PointerEvent(type,{pointerType:'touch',isPrimary:true,button:0,buttons:type==='pointerdown'?1:0,clientX:pos.x,clientY:pos.y,bubbles:true,cancelable:true,composed:true,...fields}));
          }
        },{sequence,point,other});
        assert.equal((await state()).status,'awaiting_selection',JSON.stringify(sequence));
        assert.equal(await page.evaluate(()=>__CONNECTOR_INPUT_GUARD__.state().heldInputs),0);
        assert.equal(await page.evaluate(()=>document.querySelector('[data-connector-picker-ui]').hasPointerCapture(1)),false);
        assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
      }
    });
    await t.test('UP-PK018 actual browser touch selects once without compatibility click-through',async()=>{
      await setup();const point=await center('#save');await page.touchscreen.tap(point.x,point.y);
      const first=await state();assert.equal(first.status,'selected');
      await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
      const last=await state();assert.equal(last.selection.selectionId,first.selection.selectionId);
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
      assert.equal(await page.evaluate(()=>__CONNECTOR_INPUT_GUARD__.state().heldInputs),0);
    });
    await t.test('UP-PK025 nested and chained sensitive ARIA refs never appear in hover or candidates',async()=>{
      const examples=[
        '<span hidden id="sensitive-label"><span data-sensitive="true">nested-private-value</span></span><button id="private-pick" aria-labelledby="sensitive-label" style="position:fixed;left:60px;top:350px">Pick</button>',
        '<span hidden id="chain-one" aria-labelledby="chain-two">One</span><span hidden id="chain-two" aria-labelledby="chain-one sensitive-label">Two</span><span hidden id="sensitive-label" data-sensitive="true">chained-private-value</span><button id="private-pick" aria-labelledby="chain-one" style="position:fixed;left:60px;top:350px">Pick</button>'
      ];
      for(const html of examples) {
        await setup(html);await hover('#private-pick');
        const visible=await page.locator('[data-connector-picker-ui]').innerText();assert.ok(!visible.includes('private-value'));
        await click('#private-pick');const result=await state();assert.equal(result.status,'selected');
        assert.equal(result.selection.name,'[redacted]');assert.deepEqual(result.selection.locatorCandidates,[]);
        assert.ok(!JSON.stringify(result).includes('private-value'));
      }
    });
    await t.test('UP-PK028 screenshots-off page polling never re-observes selection or invokes capture',async()=>{
      await setup('',{screenshot:{enabled:false}});
      await page.evaluate(()=>{
        window.sideObservation={hit:0,geometry:0,capture:0};
        const hit=document.elementsFromPoint.bind(document);
        document.elementsFromPoint=(...args)=>{sideObservation.hit++;return hit(...args);};
        const geometry=Element.prototype.getBoundingClientRect;
        Element.prototype.getBoundingClientRect=function(...args){sideObservation.geometry++;return geometry.apply(this,args);};
        window.__TAURI_INTERNALS__={invoke(){sideObservation.capture++;throw new Error('Unexpected page invoke');}};
        HTMLCanvasElement.prototype.toDataURL=function(){sideObservation.capture++;throw new Error('Unexpected canvas encoding');};
      });
      await click('#save');const first=await state();assert.equal(first.status,'selected');
      await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
      const before=await page.evaluate(()=>({...sideObservation}));
      await page.evaluate(()=>{document.querySelector('#save').textContent='Changed after selection';});
      for(let i=0;i<20;i++) {
        const current=await page.evaluate(i=>pick({...pickArgs,cmd:'status',includeImage:i%2===0}),i);
        assert.deepEqual(current.selection,first.selection);
      }
      assert.deepEqual(await page.evaluate(()=>sideObservation),before);
      assert.equal(before.capture,0);
      // Host screenshot scheduling is separately covered by service tests; this
      // assertion proves only that the shipped page picker has no hidden capture.
    });
    await t.test('UP-PK023 long same-prefix text mutation cannot reuse a truncated identity',async()=>{
      await setup('<button id="long-identity" style="position:fixed;top:350px;left:20px;width:600px;height:90px;overflow:hidden">Placeholder</button>');
      await page.evaluate(()=>{document.querySelector('#long-identity').textContent='X'.repeat(600)+'-first-entity';});
      const point=await center('#long-identity');await page.mouse.move(point.x,point.y);await page.mouse.down();
      await page.evaluate(()=>{document.querySelector('#long-identity').textContent='X'.repeat(600)+'-second-entity';});
      await page.mouse.up();const result=await state();
      assert.equal(result.status,'awaiting_selection');assert.equal(result.selection,null);
      assert.equal(await page.evaluate(()=>__CONNECTOR_SEMANTIC__.describe(document.querySelector('#long-identity')).truncated),true);
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
    });
    await t.test('UP-PK029 100 start/cancel cycles return real resources to baseline', {timeout:80000}, async()=>{
      await setup();await cleanup();
      const result=await page.evaluate(async()=>{
        const tracker=window.__pickerTestResources;
        const baseline=tracker.snapshot();
        const originalId=pickArgs.pickerId;
        for(let i=0;i<100;i++) {
          pickArgs.pickerId=originalId+'-cleanup-'+i;
          const started=pick({...pickArgs,cmd:'start',timeoutMs:5000});
          if(started.status!=='awaiting_selection')return {completed:i,baseline,error:started};
          const root=document.querySelector('[data-connector-picker-ui]');
          root.dispatchEvent(new PointerEvent('pointermove',{clientX:100,clientY:300,bubbles:true,composed:true}));
          window.dispatchEvent(new Event('resize'));
          pick({...pickArgs,cmd:'cancel'});
          const deadline=Date.now()+3000;
          while(pick({...pickArgs,cmd:'status'}).cleanup.status!=='confirmed') {
            if(Date.now()>deadline)return {completed:i,baseline,error:'cleanup deadline',resources:tracker.snapshot()};
            await tracker.sleep(5);
          }
          const resources=tracker.snapshot(),guard=__CONNECTOR_INPUT_GUARD__.state();
          if(JSON.stringify(resources)!==JSON.stringify(baseline)||pick.activeObservers()!==0||guard.active||guard.draining||guard.heldInputs) {
            return {completed:i+1,baseline,resources,guard,observers:pick.activeObservers()};
          }
        }
        return {completed:100,baseline,resources:tracker.snapshot(),guard:__CONNECTOR_INPUT_GUARD__.state()};
      });
      assert.equal(result.completed,100,JSON.stringify(result));assert.deepEqual(result.resources,result.baseline);
      assert.equal(result.guard.active,false);assert.equal(result.guard.draining,false);assert.equal(result.guard.heldInputs,0);
      assert.deepEqual(await page.evaluate(()=>counts),{click:0,submit:0,change:0});
    });
  } finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
});

test('picker selected-reference retention', async t => {
  const server=http.createServer((req,res)=>res.end(fixture));
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const browser=await chromium.launch({headless:true});
  const context=await browser.newContext();
  await context.addInitScript({content:`(${installResourceAccounting.toString()})();\n${fs.readFileSync('plugin/src/runtime/bootstrap.js','utf8')}\nwindow.__pickerTestResources.instrumentLifecycle();`});
  const page=await context.newPage();let counter=0;
  const start=async()=>{
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    await page.evaluate(fs.readFileSync('plugin/src/semantic/core.js','utf8'));
    await page.evaluate(source=>{window.pick=eval(source);},fs.readFileSync('plugin/src/picker/page.js','utf8'));
    // Warm Playwright's lazily installed query/input helpers before accounting.
    await page.locator('#save').boundingBox();
    return page.evaluate(id=>{
      window.pickArgs={pickerId:id,nonce:'reference-retention',context:{pageEpoch:__CONNECTOR_BOOTSTRAP__.pageEpoch}};
      window.referenceBaseline=__pickerTestResources.snapshot();window.originalReferenceDeadline=performance.now()+5000;
      return pick({...pickArgs,cmd:'start',timeoutMs:5000});
    },`reference-${++counter}`);
  };
  const select=async()=>{
    const rect=await page.locator('#save').boundingBox();await page.mouse.click(rect.x+rect.width/2,rect.y+rect.height/2);
    await page.waitForFunction(()=>pick({...pickArgs,cmd:'status'}).cleanup.status==='confirmed',null,{timeout:3500});
    return page.evaluate(()=>pick({...pickArgs,cmd:'status'}));
  };
  const geometry=()=>page.evaluate(()=>pick({...pickArgs,cmd:'geometry'}));
  try {
    await t.test('UP-PK029 missing host release expires references at the original picker deadline',async()=>{
      assert.equal((await start()).status,'awaiting_selection');
      await page.evaluate(()=>__pickerTestResources.sleep(1000));
      const selected=await select();assert.equal(selected.status,'selected');assert.equal((await geometry()).ready,true);
      await page.evaluate(()=>__pickerTestResources.sleep(Math.max(0,originalReferenceDeadline-performance.now())+80));
      assert.equal((await geometry()).error?.code,'capture_context_changed','expired selection must not keep a usable DOM reference');
      const after=await page.evaluate(()=>({report:pick({...pickArgs,cmd:'status'}),resources:__pickerTestResources.snapshot(),baseline:referenceBaseline,observers:pick.activeObservers(),counts}));
      assert.equal(after.report.status,'selected');assert.deepEqual(after.report.selection,selected.selection);assert.equal(after.report.sequence,selected.sequence);
      assert.deepEqual(after.resources,after.baseline);assert.equal(after.observers,0);assert.deepEqual(after.counts,{click:0,submit:0,change:0});
    });
    await t.test('UP-PK029 successful release removes the retention timer without changing the result',async()=>{
      await start();const selected=await select();assert.equal((await geometry()).ready,true);
      const retained=await page.evaluate(()=>({resources:__pickerTestResources.snapshot(),baseline:referenceBaseline}));
      assert.equal(retained.resources.timers,retained.baseline.timers+1,'one bounded selected-reference timer must remain');
      const released=await page.evaluate(()=>pick({...pickArgs,cmd:'release'}));assert.deepEqual(released.selection,selected.selection);assert.equal(released.status,'selected');
      assert.equal((await geometry()).error?.code,'capture_context_changed');
      const after=await page.evaluate(()=>({resources:__pickerTestResources.snapshot(),baseline:referenceBaseline,observers:pick.activeObservers()}));
      assert.deepEqual(after.resources,after.baseline);assert.equal(after.observers,0);
    });
    await t.test('UP-PK031 dispose and pagehide clear selected-reference timers and lifecycle hooks',async()=>{
      for(const action of ['dispose','pagehide']) {
        await start();const selected=await select();assert.equal((await geometry()).ready,true);
        await page.evaluate(action=>{if(action==='dispose')pick.dispose();else window.dispatchEvent(new PageTransitionEvent('pagehide'));},action);
        assert.equal((await geometry()).error?.code,'capture_context_changed',action);
        const after=await page.evaluate(()=>({report:pick({...pickArgs,cmd:'status'}),resources:__pickerTestResources.snapshot(),observers:pick.activeObservers()}));
        assert.equal(after.resources.timers,0,action);assert.equal(after.resources.lifecycle,0,action);assert.equal(after.observers,0,action);
        assert.equal(after.report.status,'selected');assert.deepEqual(after.report.selection,selected.selection);
      }
    });
  } finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
});
