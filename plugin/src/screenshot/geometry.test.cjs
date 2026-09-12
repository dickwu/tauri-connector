// Browser contract checks; these are not claimed as native WebView evidence.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch({headless:true});
  try {
    const page = await browser.newPage({viewport:{width:800,height:600}});
    await page.route('http://127.0.0.1/**',route=>route.fulfill({contentType:'text/html',body:'<!doctype html><html><body></body></html>'}));
    await page.goto('http://127.0.0.1/');
    await page.setContent('<style>body{margin:0}input{position:absolute;left:100px;top:100px;width:120px;height:30px}</style><button id="target" data-id="a">Save</button><input id="secret-token" type="password" value="never-exfiltrate"><canvas width="20" height="20"></canvas>');
    await page.evaluate(fs.readFileSync(require.resolve('../semantic/core.js'),'utf8'));
    await page.evaluate(`window.geometryTest=${fs.readFileSync(require.resolve('./geometry.js'),'utf8')}`);
    const invoke = args => page.evaluate(args=>window.geometryTest.dispatch(args),args);
    const probe=await invoke({cmd:'probe'});
    assert.equal(probe.viewport.widthCss,800);
    assert.equal(probe.sensitiveRects.length,2);
    assert.equal(JSON.stringify(probe).includes('never-exfiltrate'),false);
    assert.equal(JSON.stringify(probe).includes('secret-token'),false);
    assert.deepEqual(probe.uncoveredReasons,[]);
    await page.evaluate(()=>{const node=document.createElement('custom-secret');node.textContent='opaque';document.body.append(node);});
    assert.deepEqual((await invoke({cmd:'probe'})).uncoveredReasons,['shadow_boundary']);
    await page.evaluate(()=>document.querySelector('custom-secret').remove());
    // A test renderer paints a known color; verify masks precede toDataURL.
    await page.evaluate(()=>{window.snapdom=async()=>({toCanvas:async()=>{
      const canvas=document.createElement('canvas');canvas.width=document.documentElement.scrollWidth;canvas.height=document.documentElement.scrollHeight;
      const ctx=canvas.getContext('2d');ctx.fillStyle='rgb(255,0,0)';ctx.fillRect(0,0,canvas.width,canvas.height);return canvas;
    }});});
    const captured=await invoke({cmd:'dom_capture'});
    const masked=await page.evaluate(async base64=>{
      const image=new Image();image.src='data:image/png;base64,'+base64;await image.decode();
      const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
      const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);return [...ctx.getImageData(110,110,1,1).data];
    },captured.base64);
    assert.deepEqual(masked,[0,0,0,255]);
    const held=await invoke({cmd:'target_begin',target:{by:'css',value:'#target'}});
    assert.deepEqual((await invoke({cmd:'target_validate',targetId:held.targetId})).rect,held.rect);
    await page.evaluate(()=>document.querySelector('#target').dataset.id='b');
    await assert.rejects(()=>invoke({cmd:'target_validate',targetId:held.targetId}),/target_changed/);
    assert.deepEqual(await invoke({cmd:'target_release',targetId:held.targetId}),{released:true});
    await page.evaluate(()=>{const ui=document.createElement('div');ui.dataset.connectorPickerUi='';ui.dataset.connectorOwned='picker';ui.textContent='Picker';document.body.append(ui);});
    assert((await invoke({cmd:'probe'})).uncoveredReasons.includes('picker_decoration_visible'));
    await page.evaluate(()=>document.querySelector('[data-connector-picker-ui]').style.visibility='hidden');
    assert(!(await invoke({cmd:'probe'})).uncoveredReasons.includes('picker_decoration_visible'));
    await page.evaluate(()=>window.geometryTest.dispose());
    console.log(JSON.stringify({status:'passed',checks:10,layer:'chromium_contract',native:false}));
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
