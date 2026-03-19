//! Internal WebSocket bridge for reliable JS execution in the webview.
//!
//! Instead of relying on `window.__TAURI__` (which may not be available in all
//! WebKit content worlds), this bridge injects a small JS client that connects
//! back to the plugin via a dedicated internal WebSocket. Results are delivered
//! through this channel, completely bypassing Tauri's IPC layer.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{BridgeCommand, BridgeResult};

type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>>;

/// Manages the internal WebSocket bridge to the webview.
#[derive(Clone)]
pub struct Bridge {
    /// Port the internal WebSocket listens on
    port: u16,
    /// Channel to send scripts to the connected webview bridge client
    script_tx: mpsc::UnboundedSender<String>,
    /// Pending JS evaluation results, keyed by request ID
    pending: PendingMap,
    /// App handle for eval-based fallback execution
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
}

impl Bridge {
    /// Start the internal bridge WebSocket server.
    /// Returns the Bridge handle and the port it's listening on.
    pub fn start() -> Result<Self, String> {
        let port = find_available_port(9300, 9400)
            .ok_or_else(|| "No available port in range 9300-9400".to_string())?;

        let (script_tx, script_rx) = mpsc::unbounded_channel::<String>();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        let bridge = Self {
            port,
            script_tx,
            pending: pending.clone(),
            app_handle: Arc::new(Mutex::new(None)),
        };

        let bridge_clone = bridge.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge_clone.run_server(script_rx).await {
                eprintln!("[connector][bridge] Server error: {e}");
            }
        });

        println!("[connector][bridge] Internal bridge on port {port}");
        Ok(bridge)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Set the app handle for eval-based fallback JS execution.
    pub async fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    /// Execute JavaScript in the webview. Tries WS bridge, falls back to eval+event.
    pub async fn execute_js(
        &self,
        script: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, String> {
        // Try WS bridge with short timeout
        match self.execute_js_ws(script, timeout_ms.min(2000)).await {
            Ok(v) => return Ok(v),
            Err(_) => {
                // WS bridge timed out, fall back to eval+event path
            }
        }

        // Fallback: eval + Tauri event
        self.execute_js_via_eval(script, timeout_ms).await
    }

    async fn execute_js_ws(
        &self,
        script: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        {
            self.pending.lock().await.insert(id.clone(), tx);
        }

        let cmd = BridgeCommand { id: id.clone(), script: script.to_string() };
        let msg = serde_json::to_string(&cmd).map_err(|e| e.to_string())?;
        self.script_tx.send(msg).map_err(|_| "Bridge not connected".to_string())?;

        tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx)
            .await
            .map_err(|_| {
                let pending = self.pending.clone();
                let id = id.clone();
                tokio::spawn(async move { pending.lock().await.remove(&id); });
                "WS bridge timeout".to_string()
            })?
            .map_err(|_| "Bridge channel closed".to_string())?
    }

    async fn execute_js_via_eval(
        &self,
        script: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, String> {
        use tauri::{Listener, Manager};

        let app = self.app_handle.lock().await;
        let app = app.as_ref().ok_or("App handle not set for eval fallback")?;
        let window = app.get_webview_window("main")
            .ok_or("Window 'main' not found")?;

        let id = uuid::Uuid::new_v4().to_string();
        let event_name = format!("connector-eval-{id}");
        let (tx, rx) = oneshot::channel::<Result<serde_json::Value, String>>();
        let tx = std::sync::Mutex::new(Some(tx));

        let listener_id = app.listen(&event_name, move |event| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let payload_str = event.payload();
                // The payload may be double-quoted (string-wrapped JSON from Tauri event system)
                let inner = serde_json::from_str::<String>(payload_str)
                    .unwrap_or_else(|_| payload_str.to_string());
                match serde_json::from_str::<BridgeResult>(&inner) {
                    Ok(r) => {
                        let v = if let Some(e) = r.error { Err(e) }
                                else { Ok(r.result.unwrap_or(serde_json::Value::Null)) };
                        let _ = tx.send(v);
                    }
                    Err(e) => { let _ = tx.send(Err(format!("Parse error: {e}"))); }
                }
            }
        });

        let escaped = script.replace('\\', "\\\\").replace('`', "\\`");
        let js = format!(
            r#"(async function(){{
                try{{
                    const AF=Object.getPrototypeOf(async function(){{}}).constructor;
                    const r=await new AF('return ('+`{escaped}`+')')();
                    let p;try{{JSON.stringify(r);p=JSON.stringify({{id:'{id}',result:r}})}}catch(_){{p=JSON.stringify({{id:'{id}',result:String(r)}})}}
                    if(window.__TAURI_INTERNALS__)window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{{event:'{event_name}',payload:p}});
                }}catch(e){{
                    const p=JSON.stringify({{id:'{id}',error:e.message||String(e)}});
                    if(window.__TAURI_INTERNALS__)window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{{event:'{event_name}',payload:p}});
                }}
            }})()"#
        );

        window.eval(&js).map_err(|e| format!("eval inject failed: {e}"))?;

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms), rx
        ).await
        .map_err(|_| "Script execution timeout (eval path)".to_string())?
        .map_err(|_| "Result channel closed".to_string())?;

        app.unlisten(listener_id);
        result
    }

    async fn run_server(
        &self,
        mut script_rx: mpsc::UnboundedReceiver<String>,
    ) -> Result<(), String> {
        let listener = TokioTcpListener::bind(format!("127.0.0.1:{}", self.port))
            .await
            .map_err(|e| e.to_string())?;

        let (stream, addr) = listener
            .accept()
            .await
            .map_err(|e| e.to_string())?;

        println!("[connector][bridge] Webview client connected from {addr}");

        let mut ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .map_err(|e| e.to_string())?;

        let pending = self.pending.clone();

        // Use a single loop with select! instead of split() to avoid
        // potential buffering issues between SplitSink and SplitStream
        loop {
            tokio::select! {
                // Script to send to webview
                script_msg = script_rx.recv() => {
                    match script_msg {
                        Some(msg) => {
                            if ws_stream.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                // Result from webview
                ws_msg = ws_stream.next() => {
                    match ws_msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<BridgeResult>(&text) {
                                Ok(result) => {
                                    let mut pending = pending.lock().await;
                                    if let Some(tx) = pending.remove(&result.id) {
                                        let value = if let Some(error) = result.error {
                                            Err(error)
                                        } else {
                                            Ok(result.result.unwrap_or(serde_json::Value::Null))
                                        };
                                        let _ = tx.send(value);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[connector][bridge] Invalid result message: {e}");
                                }
                            }
                        }
                        Some(Ok(_)) => {} // Ignore non-text messages
                        Some(Err(_)) => break,
                        None => break,
                    }
                }
            }
        }

        println!("[connector][bridge] Webview client disconnected");

        Ok(())
    }
}

/// Generate the JavaScript bridge client code that gets injected into the webview.
pub fn bridge_init_script(port: u16) -> String {
    format!(
        r#"(function() {{
  if (window.__CONNECTOR_BRIDGE__) return;
  window.__CONNECTOR_BRIDGE__ = true;

  // Capture native WebSocket before frameworks (Next.js/Turbopack HMR) can patch it
  const NativeWebSocket = window.WebSocket;

  const BRIDGE_PORT = {port};
  let ws = null;
  let reconnectTimer = null;
  const consoleLogs = [];
  const MAX_LOGS = 500;

  // Intercept console methods for log capture
  const origConsole = {{}};
  ['log', 'warn', 'error', 'info', 'debug'].forEach(function(level) {{
    origConsole[level] = console[level];
    console[level] = function() {{
      const args = Array.from(arguments).map(function(a) {{
        try {{ return typeof a === 'string' ? a : JSON.stringify(a); }}
        catch (_) {{ return String(a); }}
      }});
      consoleLogs.push({{
        level: level,
        message: args.join(' '),
        timestamp: Date.now()
      }});
      if (consoleLogs.length > MAX_LOGS) consoleLogs.shift();
      origConsole[level].apply(console, arguments);
    }};
  }});

  // Expose logs for retrieval
  window.__CONNECTOR_LOGS__ = consoleLogs;

  // === IPC Invoke Wrapper (for monitoring) ===
  if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
    const _origInvoke = window.__TAURI_INTERNALS__.invoke;
    window.__CONNECTOR_ORIG_INVOKE__ = _origInvoke;
    window.__TAURI_INTERNALS__.invoke = async function(cmd, args, options) {{
      if (cmd.startsWith('plugin:connector|')) {{
        return _origInvoke.call(this, cmd, args, options);
      }}
      const t0 = Date.now();
      try {{
        const result = await _origInvoke.call(this, cmd, args, options);
        if (window.__CONNECTOR_IPC_MONITOR__) {{
          _origInvoke.call(this, 'plugin:connector|push_ipc_event', {{
            payload: {{ command: cmd, args: args || {{}}, timestamp: t0, durationMs: Date.now() - t0 }}
          }}).catch(function(){{}});
        }}
        return result;
      }} catch(e) {{
        if (window.__CONNECTOR_IPC_MONITOR__) {{
          _origInvoke.call(this, 'plugin:connector|push_ipc_event', {{
            payload: {{ command: cmd, args: args || {{}}, timestamp: t0, durationMs: Date.now() - t0, error: e.message }}
          }}).catch(function(){{}});
        }}
        throw e;
      }}
    }};
  }}

  function connect() {{
    try {{
      ws = new NativeWebSocket('ws://127.0.0.1:' + BRIDGE_PORT);
    }} catch (e) {{
      scheduleReconnect();
      return;
    }}

    ws.onopen = function() {{
      origConsole.log('[connector] Bridge connected on port ' + BRIDGE_PORT);
      // Send hello to verify bidirectional communication
      try {{ ws.send(JSON.stringify({{ id: '__bridge_hello__', result: 'connected' }})); }} catch (_) {{}}
    }};

    ws.onmessage = function(event) {{
      let cmd;
      try {{
        cmd = JSON.parse(typeof event.data === 'string' ? event.data : '');
      }} catch (e) {{
        return;
      }}

      executeCommand(cmd);
    }};

    ws.onclose = function() {{
      origConsole.log('[connector] Bridge disconnected, reconnecting...');
      scheduleReconnect();
    }};

    ws.onerror = function() {{
      // onclose will fire after this
    }};
  }}

  function scheduleReconnect() {{
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(function() {{
      reconnectTimer = null;
      connect();
    }}, 1000);
  }}

  async function executeCommand(cmd) {{
    const id = cmd.id;
    const script = cmd.script;

    try {{
      const AsyncFunction = Object.getPrototypeOf(async function(){{}}).constructor;
      const fn = new AsyncFunction('return (' + script + ')');
      const result = await fn();
      sendResult(id, result, null);
    }} catch (e) {{
      sendResult(id, null, e.message || String(e));
    }}
  }}

  function sendResult(id, result, error) {{
    if (!ws || ws.readyState !== WebSocket.OPEN) {{
      origConsole.error('[connector] Cannot send result: bridge not connected');
      return;
    }}

    const payload = {{ id: id }};
    if (error !== null && error !== undefined) {{
      payload.error = error;
    }} else {{
      try {{
        // Ensure result is JSON-serializable
        JSON.stringify(result);
        payload.result = result;
      }} catch (_) {{
        payload.result = String(result);
      }}
    }}

    ws.send(JSON.stringify(payload));
  }}

  // === Unified Snapshot Engine ===

  // Outer-scope: cached React fiber key (undefined=not looked up, null=not React)
  let fiberKey;

  const GENERIC_WRAPPERS = new Set([
    'App', 'Layout', 'ConfigProvider', 'ThemeProvider',
    'Fragment', 'Suspense', 'ErrorBoundary', 'StrictMode'
  ]);

  function findFiberKey(el) {{
    if (fiberKey !== undefined) return fiberKey;
    const keys = Object.keys(el);
    for (let i = 0; i < keys.length; i++) {{
      if (keys[i].startsWith('__reactFiber$')) {{
        fiberKey = keys[i];
        return fiberKey;
      }}
    }}
    fiberKey = null;
    return fiberKey;
  }}

  function getComponentName(el) {{
    const key = findFiberKey(el);
    if (!key) return null;
    let fiber = el[key];
    while (fiber) {{
      const t = fiber.type;
      if (t && typeof t === 'function') {{
        const name = t.displayName || t.name || null;
        if (name && !GENERIC_WRAPPERS.has(name)) return name;
      }}
      fiber = fiber.return;
    }}
    return null;
  }}

  function getRole(el) {{
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    if (tag === 'input') {{
      const type = String(el.type || 'text').toLowerCase();
      const inputMap = {{
        'checkbox': 'checkbox',
        'radio': 'radio',
        'range': 'slider',
        'search': 'searchbox',
        'number': 'spinbutton'
      }};
      return inputMap[type] || 'textbox';
    }}
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    const tagMap = {{
      'button': 'button', 'select': 'combobox', 'textarea': 'textbox',
      'img': 'img', 'nav': 'navigation', 'main': 'main',
      'header': 'banner', 'footer': 'contentinfo', 'aside': 'complementary',
      'form': 'form', 'table': 'table', 'ul': 'list', 'ol': 'list',
      'li': 'listitem', 'h1': 'heading', 'h2': 'heading', 'h3': 'heading',
      'h4': 'heading', 'h5': 'heading', 'h6': 'heading'
    }};
    return tagMap[tag] || null;
  }}

  function getName(el) {{
    // 1. aria-label
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel) return ariaLabel;

    // 2. aria-labelledby (multiple IDs)
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {{
      const parts = labelledBy.split(/\s+/);
      const texts = [];
      for (let i = 0; i < parts.length; i++) {{
        const ref = document.getElementById(parts[i]);
        if (ref) texts.push(ref.textContent.trim());
      }}
      if (texts.length > 0) return texts.join(' ');
    }}

    const tag = el.tagName;

    // 3. IMG alt
    if (tag === 'IMG') return el.getAttribute('alt') || '';

    // 4. INPUT/SELECT/TEXTAREA: label[for], then placeholder
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') {{
      if (el.id) {{
        const lbl = document.querySelector('label[for="' + el.id + '"]');
        if (lbl) return lbl.textContent.trim();
      }}
      return el.getAttribute('placeholder') || '';
    }}

    // 5. BUTTON/A/H1-H6: visible textContent
    if (tag === 'BUTTON' || tag === 'A' ||
        tag === 'H1' || tag === 'H2' || tag === 'H3' ||
        tag === 'H4' || tag === 'H5' || tag === 'H6') {{
      return (el.textContent || '').trim().substring(0, 100);
    }}

    // 6. INPUT submit/reset: value attribute
    if (tag === 'INPUT') {{
      const itype = (el.type || '').toLowerCase();
      if (itype === 'submit' || itype === 'reset') {{
        return el.getAttribute('value') || '';
      }}
    }}

    // 7. FIELDSET: first LEGEND child
    if (tag === 'FIELDSET') {{
      const legend = el.querySelector('legend');
      if (legend) return legend.textContent.trim();
    }}

    // 8. FIGURE: first FIGCAPTION child
    if (tag === 'FIGURE') {{
      const cap = el.querySelector('figcaption');
      if (cap) return cap.textContent.trim();
    }}

    // 9. TABLE: first CAPTION child
    if (tag === 'TABLE') {{
      const cap = el.querySelector('caption');
      if (cap) return cap.textContent.trim();
    }}

    // 10. title attribute (last resort)
    const title = el.getAttribute('title');
    if (title) return title;

    // 11. ::before / ::after content
    try {{
      const before = getComputedStyle(el, '::before').content;
      if (before && before !== 'none' && before !== 'normal') {{
        return before.replace(/^"|"$/g, '');
      }}
      const after = getComputedStyle(el, '::after').content;
      if (after && after !== 'none' && after !== 'normal') {{
        return after.replace(/^"|"$/g, '');
      }}
    }} catch (_) {{}}

    return '';
  }}

  // Interactive roles that get ref= attributes in ai mode
  const INTERACTIVE_ROLES = new Set([
    'button', 'link', 'textbox', 'checkbox', 'radio', 'combobox',
    'listbox', 'option', 'menuitem', 'tab', 'switch', 'slider',
    'spinbutton', 'searchbox', 'menuitemcheckbox', 'menuitemradio'
  ]);

  window.__CONNECTOR_SNAPSHOT__ = function(options) {{
    const opts = options || {{}};
    const mode = opts.mode || 'ai';
    const maxDepth = opts.maxDepth || 0;
    const maxElements = opts.maxElements || 0;
    const reactEnrich = opts.reactEnrich !== false;
    const followPortals = opts.followPortals !== false;
    const shadowDom = opts.shadowDom === true;

    const rootEl = opts.selector
      ? document.querySelector(opts.selector)
      : document.body;
    if (!rootEl) return {{ snapshot: '', refs: {{}}, meta: {{ elementCount: 0, truncated: false, portalCount: 0, virtualScrollContainers: 0 }} }};

    // State
    let elementCount = 0;
    let truncated = false;
    let refCounter = 0;
    let portalCount = 0;
    let virtualScrollCount = 0;
    const refs = {{}};
    const portalLinks = [];
    const claimedPortalIds = new Set();
    const depthMap = new WeakMap();
    const treeNodeMap = new WeakMap();

    // TreeWalker filter
    function nodeFilter(node) {{
      const tag = node.tagName;
      if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE')
        return NodeFilter.FILTER_REJECT;
      if (node.getAttribute('aria-hidden') === 'true')
        return NodeFilter.FILTER_REJECT;
      try {{
        const cs = getComputedStyle(node);
        if (cs.display === 'none') return NodeFilter.FILTER_REJECT;
        if (cs.visibility === 'hidden') return NodeFilter.FILTER_REJECT;
      }} catch (_) {{}}
      const role = node.getAttribute('role');
      if (role === 'presentation' || role === 'none')
        return NodeFilter.FILTER_SKIP;
      return NodeFilter.FILTER_ACCEPT;
    }}

    // Build a tree node for one element
    function buildNode(el, depth) {{
      const role = getRole(el);
      const name = getName(el);
      const tag = el.tagName.toLowerCase();
      const attrs = [];
      let refId = null;

      if (mode === 'ai') {{
        // Assign ref to interactive elements
        const isInteractive = (role && INTERACTIVE_ROLES.has(role)) ||
          el.hasAttribute('onclick') ||
          el.hasAttribute('tabindex') ||
          (function() {{ try {{ return getComputedStyle(el).cursor === 'pointer'; }} catch(_) {{ return false; }} }})();
        if (isInteractive) {{
          refId = 'e' + (refCounter++);
          attrs.push('ref=' + refId);
          refs[refId] = {{
            tag: tag,
            role: role || null,
            name: (name || '').substring(0, 100),
            selector: buildSelector(el),
            nth: null,
          }};
        }}
        // React component enrichment
        if (reactEnrich) {{
          const comp = getComponentName(el);
          if (comp) attrs.push('component=' + comp);
        }}
      }}

      // ARIA states
      const checked = el.getAttribute('aria-checked') || (el.checked === true ? 'true' : null);
      if (checked) attrs.push('checked=' + checked);
      const disabled = el.getAttribute('aria-disabled') || (el.disabled === true ? 'true' : null);
      if (disabled === 'true') attrs.push('disabled');
      const expanded = el.getAttribute('aria-expanded');
      if (expanded) attrs.push('expanded=' + expanded);
      const selected = el.getAttribute('aria-selected');
      if (selected) attrs.push('selected=' + selected);
      const pressed = el.getAttribute('aria-pressed');
      if (pressed) attrs.push('pressed=' + pressed);
      const level = el.getAttribute('aria-level') ||
        (/^H([1-6])$/.test(el.tagName) ? el.tagName.charAt(1) : null);
      if (level) attrs.push('level=' + level);
      const required = el.getAttribute('aria-required') || (el.required === true ? 'true' : null);
      if (required === 'true') attrs.push('required');
      const readonly = el.getAttribute('aria-readonly') || (el.readOnly === true ? 'true' : null);
      if (readonly === 'true') attrs.push('readonly');

      // Virtual scroll detection
      if (el.classList && el.classList.contains('rc-virtual-list-holder')) {{
        virtualScrollCount++;
        const inner = el.querySelector('.rc-virtual-list-holder-inner');
        const visibleCount = inner ? inner.children.length : 0;
        attrs.push('virtual-scroll');
        attrs.push('visible=' + visibleCount);
      }}

      // Portal links (aria-controls / aria-owns)
      if (followPortals) {{
        const controls = el.getAttribute('aria-controls');
        const owns = el.getAttribute('aria-owns');
        var linkedIds = [];
        if (controls) linkedIds = linkedIds.concat(controls.split(/\s+/));
        if (owns) linkedIds = linkedIds.concat(owns.split(/\s+/));
        // store for pass 2; treeNode will be attached after creation
        if (linkedIds.length > 0) {{
          for (var li = 0; li < linkedIds.length; li++) {{
            if (linkedIds[li]) {{
              portalLinks.push({{ targetId: linkedIds[li], depth: depth, treeNode: null }});
              claimedPortalIds.add(linkedIds[li]);
            }}
          }}
        }}
      }}

      // Structure mode: tag, id, classes, data-testid
      if (mode === 'structure') {{
        var structAttrs = [];
        if (el.id) structAttrs.push('id=' + el.id);
        if (el.className && typeof el.className === 'string') {{
          var cls = el.className.trim().split(/\s+/).slice(0, 5).filter(Boolean);
          if (cls.length > 0) structAttrs.push('class=' + cls.join('.'));
        }}
        var testId = el.getAttribute('data-testid');
        if (testId) structAttrs.push('data-testid=' + testId);
        return {{
          label: tag,
          name: '',
          attrs: structAttrs,
          children: [],
          depth: depth,
          el: el
        }};
      }}

      return {{
        label: role || tag,
        name: name || '',
        attrs: attrs,
        children: [],
        depth: depth,
        el: el
      }};
    }}

    // Build a minimal CSS selector for ref lookup
    function buildSelector(el) {{
      if (el.id) return '#' + el.id;
      var tag = el.tagName.toLowerCase();
      var sel = tag;
      var testId = el.getAttribute('data-testid');
      if (testId) return tag + '[data-testid="' + testId + '"]';
      if (el.className && typeof el.className === 'string') {{
        var cls = el.className.trim().split(/\s+/).slice(0, 2).filter(Boolean);
        if (cls.length > 0) sel += '.' + cls.join('.');
      }}
      // nth-child disambiguation
      if (el.parentElement) {{
        var siblings = el.parentElement.children;
        var idx = 0;
        for (var s = 0; s < siblings.length; s++) {{
          if (siblings[s] === el) {{ idx = s + 1; break; }}
        }}
        if (siblings.length > 1) sel += ':nth-child(' + idx + ')';
      }}
      return sel;
    }}

    // === Pass 1: Main DOM walk ===
    var rootNode = {{ label: 'root', name: '', attrs: [], children: [], depth: -1, el: rootEl }};
    var walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_ELEMENT, {{
      acceptNode: nodeFilter
    }});

    depthMap.set(rootEl, 0);
    var currentEl = walker.currentNode;
    if (currentEl === rootEl && currentEl.nodeType === 1) {{
      // Process root element itself if it's an element
      var rn = buildNode(currentEl, 0);
      elementCount++;
      treeNodeMap.set(currentEl, rn);
      rootNode.children.push(rn);
      // Back-link portal links
      for (var pi = portalLinks.length - 1; pi >= 0; pi--) {{
        if (portalLinks[pi].treeNode === null && portalLinks[pi].depth === 0) {{
          portalLinks[pi].treeNode = rn;
        }}
      }}
    }}

    while (true) {{
      currentEl = walker.nextNode();
      if (!currentEl) break;

      if (maxElements > 0 && elementCount >= maxElements) {{
        truncated = true;
        break;
      }}

      // Compute depth from parent
      var parentEl = currentEl.parentElement;
      var parentDepth = depthMap.has(parentEl) ? depthMap.get(parentEl) : 0;
      var myDepth = parentDepth + 1;
      depthMap.set(currentEl, myDepth);

      if (maxDepth > 0 && myDepth > maxDepth) continue;

      var node = buildNode(currentEl, myDepth);
      elementCount++;
      treeNodeMap.set(currentEl, node);

      // Attach to parent tree node
      var parentNode = treeNodeMap.has(parentEl) ? treeNodeMap.get(parentEl) : rootNode;
      parentNode.children.push(node);

      // Back-link latest portal links to their treeNode
      for (var pj = portalLinks.length - 1; pj >= 0; pj--) {{
        if (portalLinks[pj].treeNode === null) {{
          portalLinks[pj].treeNode = node;
        }} else {{
          break;
        }}
      }}

      // Shadow DOM opt-in
      if (shadowDom && currentEl.shadowRoot) {{
        var shadowWalker = document.createTreeWalker(currentEl.shadowRoot, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var sEl = shadowWalker.nextNode();
        while (sEl) {{
          if (maxElements > 0 && elementCount >= maxElements) {{ truncated = true; break; }}
          var sDepth = myDepth + 1;
          depthMap.set(sEl, sDepth);
          if (maxDepth === 0 || sDepth <= maxDepth) {{
            var sNode = buildNode(sEl, sDepth);
            elementCount++;
            treeNodeMap.set(sEl, sNode);
            var sParent = treeNodeMap.has(sEl.parentElement) ? treeNodeMap.get(sEl.parentElement) : node;
            sParent.children.push(sNode);
          }}
          sEl = shadowWalker.nextNode();
        }}
      }}
    }}

    // === Pass 2: Portal stitching ===
    if (followPortals) {{
      for (var pk = 0; pk < portalLinks.length; pk++) {{
        var link = portalLinks[pk];
        var targetEl = document.getElementById(link.targetId);
        if (!targetEl || !link.treeNode) continue;

        portalCount++;
        var baseDepth = link.depth + 1;
        var portalWalker = document.createTreeWalker(targetEl, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var portalDepthMap = new WeakMap();
        portalDepthMap.set(targetEl, baseDepth);

        // Process target root
        var targetNode = buildNode(targetEl, baseDepth);
        targetNode.attrs.push('portal');
        elementCount++;
        link.treeNode.children.push(targetNode);

        var pEl = portalWalker.nextNode();
        while (pEl) {{
          if (maxElements > 0 && elementCount >= maxElements) {{ truncated = true; break; }}
          var pParent = pEl.parentElement;
          var pParentDepth = portalDepthMap.has(pParent) ? portalDepthMap.get(pParent) : baseDepth;
          var pDepth = pParentDepth + 1;
          portalDepthMap.set(pEl, pDepth);
          if (maxDepth === 0 || pDepth <= maxDepth) {{
            var pNode = buildNode(pEl, pDepth);
            elementCount++;
            // Attach to portal parent
            var pParentNode = treeNodeMap.has(pParent) ? treeNodeMap.get(pParent) : targetNode;
            pParentNode.children.push(pNode);
            treeNodeMap.set(pEl, pNode);
          }}
          pEl = portalWalker.nextNode();
        }}
      }}

      // Orphan portals: body direct children with ant-/rc- class not claimed
      var bodyChildren = document.body.children;
      for (var oi = 0; oi < bodyChildren.length; oi++) {{
        var orphan = bodyChildren[oi];
        if (orphan.id && claimedPortalIds.has(orphan.id)) continue;
        if (treeNodeMap.has(orphan)) continue;
        var oClass = typeof orphan.className === 'string' ? orphan.className : '';
        if (!/\b(ant-|rc-)/.test(oClass)) continue;
        // Treat as orphan portal
        portalCount++;
        var oNode = buildNode(orphan, 1);
        oNode.attrs.push('orphan-portal');
        elementCount++;
        rootNode.children.push(oNode);
        // Walk children of orphan portal
        var orphanWalker = document.createTreeWalker(orphan, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var oDepthMap = new WeakMap();
        oDepthMap.set(orphan, 1);
        var oEl = orphanWalker.nextNode();
        while (oEl) {{
          if (maxElements > 0 && elementCount >= maxElements) {{ truncated = true; break; }}
          var oParent = oEl.parentElement;
          var oParentD = oDepthMap.has(oParent) ? oDepthMap.get(oParent) : 1;
          var oDep = oParentD + 1;
          oDepthMap.set(oEl, oDep);
          if (maxDepth === 0 || oDep <= maxDepth) {{
            var oChild = buildNode(oEl, oDep);
            elementCount++;
            var oParNode = treeNodeMap.has(oParent) ? treeNodeMap.get(oParent) : oNode;
            oParNode.children.push(oChild);
            treeNodeMap.set(oEl, oChild);
          }}
          oEl = orphanWalker.nextNode();
        }}
      }}
    }}

    // === Render: recursive stringify ===
    function renderNode(node, depth) {{
      var line = '  '.repeat(depth) + '- ' + node.label;
      if (node.name) line += ' "' + node.name.replace(/"/g, '\\"') + '"';
      if (node.attrs.length > 0) line += ' [' + node.attrs.join(', ') + ']';
      var lines = [line];
      for (var ci = 0; ci < node.children.length; ci++) {{
        lines.push(renderNode(node.children[ci], depth + 1));
      }}
      return lines.join('\n');
    }}

    var snapshotLines = [];
    for (var ri = 0; ri < rootNode.children.length; ri++) {{
      snapshotLines.push(renderNode(rootNode.children[ri], 0));
    }}
    var snapshot = snapshotLines.join('\n');
    if (truncated) {{
      snapshot += '\n# ... truncated (' + maxElements + ' of ' + elementCount + '+ elements shown)';
    }}

    return {{
      snapshot: snapshot,
      refs: refs,
      meta: {{
        elementCount: elementCount,
        truncated: truncated,
        portalCount: portalCount,
        virtualScrollContainers: virtualScrollCount
      }}
    }};
  }};

  // === Auto-push DOM via Tauri IPC (when available) ===
  function autoPushDom() {{
    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (!ipc || !ipc.invoke) return;

    try {{
      const result = window.__CONNECTOR_SNAPSHOT__({{
        mode: 'ai',
        maxDepth: 0,
        maxElements: 5000,
        reactEnrich: true,
        followPortals: true,
        shadowDom: false
      }});
      ipc.invoke('plugin:connector|push_dom', {{
        payload: {{
          windowId: 'main',
          html: document.body.innerHTML.substring(0, 500000),
          textContent: document.body.innerText.substring(0, 200000),
          snapshot: result.snapshot || '',
          snapshotMode: 'ai',
          refs: JSON.stringify(result.refs || {{}}),
          meta: JSON.stringify(result.meta || {{}})
        }}
      }}).catch(function() {{}});
    }} catch (_) {{}}
  }}

  // Push DOM on load and after navigation/mutations
  if (document.readyState === 'complete') {{
    setTimeout(autoPushDom, 2000);
  }} else {{
    window.addEventListener('load', function() {{ setTimeout(autoPushDom, 2000); }});
  }}

  // Re-push on significant DOM changes (debounced)
  let pushTimer = null;
  const observer = new MutationObserver(function() {{
    if (pushTimer) clearTimeout(pushTimer);
    pushTimer = setTimeout(autoPushDom, 5000);
  }});
  observer.observe(document.body, {{ childList: true, subtree: true }});

  // === Auto-push console logs via Tauri IPC ===
  let logPushTimer = null;
  let lastLogPushIndex = 0;

  function autoPushLogs() {{
    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (!ipc || !ipc.invoke) return;
    if (consoleLogs.length <= lastLogPushIndex) return;

    const newEntries = consoleLogs.slice(lastLogPushIndex).map(function(l) {{
      return {{ level: l.level, message: l.message, timestamp: l.timestamp, windowId: 'main' }};
    }});
    lastLogPushIndex = consoleLogs.length;

    ipc.invoke('plugin:connector|push_logs', {{
      payload: {{ entries: newEntries }}
    }}).catch(function() {{}});
  }}

  setInterval(autoPushLogs, 3000);

  // === Alt+Shift+Click element picker ===
  document.addEventListener('click', function(e) {{
    if (!e.altKey || !e.shiftKey) return;
    e.preventDefault();
    e.stopPropagation();

    const el = e.target;
    const rect = el.getBoundingClientRect();
    const info = {{
      tag: el.tagName.toLowerCase(),
      id: el.id || null,
      className: el.className || null,
      text: el.textContent ? el.textContent.trim().substring(0, 200) : null,
      rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
      attributes: {{}},
    }};

    Array.from(el.attributes).forEach(function(attr) {{
      info.attributes[attr.name] = attr.value;
    }});

    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (ipc && ipc.invoke) {{
      ipc.invoke('plugin:connector|set_pointed_element', {{
        payload: {{ element: info }}
      }}).catch(function() {{}});
    }}

    origConsole.log('[connector] Element picked:', info.tag, info.id || '', info.className || '');
  }}, true);

  // Start connection
  connect();
}})();
"#
    )
}

fn find_available_port(start: u16, end: u16) -> Option<u16> {
    (start..end).find(|&port| TcpListener::bind(("127.0.0.1", port)).is_ok())
}
