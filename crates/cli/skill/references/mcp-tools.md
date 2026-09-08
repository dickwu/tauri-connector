# MCP Tools Reference

Complete parameter tables for tauri-connector MCP tools. Configure in `.mcp.json`:

```json
{ "mcpServers": { "tauri-connector": { "url": "http://127.0.0.1:9556/mcp" } } }
```

The standalone MCP server (`tauri-connector-mcp`) adds one additional tool: `driver_session`. Legacy `/sse` remains available for older clients.

---

## Webview Interaction Tools

### webview_interact

Perform gestures on elements: click, double-click, focus, scroll, hover, drag.

| Param | Type | Required | Default | Description |
|---|---|---|---|---|
| `action` | string | yes | | `click`, `double-click`, `dblclick`, `focus`, `scroll`, `hover`, `hover-off`, `drag` |
| `selector` | string | | | Source element (CSS, XPath, or text). Use `@eN` refs from snapshots |
| `strategy` | string | | `css` | `css`, `xpath`, `text` |
| `x` | number | | | Source X coordinate (alternative to selector) |
| `y` | number | | | Source Y coordinate (alternative to selector) |
| `direction` | string | | | Scroll direction: `up`, `down`, `left`, `right` |
| `distance` | number | | 300 | Scroll distance in pixels |
| `targetSelector` | string | | | Drag target CSS selector |
| `targetX` | number | | | Drag target X coordinate |
| `targetY` | number | | | Drag target Y coordinate |
| `steps` | number | | 10 | Drag intermediate move events (higher = smoother) |
| `durationMs` | number | | 300 | Drag total duration in ms |
| `dragStrategy` | string | | `auto` | `auto` (checks `el.draggable`), `pointer`, `html5dnd` |

### webview_keyboard

