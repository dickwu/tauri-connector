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

/// Manages the internal WebSocket bridge to the webview.
#[derive(Clone)]
pub struct Bridge {
    /// Port the internal WebSocket listens on
    port: u16,
    /// Channel to send scripts to the connected webview bridge client
    script_tx: mpsc::UnboundedSender<String>,
    /// Pending JS evaluation results, keyed by request ID
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>>,
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
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

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

        loop {
            let (stream, addr) = listener
                .accept()
                .await
                .map_err(|e| e.to_string())?;

            println!("[connector][bridge] Webview client connected from {addr}");

            let ws_stream = tokio_tungstenite::accept_async(stream)
                .await
                .map_err(|e| e.to_string())?;

            let pending = self.pending.clone();

            // Use a single loop with select! instead of split() to avoid
            // potential buffering issues between SplitSink and SplitStream
            let mut ws_stream = ws_stream;

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
                            None => {
                                break;
                            }
                        }
                    }
                }
            }

            println!("[connector][bridge] Webview client disconnected");

            // Reconnect: create a new script_rx channel for next connection
            // The old script_rx is consumed, so we break and let the webview reconnect
            break;
        }

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

  // DOM snapshot helpers
  window.__CONNECTOR_DOM_SNAPSHOT__ = function(type, selector) {{
    const root = selector ? document.querySelector(selector) : document.body;
    if (!root) return {{ error: 'Element not found: ' + selector }};

    if (type === 'accessibility') {{
      return buildAccessibilityTree(root, 0);
    }} else {{
      return buildStructureTree(root, 0);
    }}
  }};

  function buildAccessibilityTree(el, depth) {{
    const indent = '  '.repeat(depth);
    let lines = [];
    const role = el.getAttribute('role') || getImplicitRole(el);
    const name = getAccessibleName(el);
    const tag = el.tagName.toLowerCase();

    let line = indent + '- ' + (role || tag);
    if (name) line += ' "' + name.substring(0, 100) + '"';

    const states = [];
    if (el.getAttribute('aria-expanded')) states.push('expanded=' + el.getAttribute('aria-expanded'));
    if (el.getAttribute('aria-selected')) states.push('selected=' + el.getAttribute('aria-selected'));
    if (el.getAttribute('aria-checked')) states.push('checked=' + el.getAttribute('aria-checked'));
    if (el.disabled) states.push('disabled');
    if (states.length > 0) line += ' [' + states.join(', ') + ']';

    lines.push(line);

    for (const child of el.children) {{
      const childLines = buildAccessibilityTree(child, depth + 1);
      if (typeof childLines === 'string') {{
        lines.push(childLines);
      }}
    }}

    return lines.join('\n');
  }}

  function buildStructureTree(el, depth) {{
    const indent = '  '.repeat(depth);
    let lines = [];
    const tag = el.tagName.toLowerCase();
    let line = indent + '- ' + tag;
    if (el.id) line += '#' + el.id;
    if (el.className && typeof el.className === 'string') {{
      const classes = el.className.trim().split(/\s+/).slice(0, 5);
      if (classes.length > 0 && classes[0]) line += '.' + classes.join('.');
    }}
    const testId = el.getAttribute('data-testid');
    if (testId) line += ' [data-testid="' + testId + '"]';

    lines.push(line);

    for (const child of el.children) {{
      const childLines = buildStructureTree(child, depth + 1);
      if (typeof childLines === 'string') {{
        lines.push(childLines);
      }}
    }}

    return lines.join('\n');
  }}

  function getImplicitRole(el) {{
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    if (tag === 'input') return getInputRole(el);
    const roleMap = {{
      'a': el.href ? 'link' : null,
      'button': 'button',
      'select': 'combobox',
      'textarea': 'textbox',
      'img': 'img',
      'nav': 'navigation',
      'main': 'main',
      'header': 'banner',
      'footer': 'contentinfo',
      'aside': 'complementary',
      'form': 'form',
      'table': 'table',
      'ul': 'list',
      'ol': 'list',
      'li': 'listitem',
      'h1': 'heading',
      'h2': 'heading',
      'h3': 'heading',
      'h4': 'heading',
      'h5': 'heading',
      'h6': 'heading',
    }};
    return roleMap[tag] || null;
  }}

  function getInputRole(el) {{
    const type = String(el.type || 'text').toLowerCase();
    const map = {{
      'checkbox': 'checkbox',
      'radio': 'radio',
      'range': 'slider',
      'search': 'searchbox',
      'text': 'textbox',
      'email': 'textbox',
      'tel': 'textbox',
      'url': 'textbox',
      'number': 'spinbutton',
    }};
    return map[type] || 'textbox';
  }}

  function getAccessibleName(el) {{
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel) return ariaLabel;
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {{
      const labelEl = document.getElementById(labelledBy);
      if (labelEl) return labelEl.textContent.trim();
    }}
    if (el.tagName === 'IMG') return el.alt || '';
    if (el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA') {{
      if (el.id) {{
        const label = document.querySelector('label[for="' + el.id + '"]');
        if (label) return label.textContent.trim();
      }}
      return el.placeholder || '';
    }}
    if (['BUTTON', 'A', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6'].includes(el.tagName)) {{
      return el.textContent.trim().substring(0, 100);
    }}
    return '';
  }}

  // === Auto-push DOM via Tauri IPC (when available) ===
  function autoPushDom() {{
    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (!ipc || !ipc.invoke) return;

    try {{
      const a11y = buildAccessibilityTree(document.body, 0);
      const structure = buildStructureTree(document.body, 0);
      ipc.invoke('plugin:connector|push_dom', {{
        payload: {{
          windowId: 'main',
          html: document.body.innerHTML.substring(0, 500000),
          textContent: document.body.innerText.substring(0, 200000),
          accessibilityTree: typeof a11y === 'string' ? a11y : '',
          structureTree: typeof structure === 'string' ? structure : '',
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
