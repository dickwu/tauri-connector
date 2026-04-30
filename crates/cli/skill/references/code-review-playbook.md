# Code Review Playbook

Workflow recipes for reviewing Tauri app code changes using live inspection. Each workflow uses the running app to validate code correctness beyond static analysis.

---

## Workflow 1: Visual Regression Check

Verify that code changes don't break the visual appearance of the app.

### Before the Change

```bash
# 1. Screenshot key pages/states
webview_screenshot(format: "png", maxWidth: 1280)
# Save or note the output for comparison

# 2. Snapshot the DOM structure
webview_dom_snapshot(mode: "ai")
# Note key element refs and structure
```

### After the Change

```bash
# 1. Rebuild and relaunch the app
# 2. Screenshot the same pages/states
webview_screenshot(format: "png", maxWidth: 1280)

# 3. Snapshot DOM structure again
webview_dom_snapshot(mode: "ai")
```

### What to Compare

- **Layout shifts**: Elements moved position or changed size
- **Missing elements**: Components that were present before are gone
- **Style changes**: Colors, fonts, spacing, borders changed unexpectedly
- **Overflow issues**: Content clipping or scrollbar changes
- **Responsive breakpoints**: Resize and re-check

```bash
# Check specific styles that might have changed
webview_get_styles(selector: ".header", properties: ["height", "padding", "background-color"])
webview_get_styles(selector: ".sidebar", properties: ["width", "display", "flex-direction"])

# Resize to test responsive behavior
manage_window(action: "resize", width: 768, height: 1024)
webview_screenshot()
manage_window(action: "resize", width: 1280, height: 800)
webview_screenshot()
```

---

## Workflow 2: Accessibility Audit

Review the app for accessibility compliance using the accessibility snapshot mode.

### Full Page Audit

```bash
# 1. Get accessibility tree
webview_dom_snapshot(mode: "accessibility")
tauri-connector snapshot -i --mode accessibility
```

### Checklist

Review the accessibility tree for:

**Semantic Structure**
- [ ] All interactive elements have meaningful roles (button, link, textbox, combobox, etc.)
- [ ] Headings follow correct hierarchy (h1 -> h2 -> h3, no skipped levels)
- [ ] Landmarks present: `banner`, `navigation`, `main`, `contentinfo`
- [ ] Lists use correct `list`/`listitem` structure

**Labels and Names**
- [ ] All form fields have associated labels (visible or `aria-label`)
- [ ] All buttons have descriptive text or `aria-label`
- [ ] Images have `alt` text (or `role="presentation"` for decorative)
- [ ] Icons used as buttons have accessible names

**Interactive Elements**
- [ ] All clickable elements are keyboard-focusable (`tabindex` or native interactive elements)
- [ ] Custom controls have correct ARIA roles and states
- [ ] Expandable elements have `aria-expanded` state
- [ ] Modals/drawers have `aria-modal` and trap focus

**Forms**
```bash
# Scope to form for focused audit
webview_dom_snapshot(selector: "form", mode: "accessibility")

# Check that required fields are marked
webview_search_snapshot(pattern: "required|aria-required", context: 1, mode: "accessibility")
```

- [ ] Required fields marked with `aria-required`
- [ ] Error messages associated with fields via `aria-describedby`
- [ ] Form has a submit mechanism (button type="submit")

**Dynamic Content**
```bash
# Check ARIA live regions
webview_find_element(selector: "[aria-live]", strategy: "css")
webview_execute_js(script: "(() => { return Array.from(document.querySelectorAll('[aria-live]')).map(el => ({ tag: el.tagName, live: el.getAttribute('aria-live'), text: el.textContent.slice(0, 50) })) })()")
```

- [ ] Status messages use `aria-live` regions
- [ ] Loading indicators are announced to screen readers
- [ ] Toast/notification components have appropriate ARIA

---

## Workflow 3: Component Tree Review

Review the React component hierarchy for architectural issues.

### Get the Component Tree

```bash
webview_dom_snapshot(mode: "ai", reactEnrich: true, followPortals: true)
```

### What to Look For

**Component Naming**
- Components should have meaningful names (not `_default`, `Anonymous`, or minified names in dev mode)
- Verify component boundaries align with features/domains

