// Real native WebView screenshot pixels; no mock renderer or image substitution.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {randomBytes,randomUUID} from 'node:crypto';
import {mkdirSync,mkdtempSync,openSync,closeSync,writeFileSync,readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {inflateSync} from 'node:zlib';
import {setTimeout as delay} from 'node:timers/promises';
import net from 'node:net';

const root=fileURLToPath(new URL('../../../',import.meta.url));
const output=process.env.CONNECTOR_FIXTURE_OUTPUT||mkdtempSync(resolve(tmpdir(),'connector-screenshot-native-'));
mkdirSync(output,{recursive:true,mode:0o700});
const token=randomBytes(32).toString('hex');
const storePath=resolve(output,'independent-native-store.json');
const binary=process.env.CONNECTOR_FIXTURE_BINARY||resolve(root,`target/debug/connector-workflow-fixture${process.platform==='win32'?'.exe':''}`);
const result={platform:process.platform,layer:'native_webview',source:'webview_native',tests:[]};
let child;let socket;const pending=new Map();

function rgbaPng(bytes) {
  assert.equal(bytes.subarray(0,8).toString('hex'),'89504e470d0a1a0a');
  let width,height,bpp;const chunks=[];
  for(let offset=8;offset+12<=bytes.length;) {
    const size=bytes.readUInt32BE(offset),type=bytes.toString('ascii',offset+4,offset+8),data=bytes.subarray(offset+8,offset+8+size);
    if(type==='IHDR'){width=data.readUInt32BE(0);height=data.readUInt32BE(4);assert.equal(data[8],8);assert([2,6].includes(data[9]));assert.equal(data[12],0);bpp=data[9]===6?4:3;}
    if(type==='IDAT')chunks.push(data);offset+=size+12;
  }
  assert(width*height<=16777216);const raw=inflateSync(Buffer.concat(chunks),{maxOutputLength:67108864});
  const stride=width*bpp,data=Buffer.alloc(stride*height);assert.equal(raw.length,(stride+1)*height);
  const paeth=(a,b,c)=>{const p=a+b-c,pa=Math.abs(p-a),pb=Math.abs(p-b),pc=Math.abs(p-c);return pa<=pb&&pa<=pc?a:pb<=pc?b:c;};
  for(let y=0;y<height;y++) {
    const filter=raw[y*(stride+1)];assert(filter<=4);
    for(let x=0;x<stride;x++) {
      const a=x>=bpp?data[y*stride+x-bpp]:0,b=y?data[(y-1)*stride+x]:0,c=y&&x>=bpp?data[(y-1)*stride+x-bpp]:0;
      const predict=[0,a,b,Math.floor((a+b)/2),paeth(a,b,c)][filter];
      data[y*stride+x]=(raw[y*(stride+1)+1+x]+predict)&255;
    }
  }
  return {width,height,pixel(x,y){const offset=Math.floor(y)*stride+Math.floor(x)*bpp;return [...data.subarray(offset,offset+3),bpp===4?data[offset+3]:255];}};
}
async function occupied(port){return new Promise(resolveResult=>{const s=net.createConnection({host:'127.0.0.1',port});s.once('connect',()=>{s.destroy();resolveResult(true);});s.once('error',()=>resolveResult(false));});}
function request(command) {return new Promise((resolveReply,reject)=>{const id=randomUUID();const timer=setTimeout(()=>{pending.delete(id);reject(new Error('screenshot fixture RPC timeout'));},15000);pending.set(id,{resolve:resolveReply,timer});socket.send(JSON.stringify({id,...command}));});}
async function value(command){const reply=await request(command);if(reply.error!==undefined)throw new Error(JSON.stringify(reply.error));return reply.result;}
const js=script=>value({type:'execute_js',window_id:'main',script});
const inspect=(operation,args={})=>value({type:'inspection',operation,args:{...args,authToken:token}});
const screenshot=async args=>{
  // Explicit fixture preparation is outside the operation under test. This
  // keeps WK's render loop active when the developer changes foreground apps.
  await js("window.__TAURI_INTERNALS__.invoke('fixture_window')");
  await delay(80);
  return inspect('webview_screenshot',{source:'webview_native',redaction:'required',includeImage:true,...args});
};
async function test(ids,name,fn){const start=performance.now();const details=await fn();result.tests.push({ids,name,passed:true,durationMs:performance.now()-start,...details});console.log('PASS '+name);}
function map(geometry,x,y){const[a,b,c,d,e,f]=geometry.cssToImage;return [Math.floor(a*x+c*y+e),Math.floor(b*x+d*y+f)];}
const summarize=shot=>({...shot,base64:undefined});

try {
  for(const port of [19555,19556])assert.equal(await occupied(port),false,'refusing occupied fixture ports');
  const log=openSync(resolve(output,'private-app.log'),'w',0o600);
  child=spawn(binary,[],{cwd:root,env:{...process.env,TAURI_CONNECTOR_WORKFLOW_TOKEN:token,CONNECTOR_FIXTURE_ID:`dev.connector.workflow-fixture.${randomUUID()}`,CONNECTOR_FIXTURE_EVIDENCE:storePath},stdio:['ignore',log,log]});closeSync(log);
  for(let n=0;n<100;n++){try{socket=new WebSocket('ws://127.0.0.1:19555');await new Promise((ok,no)=>{socket.addEventListener('open',ok,{once:true});socket.addEventListener('error',no,{once:true});});break;}catch{await delay(100);}}
  assert.equal(socket?.readyState,1);
  socket.addEventListener('message',event=>{const reply=JSON.parse(String(event.data)),waiter=pending.get(reply.id);if(waiter){clearTimeout(waiter.timer);pending.delete(reply.id);waiter.resolve(reply);}});
  for(let n=0;n<60;n++){try{if(await js("!!document.querySelector('#pick-password')"))break;}catch{}await delay(100);}
  for(let n=0;n<60;n++) {
    const health=await inspect('runtime_health',{windowId:'main',timeoutMs:2000});
    if(health.checks?.bridge?.connected && await js("document.readyState==='complete'"))break;
    await delay(100);
  }
  // Harness preparation is explicit and precedes captures; screenshot service
  // itself remains passive. No business button is clicked by this harness.
  await js("window.__TAURI_INTERNALS__.invoke('fixture_window')");
  const beforeStore=JSON.parse(readFileSync(storePath,'utf8'));
  const calibration=await js(`(()=>{
    const colors=[[17,231,59],[221,31,211],[51,97,227],[239,173,41]];
    const points=[[20,20],[innerWidth-28,20],[20,innerHeight-28],[innerWidth-28,innerHeight-28]];
    points.forEach(([x,y],i)=>{const el=document.createElement('div');el.id='screenshot-marker-'+i;el.style.cssText='position:fixed;z-index:10000;width:8px;height:8px;left:'+x+'px;top:'+y+'px;background:rgb('+colors[i].join(',')+')';document.body.append(el);});
    const clipped=document.createElement('div');clipped.id='screenshot-clipped';clipped.style.cssText='position:fixed;z-index:10000;left:-10px;top:210px;width:30px;height:20px;background:rgb(47,173,229)';document.body.append(clipped);
    const p=document.querySelector('#pick-password').getBoundingClientRect();
    return {points,colors,password:{x:p.x,y:p.y,width:p.width,height:p.height},width:innerWidth,height:innerHeight,dpr:devicePixelRatio,focused:document.hasFocus(),scroll:[scrollX,scrollY]};
  })()`);
  let full;
  await test(['UP-T051','UP-T056','UP-T060','UP-T062'],'native viewport corner markers and password mask match the declared affine mapping',async()=>{
    full=await screenshot({});assert.equal(full.captureSource,'webview_native');assert.deepEqual(full.fallbacks,[]);assert.equal(full.redaction.status,'applied');
    const image=rgbaPng(Buffer.from(full.base64,'base64'));
    assert.deepEqual([image.width,image.height],[full.image.widthPx,full.image.heightPx]);
    for(let i=0;i<calibration.points.length;i++){const[x,y]=calibration.points[i],point=map(full.geometry,x+4,y+4);assert.deepEqual(image.pixel(...point),[...calibration.colors[i],255],`marker ${i} pixel ${point}`);}
    const p=calibration.password;
    for(const fx of [.1,.5,.9])for(const fy of [.1,.5,.9])assert.deepEqual(image.pixel(...map(full.geometry,p.x+p.width*fx,p.y+p.height*fy)),[0,0,0,255]);
    assert.deepEqual(await js('({focused:document.hasFocus(),scroll:[scrollX,scrollY]})'),{focused:calibration.focused,scroll:calibration.scroll});
    writeFileSync(resolve(output,'masked-viewport.png'),Buffer.from(full.base64,'base64'),{mode:0o600});
    result.full=summarize(full);return{dimensions:[image.width,image.height],cssViewport:[calibration.width,calibration.height],cssToImage:full.geometry.cssToImage,markers:4,maskedPasswordSamples:9};
  });
  await test(['UP-T057','UP-PK026'],'native element crops preserve rendering and clip negative CSS origin',async()=>{
    const save=await screenshot({selector:'#pick-save'}),rendered=rgbaPng(Buffer.from(save.base64,'base64'));
    const colors=new Set();for(let y=0;y<rendered.height;y++)for(let x=0;x<rendered.width;x++)colors.add(rendered.pixel(x,y).join(','));
    assert(colors.size>8,'button crop must contain rendered button/text pixels');
    const secret=await screenshot({selector:'#pick-password'}),masked=rgbaPng(Buffer.from(secret.base64,'base64'));
    for(let y=0;y<masked.height;y++)for(let x=0;x<masked.width;x++)assert.deepEqual(masked.pixel(x,y),[0,0,0,255]);
    const clipped=await screenshot({selector:'#screenshot-clipped'}),image=rgbaPng(Buffer.from(clipped.base64,'base64'));
    assert.equal(clipped.geometry.clipped,true);assert.deepEqual(image.pixel(image.width/2,image.height/2),[47,173,229,255]);
    writeFileSync(resolve(output,'rendered-button.png'),Buffer.from(save.base64,'base64'),{mode:0o600});
    result.crops={button:summarize(save),password:summarize(secret),clipped:summarize(clipped)};return{button:[rendered.width,rendered.height],buttonColors:colors.size,password:[masked.width,masked.height],clipped:[image.width,image.height]};
  });
  await test(['UP-T065','UP-T066','UP-PK028','UP-PK035'],'protected reads keep capture identity and source failures do not fallback',async()=>{
    const read=await inspect('artifact_read',{artifactId:full.artifactId,includeImage:true});assert.equal(read.base64,full.base64);assert.equal(read.capturedAt,full.capturedAt);
    const denied=await request({type:'inspection',operation:'artifact_read',args:{artifactId:full.artifactId}});assert(denied.error);
    const unavailable=await request({type:'inspection',operation:'webview_screenshot',args:{source:'dom_rendering',authToken:token}});assert(unavailable.error);
    const saved=await screenshot({selector:'#pick-save',save:true,includeImage:false});assert.equal(saved.saveWarning.code,'protected_disk_storage_unavailable');assert.equal(saved.captureSource,'webview_native');
    const compare=await inspect('artifact_compare',{baselineId:full.artifactId,currentId:saved.artifactId});assert.equal(compare.comparable,false);assert.equal(compare.equal,null);
    return{artifactReadStable:true,anonymousReadDenied:true,explicitDomFailed:true,saveWarning:saved.saveWarning.code,comparison:compare};
  });
  assert.deepEqual(JSON.parse(readFileSync(storePath,'utf8')),beforeStore);
  result.businessEffectsChanged=false;result.passed=true;
} catch(error){result.passed=false;result.error=String(error.stack||error);console.error(result.error);process.exitCode=1;}
finally {
  for(const waiter of pending.values())clearTimeout(waiter.timer);socket?.close();
  if(child){child.kill('SIGTERM');await Promise.race([new Promise(ok=>child.once('exit',ok)),delay(3000)]);if(child.exitCode===null&&child.signalCode===null)child.kill('SIGKILL');}
  writeFileSync(resolve(output,'screenshot-native-results.json'),JSON.stringify(result,null,2),{mode:0o600});console.log(`Evidence: ${resolve(output,'screenshot-native-results.json')}`);
}
