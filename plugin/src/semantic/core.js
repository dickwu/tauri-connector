// Connector semantic version 1: bounded light-DOM subset, not a platform AX tree.
// Independently authored; algorithm choices and coverage: docs/adr/semantic-core-v1.md.
(() => {
  'use strict';
  const slot = '__CONNECTOR_SEMANTIC__';
  if (window[slot]?.version === '1') return window[slot];
  const normalize = value => String(value ?? '').replace(/\s+/gu, ' ').trim();
  const roles = new Set(('alert alertdialog application article banner blockquote button caption cell checkbox code columnheader combobox complementary contentinfo definition deletion dialog directory document emphasis feed figure form generic grid gridcell group heading img insertion link list listbox listitem log main marquee math menu menubar menuitem menuitemcheckbox menuitemradio meter navigation none note option paragraph presentation progressbar radio radiogroup region row rowgroup rowheader scrollbar search searchbox separator slider spinbutton status strong subscript suggestion superscript switch tab table tablist tabpanel term textbox time timer toolbar tooltip tree treegrid treeitem').split(' '));
  const contentRoles = new Set(('button cell checkbox columnheader gridcell heading link menuitem menuitemcheckbox menuitemradio option radio row rowheader switch tab tooltip treeitem').split(' '));
  const prohibitedNames = new Set(['caption','code','deletion','emphasis','generic','insertion','none','paragraph','presentation','strong','subscript','superscript']);
  const coverage = Object.freeze({algorithm:'connector_light_dom_subset',name:'ordered_idrefs_html_labels_alt_and_contents',description:'describedby_description_title',ariaOwns:'bounded_cycle_safe',shadowDom:'unsupported',iframe:'unsupported',generatedContent:'unsupported',platformAccessibilityTree:'unobserved'});
  const fail = (code,message) => {const error=new Error(message);error.code=code;error.stage='locating';throw error;};
  const attr = (el,key) => el?.getAttribute?.(key) ?? null;
  const tag = el => String(el?.tagName || '').toLowerCase();
  const isConnectorOwned = el => Boolean(el?.closest?.('[data-connector-picker-ui][data-connector-owned="picker"]'));
  const doc = el => el?.ownerDocument || document;
  const now = () => typeof performance === 'object' ? performance.now() : Date.now();
  function budget(options={}) { return {nodes:0, chars:0, maxNodes:Math.min(20000,Math.max(1,options.maxNodes||5000)), maxDepth:64, maxChars:65536, until:now()+Math.min(500,Math.max(1,options.timeoutMs||100))}; }
  function charge(b,depth=0) {if (++b.nodes>b.maxNodes || depth>b.maxDepth || now()>b.until) fail('semantic_budget_exceeded','Semantic traversal exceeded its node, depth or time budget');}
  function textBudget(value,b) {value=String(value||'');b.chars+=value.length;if(b.chars>b.maxChars) fail('semantic_budget_exceeded','Semantic text exceeded its character budget');return value;}
  function implicitRole(el) {
    const t=tag(el),type=String(el.type||attr(el,'type')||'text').toLowerCase();
    if(t==='input') {
      if(['button','submit','reset','image'].includes(type))return 'button';
      if(['checkbox','radio','range','number','search'].includes(type))return {checkbox:'checkbox',radio:'radio',range:'slider',number:'spinbutton',search:'searchbox'}[type];
      return ['hidden','password','file','color','date','datetime-local','month','time','week'].includes(type)?null:'textbox';
    }
    if(t==='select')return el.multiple||el.size>1?'listbox':'combobox';
    if(t==='a'||t==='area')return el.hasAttribute('href')?'link':null;
    if(t==='img'&&attr(el,'alt')==='')return 'presentation';
    if(t==='th')return attr(el,'scope')==='row'||attr(el,'scope')==='rowgroup'?'rowheader':'columnheader';
    if(t==='header'||t==='footer')return el.closest?.('article,aside,main,nav,section')?null:t==='header'?'banner':'contentinfo';
    if(t==='form'||t==='section')return attr(el,'aria-label')||attr(el,'aria-labelledby')||attr(el,'title')?(t==='form'?'form':'region'):null;
    return {button:'button',textarea:'textbox',dialog:'dialog',option:'option',optgroup:'group',img:'img',h1:'heading',h2:'heading',h3:'heading',h4:'heading',h5:'heading',h6:'heading',ul:'list',ol:'list',li:'listitem',table:'table',tbody:'rowgroup',thead:'rowgroup',tfoot:'rowgroup',tr:'row',td:'cell',nav:'navigation',main:'main',article:'article',aside:'complementary',progress:'progressbar',meter:'meter',fieldset:'group',figure:'figure',summary:'button',output:'status',hr:'separator',search:'search'}[t]||null;
  }
  function getRole(el) {
    for(const token of normalize(attr(el,'role')).split(' '))if(roles.has(token)) {
      if((token==='none'||token==='presentation') && (el.tabIndex>=0 || ['button','input','select','textarea'].includes(tag(el)) || (tag(el)==='a'&&el.hasAttribute('href'))))return implicitRole(el);
      return token;
    }
    return implicitRole(el);
  }
  function hidden(el,includeInert=true) {
    let depth=0;
    for(let current=el;current?.nodeType===1;current=current.parentElement) {
      if(++depth>256)fail('semantic_budget_exceeded','Element ancestry exceeds supported depth');
      if(current.hidden||attr(current,'aria-hidden')==='true'||(includeInert&&(current.inert||current.hasAttribute('inert'))))return true;
      const style=getComputedStyle(current);
      if(style.display==='none'||style.visibility==='hidden'||style.visibility==='collapse'||style.contentVisibility==='hidden')return true;
    }
    return false;
  }
  function isAccessibilityExposed(el) {return Boolean(el&&el.isConnected!==false&&!isConnectorOwned(el)&&!hidden(el));}
  function isVisible(el) {
    if(!el||el.isConnected===false||!el.getClientRects().length)return false;
    let depth=0;
    for(let current=el;current?.nodeType===1;current=current.parentElement) {
      if(++depth>256)fail('semantic_budget_exceeded','Element ancestry exceeds supported depth');
      const style=getComputedStyle(current);
      if(style.display==='none'||style.visibility==='hidden'||style.visibility==='collapse'||Number(style.opacity)===0)return false;
    }
    const r=el.getBoundingClientRect();return r.width>0&&r.height>0;
  }
  function isEnabled(el) {return Boolean(el&&el.isConnected!==false&&!el.matches?.(':disabled')&&!el.closest?.('[inert],[aria-disabled="true"]'));}
  function isEditable(el) {
    if(!isEnabled(el)||el.readOnly||attr(el,'aria-readonly')==='true')return false;
    if(tag(el)==='textarea')return true;
    if(tag(el)==='input')return ['text','search','tel','url','email','password','number'].includes(String(el.type||'text'));
    return Boolean(el.isContentEditable&&!el.closest?.('[contenteditable="false"]'));
  }
  function checkActionability(el,operation='click') {
    const visible=isVisible(el),enabled=isEnabled(el),editable=isEditable(el);
    const state={connected:Boolean(el&&el.isConnected!==false),visible,enabled,editable,inViewport:false,receivesEvents:false};
    if(!el||!visible)return {...state,actionable:false,reason:'not_visible'};
    const r=el.getBoundingClientRect(),view=doc(el).defaultView||window;
    const left=Math.max(0,r.left),right=Math.min(view.innerWidth,r.right),top=Math.max(0,r.top),bottom=Math.min(view.innerHeight,r.bottom);
    state.inViewport=right>left&&bottom>top;
    if(state.inViewport){const hit=doc(el).elementFromPoint((left+right)/2,(top+bottom)/2);state.receivesEvents=Boolean(hit&&(hit===el||el.contains(hit)));}
    const edit=['fill','type','edit'].includes(operation);
    return {...state,actionable:enabled&&(!edit||editable)&&state.inViewport&&state.receivesEvents,reason:!enabled?'disabled':edit&&!editable?'not_editable':!state.inViewport?'outside_viewport':!state.receivesEvents?'covered':null};
  }
  function references(el,key) {return [...new Set(normalize(attr(el,key)).split(' ').filter(Boolean))].map(id=>doc(el).getElementById(id)).filter(Boolean);}
  function ownedChildren(el) {
    const children=Array.from(el.children||[]);
    for(const child of references(el,'aria-owns'))if(child!==el&&!child.contains?.(el)&&!children.includes(child))children.push(child);
    return children;
  }
  function getAccessibleChildren(el,options={}) {
    const seen=new Set(options.visited||[]);seen.add(el);const b=budget(options),result=[];
    const visit=(child,depth)=>{charge(b,depth);if(seen.has(child))return;seen.add(child);if(!isAccessibilityExposed(child))return;const role=getRole(child);if(role==='none'||role==='presentation'){for(const nested of ownedChildren(child))visit(nested,depth+1);}else result.push(child);};
    for(const child of ownedChildren(el))visit(child,0);
    return result;
  }
  function computeName(el,b,seen,depth=0,referenced=false,contents=false,includeHidden=false) {
    charge(b,depth);if(!el||seen.has(el))return '';seen.add(el);
    if(!includeHidden&&hidden(el))return '';
    const role=getRole(el);
    if(!contents&&!referenced&&prohibitedNames.has(role))return '';
    const refs=references(el,'aria-labelledby');
    if(refs.length&&!referenced)return normalize(refs.map(ref=>computeName(ref,b,seen,depth+1,true,true,hidden(ref))).join(' '));
    const label=normalize(attr(el,'aria-label'));if(label)return textBudget(label,b);
    const labels=Array.from(el.labels||[]);
    if(labels.length&&!referenced)return normalize(labels.map(ref=>computeName(ref,b,seen,depth+1,true,true,hidden(ref))).join(' '));
    const t=tag(el),type=String(el.type||attr(el,'type')||'text').toLowerCase();
    if(t==='img'||(t==='input'&&type==='image')){const alt=attr(el,'alt');if(alt!==null)return textBudget(normalize(alt),b);}
    if(t==='input'&&['button','submit','reset'].includes(type))return textBudget(normalize(el.value||attr(el,'value')||(type==='submit'?'Submit':type==='reset'?'Reset':'')),b);
    const caption={fieldset:'LEGEND',figure:'FIGCAPTION',table:'CAPTION',svg:'title'}[t];
    if(caption){const child=Array.from(el.children||[]).find(child=>child.tagName===caption);if(child)return computeName(child,b,seen,depth+1,true,true);}
    if(contents||referenced||contentRoles.has(role)) {
      const parts=[];
      const nodes=el.childNodes?Array.from(el.childNodes):[];
      for(const child of nodes){if(child.nodeType===3)parts.push(textBudget(child.textContent,b));else if(child.nodeType===1)parts.push(computeName(child,b,seen,depth+1,referenced,true,includeHidden));}
      // Minimal DOM shims used by legacy unit tests do not expose childNodes.
      if(!el.childNodes&&el.textContent)parts.push(textBudget(el.textContent,b));
      for(const child of references(el,'aria-owns'))if(child!==el&&!child.contains?.(el))parts.push(computeName(child,b,seen,depth+1,referenced,true,includeHidden));
      const value=normalize(parts.join(' '));if(value)return value;
    }
    const title=normalize(attr(el,'title'));if(title)return textBudget(title,b);
    // Explicit compatibility fallback; coverage is a subset, not full AccName.
    if(['input','textarea'].includes(t))return textBudget(normalize(attr(el,'placeholder')),b);
    return '';
  }
  const getAccessibleName=el=>computeName(el,budget(),new Set());
  function getAccessibleDescription(el) {
    const refs=references(el,'aria-describedby');
    if(refs.length){const b=budget(),seen=new Set([el]);return normalize(refs.map(ref=>computeName(ref,b,seen,0,true,true,hidden(ref))).join(' '));}
    if(el.hasAttribute('aria-description'))return normalize(attr(el,'aria-description')).slice(0,65536);
    const title=normalize(attr(el,'title'));return title&&title!==getAccessibleName(el)?title:'';
  }
  function getLabelTexts(el) {
    const b=budget();const labels=Array.from(el.labels||[]).map(label=>computeName(label,b,new Set([el]),0,true,true,hidden(label)));
    if(attr(el,'aria-label'))labels.push(normalize(attr(el,'aria-label')));
    const refs=references(el,'aria-labelledby');if(refs.length)labels.push(normalize(refs.map(ref=>computeName(ref,b,new Set([el]),0,true,true,hidden(ref))).join(' ')));
    return [...new Set(labels.map(normalize).filter(Boolean))];
  }
  function getAriaStates(el) {
    const states={};
    for(const key of ['disabled','checked','expanded','selected','pressed','invalid','required','readonly','busy','modal']) {
      const value=attr(el,'aria-'+key);if(value!==null&&value!=='')states[key]=value==='true'?true:value==='false'?false:(key==='checked'||key==='pressed')&&value==='mixed'?'mixed':key==='invalid'&&['grammar','spelling'].includes(value)?value:true;
      else if(key==='disabled'&&el.matches?.(':disabled'))states[key]=true;
      else if(key==='checked'&&tag(el)==='input'&&['checkbox','radio'].includes(el.type))states[key]=el.indeterminate?'mixed':Boolean(el.checked);
      else if(key==='selected'&&tag(el)==='option')states[key]=Boolean(el.selected);
      else if(key==='required'&&el.required)states[key]=true;
      else if(key==='readonly'&&el.readOnly)states[key]=true;
    }
    return states;
  }
  const literal=value=>value&&typeof value==='object'&&Object.prototype.hasOwnProperty.call(value,'literal')?value.literal:value;
  function string(value,field) {value=literal(value);if(typeof value!=='string'||!value.trim())fail('invalid_spec',field+' must be a nonempty string');return value;}
  function resolveCandidates(locator,scope=document,options={}) {
    const b=budget(options);
    function resolve(query,root,depth=0) {
      charge(b,depth);if(!query||typeof query!=='object'||Array.isArray(query))fail('invalid_spec','A structured locator is required');
      if(depth>4)fail('invalid_spec','Locator scope exceeds four levels');
      if(query.shadowDom||query.frame||query.iframe||root?.nodeType===11||root?.ownerDocument&&root.ownerDocument!==document)fail('unsupported_feature','Shadow DOM and iframe locator boundaries are unsupported');
      const by=query.by,value=string(query.value,'target.value');if(!['role','label','testId','css'].includes(by))fail('unsupported_feature','Unsupported locator kind');
      if(query.scope){const scopes=resolve(query.scope,root,depth+1);if(scopes.length>1)fail('ambiguous_target','Strict scope matched multiple elements');if(!scopes.length)return [];root=scopes[0];}
      const expected=query.name===undefined?null:normalize(string(query.name,'target.name'));
      const attribute=query.entity?string(query.entity.attribute,'target.entity.attribute'):null;
      const identity=query.entity?string(query.entity.value,'target.entity.value'):null;
      let candidates;try{candidates=root.querySelectorAll(by==='css'?value:'*');}catch(_){fail('invalid_spec','Invalid CSS locator');}
      const result=[];
      for(const el of candidates){charge(b);if(el.isConnected===false||isConnectorOwned(el))continue;if(by==='role'&&(getRole(el)!==value||!isAccessibilityExposed(el)))continue;if(by==='label'&&(!isAccessibilityExposed(el)||!getLabelTexts(el).includes(normalize(value))))continue;if(by==='testId'&&attr(el,'data-testid')!==value)continue;if(expected!==null&&getAccessibleName(el)!==expected)continue;if(attribute&&attr(el,attribute)!==identity)continue;result.push(el);}
      return result;
    }
    return resolve(locator,scope);
  }
  const sensitiveName=value=>/(?:^|[^a-z0-9])(?:password|passwd|secret|token|auth|authorization|authentication|cookie|credential|api[-_]?key|one[-_]?time[-_]?code)(?:$|[^a-z0-9])/iu.test(String(value||'').replace(/([a-z])([A-Z])/gu,'$1-$2'));
  function directlySensitive(el,attribute) {
    return tag(el)==='input'&&el.type==='password'||Boolean(el?.closest?.('[data-sensitive="true"],[data-connector-sensitive="true"]'))||['id','name','autocomplete'].some(key=>sensitiveName(attr(el,key)))||sensitiveName(attribute);
  }
  function isSensitive(el,attribute) {
    const pending=[el],seen=new Set(),limit=budget({maxNodes:1000,timeoutMs:30});
    try {
      if (sensitiveName(attribute)) return true;
      // Follow the graph that can supply alternate text, not only direct
      // references. Sensitive descendants of a referenced label stay sensitive
      // when the label is referenced again. Cycles and hostile trees fail closed.
      while (pending.length) {
        const current=pending.pop();
        if (!current || seen.has(current)) continue;
        seen.add(current);charge(limit);
        if (directlySensitive(current)) return true;
        for (const child of current.children||[]) pending.push(child);
        for (const key of ['aria-labelledby','aria-describedby','aria-owns']) {
          for (const ref of references(current,key)) pending.push(ref);
        }
        for (const label of current.labels||[]) pending.push(label);
        if (pending.length>limit.maxNodes) return true;
      }
      return false;
    } catch (_) {return true;}
  }
  function bounded(value,maxBytes=512) {
    const source=String(value||''),parts=[];let bytes=0;
    for (const character of source) {
      const length=new TextEncoder().encode(character).length;
      if (bytes+length>maxBytes) return {value:parts.join(''),truncated:true};
      parts.push(character);bytes+=length;
    }
    return {value:source,truncated:false};
  }
  function describe(el) {
    const sensitive=isSensitive(el),role=getRole(el),warnings=[];
    const read=fn=>{
      if (sensitive) return {...bounded('[redacted]'),source:'redacted'};
      try {return {...bounded(fn(el)),source:'computed'};}
      catch(error) {if(error.code!=='semantic_budget_exceeded')throw error;warnings.push('semantic_budget_exceeded');return {value:'',truncated:true,source:'unavailable'};}
    };
    const name=read(getAccessibleName),description=read(getAccessibleDescription);
    return {tag:bounded(tag(el),128).value,role,name:name.value,description:description.value,states:getAriaStates(el),semanticVersion:'1',accessibilityExposed:isAccessibilityExposed(el),visible:isVisible(el),sensitive,redacted:sensitive,truncated:name.truncated||description.truncated,sources:{role:role?'computed':'unsupported',name:name.source,description:description.source},warnings:[...new Set(warnings)],coverage};
  }
  function locatorCandidates(el) {
    if(!el||el.isConnected===false||isConnectorOwned(el)||isSensitive(el)||el.getRootNode?.()!==undefined&&el.getRootNode()!==document)return [];
    const candidates=[],locators=[];let entity;
    for(const attribute of ['data-entity-id','data-record-id','data-row-key','data-id','data-key']){const value=attr(el,attribute);if(value&&value.length<=256&&!sensitiveName(attribute)){entity={attribute,value};break;}}
    const role=getRole(el),testId=attr(el,'data-testid');
    let name='';try{name=getAccessibleName(el);}catch(error){if(error.code!=='semantic_budget_exceeded')throw error;}
    if(testId&&testId.length<=256)locators.push({by:'testId',value:testId});
    if(role&&name&&name.length<=256)locators.push({by:'role',value:role,name});
    if(el.id&&el.id.length<=256)locators.push({by:'css',value:'#'+CSS.escape(el.id)});
    const path=[];let current=el;
    while(current&&current.nodeType===1&&path.length<12){let part=tag(current);if(!/^[a-z][a-z0-9-]*$/u.test(part))break;const parent=current.parentElement;if(parent){const siblings=Array.from(parent.children).filter(x=>x.tagName===current.tagName);if(siblings.length>1)part+=':nth-of-type('+(siblings.indexOf(current)+1)+')';}path.unshift(part);current=parent;}
    if(path.length&&current===null)locators.push({by:'css',value:path.join(' > ')});
    for(const locator of locators){if(entity)locator.entity=entity;try{const matches=resolveCandidates(locator);if(matches.length===1&&matches[0]===el)candidates.push({locator,verified:true,matchCount:1,semanticVersion:'1',reason:'unique_same_element',stability:entity?'entity_bound':'current_document_only'});}catch(_){/* A candidate is never verified by fallback or first match. */}}
    return candidates;
  }
  const documentEpoch = window.__CONNECTOR_BOOTSTRAP__?.pageEpoch || crypto.randomUUID();
  const refRecords = new Map();
  let refBytes = 0;
  const refEpoch = () => window.__CONNECTOR_BOOTSTRAP__?.pageEpoch || documentEpoch;
  function elementIdentity(element) {
    const attributes = {};
    for (const key of ['id','data-entity-id','data-record-id','data-row-key','data-id','data-key','data-testid']) {
      const value = attr(element,key);
      if (value !== null) attributes[key] = value;
    }
    return JSON.stringify({tag:tag(element),role:getRole(element),name:getAccessibleName(element),attributes});
  }
  function rememberRef(element) {
    const fingerprint = elementIdentity(element);
    const bytes = new TextEncoder().encode(fingerprint).length + 128;
    // Ref storage is page-local, bounded, and never exported with raw identity
    // attributes. Evicted refs fail closed; they cannot rebind via a CSS fallback.
    if (bytes > 16384) return {identity:null,semanticVersion:'1',pageEpoch:refEpoch(),unavailable:'identity_budget_exceeded'};
    while (refRecords.size >= 20000 || refBytes + bytes > 4 * 1024 * 1024) {
      const oldest = refRecords.keys().next().value;
      refBytes -= refRecords.get(oldest).bytes;
      refRecords.delete(oldest);
    }
    const identity = crypto.randomUUID(), pageEpoch = refEpoch();
    refRecords.set(identity,{element,fingerprint,pageEpoch,bytes});refBytes += bytes;
    return {identity,semanticVersion:'1',pageEpoch};
  }
  function resolveRef(identity,context={}) {
    if (context.semanticVersion !== '1' || context.pageEpoch !== refEpoch() || window[slot] !== api) return null;
    const record = refRecords.get(identity);
    if (!record || record.pageEpoch !== context.pageEpoch || !record.element.isConnected || isConnectorOwned(record.element)) return null;
    try {return elementIdentity(record.element) === record.fingerprint ? record.element : null;}
    catch (_) {return null;}
  }
  function clearRefs() {refRecords.clear();refBytes=0;}
  window.addEventListener?.('pagehide',clearRefs);
  function dispose() {
    clearRefs();
    window.removeEventListener?.('pagehide',clearRefs);
    if (window[slot] === api) delete window[slot];
  }
  const api=Object.freeze({version:'1',semanticVersion:'1',coverage,normalize,isConnectorOwned,getRole,getAccessibleName,getAccessibleDescription,getAriaStates,getAccessibleChildren,isAccessibilityExposed,isVisible,isEnabled,isEditable,checkActionability,resolveCandidates,getLabelTexts,isSensitive,describe,locatorCandidates,rememberRef,resolveRef,clearRefs,dispose});
  Object.defineProperty(window,slot,{value:api,configurable:true,writable:false});return api;
})()
