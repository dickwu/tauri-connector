import assert from 'node:assert/strict';
import {test} from 'node:test';
import {deflateSync} from 'node:zlib';
import {existsSync,mkdtempSync,readFileSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import vm from 'node:vm';
import {decodeRgbaPng, verifyNativePixels, assertFrontendBoot, assertEscapeDelivered, verifyPickerSurvivesStall} from './native-evidence.mjs';

// Synthetic PNGs only test the decoder/validator; they are never native evidence.
function png(width,height,pixels,{filters=[0],colorType=6,rawOverride}={}) {
  const stride=width*4,raw=Buffer.alloc((stride+1)*height);
  const paeth=(a,b,c)=>{const p=a+b-c,pa=Math.abs(p-a),pb=Math.abs(p-b),pc=Math.abs(p-c);return pa<=pb&&pa<=pc?a:pb<=pc?b:c;};
  for(let y=0;y<height;y++)for(let x=0;x<stride;x++) {
    const f=filters[y%filters.length],i=y*stride+x,a=x>=4?pixels[i-4]:0,b=y?pixels[i-stride]:0,c=y&&x>=4?pixels[i-stride-4]:0;
    raw[y*(stride+1)]=f;raw[y*(stride+1)+1+x]=(pixels[i]-[0,a,b,Math.floor((a+b)/2),paeth(a,b,c)][f])&255;
  }
  const chunk=(type,data)=>{const b=Buffer.alloc(data.length+12);b.writeUInt32BE(data.length);b.write(type,4);data.copy(b,8);let crc=0xffffffff;for(const byte of b.subarray(4,-4)){crc^=byte;for(let n=0;n<8;n++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}b.writeUInt32BE((crc^0xffffffff)>>>0,b.length-4);return b;};
  const header=Buffer.alloc(13);header.writeUInt32BE(width);header.writeUInt32BE(height,4);header[8]=8;header[9]=colorType;
  return Buffer.concat([Buffer.from('89504e470d0a1a0a','hex'),chunk('IHDR',header),chunk('IDAT',deflateSync(rawOverride||raw)),chunk('IEND',Buffer.alloc(0))]);
}

test('RGBA decoder reconstructs all five PNG filters and bounds pixel reads',()=>{
  const pixels=Buffer.from(Array.from({length:7*5*4},(_,i)=>(i*97+31)%256));
  const decoded=decodeRgbaPng(png(7,5,pixels,{filters:[0,1,2,3,4]}));
  for(let y=0;y<5;y++)for(let x=0;x<7;x++)assert.deepEqual(decoded.pixel(x,y),[...pixels.subarray((y*7+x)*4,(y*7+x+1)*4)]);
  for(const p of [[-1,0],[7,0],[0,5],[NaN,0]])assert.throws(()=>decoded.pixel(...p),/outside/);
});

test('PNG decoder rejects truncated, corrupt, unsupported and oversized input',()=>{
  const good=png(2,2,Buffer.alloc(16));
  assert.throws(()=>decodeRgbaPng(good.subarray(0,-1)),/truncated/);
  const corrupt=Buffer.from(good);corrupt[29]^=1;assert.throws(()=>decodeRgbaPng(corrupt),/CRC/);
  assert.throws(()=>decodeRgbaPng(png(2,2,Buffer.alloc(16),{colorType:2})),/RGBA8/);
  assert.throws(()=>decodeRgbaPng(png(2,2,Buffer.alloc(16),{rawOverride:Buffer.alloc(19)})),/scanline/);
  assert.throws(()=>decodeRgbaPng(png(2,2,Buffer.alloc(16),{rawOverride:Buffer.from([5,...Array(17).fill(0)])})),/filter/);
  const oversized=png(1,1,Buffer.alloc(4));oversized.writeUInt32BE(0xffffffff,16);
  // Repair the header CRC to reach the dimension budget guard.
  let crc=0xffffffff;for(const byte of oversized.subarray(12,29)){crc^=byte;for(let n=0;n<8;n++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}oversized.writeUInt32BE((crc^0xffffffff)>>>0,29);
  assert.throws(()=>decodeRgbaPng(oversized),/dimensions/);
});

function syntheticShot({scale=1,mask=true,marker=true}={}) {
  const width=600*scale,height=400*scale,pixels=Buffer.alloc(width*height*4,255);
  const fill=(x,y,w,h,color)=>{for(let py=y*scale;py<(y+h)*scale;py++)for(let px=x*scale;px<(x+w)*scale;px++)Buffer.from(color).copy(pixels,(py*width+px)*4);};
  const points=[[20,20],[572,20],[20,372],[572,372]],colors=[[17,231,59,255],[221,31,211,255],[51,97,227,255],[239,173,41,255]];
  for(let i=0;i<4;i++)if(marker||i!==3)fill(...points[i],8,8,colors[i]);
  if(mask)fill(200,20,180,24,[0,0,0,255]);
  return {captureSource:'webview_native',fallbacks:[],redaction:{status:'applied',policy:'required',maskedRegions:1,uncoveredReasons:[]},image:{widthPx:width,heightPx:height,mimeType:'image/png'},viewport:{widthCss:600,heightCss:400},geometry:{coordinateSpace:'layout_viewport_css',clipped:false,cssToImage:[scale,0,0,scale,0,0]},base64:png(width,height,pixels).toString('base64')};
}
test('native pixel proof verifies scaled corner geometry and password mask without retaining pixels',()=>{
  for(const scale of [1,2]) {
    const result=verifyNativePixels(syntheticShot({scale}));
    assert.equal(result.cornerMarkers,4);assert.equal(result.maskedPasswordSamples,9);assert.match(result.pngSha256,/^[0-9a-f]{64}$/);
    assert.equal(JSON.stringify(result).includes('base64'),false);
  }
});
test('native pixel proof rejects unmasked images, false geometry, wrong source and metadata-only success',()=>{
  assert.throws(()=>verifyNativePixels(syntheticShot({mask:false})),/password mask/);
  assert.throws(()=>verifyNativePixels(syntheticShot({marker:false})),/corner marker/);
  for(const change of [{captureSource:'dom_rendering'},{base64:undefined},{redaction:{status:'applied',policy:'optional'}},{geometry:{coordinateSpace:'layout_viewport_css',clipped:false,cssToImage:[1,0,0,1,0,5]}},{image:{widthPx:1,heightPx:1,mimeType:'image/png'}}])assert.throws(()=>verifyNativePixels({...syntheticShot(),...change}));
});
test('feature-off boot requires frontend readiness, the same PID, disabled feature and no hooks',()=>{
  const boot={processId:321,windowId:'main',connectorFeature:false,documentReadyState:'complete',fixtureReady:true,connectorGlobals:[],hookMarkers:[]};
  assert.doesNotThrow(()=>assertFrontendBoot(boot,321));
  for(const change of [{processId:322},{connectorFeature:true},{fixtureReady:false},{documentReadyState:'loading'},{connectorGlobals:['__CONNECTOR_BRIDGE__']},{hookMarkers:['history.pushState.__connectorWrapped']},{hookMarkers:null}])assert.throws(()=>assertFrontendBoot({...boot,...change},321));
});
test('Escape sanity requires independent keydown and keyup effects with no other store mutation',()=>{
  const before={saveCalls:0,tasks:[],inputEffects:{click:1}};
  assert.doesNotThrow(()=>assertEscapeDelivered(before,{...before,inputEffects:{click:1,escapeKeydown:1,escapeKeyup:1}}));
  for(const after of [{...before,inputEffects:{click:1,escapeKeydown:1}},{...before,inputEffects:{click:1,escapeKeyup:1}},{...before,saveCalls:1,inputEffects:{click:1,escapeKeydown:1,escapeKeyup:1}}])assert.throws(()=>assertEscapeDelivered(before,after));
});
test('fixture app independently forwards both Escape events through its normal Rust command',()=>{
  const html=readFileSync(new URL('../frontend/index.html',import.meta.url),'utf8');
  const script=html.match(/<script id="fixture-input">([\s\S]*?)<\/script>/)?.[1];assert.ok(script,'fixture input script missing');
  const listeners={},calls=[];
  vm.runInNewContext(script,{window:{__TAURI_INTERNALS__:{invoke:(command,args)=>calls.push({command,args})}},document:{querySelector:()=>({addEventListener(){}}),addEventListener:(name,handler)=>{listeners[name]=handler;}}});
  for(const kind of ['keydown','keyup']){assert.equal(typeof listeners[kind],'function');listeners[kind]({key:'a'});listeners[kind]({key:'Escape'});}
  assert.deepEqual(calls.map(call=>[call.command,call.args.kind]),[['fixture_record_input','escapeKeydown'],['fixture_record_input','escapeKeyup']]);
});

test('frontend boot diagnostic waits for rendered content and finds non-enumerable connector hooks',async()=>{
  const html=readFileSync(new URL('../frontend/index.html',import.meta.url),'utf8');
  const script=html.match(/<script id="fixture-boot">([\s\S]*?)<\/script>/)?.[1];assert.ok(script);
  let loaded,rendered=false;const calls=[];
  const invoke=(command,args)=>calls.push({command,args});
  Object.defineProperty(invoke,'__connectorCaptureOwner',{value:'private-owner-value'});
  const window={__TAURI_INTERNALS__:{invoke},__WORKFLOW_FIXTURE__:{},addEventListener:(event,handler)=>{assert.equal(event,'load');loaded=handler;}};
  Object.defineProperty(window,'__CONNECTOR_BRIDGE__',{value:true});
  vm.runInNewContext(script,{window,history:{},console:{},document:{readyState:'complete',querySelector:()=>rendered?{}:null},setTimeout:callback=>{rendered=true;callback();}});
  await loaded();
  assert.equal(calls.length,1);assert.equal(calls[0].command,'fixture_frontend_ready');
  const diagnostic=JSON.parse(JSON.stringify(calls[0].args.diagnostic));
  assert.deepEqual(diagnostic,{documentReadyState:'complete',fixtureReady:true,connectorGlobals:['__CONNECTOR_BRIDGE__'],hookMarkers:['invoke.__connectorCaptureOwner']});
  assert.equal(JSON.stringify(diagnostic).includes('private-owner-value'),false);
});

test('fixture main-thread stall waits for normal Rust command release and reports elapsed time',async()=>{
  const html=readFileSync(new URL('../frontend/index.html',import.meta.url),'utf8');
  const script=html.match(/<script id="fixture-stall">([\s\S]*?)<\/script>/)?.[1];assert.ok(script);
  let click,release,clock=0;const calls=[];
  vm.runInNewContext(script,{document:{querySelector:()=>({addEventListener:(event,handler)=>{assert.equal(event,'click');click=handler;}})},window:{__TAURI_INTERNALS__:{invoke:(command,args)=>{calls.push({command,args});if(command==='fixture_wait_for_stall')return new Promise(resolve=>{release=resolve;});}}},performance:{now:()=>{clock+=100;return clock;}}});
  const target={disabled:false},done=click({currentTarget:target});
  assert.equal(target.disabled,true);assert.equal(clock,0);assert.equal(calls.length,1);
  release();await done;assert.equal(calls[1].command,'fixture_stall_finished');assert.ok(calls[1].args.elapsedMs>=1000);
});

test('stall harness contract releases only after awaiting state and preserves picker deadline',async()=>{
  // Synthetic reports exercise harness ordering only; never counted as native.
  const directory=mkdtempSync(resolve(tmpdir(),'connector-stall-contract-')),stallPath=resolve(directory,'stall.json'),signal=resolve(directory,'stall.signal');
  const original={pickerId:'fixture-picker',deadlineAtMs:123456,context:{pageEpoch:'same-page'},status:'awaiting_selection',revision:2};
  const selected={...original,status:'selected',revision:3,cleanup:{status:'confirmed'},screenshot:{status:'not_requested'},selection:{name:'Save'}};
  let sawAwaiting=false,clicks=0;const timer=setInterval(()=>{if(existsSync(signal)){assert.equal(sawAwaiting,true);writeFileSync(stallPath,JSON.stringify({phase:'finished',elapsedMs:1001}));}},5);
  try {
    const proof=await verifyPickerSurvivesStall({
      rpc:{pick:async args=>{if(args.action==='start'){assert.equal(args.captureScreenshot,false);assert.equal(args.timeoutMs,10000);}return clicks?selected:original;}},store:()=>({saveCalls:0,inputEffects:{}}),stallPath,
      arm:async()=>writeFileSync(stallPath,JSON.stringify({phase:'armed'})),awaiting:async report=>{assert.equal(existsSync(signal),false);sawAwaiting=true;return report;},click:async()=>{clicks++;},settled:async()=>selected
    });
    assert.equal(clicks,1);assert.equal(proof.deadlineAtMs,original.deadlineAtMs);assert.equal(proof.businessEffectsChanged,false);
  } finally {clearInterval(timer);rmSync(directory,{recursive:true,force:true});}
});