Type text or press keys with optional modifiers.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `type` or `press` |
| `text` | string | | Text to type character-by-character (for `type` action) |
| `key` | string | | Key name for `press`: `Enter`, `Tab`, `Escape`, `Backspace`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Home`, `End`, `PageUp`, `PageDown`, `Delete`, `Space`, `F1`-`F12`, or any single character |
| `modifiers` | string[] | | `ctrl`, `shift`, `alt`, `meta` |

### webview_wait_for

Poll for an element state, text, URL glob, document load state, or JavaScript condition (100ms interval). Conditions are combined when more than one is supplied.

| Param | Type | Default | Description |
|---|---|---|---|
| `selector` | string | | CSS, XPath, or text to wait for |
| `strategy` | string | `css` | `css`, `xpath`, `text` |
| `text` | string | | Text content to wait for (alternative to selector) |
| `url` | string | | Glob pattern matched against `location.href` (for example `**/settings*`) |
| `loadState` | string | | `domcontentloaded` or `load`; `networkidle` explicitly returns `unsupported_condition` |
| `fn` | string | | JavaScript expression/function/body that returns truthy |
| `state` | string | `attached` | Selector state: `attached`, `detached`, `visible`, `hidden` |
| `timeout` | number | 5000 | Timeout in ms |

Timeouts return `{ found: false, timeout: true, elapsed_ms }`; successful waits return `{ found: true, elapsed_ms }`.

### webview_locator

Find an element by semantic locator and optionally act on the matched element. Use this when refs are unavailable or stale.

| Param | Type | Default | Description |
|---|---|---|---|
| `role` | string | | ARIA or implicit role, e.g. `button`, `textbox`, `link` |
| `text` | string | | Visible text locator |
| `label` | string | | Associated label text for form controls |
| `placeholder` | string | | Placeholder text |
| `alt` | string | | Image alt text |
| `title` | string | | Title attribute |
| `testId` | string | | `data-testid`, `data-test-id`, `data-test`, or `testid` |
| `name` | string | | Accessible-name filter applied after the primary locator |
| `exact` | boolean | false | Exact text/name match instead of case-insensitive substring |
| `first` | boolean | false | Force first match |
| `last` | boolean | false | Force last match |
| `nth` | number | | Zero-based match index |
| `action` | string | | `click`, `fill`, `type`, `hover`, `focus`, `check`, `uncheck`, `text` |
| `value` | string | | Value for `fill` or `type` |
| `windowId` | string | | Target window |

---

## DOM & Inspection Tools

### webview_dom_snapshot

Get a structured DOM tree. The `ai` mode includes ref IDs, React component names, portal stitching, and virtual scroll detection.

| Param | Type | Default | Description |
|---|---|---|---|
| `mode` | string | `ai` | `ai` (refs + React enrichment), `accessibility` (ARIA roles/names), `structure` (tags/classes only) |
| `selector` | string | | CSS selector to scope to a subtree |
| `maxDepth` | number | unlimited | Maximum tree depth |
| `maxElements` | number | unlimited | Maximum element count |
| `maxTokens` | number | 4000 (MCP), 0 elsewhere | Token budget for inline output. Overflow spills to on-disk subtree files. `0` = unlimited. |
| `noSplit` | boolean | false | Disable subtree file splitting -- return full inline output regardless of budget |
| `reactEnrich` | boolean | true | Include React component names from fiber internals |
| `followPortals` | boolean | true | Stitch portals (detected via `aria-controls`/`aria-owns`) to their triggers |
| `shadowDom` | boolean | false | Traverse shadow DOM boundaries |
| `windowId` | string | | Target a specific window (from `manage_window(action: "list")`) |

**Output when a split occurs** (`meta.split == true`):

```jsonc
{
  "snapshot": "<inline layout skeleton with `file=subtree-K.txt` markers>",
  "refs": { "e0": {...}, "e1": {...} },
  "meta": {
    "split": true,
    "snapshotId": "<uuid>",
    "allRefsPath": "<log_dir>/snapshots/<snapshotId>/refs.json",
    "subtreeFiles": [
      { "name": "subtree-0.txt", "label": "main>ul", "path": "...", "estimatedTokens": 3200 }
    ]
  }
}
```

Read spilled subtrees with the `Read` tool on the `path`, or via CLI: `tauri-connector snapshots read <uuid> subtree-0.txt`. `webview_search_snapshot` automatically matches against the merged full text (skeleton + all subtree contents), so searches never hide behind the budget.

### webview_find_element

Find elements by CSS, XPath, visible text, or regex pattern.

| Param | Type | Required | Default | Description |
|---|---|---|---|---|
| `selector` | string | yes | | Search query (CSS selector, XPath, text, or regex pattern) |
| `strategy` | string | | `css` | `css`, `xpath`, `text`, `regex` |
| `target` | string | | `text` | What regex matches against: `text`, `class`, `id`, `attr`, `all` |

### webview_search_snapshot

Regex search over the DOM snapshot with context lines. Uses cached snapshot if fresh (<10 seconds).

| Param | Type | Required | Default | Description |
|---|---|---|---|---|
| `pattern` | string | yes | | Regex pattern to search for |
| `context` | number | | 2 | Context lines around matches (max 10) |
| `mode` | string | | `ai` | Snapshot mode: `ai`, `accessibility`, `structure` |

### get_cached_dom

Returns the DOM that the frontend automatically pushes on page load and DOM mutations (5-second debounce). Faster than `webview_dom_snapshot` because there's no round-trip -- the data is already cached server-side.

| Param | Type | Description |
|---|---|---|
| `windowId` | string | Target window (optional) |

### webview_get_styles

Get computed CSS properties for a CSS-selected element.

| Param | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector |
| `properties` | string[] | | Specific properties to return (returns all if omitted) |

### webview_execute_js

Execute arbitrary JavaScript in the webview. Use IIFE for return values.

| Param | Type | Required | Description |
|---|---|---|---|
| `script` | string | yes | JavaScript code. Wrap in `(() => { return value; })()` to get a return value |
| `windowId` | string | | Target window |

### webview_screenshot

Native window capture via `xcap`. Falls back to `@zumer/snapdom` if unavailable. Returns MCP image content.

| Param | Type | Default | Description |
|---|---|---|---|
| `format` | string | `png` | `png`, `jpeg`, `webp` |
| `quality` | number | 80 | JPEG/WebP quality (0-100) |
| `maxWidth` | number | | Max width in pixels (maintains aspect ratio) |
| `windowId` | string | | Target window |
| `selector` | string | | Optional CSS selector or `@eN` ref for element-scoped capture |
| `save` | boolean | false | Save the capture as an artifact |
| `outputDir` | string | connector artifact dir | Directory for saved artifacts |
| `nameHint` | string | | Slug included in generated artifact filenames |
| `overwrite` | boolean | false | Allow replacing the requested artifact path |
| `annotate` | boolean | false | Overlay numbered labels for `@eN` refs from the latest `ai` snapshot |

When `annotate` is true, run `webview_dom_snapshot(mode: "ai")` first. The returned image overlays labels like `[1]` for `@e1`; the response includes `annotations`, `snapshotId`, and `refsPath`. When `save` is true, the artifact manifest includes final path, `sha256`, `refsPath`, `snapshotId`, `windowId`, `selector`, width, and height.

### webview_get_pointed_element

Returns metadata about the element last captured via Alt+Shift+Click in the app. The bridge injects an event listener that stores the clicked element's tag, classes, ID, text, bounding box, and computed styles. Useful for identifying elements by pointing at them visually.

No parameters.

### webview_select_element

Visual element picker (placeholder -- not yet implemented).

---

## Window Tools

### manage_window

List windows, get info about a specific window, or resize.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `list`, `info`, `resize` |
| `windowId` | string | | Target window (for `info` and `resize`) |
| `width` | number | | New width (for `resize`) |
| `height` | number | | New height (for `resize`) |

---

## IPC & Event Tools

### ipc_get_backend_state

Returns app metadata: name, version, debug/release mode, OS, arch, Tauri version, webview version, window list with labels/URLs, and timestamp.

No parameters.

### ipc_execute_command

Call any Tauri IPC command (same as `window.__TAURI_INTERNALS__.invoke()`).

| Param | Type | Required | Description |
|---|---|---|---|
| `command` | string | yes | The Tauri command name |
| `args` | object | | Command arguments (JSON object) |

### ipc_monitor

Start or stop IPC call monitoring. When active, every `invoke()` call is logged to `ipc.log` with command name, args, duration, and error (if any).

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `start` or `stop` |

### ipc_get_captured

Read captured IPC calls from `ipc.log`.

| Param | Type | Default | Description |
|---|---|---|---|
| `filter` | string | | Substring match on command name |
| `pattern` | string | | Regex match on command name |
| `limit` | number | 50 | Max entries to return |
| `since` | number | | Epoch ms -- only return entries after this time |

### ipc_emit_event

Emit a Tauri event via `app.emit()`.

| Param | Type | Required | Description |
|---|---|---|---|
| `eventName` | string | yes | Event name |
| `payload` | any | | Event payload (JSON) |

### ipc_listen

Start or stop listening for specific Tauri events.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `start` or `stop` |
| `events` | string[] | | Event names to listen for (required for `start`) |

### event_get_captured

Read captured Tauri events from `events.log`.

| Param | Type | Default | Description |
|---|---|---|---|
| `event` | string | | Exact event name filter |
| `pattern` | string | | Regex match on event name or payload |
| `limit` | number | 50 | Max entries to return |
| `since` | number | | Epoch ms -- only return entries after this time |

---

## Log Tools

### read_logs

Read console logs captured from the webview (in-memory buffer, up to 500 entries).

| Param | Type | Default | Description |
|---|---|---|---|
| `lines` | number | 50 | Number of log entries to return |
| `filter` | string | | Substring match on message |
| `pattern` | string | | Regex match on message |
| `level` | string | | Comma-separated levels: `log`, `info`, `warn`, `error`, `debug` |

### read_log_file

Read historical logs from JSONL files (persisted across app restarts).

| Param | Type | Required | Default | Description |
|---|---|---|---|---|
| `source` | string | yes | | `console`, `ipc`, `events`, `runtime` |
| `lines` | number | | 50 | Number of entries |
| `level` | string | | | Level filter (console only): `log`, `info`, `warn`, `error`, `debug` |
| `pattern` | string | | | Regex match |
| `since` | number | | | Epoch ms filter |
| `windowId` | string | | | Window filter (console/runtime) |

### clear_logs

Clear log files.

| Param | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | `console`, `ipc`, `events`, `runtime`, `all` |

### runtime_get_captured

Read runtime-level frontend captures from `runtime.log`.

| Param | Type | Default | Description |
|---|---|---|---|
| `kind` | string | | Comma-separated kinds: `window_error`, `unhandledrejection`, `network`, `navigation`, `resource_error` |
| `level` | string | | Comma-separated levels: `error`, `warn`, `info` |
| `pattern` | string | | Regex match on serialized entry |
| `since` | number | | Epoch ms filter |
| `sinceMark` | string | | Debug mark id from `debug_mark` |
| `limit` | number | 100 | Max entries |
| `windowId` | string | | Target window |

### runtime_clear

Clear `runtime.log`. No parameters.

---

## Artifact Tools

### artifact_list

List artifact metadata from `<log_dir>/artifacts/manifest.jsonl`.

| Param | Type | Default | Description |
|---|---|---|---|
| `kind` | string | | Filter by artifact kind, e.g. `screenshot` |
| `limit` | number | 100 | Max entries |

### artifact_read

Read an artifact by id or path.

| Param | Type | Required | Description |
|---|---|---|---|
| `artifact` | string | yes | Artifact id or path |
| `artifactId` | string | | Alias for `artifact` |

### artifact_compare

Compare two artifacts or paths. Same-path comparisons are rejected.

| Param | Type | Required | Description |
|---|---|---|---|
| `before` | string | yes | Before artifact id or path |
| `after` | string | yes | After artifact id or path |
| `threshold` | number | | Maximum allowed difference ratio, 0--1 (default 0) |

Result: `{ pixelsDifferent, percentDifferent (0--1), threshold, passed, beforePath, afterPath, metric: "byte-diff" }`. The comparison is a raw byte diff of the two files, not a perceptual pixel diff: `passed` is `percentDifferent <= threshold`. Byte-identical means truly unchanged; any nonzero diff means "something changed" -- inspect both images visually before judging a regression.

### artifact_prune

Prune older manifest entries and optionally delete files.

| Param | Type | Default | Description |
|---|---|---|---|
| `keep` | number | 50 | Newest matching artifacts to keep |
| `kind` | string | | Optional kind filter |
| `deleteFiles` | boolean | true | Delete pruned files from disk |

---

## Debug Tools

### debug_mark

Create a timestamp mark for later diff filters.

| Param | Type | Description |
|---|---|---|
| `label` | string | Optional human label |

### debug_snapshot

Collect bridge/app state plus optional DOM, screenshot, logs, IPC, events, and runtime captures.

| Param | Type | Default | Description |
|---|---|---|---|
| `windowId` | string | `main` | Target window |
| `includeDom` | boolean | true | Include DOM snapshot |
| `includeScreenshot` | boolean | false | Include saved screenshot artifact |
| `includeLogs` | boolean | true | Include console errors/warnings |
| `includeIpc` | boolean | false | Include IPC captures |
| `includeEvents` | boolean | false | Include event captures |
| `includeRuntime` | boolean | true | Include runtime captures |
| `since` | number | | Epoch ms filter |
| `sinceMark` | string | | Mark id from `debug_mark` |
| `maxTokens` | number | 4000 | DOM snapshot inline budget |
| `screenshotNameHint` | string | | Screenshot artifact name hint |

### webview_act_and_verify

Perform one action, wait for a selector/text, and collect fresh evidence since an internal mark.

| Param | Type | Description |
|---|---|---|
| `action` | string | `click`, `fill`, `type`, `press`, `drag`, `hover` |
| `selector` | string | Source selector or `@eN` |
| `text` | string | Text for fill/type |
| `key` | string | Key for press |
| `targetSelector` | string | Drag target |
| `waitForSelector` | string | Selector expected after action |
| `waitForText` | string | Text expected after action |
| `timeout` | number | Wait timeout in ms |
| `verifyDom` | boolean | Include DOM snapshot |
| `verifyScreenshot` | boolean | Include screenshot artifact |
| `includeLogs` | boolean | Include log diff |
| `includeIpc` | boolean | Include IPC diff |
| `includeRuntime` | boolean | Include runtime diff |

---

## Workflow tools

Known multi-step intent should use the app-owned workflow executor. Token-based authorization is required for durable operations; pass `authToken` outside the spec in embedded MCP. The standalone server reads `TAURI_CONNECTOR_WORKFLOW_TOKEN` unless an explicit token is supplied. The host must configure the matching token (minimum 32 bytes).

| Tool | Arguments | Meaning |
| --- | --- | --- |
| `workflow_capabilities` | optional `windowId` | Actual ops, conditions, recovery, authorization and collection support |
| `workflow_run` | `spec`, optional `waitMs` (0–30000, default 1000), `authToken` | Create/get one logical run; waiting expiration does not cancel execution |
| `workflow_get` | `runId`, optional numeric `cursor`, optional `include: ["steps", "events", "evidence"]`, `evidenceId`, numeric `offset`, `authToken` | Read progress and retained evidence without replaying actions |
| `workflow_cancel` | `runId`, `authToken` | Stop future dispatch; distinguish in-flight effects from cancellation |
| `workflow_resume` | `runId`, `expectedRevision`, `checkpointId`, `intent: "continue" \| "reconcile"`, `authToken` | Continue only an undispatched paused step, or recheck a postcondition without replay |

Required spec fields: `schemaVersion: 1`, `runKey`, and 1–100 `steps`. Optional `mode` is only `strict`; `schedule` is only `sequential`; `windowId` defaults to `main`; `deadlineMs` defaults to 60000 and cannot exceed 300000. `inputs` is an object. `evidence.maxInlineBytes` is 1024–65536 (default 16384). `defaults` accepts `stepTimeoutMs`, `locatorTimeoutMs`, `pollIntervalMs`, and `failureEvidenceGraceMs`. The `workflow_run` schema defines all accepted fields and union variants.

Each step has an `id` and `op`: `click(target)`, `fill(target,value)`, `type(target,value)`, `press(key,target?)`, `wait(condition)`, `query(target,query)`, or trusted `tool(tool,args,bindings?)`. The trusted tool set is `bridge_status` and `ipc_get_backend_state`. Queries use `{"kind":"value"}`, `{"kind":"text"}`, or `{"kind":"attribute","name":"data-id"}`. Any step can declare `expect` and `timeoutMs`; a failed required expectation blocks later steps.

Locators use `{"by":"role"|"label"|"testId"|"css","value":...}` with optional `name`, nested `scope`, and `entity:{"attribute":"data-id","value":...}`. Constraints intersect and must yield one actionable target. Fill replaces that target's value; type appends. Legacy `@ref` fallback, native-input guarantees, arbitrary JS, unknown IPC and parallel execution are unsupported.

Expressions are scalar literals, `{"literal":<any JSON>}`, `{"fromInput":{"key":"title"}}`, or `{"fromStep":{"stepId":"read","pointer":"/value"}}`. Step references address prior outcome data and support escaped JSON Pointer tokens. Objects outside declared expression slots remain business data.

Conditions: `element(target,state)`, `valueEquals(target,expected)`, `textContains(target,expected)`, `attributeEquals(target,name,expected)`, `result(stepId,pointer,operator,expected?)`, and `all/any(conditions)`. Element states are visible/hidden/attached/detached/enabled/editable. Result operators are eq/exists/nonEmptyString. State-only observations cannot prove business persistence or action causality.

For a retained reference, call `workflow_get(runId: ..., evidenceId: ..., offset: 0)`. It returns minimal run metadata plus `evidencePage: { id, offset, content, nextOffset, totalBytes }`. `content` is a bounded JSON text chunk; follow `nextOffset` to retrieve later chunks and reassemble the text. Offsets count UTF-8 bytes; `offset` requires `evidenceId`; `nextOffset: null` marks the final chunk. This path makes each retained evidence reference readable even when the report summary is truncated.

Reports separate `execution`, `verification`, `effect`, `goalStatus`, and `originalTestVerdict`. Use the returned `runId`, `revision`, `checkpointId`, `allowedNextActions`, and coverage. Reuse the original `runKey` and identical spec after a lost submission response. Resume cannot replay an uncertain write or restart historical work after application restart. Reconciliation preserves the original failed verdict. Cancellation is not rollback. Workflow lifecycle calls cannot be nested in batches.

## Batch Tool

### batch_actions

Run several tool calls from one JSON spec -- sequentially, in parallel, or DAG-ordered via `dependsOn` -- and get back per-action run logs (status, timing, result/error). Available in both MCP servers; the CLI equivalent is `tauri-connector batch`.

| Param | Type | Default | Description |
|---|---|---|---|
| `actions` | array | required | Actions to run (see below) |
| `mode` | string | `sequential` | `sequential` runs in spec order; `parallel` starts everything at once, ordered only by `dependsOn` |
| `stopOnError` | boolean | true | Stop starting new actions after the first failure; unstarted actions log as `skipped` |
| `maxParallel` | number | unlimited | Cap on concurrently running actions |
| `timeoutMs` | number | | Default per-action timeout in ms |
| `save` | string | | Also write the full report as pretty JSON to this file path (absolute path recommended) |

Each entry in `actions`:

| Param | Type | Description |
|---|---|---|
| `tool` | string | Any tool name except `batch_actions`, `driver_session`, and `workflow_*` lifecycle tools |
| `args` | object | Tool arguments, same shape as a direct call (`@eN` refs and `windowId` work as usual) |
| `id` | string | Stable id other actions reference in `dependsOn` |
| `dependsOn` | array | Ids that must all succeed before this action starts |
| `timeoutMs` | number | Per-action timeout override in ms |
| `omitResult` | boolean | Drop the tool result from its log entry (keep status/timing) to save tokens |

Example -- click, wait, then collect evidence in parallel:

```json
batch_actions(mode: "parallel", actions: [
  { "id": "open", "tool": "webview_interact",
    "args": { "action": "click", "selector": "@e5" } },
  { "id": "settle", "tool": "webview_wait_for",
    "args": { "selector": ".modal", "timeout": 5000 }, "dependsOn": ["open"] },
  { "tool": "read_logs", "args": { "level": "error" }, "dependsOn": ["settle"] },
  { "tool": "webview_screenshot",
    "args": { "save": true, "nameHint": "after-open" }, "dependsOn": ["settle"] }
])
```

The response is the run report (also what `save` writes):

```json
{
  "ok": true, "mode": "parallel", "total": 4,
  "succeeded": 4, "failed": 0, "skipped": 0,
  "startedAt": 1756350000000, "durationMs": 640,
  "logs": [
    { "index": 0, "id": "open", "tool": "webview_interact", "status": "ok",
      "startedAtMs": 0, "durationMs": 120, "result": { "clicked": true } }
  ]
}
```

- Log `status` is `ok`, `error` (with an `error` message), or `skipped` (a dependency failed, or `stopOnError` aborted the batch). `startedAtMs` is the offset from batch start, so overlapping ranges show real concurrency.
- Typed execution/verification failures, including wait timeout and failed `act_and_verify`, fail the action and its success dependencies. Arbitrary business JSON containing `error` is preserved. Failed outcomes retain diagnostic data in `outcome`.
- With `stopOnError: false`, sequential mode still runs the remaining actions in order after a failure -- only explicit `dependsOn` edges on the failed action skip their dependents. That is the "run all checks, report which failed" shape.
- Mix order and concurrency: `mode: "parallel"` plus `dependsOn` chains gives a DAG -- independent actions overlap while dependent ones wait.
- Batch screenshots default to `save: true`; a valid saved artifact replaces inline base64 in reports. Explicit `save: false` preserves legacy inline output. If saving fails or a usable artifact is unavailable, diagnostics/evidence are retained rather than silently discarded.
- Application-owned resource arbitration is shared with workflows and single tools. Conflicting operations return `resource_busy`; independent read-only diagnostics remain available during uncertain write isolation.
- Report-save failures return the business report plus `persistenceWarning`; `savedTo` is present only after a successful write. Do not repeat successful actions to repair report persistence.

---

## Setup & Info Tools

### get_setup_instructions

Returns the embedded setup guide for adding the plugin to a Tauri v2 project. No parameters.

### list_devices

Returns a message confirming the MCP server is embedded in the Tauri app. No parameters.

---

## Standalone MCP Server Only

### driver_session

Manage the WebSocket connection to the running Tauri app. Only available in the standalone MCP server (`tauri-connector-mcp`), not the embedded server.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `start`, `stop`, `status` |
| `host` | string | | WebSocket host (default: `127.0.0.1`) |
| `port` | number | | WebSocket port (default: `9555`) |
