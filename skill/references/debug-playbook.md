# Debug Playbook

Step-by-step recipes for debugging common Tauri app issues. Each recipe follows the Snapshot -> Act -> Verify pattern and shows both MCP and CLI commands.

---

## Recipe 1: App Crashes or Blank Screen on Launch

The app opens but shows a white screen or crashes immediately.

**Step 1: Check console errors**
```bash
# MCP
read_logs(level: "error", lines: 50)

# CLI
tauri-connector logs -l error -n 50
```

**Step 2: Check backend state**
```bash
ipc_get_backend_state()
tauri-connector state
```
If this fails, the WebSocket server didn't start -- check Rust build logs.

**Step 3: Check if the webview loaded at all**
```bash
webview_execute_js(script: "(() => { return { url: location.href, readyState: document.readyState, title: document.title } })()")
tauri-connector eval "JSON.stringify({url:location.href,state:document.readyState})"
```

**Step 4: Screenshot for visual evidence**
```bash
webview_screenshot(format: "png", maxWidth: 1280)
tauri-connector screenshot /tmp/blank-screen.png
```

**Common causes:**
- Missing `withGlobalTauri: true` in tauri.conf.json
- Frontend dev server not running (check the URL in state output)
- JS bundle errors (check console errors for syntax/import failures)
- Missing permissions in capabilities file

---

## Recipe 2: Button Click Does Nothing

A button or action in the UI has no visible effect.

**Step 1: Snapshot and identify the element**
```bash
webview_dom_snapshot(mode: "ai")
tauri-connector snapshot -i
```

**Step 2: Start IPC monitoring before clicking**
```bash
ipc_monitor(action: "start")
tauri-connector ipc monitor
```

**Step 3: Click the button**
```bash
webview_interact(action: "click", selector: "@e5")
tauri-connector click @e5
```

**Step 4: Check what happened**
```bash
# Did an IPC call fire?
ipc_get_captured(limit: 10)
tauri-connector ipc captured -l 10

# Any errors?
read_logs(level: "error,warn", lines: 10)
tauri-connector logs -l error,warn -n 10

# Did the DOM change?
webview_dom_snapshot(mode: "ai")
tauri-connector snapshot -i
```

**Step 5: Clean up**
```bash
ipc_monitor(action: "stop")
```

**Diagnosis guide:**
- No IPC call fired -> Event handler not attached or element is wrong target (check if clicking a child element)
- IPC call fired with error -> Backend command is failing (check the error field)
- IPC call succeeded but no DOM change -> Frontend state update issue (check React state)
- Console error on click -> JS error in handler

---

## Recipe 3: Form Submission Fails

A form doesn't submit or shows unexpected validation errors.

**Step 1: Scope snapshot to the form**
```bash
webview_dom_snapshot(selector: "form", mode: "ai")
tauri-connector snapshot -i -s "form"
```

**Step 2: Check current field values**
```bash
tauri-connector get value @e3   # email field
tauri-connector get value @e5   # password field
```

**Step 3: Start monitoring, then submit**
```bash
ipc_monitor(action: "start")
webview_interact(action: "click", selector: "@e8")  # submit button
```

**Step 4: Check results**
```bash
# IPC calls during submission
ipc_get_captured(limit: 10)

# Any validation errors appeared?
webview_find_element(selector: "error|invalid|required", strategy: "regex", target: "class")
webview_dom_snapshot(selector: "form", mode: "ai")

# Console errors
read_logs(level: "error", lines: 10)
```

**Step 5: Check for validation message text**
```bash
webview_search_snapshot(pattern: "required|invalid|error|please", context: 2)
```

---

## Recipe 4: Slow IPC Response

An action takes too long to complete.

**Step 1: Monitor IPC with timing**
```bash
ipc_monitor(action: "start")
```

**Step 2: Trigger the slow action**
```bash
webview_interact(action: "click", selector: "@e5")
```

**Step 3: Wait, then check captured IPC**
```bash
# Each captured entry includes duration_ms
ipc_get_captured(limit: 20)
```

**Step 4: Identify the slow command**
Look for entries where `duration_ms` is high. Cross-reference with:
```bash
read_logs(pattern: "slow|timeout|performance", lines: 50)
```

---

## Recipe 5: Event Not Firing

Expected app events aren't being emitted.

**Step 1: Start listening for the expected events**
```bash
ipc_listen(action: "start", events: ["state:update", "data:saved", "user:action"])
tauri-connector events listen state:update,data:saved,user:action
```

**Step 2: Trigger the action that should emit events**
```bash
webview_interact(action: "click", selector: "@e5")
```

**Step 3: Check captured events**
```bash
event_get_captured(limit: 20)
tauri-connector events captured -l 20
```

**Step 4: If no events captured, check IPC instead**
```bash
ipc_monitor(action: "start")
# Re-trigger action
ipc_get_captured(limit: 20)
```

**Step 5: Clean up**
```bash
ipc_listen(action: "stop")
ipc_monitor(action: "stop")
```

---

## Recipe 6: Incorrect DOM State After Navigation

