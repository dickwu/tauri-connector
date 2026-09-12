(() => {
  'use strict';
  const OWNED = '[data-connector-picker-ui][data-connector-owned="picker"]';
  const MAX_NODES = 20000;
  const MAX_REGIONS = 2048;
  const targets = new Map();
  const pendingFrames = new Set();
  const rectValue = node => {const r=node.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height};};
  const signature = node => JSON.stringify({tag:node.localName,
    attrs:[...node.attributes].filter(a=>a.name==='id'||a.name==='name'||a.name==='type'||a.name==='role'||a.name.startsWith('data-')||a.name==='aria-label')
      .slice(0,128).map(a=>[a.name,a.value.slice(0,2048)]),text:(node.textContent||'').slice(0,2048)});
  async function geometry(args) {
    if (!args || !['probe','frame_ready','dom_capture','target_begin','target_validate','target_release'].includes(args.cmd)) throw new Error('invalid_arguments: geometry command');
    if (args.cmd === 'frame_ready') {
      await new Promise((resolve,reject)=>{
        let raf=0,timer=0,done=false;
        const finish=error=>{
          if(done)return;done=true;cancelAnimationFrame(raf);clearTimeout(timer);pendingFrames.delete(cancel);
          if(error)reject(new Error(error));else resolve();
        };
        const cancel=()=>finish('capture_context_changed');
        pendingFrames.add(cancel);
        timer=setTimeout(()=>finish('capture_not_ready: render_frame_timeout'),1000);
        raf=requestAnimationFrame(()=>{raf=requestAnimationFrame(()=>finish());});
      });
      return {ready:true};
    }
    if (args.cmd.startsWith('target_')) {
      for (const [key,value] of targets) if (value.until < performance.now()) {clearTimeout(value.timer);targets.delete(key);}
      if (args.cmd === 'target_release') {const held=targets.get(args.targetId);if(held)clearTimeout(held.timer);return {released:targets.delete(args.targetId)};}
      if (args.cmd === 'target_begin') {
        if (targets.size>=4) throw new Error('capture_budget_exceeded: target handles');
        const sem=window.__CONNECTOR_SEMANTIC__;
        if (!sem) throw new Error('runtime_missing: semantic resolver');
        const nodes=sem.resolveCandidates(args.target);
        if (nodes.length!==1) throw new Error(nodes.length?'ambiguous_target':'target_not_found');
        const node=nodes[0];
        if (!node.isConnected || node.closest(OWNED)) throw new Error('target_changed');
        const targetId=crypto.randomUUID();
        const rect=rectValue(node);
        const timer=setTimeout(()=>targets.delete(targetId),30000);
        targets.set(targetId,{node,target:args.target,signature:signature(node),rect,until:performance.now()+30000,timer});
        return {targetId,rect};
      }
      const held=targets.get(args.targetId);
      if (!held) throw new Error('target_changed: screenshot target expired');
      const nodes=window.__CONNECTOR_SEMANTIC__.resolveCandidates(held.target);
      if (nodes.length!==1 || nodes[0]!==held.node || !held.node.isConnected
        || signature(held.node)!==held.signature || JSON.stringify(rectValue(held.node))!==JSON.stringify(held.rect))
        throw new Error('target_changed: screenshot entity or geometry');
      return {rect:held.rect};
    }
    if (args.cmd === 'dom_capture') {
      const before=await geometry({cmd:'probe'});
      if (!before.ready || before.uncoveredReasons.length) throw new Error('redaction_unavailable');
      const library = window.snapdom;
      const capture = typeof library === 'function' ? library : library && library.snapdom;
      if (typeof capture !== 'function') throw new Error('capture_backend_unavailable: host-bundled snapdom');
      const width = window.innerWidth;
      const height = window.innerHeight;
      const dpr = window.devicePixelRatio || 1;
      const fullWidth = document.documentElement.scrollWidth;
      const fullHeight = document.documentElement.scrollHeight;
      if (fullWidth * fullHeight * dpr * dpr > 16000000 || width <= 0 || height <= 0)
        throw new Error('capture_budget_exceeded: DOM canvas extent');
      // Use the existing host-bundled renderer, never a remote helper/import.
      const snapshot = await capture(document.documentElement, {scale:dpr});
      const full = await snapshot.toCanvas();
      const sx = full.width / fullWidth;
      const sy = full.height / fullHeight;
      if (!Number.isFinite(sx) || !Number.isFinite(sy) || sx <= 0 || sy <= 0 || Math.abs(sx-sy) > 0.01)
        throw new Error('geometry_unknown: DOM rendering extent');
      const output = document.createElement('canvas');
      output.width = Math.round(width*sx);
      output.height = Math.round(height*sy);
      if (output.width*output.height > 16000000) throw new Error('capture_budget_exceeded: DOM output');
      const ctx = output.getContext('2d');
      if (!ctx) throw new Error('capture_failed: DOM canvas context');
      ctx.drawImage(full, window.scrollX*sx, window.scrollY*sy, width*sx, height*sy,
        0, 0, output.width, output.height);
      if (JSON.stringify(before)!==JSON.stringify(await geometry({cmd:'probe'}))) throw new Error('capture_context_changed');
      // Mask inside controlled canvas memory before base64 crosses the bridge.
      ctx.fillStyle='#000';
      for (const rect of before.sensitiveRects) {
        const left=Math.floor(rect.x*sx),top=Math.floor(rect.y*sy);
        ctx.fillRect(left,top,Math.ceil((rect.x+rect.width)*sx)-left,Math.ceil((rect.y+rect.height)*sy)-top);
      }
      const url = output.toDataURL('image/png');
      if (url.length > 22369628) throw new Error('capture_budget_exceeded: DOM encoded bytes');
      return {base64:url.slice(url.indexOf(',')+1)};
    }
    const vv = window.visualViewport;
    const viewport = {widthCss: window.innerWidth, heightCss: window.innerHeight,
      devicePixelRatio: window.devicePixelRatio, visualScale: vv ? vv.scale : 1,
      offsetLeft: vv ? vv.offsetLeft : 0, offsetTop: vv ? vv.offsetTop : 0};
    const sensitiveRects = [];
    const uncoveredReasons = [];
    const addReason = reason => { if (!uncoveredReasons.includes(reason)) uncoveredReasons.push(reason); };
    // Applications may declare additional viewport regions. This mechanism
    // only adds masks; it never changes the required policy or removes fields.
    const declared=window.__CONNECTOR_SENSITIVE_REGIONS__;
    if (declared !== undefined) {
      if (!Array.isArray(declared) || declared.length>256) addReason('invalid_declared_regions');
      else for (const region of declared) {
        if (!region || ![region.x,region.y,region.width,region.height].every(Number.isFinite)
          || region.width<0 || region.height<0) addReason('invalid_declared_regions');
        else sensitiveRects.push({x:region.x,y:region.y,width:region.width,height:region.height});
      }
    }
    // No field values, identifiers, names or page text leave this probe.
    const tree = document.createTreeWalker(document.documentElement, NodeFilter.SHOW_ELEMENT);
    let count = 0;
    let node = tree.currentNode;
    while (node) {
      if (++count > MAX_NODES) { addReason('node_budget'); break; }
      if (!node.closest(OWNED)) {
        const rect = node.getBoundingClientRect();
        const style = window.getComputedStyle(node);
        if (rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden') {
          const tag = node.localName;
          const fieldHint = ((node.getAttribute('name') || '') + ' ' + (node.id || '') + ' ' + (node.getAttribute('autocomplete') || '')).toLowerCase();
          const sensitive = (tag === 'input' && node.type === 'password')
            || node.hasAttribute('data-connector-sensitive') || node.getAttribute('data-sensitive') === 'true'
            || (['input','textarea','select'].includes(tag) && /password|secret|token|api.?key|credit.?card|cc-number|ssn/.test(fieldHint));
          // Canvas/video/frame pixels have no inspectable field semantics;
          // mask their complete visible host region instead of inspecting data.
          const opaque = ['canvas','video','iframe','object','embed'].includes(tag);
          if (sensitive || opaque) {
            if (sensitiveRects.length >= MAX_REGIONS) addReason('region_budget');
            else sensitiveRects.push({x:rect.x-2, y:rect.y-2, width:rect.width+4, height:rect.height+4});
          }
          if (node.shadowRoot || tag.includes('-')) addReason('shadow_boundary');
          if (sensitive && style.overflow === 'visible') {
            // Include visible descendants that extend outside a declared box.
            for (const child of node.children) {
              const childRect = child.getBoundingClientRect();
              if (childRect.left < rect.left || childRect.top < rect.top || childRect.right > rect.right || childRect.bottom > rect.bottom)
                addReason('sensitive_overflow');
            }
          }
          if (sensitive && (style.textShadow !== 'none' || style.filter !== 'none')) addReason('sensitive_paint_overflow');
        }
      }
      node = tree.nextNode();
    }
    for (const ui of document.querySelectorAll(OWNED)) {
      if (getComputedStyle(ui).visibility !== 'hidden' && getComputedStyle(ui).display !== 'none') addReason('picker_decoration_visible');
    }
    return {viewport, sensitiveRects, uncoveredReasons, ready:document.readyState !== 'loading',
      documentStamp:performance.timeOrigin, scrollX:window.scrollX, scrollY:window.scrollY,
      documentWidth:document.documentElement.scrollWidth, documentHeight:document.documentElement.scrollHeight};
  }
  return {dispatch:geometry,dispose:()=>{
    for(const cancel of pendingFrames)cancel();pendingFrames.clear();
    for(const held of targets.values())clearTimeout(held.timer);targets.clear();return {cleaned:true};
  }};
})()
