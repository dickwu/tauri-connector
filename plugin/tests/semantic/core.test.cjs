// Executes shipped semantic, workflow, legacy locator and snapshot source in Chromium.
// Browser evidence only; this is not a native WebView test.
const {readFile} = require('node:fs/promises');
const {chromium} = require('playwright');
const {createServer} = require('node:http');
const {join} = require('node:path');
async function corpus() {
  const sem = window.__CONNECTOR_SEMANTIC__;
  const results = [];
  const eq = (a,b) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${JSON.stringify(a)} != ${JSON.stringify(b)}`); };
  const ok = (x,m='assertion failed') => { if (!x) throw new Error(m); };
  const test = async (name,fn) => { try { document.body.innerHTML = ''; await fn(); results.push({name,passed:true}); } catch(e) { results.push({name,passed:false,error:e.stack}); } };
  const fixture = html => {document.body.innerHTML=html; return document.body.firstElementChild;};
  const error = (fn, code) => {try {fn(); throw new Error('did not reject');} catch(e) {eq(e.code,code);} };
  await test('UP-T037 UP-T038 ordered IDREF, hidden references, missing IDs and label precedence', () => {
    fixture('<button id="pick" aria-label="Wrong" aria-labelledby="b missing a">Wrong</button><span id="a" hidden>First</span><span id="b">Second</span>');
    const el=document.querySelector('button'); eq(sem.getAccessibleName(el),'Second First');
    eq(sem.resolveCandidates({by:'role',value:'button',name:'Second First'})[0],el);
    const snap=window.__CONNECTOR_SNAPSHOT__({mode:'ai'}); ok(snap.snapshot.includes('Second First')); eq(snap.meta.semanticVersion,sem.version);
    eq(sem.getAccessibleName(fixture('<button aria-labelledby="missing" aria-label="Fallback">Text</button>')),'Fallback');
    fixture('<button aria-labelledby="visible-label">Text</button><span id="visible-label">Visible <span hidden>Excluded</span></span>');eq(sem.getAccessibleName(document.querySelector('button')),'Visible');
  });
  await test('UP-T039 form labels, input defaults, icon alt and valid role fallback', () => {
    fixture('<label for="x">Task <span>Name</span></label><input id="x"><button id="icon"><img alt="Delete"></button><input id="submit" type="submit"><button role="invalid button">Open</button><input type="password">');
    eq(sem.getAccessibleName(document.querySelector('#x')),'Task Name'); eq(sem.getAccessibleName(document.querySelector('#icon')),'Delete');
    eq(sem.getAccessibleName(document.querySelector('#submit')),'Submit'); eq(sem.getRole(document.querySelector('[role]')),'button'); eq(sem.getRole(document.querySelector('[type=password]')),null);
  });
  await test('UP-T040 description remains separate from locator name', () => {
    const el=fixture('<button aria-label="Save" aria-describedby="help">X</button><span hidden id="help">Writes changes</span>');
    eq(sem.getAccessibleName(el),'Save'); eq(sem.getAccessibleDescription(el),'Writes changes'); eq(sem.resolveCandidates({by:'role',value:'button',name:'Writes changes'}).length,0);
    ok(window.__CONNECTOR_SNAPSHOT__({mode:'ai'}).snapshot.includes('description="Writes changes"'));
  });
  await test('U3.3 required ARIA states preserve false and mixed', () => {
    const el=fixture('<button aria-disabled="true" aria-checked="mixed" aria-expanded="false" aria-selected="true" aria-pressed="mixed" aria-invalid="grammar" aria-required="true" aria-readonly="true" aria-busy="true" aria-modal="true">Save</button>');
    eq(sem.getAriaStates(el),{disabled:true,checked:'mixed',expanded:false,selected:true,pressed:'mixed',invalid:'grammar',required:true,readonly:true,busy:true,modal:true});
  });
  await test('UP-T041 presentation wrappers preserve accessible descendants', () => {
    const el=fixture('<section><div role="presentation"><button>Save</button></div></section>');
    eq(sem.getAccessibleChildren(el).map(x=>x.tagName),['BUTTON']); ok(window.__CONNECTOR_SNAPSHOT__({mode:'accessibility'}).snapshot.includes('Save'));
  });
  await test('UP-T042 aria-owns prevents cycles and duplicate nodes', () => {
    fixture('<div id="a" aria-owns="b b c"><span id="b" aria-owns="a">B</span></div><span id="c" aria-owns="a">C</span>');
    const visited=new Set(); const stack=[document.querySelector('#a')]; while(stack.length) {const el=stack.pop(); if(visited.has(el)) throw new Error('duplicate'); visited.add(el); stack.push(...sem.getAccessibleChildren(el,{visited}));}
    eq(visited.size,3);
  });
  await test('UP-T043 visual exposure and enabled/editable remain distinct', () => {
    fixture('<button aria-hidden="true">Visible</button><div inert><input></div><div style="display:none"><button id="hidden">Hidden</button></div>');
    const el=document.querySelector('button'); eq(sem.isAccessibilityExposed(el),false); eq(sem.isVisible(el),true); eq(sem.isEnabled(el),true);
    eq(sem.isAccessibilityExposed(document.querySelector('input')),false); eq(sem.isEnabled(document.querySelector('input')),false); eq(sem.isVisible(document.querySelector('#hidden')),false);
  });
  await test('UP-T044 UP-T045 legacy action rejects ambiguity and intersections remain strict', async () => {
    fixture('<section role="dialog" aria-label="A"><button data-id="1">Save</button><button data-id="2">Save</button></section><button data-id="2">Save</button>');
    let clicks=0; document.body.addEventListener('click',()=>clicks++,{once:true});
    const res=await window.__semanticLocate({role:'button',name:'Save',exact:true,action:'click'}); eq(res.code,'ambiguous_target'); eq(clicks,0);
    const matches=sem.resolveCandidates({scope:{by:'role',value:'dialog',name:'A'},by:'role',value:'button',name:'Save',entity:{attribute:'data-id',value:'2'}}); eq(matches.length,1); eq(matches[0].parentElement.getAttribute('aria-label'),'A');
  });
  await test('UP-T043 legacy wait uses shared inherited visibility and enabled/editable', () => {
    fixture('<div style="display:none"><input id="hidden"></div><div inert><input id="inert"></div><input id="readonly" readonly><input id="editable">');
    eq(window.__semanticWaitState(document.querySelector('#hidden'),'visible'),false);
    eq(window.__semanticWaitState(document.querySelector('#inert'),'enabled'),false);
    eq(window.__semanticWaitState(document.querySelector('#readonly'),'editable'),false);
    eq(window.__semanticWaitState(document.querySelector('#editable'),'editable'),true);
  });
  await test('UP-T049 unsupported tree boundaries fail explicitly', () => {
    fixture('<iframe></iframe><div id="host"></div>'); const shadow=document.querySelector('#host').attachShadow({mode:'open'}); shadow.innerHTML='<button>Shadow</button>';
    error(()=>sem.resolveCandidates({by:'role',value:'button',shadowDom:true}),'unsupported_feature'); error(()=>sem.resolveCandidates({by:'css',value:'button'},shadow),'unsupported_feature');
  });
  await test('UP-T050 traversal budget exits without returning incomplete candidates', () => {
    fixture('<div>'+Array.from({length:100},(_,i)=>`<button>B${i}</button>`).join('')+'</div>');
    error(()=>sem.resolveCandidates({by:'role',value:'button'},document,{maxNodes:20}),'semantic_budget_exceeded');
  });
  await test('UP-PK024 verified candidates handle CSS specials and current entity identity', () => {
    const el=fixture('<button id="a:b &quot;[]" data-testid="save&quot;]" data-id="one">Save</button>');
    const candidates=sem.locatorCandidates(el); ok(candidates.length>0); for(const c of candidates) {ok(c.verified); const found=sem.resolveCandidates(c.locator); eq(found.length,1); ok(found[0]===el); eq(c.semanticVersion,sem.version);}
    const bound=candidates.find(c=>c.locator.entity); ok(bound,'missing entity-bound candidate'); el.setAttribute('data-id','two'); eq(sem.resolveCandidates(bound.locator).length,0);
  });
  await test('UP-T073 UP-PK025 sensitive metadata and locator values never escape', () => {
    const el=fixture('<input type="password" id="password" aria-label="do-not-emit" data-testid="do-not-emit" value="do-not-emit">');
    const metadata=sem.describe(el); ok(!JSON.stringify(metadata).includes('do-not-emit')); eq(metadata.name,'[redacted]');
    ok(!JSON.stringify(sem.locatorCandidates(el)).includes('do-not-emit')); eq(sem.getAccessibleName(el),'do-not-emit');
  });
  await test('UP-PK025 UTF-8 output bounds, sensitive references and huge text remain safe', () => {
    let el=fixture('<button aria-label="'+ '😀'.repeat(200) + '">Text</button>');
    let metadata=sem.describe(el);ok(metadata.truncated);eq(new TextEncoder().encode(metadata.name).length,512);ok(!metadata.name.includes('\ufffd'));
    el=fixture('<button aria-labelledby="private">Safe</button><span data-sensitive="true" id="private">private-value</span>');
    ok(!JSON.stringify(sem.describe(el)).includes('private-value'));eq(sem.locatorCandidates(el).length,0);
    el=fixture('<button>'+ 'X'.repeat(70000) + '</button>');metadata=sem.describe(el);ok(metadata.truncated);eq(metadata.sources.name,'unavailable');ok(JSON.stringify(metadata).length<2000);
  });
  await test('UP-PK025 nested and chained sensitive naming references are redacted cycle-safely', () => {
    fixture('<span id="label"><span data-sensitive="true">nested-private-value</span></span><button id="save" aria-labelledby="label">Save</button>');
    let el=document.querySelector('button');ok(sem.isSensitive(el));ok(!JSON.stringify(sem.describe(el)).includes('nested-private-value'));ok(!JSON.stringify(sem.locatorCandidates(el)).includes('nested-private-value'));
    fixture('<span id="one" aria-labelledby="two">one</span><span id="two" aria-labelledby="one secret">two</span><span id="secret" data-sensitive="true">chained-private-value</span><button aria-labelledby="one">Save</button>');
    el=document.querySelector('button');ok(sem.isSensitive(el));eq(sem.locatorCandidates(el).length,0);ok(!JSON.stringify(sem.describe(el)).includes('chained-private-value'));
  });
  await test('UP-T042 snapshot ownership cycles terminate and count each node once', () => {
    fixture('<div id="a" aria-owns="b b"><button id="b" aria-owns="a c c">B</button></div><button id="c" aria-owns="b">C</button>');
    const snap=window.__CONNECTOR_SNAPSHOT__({mode:'ai'});eq(snap.meta.elementCount,4);eq(Object.values(snap.refs).filter(x=>x.selector==='#b').length,1);eq(Object.values(snap.refs).filter(x=>x.selector==='#c').length,1);
  });
  await test('UP-T071 picker-owned UI excluded from semantic and snapshot output', () => {
    fixture('<button>Business</button><div data-connector-picker-ui data-connector-owned="picker"><button>Picker secret UI</button></div>');
    eq(sem.resolveCandidates({by:'role',value:'button'}).length,1);ok(!window.__CONNECTOR_SNAPSHOT__({mode:'structure'}).snapshot.includes('Picker secret UI'));ok(!window.__CONNECTOR_SNAPSHOT__({mode:'ai'}).snapshot.includes('Picker secret UI'));
  });
  await test('UP-T046 modern refs reject reused entity, replaced node, page and semantic changes', () => {
    let el=fixture('<button data-id="one">Save</button>');
    const stamped=sem.rememberRef(el);ok(sem.resolveRef(stamped.identity,stamped)===el);
    el.setAttribute('data-id','two');eq(sem.resolveRef(stamped.identity,stamped),null);
    const next=sem.rememberRef(el);el.outerHTML='<button data-id="two">Save</button>';eq(sem.resolveRef(next.identity,next),null);
    el=document.querySelector('button');const current=sem.rememberRef(el);eq(sem.resolveRef(current.identity,{...current,pageEpoch:'stale'}),null);eq(sem.resolveRef(current.identity,{...current,semanticVersion:'stale'}),null);
    sem.clearRefs();eq(sem.resolveRef(current.identity,current),null);
  });
  await test('UP-T048 AI budget split, refs, React, portal and overlays preserved', () => {
    fixture('<section>'+ Array.from({length:20},(_,i)=>`<button class="distinct-${i}">Background ${i}</button>`).join('')+'</section><section role="dialog" aria-label="Editor" aria-modal="true" style="position:fixed;left:0;top:0;width:300px;height:200px;z-index:100;background:white"><button id="react" aria-controls="portal">Focused Save</button></section><div id="portal" role="listbox"><div role="option">Option</div></div>');
    const el=document.querySelector('#react'); function SaveButton(){} el.__reactFiber$fixture={type:SaveButton,return:null}; el.focus();
    const snap=window.__CONNECTOR_SNAPSHOT__({mode:'ai',maxTokens:150}); ok(snap.meta.split,'budget did not split'); ok(snap.subtrees.length>0,'missing subtrees'); ok(snap.meta.overlays.length>0,'missing overlay '+JSON.stringify(snap)); ok(snap.snapshot.includes('# overlays:'),'missing overlay header'); ok(JSON.stringify(snap).includes('component=SaveButton'),'missing React '+JSON.stringify(snap)); ok(snap.meta.portalCount>0,'missing portal'); ok(Object.keys(snap.refs).length>0,'missing inline refs');
  });
  await test('text locator picks the deepest match and normalises NBSP', () => {
    fixture('<main><div class="card"><span id="label">Save&nbsp;draft</span></div><p hidden>Save draft</p></main>');
    const t=window.__connectorText; eq(t.find('Save draft').id,'label'); eq(t.find('Save  draft').id,'label'); eq(t.find('missing'),null);
  });
  await test('wait text sees NBSP, hidden-from-innerText text, input values and same-origin iframes', async () => {
    fixture('<div>Total:&nbsp;42</div><input id="q"><iframe id="f"></iframe><div style="visibility:hidden">Offscreen copy</div>');
    const t=window.__connectorText; ok(t.has('Total: 42'),'nbsp'); ok(t.has('Offscreen copy'),'textContent fallback'); ok(!t.has('Nope'));
    document.querySelector('#q').value='typed value'; ok(t.has('typed value'),'input value');
    const frame=document.querySelector('#f'); frame.contentDocument.body.innerHTML='<p>Inside frame</p>'; ok(t.has('Inside frame'),'iframe');
    fixture('<script>var secret = "Script body";</script>'); ok(!t.has('Script body'),'script text ignored');
  });
  await test('click dispatches the full pointer sequence at the hit element and reports cover', () => {
    fixture('<div id="menu" style="width:100px;height:40px"><span id="inner" style="display:block;height:100%">Open</span></div>');
    const seen=[]; for (const type of ['pointerdown','mousedown','pointerup','mouseup','click']) document.querySelector('#menu').addEventListener(type,e=>seen.push(type+':'+e.target.id));
    const r=window.__connectorClick(document.querySelector('#menu'));
    eq(seen,['pointerdown:inner','mousedown:inner','pointerup:inner','mouseup:inner','click:inner']); eq(r.mismatch,false); eq(r.dispatched,true);
    fixture('<button id="under" style="position:absolute;left:0;top:0;width:100px;height:40px">Under</button><div id="cover" style="position:absolute;left:0;top:0;width:200px;height:200px"></div>');
    let clicked=0; document.querySelector('#under').addEventListener('click',()=>clicked++);
    const covered=window.__connectorClick(document.querySelector('#under')); eq(covered.mismatch,true); eq(covered.hit.id,'cover'); eq(clicked,1);
    fixture('<button id="off" disabled>Off</button>'); const off=window.__connectorClick(document.querySelector('#off')); eq(off.code,'not_actionable'); eq(off.dispatched,false);
  });
  await test('oversized wrappers split recursively instead of spilling the whole page', () => {
    fixture('<div id="app"><div class="shell"><nav>'+Array.from({length:4},(_,i)=>`<button class="nav-${i}">Nav ${i}</button>`).join('')+'</nav><main>'+Array.from({length:60},(_,i)=>`<button class="item-${i}">Item number ${i}</button>`).join('')+'</main></div></div>');
    const snap=window.__CONNECTOR_SNAPSHOT__({mode:'ai',maxTokens:300});
    ok(snap.meta.split,'did not split'); ok(snap.snapshot.includes('Nav 0'),'nav not inline: '+snap.snapshot);
    ok(Object.keys(snap.refs).length>=4,'too few inline refs '+Object.keys(snap.refs).length);
    ok(snap.subtrees.length>0,'no subtrees'); ok(Object.keys(snap.allRefs).length>=64,'allRefs incomplete');
    ok(!snap.subtrees.some(t=>t.content.includes('Nav 0')),'nav was spilled as well');
  });
  await test('budgeted snapshots render an open overlay before the background page', () => {
    fixture('<main>'+Array.from({length:80},(_,i)=>`<button class="bg-${i}">Background button ${i}</button>`).join('')+'</main><div role="dialog" aria-label="Confirm" aria-modal="true" style="position:fixed;left:0;top:0;width:300px;height:200px;z-index:50"><button id="ok">Confirm delete</button></div>');
    const snap=window.__CONNECTOR_SNAPSHOT__({mode:'ai',maxTokens:250});
    ok(snap.snapshot.includes('Confirm delete'),'overlay not inline: '+snap.snapshot);
    ok(snap.snapshot.includes('[overlay o1: shown above]'),'missing overlay pointer');
    ok(snap.snapshot.indexOf('Confirm delete')<snap.snapshot.indexOf('Background button'),'overlay not first');
    const small=window.__CONNECTOR_SNAPSHOT__({mode:'ai',maxTokens:100000});
    ok(!small.snapshot.includes('shown above'),'fitting output must keep DOM order');
  });
  await test('large containers are not redacted when the descendant budget runs out', () => {
    fixture('<section aria-label="Workspace" id="ws">'+'<span>x</span>'.repeat(1500)+'<input type="password"></section><button id="big">'+'<span>y</span>'.repeat(1500)+'</button>');
    ok(!sem.isSensitive(document.querySelector('#ws')),'container redacted on budget');
    ok(sem.isSensitive(document.querySelector('#big')),'name-from-content roles must stay fail-closed');
    fixture('<form aria-label="Sign in" id="s"><input type="password"></form><button id="b"><span data-sensitive="true">x</span></button><section aria-labelledby="l" id="r"></section><div id="l"><input type="password"></div>');
    ok(!sem.isSensitive(document.querySelector('#s')),'form named by aria-label redacted because of a password field');
    ok(!sem.isSensitive(document.body),'body redacted because of a password field');
    ok(sem.isSensitive(document.querySelector('#b')),'content-named button with sensitive child must be redacted');
    ok(sem.isSensitive(document.querySelector('#r')),'labelledby subtree with password must be redacted');
  });
  await test('accessibility snapshots show direct text leaves and ai mode hides them', () => {
    fixture('<div class="total">Total:&nbsp;$42</div><button>Save</button><input type="password" value="hunter2">');
    const a=window.__CONNECTOR_SNAPSHOT__({mode:'accessibility'}).snapshot;
    ok(a.includes('- text "Total: $42"'),a); ok(!a.includes('text "Save"'),'duplicate text for named button'); ok(!a.includes('hunter2'));
    ok(!window.__CONNECTOR_SNAPSHOT__({mode:'ai'}).snapshot.includes('- text '),'ai mode shows text leaves');
  });
  return {tests:results.length,failures:results.filter(x=>!x.passed).length,evidenceKind:'browser_chromium',results};
}
(async()=>{
  const semantic=await readFile(join(__dirname,'../../src/semantic/core.js'),'utf8');
  const locator=await readFile(join(__dirname,'../../js/locator.js'),'utf8');
  const bridge=await readFile(join(__dirname,'../../src/bridge.rs'),'utf8');
  const handlers=await readFile(join(__dirname,'../../src/handlers.rs'),'utf8');
  const textJs=await readFile(join(__dirname,'../../js/text.js'),'utf8');
  const clickJs=await readFile(join(__dirname,'../../js/click.js'),'utf8');
  const waitState=handlers.split('const __connectorElementState = ')[1].split('            }};')[0].replaceAll('{{','{').replaceAll('}}','}')+'}';
  const snapshot=bridge.split('// === Unified Snapshot Engine ===')[1].split('// === Auto-push DOM via Tauri IPC')[0].replaceAll('{{','{').replaceAll('}}','}').replace('{semantic_source}',semantic);
  const server=createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><html><body></body></html>');});
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  let browser;
  try {
    browser=await chromium.launch({headless:true});const page=await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    const result=await page.evaluate(`(async()=>{${semantic};${snapshot};window.__semanticLocate=${locator};window.__semanticWaitState=${waitState};window.__connectorText=${textJs};window.__connectorClick=${clickJs};return (${corpus.toString()})();})()`);
    console.log(JSON.stringify({...result,browser:browser.version()},null,2));if(result.failures)process.exitCode=1;
  } finally {await browser?.close();await new Promise(r=>server.close(r));}
})().catch(e=>{console.error(e);process.exitCode=1;});