After navigating to a new page/route, elements are missing or in wrong state.

**Step 1: Snapshot before navigation**
```bash
webview_dom_snapshot(mode: "ai")
```

**Step 2: Navigate**
```bash
webview_interact(action: "click", selector: "@e5")  # navigation link
```

**Step 3: Wait for new page to load**
```bash
webview_wait_for(selector: ".new-page-indicator", timeout: 10000)
# or
webview_wait_for(text: "Expected Page Title", strategy: "text", timeout: 10000)
```

**Step 4: Snapshot the new state**
```bash
webview_dom_snapshot(mode: "ai")
```

**Step 5: Compare the two snapshots**
Look for:
- Missing expected elements
- Stale elements from previous page
- Incorrect content or attributes
- Missing React components (check component names in ai mode)

---

## Recipe 7: Drag and Drop Not Working

Drag operation has no effect or drops in wrong position.

**Step 1: Identify source and target**
```bash
webview_dom_snapshot(mode: "ai")
tauri-connector snapshot -i
```

**Step 2: Check if source has `draggable` attribute**
```bash
webview_execute_js(script: "(() => { const el = document.querySelector('#source'); return { draggable: el.draggable, tagName: el.tagName } })()")
```

**Step 3: Try different strategies**
```bash
# Default auto-detection
webview_interact(action: "drag", selector: "@e3", targetSelector: "@e7")

# Force pointer (for dnd-kit, SortableJS)
webview_interact(action: "drag", selector: "@e3", targetSelector: "@e7", dragStrategy: "pointer", steps: 20, durationMs: 800)

# Force HTML5 DnD (for draggable="true" elements)
webview_interact(action: "drag", selector: "@e3", targetSelector: "@e7", dragStrategy: "html5dnd", steps: 15)
```

**Step 4: Verify result**
```bash
webview_dom_snapshot(mode: "ai")
```

**Troubleshooting:**
- No movement at all -> Wrong strategy. Try both `pointer` and `html5dnd`
- Drag starts but doesn't register -> Increase `steps` (some libs need >5px movement threshold)
- Drops in wrong position -> Increase `steps` and `durationMs` for more precise pacing
- Library-specific: dnd-kit needs `pointer`, react-beautiful-dnd needs `html5dnd`

---

## Recipe 8: Memory Leak Investigation

App gets slower over time or uses increasing memory.

**Step 1: Baseline state**
```bash
webview_execute_js(script: "(() => { return { heap: performance.memory?.usedJSHeapSize, nodes: document.querySelectorAll('*').length } })()")
```

**Step 2: Perform the suspected leaky action multiple times**
```bash
# Example: open and close a modal 10 times
webview_interact(action: "click", selector: "@e5")  # open
webview_wait_for(selector: ".ant-modal", timeout: 3000)
webview_interact(action: "click", selector: ".ant-modal-close")
webview_wait_for(text: "", timeout: 1000)  # wait for close
# Repeat...
```

**Step 3: Check state after repetitions**
```bash
webview_execute_js(script: "(() => { return { heap: performance.memory?.usedJSHeapSize, nodes: document.querySelectorAll('*').length } })()")
```

**Step 4: Check for detached DOM nodes**
```bash
webview_execute_js(script: "(() => { const portals = document.querySelectorAll('[class*=portal], [class*=popup], [class*=tooltip], [class*=dropdown]'); return { count: portals.length, elements: Array.from(portals).map(e => e.className).slice(0, 10) } })()")
```

**Step 5: Check console for warnings**
```bash
read_logs(level: "warn", pattern: "leak|detach|unmount|cleanup")
```

---

## Recipe 9: Console Error Triage

Sort through a pile of console errors to find the root cause.

**Step 1: Get all errors**
```bash
read_logs(level: "error", lines: 200)
```

**Step 2: Group by pattern**
```bash
read_logs(level: "error", pattern: "TypeError")
read_logs(level: "error", pattern: "NetworkError|fetch|CORS")
read_logs(level: "error", pattern: "Cannot read|undefined|null")
```

**Step 3: Check historical errors (survives restarts)**
```bash
read_log_file(source: "console", level: "error", lines: 500)
```

**Step 4: Check if errors correlate with IPC failures**
```bash
ipc_monitor(action: "start")
# Trigger the error-producing action
read_logs(level: "error", lines: 5)
ipc_get_captured(limit: 5)
```

---

## Recipe 10: Multi-Window Debugging

Debug issues across multiple Tauri windows.

**Step 1: List all windows**
```bash
manage_window(action: "list")
tauri-connector windows
```

**Step 2: Snapshot each window**
```bash
webview_dom_snapshot(mode: "ai", windowId: "main")
webview_dom_snapshot(mode: "ai", windowId: "settings")
```

**Step 3: Check logs per window**
```bash
read_log_file(source: "console", windowId: "main", lines: 50)
read_log_file(source: "console", windowId: "settings", lines: 50)
```

**Step 4: Screenshot each**
```bash
webview_screenshot(windowId: "main")
webview_screenshot(windowId: "settings")
```
