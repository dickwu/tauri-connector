(function installCapture(root, suppliedTransport) {
  'use strict';
  if (root.__CONNECTOR_CAPTURE__) return root.__CONNECTOR_CAPTURE__;
  const encoder = new TextEncoder();
  const owners = [];
  const transportOwners = [], pendingCallbacks = new Map();
  const invokeObject=root.__TAURI_INTERNALS__ || root.__TAURI__?.core;
  const invokeOwner=invokeObject && typeof invokeObject.invoke==='function' ? {object:invokeObject,original:invokeObject.invoke} : null;
  let hookSource='frontend_invoke_wrapper';
  const queue = [];
  const drainWaiters = new Set();
  let config = {enabled:false};
  let configurationGeneration = 0, probeSequence = 0, activeProbe = null;
  const SELF_TEST = 'plugin:connector|capture_self_test';
  const sessionLeases=new Map(), leaseHistory=new Map();let watchdogTimer=null,localExpired=false;
  let queuedBytes = 0, droppedEvents = 0, sourceSequence = 0, invocationSequence = 0;
  let timer = null, disposed = false, inFlight = false;
  const pageNonce = typeof crypto !== 'undefined' && crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + '-' + Math.random().toString(36).slice(2);
  const sensitive = /secret|password|passwd|token|cookie|auth|header|credential|session.?key|private.?key/i;
  const byteLength = value => encoder.encode(JSON.stringify(value)).length;
  const clock = () => typeof performance !== 'undefined' ? performance.now() : Date.now();
  function scheduleWatchdog(){
    if(watchdogTimer!==null)clearTimeout(watchdogTimer);watchdogTimer=null;
    if(disposed||!config.enabled||!sessionLeases.size)return;
    const next=Math.min(...[...sessionLeases.values()].map(lease=>lease.deadline));
    watchdogTimer=setTimeout(()=>{watchdogTimer=null;pruneLeases();scheduleWatchdog();},Math.max(1,next-clock()));
  }
  function updateLeasePolicy(expired=false){
    config={...config,enabled:sessionLeases.size>0,
      resultPolicy:[...sessionLeases.values()].some(s=>s.resultPolicy==='preview')?'preview':'metadata',
      argumentPolicy:[...sessionLeases.values()].some(s=>s.argumentPolicy==='preview')?'preview':'metadata'};
    if(!config.enabled){
      if(expired)localExpired=true;
      if(watchdogTimer!==null)clearTimeout(watchdogTimer);watchdogTimer=null;releaseOwned();
      if(expired){droppedEvents+=queue.length;queue.length=0;queuedBytes=0;if(timer!==null)clearTimeout(timer);timer=null;for(const notify of drainWaiters)notify();}
    }
  }
  function pruneLeases(){
    const now=clock();let changed=false;
    for(const [id,lease]of sessionLeases){if(lease.deadline<=now){sessionLeases.delete(id);changed=true;}}
    if(changed){configurationGeneration++;updateLeasePolicy(true);scheduleWatchdog();}
  }
  function configureLeases(next){
    const now=clock(),leases=next.enabled?(Array.isArray(next.sessions)?next.sessions:[{captureSessionId:next.sourceId,remainingMs:600000,resultPolicy:next.resultPolicy,argumentPolicy:next.argumentPolicy}]):[];
    sessionLeases.clear();localExpired=false;
    for(const lease of leases.slice(0,8)){
      if(typeof lease.captureSessionId!=='string'||!Number.isFinite(lease.remainingMs))continue;
      const deadline=Math.min(leaseHistory.get(lease.captureSessionId)??Infinity,now+Math.max(0,Math.min(600000,lease.remainingMs)));
      leaseHistory.set(lease.captureSessionId,deadline);
      if(deadline>now)sessionLeases.set(lease.captureSessionId,{deadline,resultPolicy:lease.resultPolicy,argumentPolicy:lease.argumentPolicy});else localExpired=true;
    }
    while(leaseHistory.size>64)leaseHistory.delete(leaseHistory.keys().next().value);
    updateLeasePolicy(localExpired);scheduleWatchdog();
  }
  function expireLeases(ids){
    const allowed=new Set(Array.isArray(ids)?ids:[]);
    let changed=false;
    for(const id of sessionLeases.keys()){if(!allowed.has(id)){sessionLeases.delete(id);changed=true;}}
    if(changed)configurationGeneration++;
    pruneLeases();updateLeasePolicy(true);scheduleWatchdog();return status();
  }
  function preview(value, policy, settings, command) {
    if (policy !== 'preview' || !settings.allowedCommands?.includes(command)) return {value:null,capture:{status:'omitted',truncated:false}};
    let redacted = false, truncated = false, unserializable = false, budget = 3500;
    const seen = new Set();
    const paths = settings.allowedPaths || [];
    function visit(v, depth, path) {
      if (budget <= 32 || depth > 4) {truncated = true; return {type:'truncated'};}
      budget -= 24;
      if (v === null || typeof v === 'boolean') return v;
      if (typeof v === 'number') return Number.isFinite(v) ? v : {type:'number',value:String(v)};
      if (typeof v === 'string') {
        if (!paths.includes(path)) {redacted = true; return {type:'string',length:v.length};}
        const chars=[]; let bytes=0; const limit=Math.max(0,Math.min(1024,budget));
        for (const c of v) {const n=encoder.encode(c).length;if(bytes+n>limit){truncated=true;break;}chars.push(c);bytes+=n;}
        budget -= bytes; return chars.join('');
      }
      if (typeof v === 'bigint') return {type:'bigint'};
      if (typeof v === 'undefined' || typeof v === 'symbol' || typeof v === 'function') return {type:typeof v};
      if (seen.has(v)) {unserializable=true;return {type:'circular'};} seen.add(v);
      try {
        if (ArrayBuffer.isView(v)) {
          const typed=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype),'byteLength').get;
          let size;try{size=typed.call(v);}catch(_){size=Object.getOwnPropertyDescriptor(DataView.prototype,'byteLength').get.call(v);}
          return {type:'binary',byteLength:size};
        }
        if (v instanceof ArrayBuffer) return {type:'ArrayBuffer',byteLength:Object.getOwnPropertyDescriptor(ArrayBuffer.prototype,'byteLength').get.call(v)};
        if (typeof Response !== 'undefined' && v instanceof Response) return {type:'Response',body:'unobserved'};
        if (typeof ReadableStream !== 'undefined' && v instanceof ReadableStream) return {type:'ReadableStream',body:'unobserved'};
      } catch (_) { /* The generic descriptor path never invokes collection getters. */ }
      try {
        if (v instanceof Map) return {type:'Map',size:Object.getOwnPropertyDescriptor(Map.prototype,'size').get.call(v)};
        if (v instanceof Set) return {type:'Set',size:Object.getOwnPropertyDescriptor(Set.prototype,'size').get.call(v)};
      } catch (_) {unserializable=true;return {type:'unserializable'};}
      if (v instanceof Error) {redacted=true;return {type:'Error',message:'[redacted]'};}
      const out=Array.isArray(v)?[]:{};
      // Enumerate at most 50 properties; do not use JSON.stringify/toJSON on business values.
      let count=0;
      for (const key in v) {
        if (!Object.prototype.hasOwnProperty.call(v,key)) continue;
        if (count++ >= 50) {truncated=true;break;}
        if (sensitive.test(key)) {redacted=true;continue;}
        const descriptor=Object.getOwnPropertyDescriptor(v,key);
        if (!descriptor || !Object.prototype.hasOwnProperty.call(descriptor,'value')) {unserializable=true;continue;}
        if (key.length>128 || key==='__proto__' || key==='constructor' || key==='prototype') {redacted=true;continue;}
        const next=path ? path+'.'+key : key;
        out[key]=visit(descriptor.value,depth+1,next);
        if (budget<=32) {truncated=true;break;}
      }
      return out;
    }
    try {
      let result=visit(value,0,'');
      if(byteLength(result)>4096){result={type:'truncated',reason:'preview_byte_limit'};truncated=true;}
      return {value:result,capture:{status:truncated?'truncated':unserializable?'unserializable':redacted?'redacted':'captured',truncated}};
    } catch (_) {return {value:{type:'unserializable'},capture:{status:'unserializable',truncated:false}};}
  }
  function send(payload, generation) {
    try {
      inFlight = true;
      const owner=invokeOwner;
      const promise=suppliedTransport ? suppliedTransport(payload) : owner?.original.call(owner.object,'plugin:connector|push_capture_events',{payload});
      const done=ack=>{if(!disposed && generation===configurationGeneration && payload.sourceId===config.sourceId && ack?.continueCapture===false){stop();droppedEvents+=queue.length;queue.length=0;queuedBytes=0;if(timer!==null)clearTimeout(timer);timer=null;}inFlight=false;for(const notify of drainWaiters)notify();if(!disposed && queue.length && timer===null)timer=setTimeout(flush,0);};
      if(promise && typeof promise.then==='function') promise.then(done,()=>{droppedEvents+=payload.events.length;done();}); else done();
    } catch (_) {inFlight=false;droppedEvents+=payload.events.length;}
  }
  function drain() {
    if(!queue.length&&!inFlight)return Promise.resolve({...status(),drainConfirmed:true});
    if(drainWaiters.size>=8)return Promise.resolve({...status(),drainConfirmed:false});
    return new Promise(resolve=>{
      let deadline=null,finished=false;
      const finish=confirmed=>{if(finished)return;finished=true;drainWaiters.delete(check);if(deadline!==null)clearTimeout(deadline);resolve({...status(),drainConfirmed:confirmed});};
      const check=()=>{if(!queue.length&&!inFlight)finish(true);else if(disposed)finish(false);};
      drainWaiters.add(check);deadline=setTimeout(()=>finish(false),1500);
      if(timer!==null)clearTimeout(timer);timer=null;flush();check();
    });
  }
  function flush() {
    timer=null;
    if(disposed || inFlight || queue.length===0)return;
    // One bounded batch per turn; telemetry is never awaited by a business invocation.
    const source=queue[0].event.sourceId,generation=queue[0].generation;let count=0;while(count<Math.min(50,queue.length)&&queue[count].event.sourceId===source&&queue[count].generation===generation)count++;
    const batch=queue.splice(0,count); for(const item of batch)queuedBytes-=item.bytes;
    const bySource=new Map();
    for(const item of batch){const key=item.event.sourceId; if(!bySource.has(key))bySource.set(key,[]);bySource.get(key).push(item.event);}
    for(const [sourceId,events] of bySource)send({sourceId,events,droppedEvents},generation);
    if(queue.length && !inFlight && timer===null)timer=setTimeout(flush,0);
  }
  function enqueue(event) {
    try {
      const bytes=byteLength(event);
      if(bytes>16384 || queue.length>=1000 || queuedBytes+bytes>4*1024*1024){droppedEvents++;return;}
      queue.push({event,bytes,generation:configurationGeneration});queuedBytes+=bytes;
      if(timer===null)timer=setTimeout(flush,0);
    } catch (_) {droppedEvents++;}
  }
  function record(settings, id, phase, command, wall, began, value) {
    try {
      if(command===SELF_TEST) {
        const probe=activeProbe;
        if(probe && probe.generation===configurationGeneration) {
          if(phase==='started' && value?.nonce===probe.nonce) {probe.invocationId=id;probe.started=true;}
          else if(id===probe.invocationId && phase==='succeeded' && value===probe.nonce) probe.completed=true;
        }
        return;
      }
      pruneLeases();
      if(!settings.enabled || !config.enabled || settings.sourceId!==config.sourceId || disposed)return;
      const policy=phase==='started'?config.argumentPolicy:config.resultPolicy;
      const p=preview(value,policy,config,command);
      const event={schemaVersion:2,sourceId:settings.sourceId,invocationId:id,sourceSequence:++sourceSequence,context:settings.context,phase,command:String(command).slice(0,256),wallTimeMs:wall,source:hookSource,businessPersistence:'unobserved',correlation:{kind:'none',actionId:null}};
      if(phase==='started'){event.argsPreview=p.value;event.argsCapture=p.capture;}
      else {event.durationMs=Math.max(0,clock()-began);event[phase==='failed'?'errorPreview':'resultPreview']=p.value;event.resultCapture=p.capture;}
      enqueue(event);
    } catch (_) {droppedEvents++;}
  }
  function legacy(owner, receiver, command, args, wall, began, failed) {
    if(!root.__CONNECTOR_IPC_MONITOR__ || command.startsWith('plugin:connector|'))return;
    try {
      const p=preview(args,'metadata',{},command);
      const task=owner.original.call(receiver,'plugin:connector|push_ipc_event',{payload:{command,args:p.value,timestamp:wall,durationMs:Math.max(0,clock()-began),...(failed?{error:'IPC invocation failed (details omitted)'}:{})}});
      if(task&&typeof task.catch==='function')task.catch(()=>{});
    } catch (_) {}
  }
  function install(object) {
    if(!object || typeof object.invoke!=='function')return;
    const descriptor=Object.getOwnPropertyDescriptor(object,'invoke');
    if(descriptor && !descriptor.writable && !descriptor.configurable)return;
    const owner={object,original:object.invoke,wrapper:null,descriptor};
    const wrapper=function(command) {
      if(typeof command!=='string'||(command.startsWith('plugin:connector|') && !(command===SELF_TEST && activeProbe)))return owner.original.apply(this,arguments);
      const settings=config;if(!settings.enabled&&!root.__CONNECTOR_IPC_MONITOR__)return owner.original.apply(this,arguments); const receiver=this; const args=arguments[1]; const wall=Date.now();const began=clock();
      const id=pageNonce+':'+(++invocationSequence);
      record(settings,id,'started',command,wall,began,args);
      let result;
      try {result=owner.original.apply(receiver,arguments);}
      catch(error){record(settings,id,'failed',command,wall,began,error);legacy(owner,receiver,command,args,wall,began,true);throw error;}
      const success=value=>{record(settings,id,'succeeded',command,wall,began,value);legacy(owner,receiver,command,args,wall,began,false);return value;};
      const failure=error=>{record(settings,id,'failed',command,wall,began,error);legacy(owner,receiver,command,args,wall,began,true);throw error;};
      // Native Promise observation returns the derived promise: ignored rejections remain
      // unhandled on that returned promise. The original value/reason is never replaced.
      try {return Promise.prototype.then.call(result,success,failure);} catch (_) {success(result);return result;}
    };
    owner.wrapper=wrapper; Object.defineProperty(wrapper,'__connectorCaptureOwner',{value:pageNonce});
    try{if(descriptor?.configurable)Object.defineProperty(object,'invoke',{...descriptor,value:wrapper});else object.invoke=wrapper;}catch(_){return;}
    owners.push(owner);
    root.__CONNECTOR_ORIG_INVOKE__ ||= owner.original;
  }

  function setProperty(owner,value,restore=false){
    if(owner.descriptor?.configurable)Object.defineProperty(owner.object,owner.key,restore?owner.descriptor:{...owner.descriptor,value});
    else owner.object[owner.key]=value;
  }
  function restoreCall(call){
    pendingCallbacks.delete(call.key);
    for(const item of call.callbacks){if(Map.prototype.get.call(call.map,item.id)===item.wrapper)Map.prototype.set.call(call.map,item.id,item.original);}
  }
  function transportCall(command,okId,errorId,payload){
    pruneLeases();
    if((!config.enabled&&!root.__CONNECTOR_IPC_MONITOR__)||typeof command!=='string'||(command.startsWith('plugin:connector|') && !(command===SELF_TEST && activeProbe)))return null;
    const map=root.__TAURI_INTERNALS__?.callbacks;
    if(!map||!Number.isInteger(okId)||!Number.isInteger(errorId))return null;
    const key=okId+':'+errorId;
    if(pendingCallbacks.has(key))return pendingCallbacks.get(key);
    const good=Map.prototype.get.call(map,okId),bad=Map.prototype.get.call(map,errorId);
    if(typeof good!=='function'||typeof bad!=='function')return null;
    if(pendingCallbacks.size>=1000){const oldest=pendingCallbacks.values().next().value;restoreCall(oldest);droppedEvents++;}
    const settings=config,wall=Date.now(),began=clock(),id=pageNonce+':'+(++invocationSequence);
    const call={key,map,callbacks:[],settings,wall,began,id,command};
    for(const [callbackId,original,phase] of [[okId,good,'succeeded'],[errorId,bad,'failed']]){
      const wrapped=function(){
        restoreCall(call);
        record(settings,id,phase,command,wall,began,arguments[0]);
        legacy(invokeOwner,invokeOwner.object,command,null,wall,began,phase==='failed');
        return original.apply(this,arguments);
      };
      call.callbacks.push({id:callbackId,original,wrapper:wrapped});
      Map.prototype.set.call(map,callbackId,wrapped);
    }
    pendingCallbacks.set(key,call);record(settings,id,'started',command,wall,began,payload);return call;
  }
  function observeProperty(object,key,inspect){
    if(!object||typeof object[key]!=='function')return;
    const descriptor=Object.getOwnPropertyDescriptor(object,key);
    if(descriptor&&!descriptor.writable&&!descriptor.configurable)return;
    const owner={object,key,descriptor,original:object[key],wrapper:null};
    owner.wrapper=function(){
      let call=null;
      try{call=inspect(arguments);}catch(_){droppedEvents++;}
      try{return owner.original.apply(this,arguments);}catch(error){
        if(call){restoreCall(call);record(call.settings,call.id,'failed',call.command,call.wall,call.began,error);}throw error;
      }
    };
    try{setProperty(owner,owner.wrapper);transportOwners.push(owner);}catch(_){}
  }
  function installTransport(){
    if(!root.__TAURI_INTERNALS__?.callbacks)return;
    hookSource='frontend_tauri_transport_callbacks';
    observeProperty(root,'fetch',args=>{
      if(!config.enabled&&!root.__CONNECTOR_IPC_MONITOR__)return null;
      if(typeof args[0]!=='string'||!args[1]||args[1].method!=='POST')return null;
      const url=new URL(args[0]);
      if(!((url.protocol==='ipc:'&&url.hostname==='localhost')||(['http:','https:'].includes(url.protocol)&&url.hostname==='ipc.localhost')))return null;
      const headers=args[1].headers;if(!headers||typeof headers.get!=='function')return null;
      const ok=headers.get('Tauri-Callback'),bad=headers.get('Tauri-Error');if(ok===null||bad===null)return null;
      const command=decodeURIComponent(url.pathname.slice(1));
      let payload=null;
      if((config.argumentPolicy==='preview'||command===SELF_TEST)&&typeof args[1].body==='string'&&args[1].body.length<=16384){try{payload=JSON.parse(args[1].body);}catch(_){}}
      return transportCall(command,Number(ok),Number(bad),payload);
    });
    observeProperty(root.ipc,'postMessage',args=>{
      if((!config.enabled&&!root.__CONNECTOR_IPC_MONITOR__)||typeof args[0]!=='string')return null;
      // postMessage envelopes are bounded before parsing, and never retained or sent.
      if(args[0].length>65536){droppedEvents++;return null;}
      const message=JSON.parse(args[0]);
      return transportCall(message.cmd,Number(message.callback),Number(message.error),message.payload);
    });
  }
  install(root.__TAURI_INTERNALS__ || root.__TAURI__?.core);
  if(!owners.length)installTransport();
  function status(){pruneLeases();return {status:localExpired&&!config.enabled?'expired':config.enabled?'active':'stopped',activeSessions:sessionLeases.size,applied:config.enabled&&!disposed,desired:!!config.enabled,context:config.context||null,sourceId:config.sourceId||null,droppedEvents,queuedEvents:queue.length,queuedBytes,coverage:{frontendInvoke:hookSource,cachedBeforeInstall:'unobserved',rustTrace:'unobserved',businessPersistence:'unobserved',promiseIdentity:hookSource==='frontend_invoke_wrapper'?'derived_promise':'preserved',selfTest:'wrapper_ownership_checked'}};}
  function ensureInstalled(){for(const owner of transportOwners){if(owner.object[owner.key]===owner.original)setProperty(owner,owner.wrapper);}for(const owner of owners){if(owner.object.invoke===owner.original){if(owner.descriptor?.configurable)Object.defineProperty(owner.object,'invoke',{...owner.descriptor,value:owner.wrapper});else owner.object.invoke=owner.wrapper;}}}
  function releaseOwned(){if(root.__CONNECTOR_IPC_MONITOR__)return;for(const call of [...pendingCallbacks.values()])restoreCall(call);for(const owner of transportOwners){if(owner.object[owner.key]===owner.wrapper)setProperty(owner,owner.original,true);}for(const owner of owners){if(owner.object.invoke===owner.wrapper){if(owner.descriptor?.configurable)Object.defineProperty(owner.object,'invoke',owner.descriptor);else owner.object.invoke=owner.original;}}}
  function stop(){configurationGeneration++;sessionLeases.clear();if(watchdogTimer!==null)clearTimeout(watchdogTimer);watchdogTimer=null;config={...config,enabled:false};releaseOwned();return status();}
  function dispose(){stop();disposed=true;for(const call of [...pendingCallbacks.values()])restoreCall(call);for(const owner of transportOwners){if(owner.object[owner.key]===owner.wrapper)setProperty(owner,owner.original,true);}for(const notify of drainWaiters)notify();if(timer!==null)clearTimeout(timer);timer=null;droppedEvents+=queue.length;queue.length=0;queuedBytes=0;for(const owner of owners){if(owner.object.invoke===owner.wrapper){if(owner.descriptor?.configurable)Object.defineProperty(owner.object,'invoke',owner.descriptor);else owner.object.invoke=owner.original;}}root.removeEventListener?.('pagehide',dispose,true);if(root.__CONNECTOR_CAPTURE__===controller)delete root.__CONNECTOR_CAPTURE__;}
  async function selfTest(){
    const owner=invokeOwner;
    if(!owner || (activeProbe && activeProbe.generation===configurationGeneration))return {selfTest:'unavailable',applied:false};
    const probe={nonce:pageNonce+':self-test:'+(++probeSequence),generation:configurationGeneration,started:false,completed:false};
    activeProbe=probe;
    let deadline=null;
    try {
      // Exercise the current public path, including a later delegating wrapper.
      // An echo through a cached original is not proof that observation works.
      const reply=await Promise.race([
        Promise.resolve(owner.object.invoke.call(owner.object,SELF_TEST,{nonce:probe.nonce})),
        new Promise(resolve=>{deadline=setTimeout(()=>resolve(null),1500);})
      ]);
      const observed=probe.generation===configurationGeneration && probe.started && probe.completed && reply===probe.nonce;
      return observed ? {...status(),selfTest:'roundtrip_confirmed'} : {selfTest:'unobserved',applied:false};
    } catch (_) {return {selfTest:'failed',applied:false};}
    finally {if(deadline!==null)clearTimeout(deadline);if(activeProbe===probe)activeProbe=null;}
  }
  const controller={
    configure(next){
      const actual=root.__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
      const epoch=root.__CONNECTOR_BOOTSTRAP__?.pageEpoch || root.__CONNECTOR_MONITOR_PAGE_EPOCH__;
      if(actual!==next.windowId || (epoch && next.context?.pageEpoch!==epoch))return {code:'target_changed',applied:false};
      if(disposed||(owners.length===0&&transportOwners.length===0))return {code:'runtime_unavailable',applied:false};
      configurationGeneration++;config={...next};configureLeases(next);if(config.enabled)ensureInstalled();else releaseOwned();return status();
    },status,stop,dispose,selfTest,drain,expireLeases,setLegacy(enabled){if(enabled)ensureInstalled();else if(!config.enabled)releaseOwned();},async dispatch(args){if(args.action==='stop')return stop();if(args.action==='status')return status();const reply=controller.configure(args);if(reply.code)return reply;if(args.drain)return drain();if(!reply.applied||!args.selfTest)return reply;return selfTest();}
  };
  root.__CONNECTOR_CAPTURE__=controller;root.addEventListener?.('pagehide',dispose,true);return controller;
})