**Portal Management**
- Portal content should be stitched to its trigger in the snapshot
- Check that modals/dropdowns/tooltips are properly associated
- Verify portals are cleaned up (no orphaned portal content)

```bash
# Find all portal elements
webview_search_snapshot(pattern: "portal", context: 2)

# Check for orphaned portals (visible but no trigger)
webview_execute_js(script: "(() => { const portals = document.querySelectorAll('.ant-modal-root, .ant-dropdown, .ant-tooltip, .ant-popover'); return portals.length + ' portal containers found' })()")
```

**Virtual Scroll**
- Virtual scroll containers should be annotated in the snapshot
- Verify visible item count matches expectations

**Deep Nesting**
```bash
# Check nesting depth
webview_dom_snapshot(mode: "ai", maxDepth: 3)
```
If max depth 3 cuts off important content, the component tree may be too deeply nested.

---

## Workflow 4: IPC Contract Validation

Verify that frontend IPC calls match the expected backend command contracts.

### Capture All IPC During a User Flow

```bash
# 1. Clear old logs and start fresh
clear_logs(source: "ipc")
ipc_monitor(action: "start")

# 2. Walk through the user flow step by step
webview_dom_snapshot(mode: "ai")
webview_interact(action: "click", selector: "@e5")   # Step 1
webview_wait_for(text: "Loaded")
webview_interact(action: "click", selector: "@e8")   # Step 2
# ... continue through flow

# 3. Get all captured IPC
ipc_get_captured(limit: 100)

# 4. Stop monitoring
ipc_monitor(action: "stop")
```

### Validation Checklist

For each IPC call, verify:

- [ ] **Command name** matches a registered Tauri command
- [ ] **Arguments** contain all required fields with correct types
- [ ] **No unexpected errors** in responses
- [ ] **No duplicate calls** (same command fired multiple times for one action)
- [ ] **Timing** is reasonable (`duration_ms` under expected thresholds)
- [ ] **Ordering** is correct (dependent calls happen after their prerequisites)

### Test Edge Cases

```bash
# Execute commands directly with invalid args to verify error handling
ipc_execute_command(command: "update_user", args: {})
ipc_execute_command(command: "update_user", args: {"id": -1})
ipc_execute_command(command: "update_user", args: {"id": "not-a-number"})
```

---

## Workflow 5: DOM Structure Review

Review the DOM output of a component or page for structural correctness.

### Scoped Snapshot

```bash
# Snapshot specific components
webview_dom_snapshot(selector: ".user-profile", mode: "ai")
webview_dom_snapshot(selector: ".data-table", mode: "ai")
webview_dom_snapshot(selector: ".navigation", mode: "ai")
```

### What to Look For

**Semantic HTML**
```bash
# Check for divs used as buttons
webview_find_element(selector: "div[onclick], span[onclick]", strategy: "css")

# Find non-semantic containers
webview_search_snapshot(pattern: "div > div > div > div", context: 1)
```

- [ ] Buttons use `<button>` or `<a>`, not `<div onclick>`
- [ ] Lists use `<ul>/<ol>/<li>`, not nested `<div>`s
- [ ] Tables use `<table>/<tr>/<td>`, not CSS grid with divs
- [ ] Forms use `<form>` with `<label>` elements

**Data Attributes**
```bash
# Check test ID coverage
webview_search_snapshot(pattern: "data-testid", context: 1)

# Count elements with vs without test IDs
webview_execute_js(script: "(() => { const interactive = document.querySelectorAll('button, a, input, select, textarea'); const withId = document.querySelectorAll('[data-testid]'); return { interactive: interactive.length, withTestId: withId.length } })()")
```

**Empty States**
```bash
# Check that empty lists show appropriate UI
webview_execute_js(script: "(() => { const lists = document.querySelectorAll('[class*=list], [class*=table]'); return Array.from(lists).map(l => ({ class: l.className.slice(0,40), children: l.children.length })) })()")
```

---

## Workflow 6: Event Flow Verification

Validate that user actions trigger the correct sequence of Tauri events.

### Define Expected Flow

Before testing, document the expected event sequence:
```
Action: User submits form
Expected: data:saving -> data:saved -> ui:refresh
```

### Capture and Validate

