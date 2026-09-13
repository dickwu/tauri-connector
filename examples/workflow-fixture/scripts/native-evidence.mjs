import assert from 'node:assert/strict';
import {createHash,randomUUID} from 'node:crypto';
import {existsSync,readFileSync,writeFileSync} from 'node:fs';
import {setTimeout as delay} from 'node:timers/promises';
import {inflateSync} from 'node:zlib';

// The protected host encoder emits RGBA8, non-interlaced PNG. Fail closed on
// other formats and bounded sizes instead of accepting metadata as pixel proof.
export function decodeRgbaPng(bytes) {
  assert.ok(Buffer.isBuffer(bytes)&&bytes.length<=64*1024*1024,'PNG byte budget');
  assert.equal(bytes.subarray(0,8).toString('hex'),'89504e470d0a1a0a','PNG signature');
  let width,height,ended=false;const chunks=[];
  for(let offset=8;offset<bytes.length;) {
    assert.ok(offset+12<=bytes.length,'truncated PNG chunk');
    const size=bytes.readUInt32BE(offset),end=offset+size+12;
    assert.ok(end<=bytes.length,'truncated PNG data');
    const type=bytes.toString('ascii',offset+4,offset+8),data=bytes.subarray(offset+8,end-4);
    let crc=0xffffffff;for(const byte of bytes.subarray(offset+4,end-4)){crc^=byte;for(let n=0;n<8;n++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}
    assert.equal((crc^0xffffffff)>>>0,bytes.readUInt32BE(end-4),'PNG CRC mismatch');
    assert.ok(width!==undefined||type==='IHDR','PNG header must be first');
    if(type==='IHDR') {
      assert.ok(width===undefined&&size===13,'PNG header');
      width=data.readUInt32BE(0);height=data.readUInt32BE(4);
      assert.ok(width>0&&height>0&&width*height<=16777216,'PNG dimensions exceed budget');
      assert.deepEqual([...data.subarray(8)],[8,6,0,0,0],'PNG must be RGBA8 non-interlaced');
    } else if(type==='IDAT')chunks.push(data);
    else if(type==='IEND'){assert.equal(size,0,'PNG end chunk');assert.equal(end,bytes.length,'PNG trailing data');ended=true;}
    else assert.ok(type[0]===type[0].toLowerCase(),'Unsupported critical PNG chunk');
    offset=end;
  }
  assert.ok(ended&&chunks.length,'PNG image/end missing');
  const stride=width*4,raw=inflateSync(Buffer.concat(chunks),{maxOutputLength:(stride+1)*height+1});
  assert.equal(raw.length,(stride+1)*height,'PNG scanline length mismatch');
  const pixels=Buffer.alloc(stride*height);
  const paeth=(a,b,c)=>{const p=a+b-c,pa=Math.abs(p-a),pb=Math.abs(p-b),pc=Math.abs(p-c);return pa<=pb&&pa<=pc?a:pb<=pc?b:c;};
  for(let y=0;y<height;y++) {
    const filter=raw[y*(stride+1)];assert.ok(filter<=4,'Unsupported PNG filter');
    for(let x=0;x<stride;x++) {
      const i=y*stride+x,a=x>=4?pixels[i-4]:0,b=y?pixels[i-stride]:0,c=y&&x>=4?pixels[i-stride-4]:0;
      pixels[i]=(raw[y*(stride+1)+1+x]+[0,a,b,Math.floor((a+b)/2),paeth(a,b,c)][filter])&255;
    }
  }
  return {width,height,pixel(x,y){assert.ok(Number.isInteger(x)&&Number.isInteger(y)&&x>=0&&y>=0&&x<width&&y<height,'Pixel outside PNG');return [...pixels.subarray((y*width+x)*4,(y*width+x+1)*4)];}};
}

export function verifyNativePixels(shot) {
  assert.equal(shot.captureSource,'webview_native');assert.deepEqual(shot.fallbacks,[]);
  assert.equal(shot.redaction?.status,'applied');assert.equal(shot.redaction?.policy,'required');
  assert.ok(shot.redaction.maskedRegions>0);assert.deepEqual(shot.redaction.uncoveredReasons,[]);
  assert.equal(shot.image?.mimeType,'image/png');
  const base64=shot.base64??shot.image.base64;
  assert.ok(typeof base64==='string'&&base64.length>0&&base64.length<=90*1024*1024,'PNG image bytes required');
  const bytes=Buffer.from(base64,'base64'),image=decodeRgbaPng(bytes);
  assert.deepEqual([image.width,image.height],[shot.image.widthPx,shot.image.heightPx],'Encoded PNG dimensions must match metadata');
  const {widthCss:width,heightCss:height}=shot.viewport||{};
  assert.ok(Number.isFinite(width)&&Number.isFinite(height)&&width>=440&&height>=200,'Fixture viewport dimensions');
  assert.equal(shot.geometry?.coordinateSpace,'layout_viewport_css');assert.equal(shot.geometry.clipped,false);
  const matrix=shot.geometry.cssToImage;
  assert.ok(Array.isArray(matrix)&&matrix.length===6&&matrix.every(Number.isFinite),'Finite CSS mapping required');
  const [a,b,c,d,e,f]=matrix;
  // Affine zero offsets may serialize as -0.0; both signs represent zero.
  assert.ok([b,c,e,f].every(value=>value===0),'Full viewport mapping must have no crop or translation');
  assert.ok(a>0&&d>0&&Math.abs(a*width-image.width)<=1&&Math.abs(d*height-image.height)<=1,'CSS scale must match PNG extent');
  const pixel=(x,y)=>image.pixel(Math.floor(a*x+c*y+e),Math.floor(b*x+d*y+f));
  // Coordinates and colors are fixed by frontend/index.html, independently of
  // DOM queries or the screenshot service's own sensitive-region metadata.
  const markers=[[24,24,[17,231,59,255]],[width-24,24,[221,31,211,255]],[24,height-24,[51,97,227,255]],[width-24,height-24,[239,173,41,255]]];
  for(const [x,y,color] of markers)assert.deepEqual(pixel(x,y),color,`Fixture corner marker at ${x},${y}`);
  for(const fx of [.1,.5,.9])for(const fy of [.1,.5,.9])assert.deepEqual(pixel(200+180*fx,20+24*fy),[0,0,0,255],'Required password mask pixel');
  return {captureSource:shot.captureSource,redactionPolicy:shot.redaction.policy,dimensions:[image.width,image.height],viewportCss:[width,height],cssToImage:matrix,cornerMarkers:4,maskedPasswordSamples:9,pngSha256:createHash('sha256').update(bytes).digest('hex')};
}

export function assertFrontendBoot(boot,pid) {
  assert.equal(boot.processId,pid,'Frontend boot must originate from launched fixture PID');
  assert.equal(boot.windowId,'main');assert.equal(boot.connectorFeature,false,'Connector feature must be compiled out');
  assert.equal(boot.documentReadyState,'complete','Frontend document must finish loading');
  assert.equal(boot.fixtureReady,true,'Frontend fixture must actually render');
  assert.deepEqual(boot.connectorGlobals,[],'Feature-off frontend must have no connector globals');
  assert.deepEqual(boot.hookMarkers,[],'Feature-off frontend must have no connector hook markers');
}

export function assertEscapeDelivered(before,after) {
  const expected=structuredClone(before);
  for(const kind of ['escapeKeydown','escapeKeyup'])expected.inputEffects[kind]=(before.inputEffects[kind]||0)+1;
  assert.deepEqual(after,expected,'Native Escape must reach both Rust input counters with no other effect');
}

export async function verifyPickerSurvivesStall({rpc,store,arm,click,awaiting,settled,stallPath}) {
  const evidence=async phase=>{
    const deadline=Date.now()+5000;
    while(Date.now()<deadline) {
      if(existsSync(stallPath)){const value=JSON.parse(readFileSync(stallPath,'utf8'));if(value.phase===phase)return value;}
      await delay(25);
    }
    assert.fail(`Fixture stall evidence did not reach ${phase}`);
  };
  const before=store();await arm();await evidence('armed');
  const original=await awaiting(await rpc.pick({action:'start',requestKey:randomUUID(),timeoutMs:10000,captureScreenshot:false}));
  assert.equal(original.status,'awaiting_selection');assert.ok(Number.isFinite(original.deadlineAtMs));
  writeFileSync(stallPath.replace(/\.json$/,'.signal'),'release',{mode:0o600});
  const stalled=await evidence('finished');assert.ok(stalled.elapsedMs>=1000&&stalled.elapsedMs<=5000);
  const recovered=await rpc.pick({action:'get',pickerId:original.pickerId,waitMs:0});
  assert.equal(recovered.status,'awaiting_selection','A same-page stall must not invalidate a real active picker');
  assert.equal(recovered.pickerId,original.pickerId);assert.equal(recovered.deadlineAtMs,original.deadlineAtMs);
  assert.deepEqual(recovered.context,original.context);assert.deepEqual(store(),before);
  await click();const selected=await settled(recovered);
  assert.equal(selected.status,'selected');assert.equal(selected.cleanup.status,'confirmed');assert.equal(selected.screenshot.status,'not_requested');
  assert.equal(selected.selection.name,'Save');assert.equal(selected.deadlineAtMs,original.deadlineAtMs);
  const again=await rpc.pick({action:'get',pickerId:original.pickerId});
  assert.deepEqual(again.selection,selected.selection);assert.equal(again.revision,selected.revision);
  await delay(150);assert.deepEqual(store(),before);
  return {pickerId:original.pickerId,deadlineAtMs:original.deadlineAtMs,stallElapsedMs:stalled.elapsedMs,nativeSelectionClicks:1,businessEffectsChanged:false};
}
