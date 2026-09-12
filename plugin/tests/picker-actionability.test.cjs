const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const {chromium}=require('playwright');

test('UP-T037 UP-PK021 selected metadata uses strict actionability when the center is covered',async()=>{
  const browser=await chromium.launch({headless:true});
  try {
    const page=await browser.newPage();
    await page.addInitScript({content:fs.readFileSync('plugin/src/runtime/bootstrap.js','utf8')});
    await page.route('http://127.0.0.1/picker-actionability',route=>route.fulfill({body:'<!doctype html><button id="target" style="position:fixed;left:100px;top:300px;width:240px;height:50px">Partly covered</button><div style="position:fixed;left:180px;top:300px;width:170px;height:50px;z-index:500;background:white"></div>'}));
    await page.goto('http://127.0.0.1/picker-actionability');
    await page.evaluate(fs.readFileSync('plugin/src/semantic/core.js','utf8'));
    await page.evaluate(source=>{window.pick=eval(source);},fs.readFileSync('plugin/src/picker/page.js','utf8'));
    await page.evaluate(()=>{window.args={pickerId:'partial-cover',nonce:'fixture',context:{pageEpoch:window.__CONNECTOR_BOOTSTRAP__.pageEpoch}};return pick({...args,cmd:'start',timeoutMs:5000});});
    await page.mouse.click(110,325);
    const result=await page.evaluate(()=>pick({...args,cmd:'status'}));
    assert.equal(result.status,'selected');assert.equal(result.selection.tag,'button');
    assert.equal(result.selection.actionable,false,'An observable edge does not make the strict center-click actionable');
  } finally {await browser.close();}
});