```bash
# 1. Clear and start
clear_logs(source: "events")
ipc_listen(action: "start", events: ["data:saving", "data:saved", "ui:refresh", "error:*"])

# 2. Perform the action
webview_interact(action: "click", selector: "@e8")  # submit

# 3. Wait for completion
webview_wait_for(text: "Saved", timeout: 10000)

# 4. Check event sequence
event_get_captured(limit: 20)
```

### Validation

- [ ] All expected events fired in correct order
- [ ] No unexpected events (especially error events)
- [ ] Event payloads contain expected data
- [ ] No duplicate events
- [ ] Timing between events is reasonable

---

## Workflow 7: State Management Review

Verify that app state updates correctly after actions.

### Read State Before and After

```bash
# 1. Capture initial state
webview_execute_js(script: "(() => { return JSON.stringify(window.__APP_STATE__ || 'no global state') })()")

# Or for React: read component state
webview_execute_js(script: "(() => { const root = document.getElementById('root'); const fiber = root?._reactRootContainer?._internalRoot?.current; return fiber ? 'React root found' : 'No React root' })()")

# 2. Perform action
webview_interact(action: "click", selector: "@e5")

# 3. Capture state after
webview_execute_js(script: "(() => { return JSON.stringify(window.__APP_STATE__ || 'no global state') })()")
```

### Zustand Store Inspection (common in admin/front apps)

```bash
webview_execute_js(script: "(() => { const stores = Object.keys(window).filter(k => k.includes('Store') || k.includes('store')); return stores })()")
```

---

## Workflow 8: Error Boundary Review

Verify that error boundaries catch and display errors gracefully.

### Trigger Errors Deliberately

```bash
# 1. Inject an error into a component
webview_execute_js(script: "(() => { const btn = document.querySelector('.action-button'); if (btn) { btn.addEventListener('click', () => { throw new Error('Test error boundary'); }, { once: true }); return 'Error handler attached'; } })()")

# 2. Click to trigger
webview_interact(action: "click", selector: ".action-button")

# 3. Check what the user sees
webview_dom_snapshot(mode: "ai")
webview_screenshot()

# 4. Check console
read_logs(level: "error", lines: 10)
```

### Check Error Display

- [ ] Error boundary catches the error (no blank screen)
- [ ] User-friendly message displayed (not raw stack trace)
- [ ] Retry/recover option available
- [ ] Other parts of the app still work (error is contained)

---

## Workflow 9: Performance Spot Check

Quick performance review using available tools.

### DOM Size Check
```bash
webview_execute_js(script: "(() => { return { totalNodes: document.querySelectorAll('*').length, bodySize: document.body.innerHTML.length, scripts: document.querySelectorAll('script').length, styles: document.querySelectorAll('style, link[rel=stylesheet]').length } })()")
```

### Check for Excessive Renders (React)
```bash
# Monitor console for React dev mode warnings
read_logs(level: "warn", pattern: "render|performance|slow")
```

### Check Network Activity via IPC
```bash
# Monitor IPC to see if too many calls are made
ipc_monitor(action: "start")
# Navigate around the app
ipc_get_captured(limit: 100)
# Look for duplicate or excessive calls
```

### Load Time Check
```bash
webview_execute_js(script: "(() => { const perf = performance.getEntriesByType('navigation')[0]; return perf ? { domContentLoaded: Math.round(perf.domContentLoadedEventEnd - perf.startTime), loadEvent: Math.round(perf.loadEventEnd - perf.startTime), domInteractive: Math.round(perf.domInteractive - perf.startTime) } : 'No navigation timing' })()")
```

---

## Quick Review Checklist

Use this condensed checklist for rapid code review validation:

```bash
# 1. Visual check
webview_screenshot(format: "png", maxWidth: 1280)

# 2. Accessibility check
webview_dom_snapshot(mode: "accessibility")

# 3. Component structure
webview_dom_snapshot(mode: "ai", reactEnrich: true)

# 4. Console health
read_logs(level: "error,warn", lines: 50)

# 5. IPC correctness (if changes touch backend)
ipc_monitor(action: "start")
# ... trigger relevant actions ...
ipc_get_captured(limit: 20)
ipc_monitor(action: "stop")

# 6. DOM size / performance
webview_execute_js(script: "(() => { return { nodes: document.querySelectorAll('*').length, heap: performance.memory?.usedJSHeapSize } })()")
```
