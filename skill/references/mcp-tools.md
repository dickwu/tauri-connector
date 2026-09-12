# MCP Tools Reference

Complete parameter tables for tauri-connector MCP tools. Configure in `.mcp.json`:

```json
{ "mcpServers": { "tauri-connector": { "url": "http://127.0.0.1:9556/mcp" } } }
```

The standalone MCP server (`tauri-connector-mcp`) adds one additional tool, `driver_session`, and reads `TAURI_CONNECTOR_WORKFLOW_TOKEN` from its environment for the `workflow_*` tools (put it in the `env` block of its `.mcp.json` entry). Legacy `/sse` remains available for older clients.

**Response envelope (0.15+).** Every tool result can carry a typed `outcome` (`structuredContent.outcome` on success, `structuredContent.{error, outcome}` on failure) with `execution`, `verification`, `effect`, and a stable `error.code` -- see "Run report fields" and "Error codes" under Workflow tools. A legacy handler error without a typed outcome surfaces as `outcome_unknown`. Mutating tools, plus `webview_screenshot`, `webview_dom_snapshot`, `webview_find_element`, `webview_get_styles`, and `webview_wait_for`, take an application-owned lease on the target window and can return `resource_busy` while a workflow, batch, or another tool holds it; read-only diagnostics such as `read_logs`, `bridge_status`, `ipc_get_backend_state`, `artifact_list`, and `artifact_read` are never blocked. A lease quarantined by an uncertain write (`quarantined: true`) is released only by restarting the app -- no tool clears it. Mutating tools can also fail with `persistence_unavailable` when the app's data directory is unusable, because the lease journal must be writable first.

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

Timeouts return `{ found: false, timeout: true, elapsed_ms, lastObservation }`; successful waits return `{ found: true, elapsed_ms, lastObservation }`. A predicate that throws, or a page unload mid-wait, returns `{ found: false, code: "observation_failed", error }`. A direct call reports the timeout as data (no MCP `isError`); inside `batch_actions`, `webview_act_and_verify`, or a workflow `wait` step the same timeout is a failure (`condition_timeout`) that fails the action and skips its dependents. `effect` is `none` unless a custom `fn` predicate was supplied (`possible`, since your predicate may have side effects).

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

All supplied locator constraints intersect (AND), then `first`/`last`/`nth` pick from that intersection. `fill` replaces the matched control's value and `type` appends, both through native setters plus `beforeinput`/`input`/`change` events so controlled React inputs update.

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
| `maxTokens` | number | 4000 | Token budget for inline output (same default over MCP, WebSocket, and CLI). Overflow spills to on-disk subtree files. `0` = unlimited. |
| `noSplit` | boolean | false | Disable subtree file splitting -- return full inline output regardless of budget. The embedded MCP server (0.15) ignores this flag: pass `maxTokens: 0` there. The standalone server and the CLI (`--no-split`) honor it |
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

### app_identity and runtime_health

`app_identity` requires `authToken` and returns current application-instance/workspace identity. `runtime_health` accepts `authToken`, `windowId` (default `main`), `depth` (`transport`, `bridge`, `runtime`; default `runtime`) and `timeoutMs` (100–10000; default 2000). Transport, bridge and runtime results are separate; a reachable socket does not prove a ready page. Health is bounded observation: it does not reload a page, retry a write, change a workflow verdict or release quarantine. Anonymous base capabilities remain minimal and do not expose workspace paths or page/capture content.

### ipc_capture and ipc_query

These authenticated tools provide independent, application-owned capture sessions. They observe JavaScript invoke boundaries; they are not Rust tracing, backend task completeness or proof of durable business storage. Legacy anonymous `ipc_get_captured` and clear operations do not expose or delete these protected results.

```json
{"action":"start","windowId":"main","options":{"resultPolicy":"preview","argumentPolicy":"metadata","followPages":false,"commands":["fixture_save"]}}
```

Start confirms page hook readiness and returns `captureSessionId`; `status` and `stop` require that ID. Stopping one session leaves other sessions and the business invocation running. Result/argument policies are `metadata` (default) or bounded redacted `preview`; previews additionally require host-configured command and field-path allowlists (`TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS`, `TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS`). A client cannot broaden these host policies. Serialized previews never replace the original return value or rejection reason. Unsupported hook boundaries and observation gaps are reported.

`ipc_query` requires `captureSessionId` and accepts `cursor` (the opaque string returned as `nextCursor`, passed unchanged), `invocationId`, `phase` (`started`, `succeeded`, `failed`), `limit` (1–500; default 100), and `maxBytes` (1024–65536; default 32768). Follow the returned cursor and coverage/gap indicators. A missing terminal event remains pending or observation-interrupted; an empty page does not prove that no IPC occurred. Buffer loss does not erase workflow history or relax unknown-write isolation.

```json
{"captureSessionId":"session-returned-by-start","limit":100,"maxBytes":32768}
```

All shown JSON requires envelope authorization from the host adapter; no credential belongs in a workflow spec or printed evidence.

### webview_execute_js

Execute arbitrary JavaScript in the webview. Use IIFE for return values.

| Param | Type | Required | Description |
|---|---|---|---|
| `script` | string | yes | JavaScript code. Wrap in `(() => { return value; })()` to get a return value |
| `windowId` | string | | Target window |

### webview_screenshot

Legacy requests retain the window `xcap`/DOM renderer path. Rich inspection requests add authenticated `source`, strict `target`, `redaction:"required"` and `allowWindowPreparation` fields. `source` is `webview_native`, `window_native`, `dom_rendering`, or explicit `auto`; an explicit backend failure never silently changes source. Inspect actual source, fallback attempts, capture context, geometry and redaction metadata before comparing images. A WebView image, desktop window image and DOM-rendered reconstruction are distinct evidence sources. Native platform behavior must be checked against the application's advertised capability and current native evidence.

`target` is a structured strict locator and conflicts with legacy `selector`. Required masks must be applied before image output or artifact registration. If geometry cannot safely map sensitive regions, image delivery fails closed. The default does not focus, restore or scroll the window; authorized preparation must be reported. A screenshot/artifact failure does not change an already executed workflow result or authorize a replay.

| Param | Type | Default | Description |
|---|---|---|---|
| `format` | string | `png` embedded, `jpeg` standalone | `png`, `jpeg`, `webp` -- pass it explicitly when the format matters |
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

Start a real, application-owned element picker, or read/cancel its retained handle. The user points at an element and confirms with the primary pointer button or Enter; Escape and a visible cancel control end the interaction. Starting selection installs input guards and an overlay. It does not ask the user to inject JavaScript or reuse the legacy pointed-element cache.

All actions require `authToken` in the call envelope. The CLI and standalone MCP supply `TAURI_CONNECTOR_WORKFLOW_TOKEN` privately. The same token represents one authorization domain across connections; a connection ID is not a separate user identity.

| Param | Actions | Default / contract |
| --- | --- | --- |
| `action` | all | `start`, `get`, `cancel`; omitted action starts and waits up to 10000 ms |
| `windowId` | start / omitted | `main`; get/cancel use the retained context and reject retargeting |
| `requestKey` | start / omitted | Optional 1–128 UTF-8 bytes; reuse the same key and options after a lost response |
| `timeoutMs` | start / omitted | 60000; range 5000–120000; total lifetime, not renewed by polling |
| `timeout` | start / omitted | Millisecond compatibility alias; must equal `timeoutMs` if both appear |
| `waitMs` | start / get / omitted | 0–10000; explicit start/get default 0, omitted action default 10000 |
| `pickerId` | get / cancel | Required retained handle; forbidden for start |
| `captureScreenshot` | start / omitted | true; false schedules no screenshot task |
| `screenshotSource` | start / omitted | `auto`, `webview_native`, `window_native`, `dom_rendering` |
| `includeImage` | start / get / omitted | false; only includes an already captured, redacted image |

`captureScreenshot:false` conflicts with an explicit `screenshotSource` or start-time `includeImage:true`. Unknown fields are rejected. `get` never repeats hit testing, selection or capture. A repeated request key returns the original object without refreshing its deadline; changing its window or capture options returns `request_key_conflict`. Keep the returned `retainedUntil` boundary: this is bounded in-process deduplication, not workflow journal history.

The primary states are `created`, `installing`, `awaiting_selection`, `selected`, `cancelled`, `expired`, `target_changed`, and `failed`. Read `screenshot.status`, `cleanup.status`, and `resultComplete` separately. A selected element remains selected when its screenshot fails, with a warning. `includeImage:true` adds MCP image content only when redaction and output budgets permit; text/structured metadata is retained. Cancelling an already selected object cannot replace its selection with a cancelled state.

```json
{"action":"start","windowId":"main","requestKey":"isolated-picker-1","timeoutMs":60000,"waitMs":0,"captureScreenshot":true,"screenshotSource":"auto"}
```

```json
{"action":"get","pickerId":"picker-returned-by-start","waitMs":10000,"includeImage":true}
```

```json
{"action":"cancel","pickerId":"picker-returned-by-start"}
```

These examples omit credentials; an authorized adapter must insert the host token. Direct WS sends the same arguments in `{"id":"request-1","type":"inspection","operation":"webview_select_element","args":{...}}`. Embedded and standalone MCP call the same application-side object. After external client disconnect, get/cancel still address the retained picker. Internal page loss, navigation, window destruction and token revocation require invalidation and cleanup.

Returned locator candidates carry their semantic version and were checked for uniqueness at capture time. Revalidate the application/window/page context and candidate before a later action: a selection is an observation, not permission to skip strict matching or actionability. Iframe/canvas/shadow hosts do not imply access to their internal objects. `businessActionDispatched:false` records that the connector did not dispatch a business action; it is not proof that earlier application-global listeners, CSS hover or background work had zero effect.

Picker cannot modify an existing workflow spec, run key, original verdict, resume policy or unknown-write quarantine. An ambiguous failed workflow remains failed; the caller may separately prepare new undispatched work using a verified candidate.

---

## Window Tools

### manage_window

List windows, get info about a specific window, or resize.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `list`, `info`, `resize` |
| `windowId` | string | | Target window (for `info` and `resize`) |
| `width` | number | | New width (for `resize`). Always pass both dimensions: the standalone server rejects a missing one, the embedded server silently falls back to 800x600 |
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

Start or stop IPC call monitoring in one window. When active, every `invoke()` call is logged to `ipc.log` with command name, args, duration, and error (if any).

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `start` or `stop` |
| `windowId` | string | | Window whose page should monitor (default `main`); monitoring is per window |

The page must acknowledge the change: the response reports `windowId`, `desired`, `applied` (`null` until acknowledged), `pageEpoch`, and `acknowledgedAt`, and the call fails (`observation_failed`) when the selected window never acknowledges -- a stale flag is never reported as success. Start monitoring in every window whose IPC you need.

### ipc_get_captured

Read captured IPC calls from `ipc.log`.

| Param | Type | Default | Description |
|---|---|---|---|
| `filter` | string | | Substring match on command name |
| `pattern` | string | | Regex match on command name |
| `limit` | number | 100 | Max entries to return |
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
| `limit` | number | 100 | Max entries to return |
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
| `lines` | number | | 100 | Number of entries |
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

Defaults: `timeout` 5000, `includeLogs` true, `includeRuntime` true, `verifyDom` / `verifyScreenshot` / `includeIpc` false. `fill`, `type`, and `press` act on the element `selector` resolves to (exactly one match, else `target_not_found` / `ambiguous_target`) rather than on whatever has focus; `fill` replaces the value, `type` appends. The wait is skipped when the action itself failed.

The response carries `verdict` (`failed` when the action or the wait failed, `inconclusive` when neither `waitForSelector` nor `waitForText` was given, `passed` otherwise), `mark`, `actionResult`, `waitResult` plus a typed `waitOutcome`, the requested evidence diffs, and `suggestions`. Always pass a wait condition when you need a verdict. Inside `batch_actions`, a `failed` verdict fails the action (`postcondition_failed`) and skips its dependents.

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

Further limits: a spec is at most 256 KiB; `runKey` and step ids are non-empty and at most 1024 bytes; any single literal or bound value is at most 64 KiB; locator `scope` nests at most 4 deep; at most 100 condition nodes nested at most 8 deep; `pollIntervalMs` is at least 10 and `failureEvidenceGraceMs` at most 2000. The app executes at most 4 workflows concurrently and queues 16 more; beyond that `workflow_run` returns `resource_busy` ("Active and queued workflow capacity reached"). Up to 1000 runs are retained without eviction.

`status: completed` is not success by itself: the exit-code mapping shared by the CLI and both MCP servers returns `1` when `goalStatus` or `originalTestVerdict` is `failed` and `2` when either is `inconclusive`; `failed`, `cancelled`, `expired`, and `rejected` map to `1`; every other status (queued, running, paused, interrupted) maps to `2`. MCP sets `isError` only for the exit-1 case, so a paused or uncertain run arrives as an ordinary successful tool result -- always read `status`, `reason`, and `allowedNextActions` from the body.

Each step has an `id` and `op`: `click(target)`, `fill(target,value)`, `type(target,value)`, `press(key,target?)`, `wait(condition)`, `query(target,query)`, or trusted `tool(tool,args,bindings?)`. The trusted tool set is `bridge_status` and `ipc_get_backend_state`. Queries use `{"kind":"value"}`, `{"kind":"text"}`, or `{"kind":"attribute","name":"data-id"}`. Any step can declare `expect` and `timeoutMs`; a failed required expectation blocks later steps.

Locators use `{"by":"role"|"label"|"testId"|"css","value":...}` with optional `name`, nested `scope`, and `entity:{"attribute":"data-id","value":...}`. Constraints intersect and must yield one actionable target. Fill replaces that target's value; type appends. Legacy `@ref` fallback, native-input guarantees, arbitrary JS, unknown IPC and parallel execution are unsupported.

Expressions are scalar literals, `{"literal":<any JSON>}`, `{"fromInput":{"key":"title"}}`, or `{"fromStep":{"stepId":"read","pointer":"/value"}}`. Step references address prior outcome data and support escaped JSON Pointer tokens. Objects outside declared expression slots remain business data.

Conditions: `element(target,state)`, `valueEquals(target,expected)`, `textContains(target,expected)`, `attributeEquals(target,name,expected)`, `result(stepId,pointer,operator,expected?)`, and `all/any(conditions)`. Element states are visible/hidden/attached/detached/enabled/editable. Result operators are eq/exists/nonEmptyString. State-only observations cannot prove business persistence or action causality.

A `query` on a password input, on anything inside `[data-sensitive="true"]` / `[data-connector-sensitive="true"]`, or on an element (or requested attribute) whose `id`/`name`/`autocomplete` looks like a credential (`password`, `secret`, `token`, `auth…`, `cookie`, `credential`, `api-key`, `one-time-code`) fails with `capability_unavailable`; reports and journals also redact credential-named keys as `[redacted]`. Scoped failure evidence is structural only -- tag, role, visible/enabled/editable for at most 40 elements, never text, values, or ids -- so capture any value you need with a `query` step. Actionability for `click`/`fill`/`type` requires visible, enabled (and editable for input), inside the viewport, and not covered by another element at the hit point; otherwise the step fails with `not_actionable` before dispatch.

For a retained reference, call `workflow_get(runId: ..., evidenceId: ..., offset: 0)`. It returns minimal run metadata plus `evidencePage: { id, offset, content, nextOffset, totalBytes }`. `content` is a bounded JSON text chunk; follow `nextOffset` to retrieve later chunks and reassemble the text. Offsets count UTF-8 bytes; `offset` requires `evidenceId`; `nextOffset: null` marks the final chunk. This path makes each retained evidence reference readable even when the report summary is truncated.

Reports separate `execution`, `verification`, `effect`, `goalStatus`, and `originalTestVerdict`. Use the returned `runId`, `revision`, `checkpointId`, `allowedNextActions`, and coverage. Reuse the original `runKey` and identical spec after a lost submission response. Resume cannot replay an uncertain write or restart historical work after application restart. Reconciliation preserves the original failed verdict. Cancellation is not rollback. Workflow lifecycle calls cannot be nested in batches.

### Minimal spec

```json
{
  "schemaVersion": 1,
  "runKey": "create-task-smoke-001",
  "inputs": { "title": "Isolated test task" },
  "steps": [
    { "id": "open", "op": "click",
      "target": { "by": "role", "value": "button", "name": "New task" },
      "expect": { "kind": "element", "target": { "by": "role", "value": "dialog", "name": "New task" }, "state": "visible" } },
    { "id": "title", "op": "fill",
      "target": { "scope": { "by": "role", "value": "dialog", "name": "New task" }, "by": "label", "value": "Title" },
      "value": { "fromInput": { "key": "title" } } },
    { "id": "save", "op": "click",
      "target": { "scope": { "by": "role", "value": "dialog", "name": "New task" }, "by": "role", "value": "button", "name": "Save" },
      "expect": { "kind": "element",
        "target": { "by": "testId", "value": "task-row", "entity": { "attribute": "data-task-title", "value": { "fromInput": { "key": "title" } } } },
        "state": "visible" } },
    { "id": "read-id", "op": "query",
      "target": { "by": "testId", "value": "task-row", "entity": { "attribute": "data-task-title", "value": { "fromInput": { "key": "title" } } } },
      "query": { "kind": "attribute", "name": "data-task-id" } }
  ],
  "goal": { "kind": "result", "stepId": "read-id", "pointer": "/value", "operator": "nonEmptyString" }
}
```

`goal` is an optional final condition evaluated after the last step; without one, `goalStatus` is `not_requested`. A `query` step's result lands in that step's outcome `data` (for example `{"value": "..."}`), which later `fromStep` expressions and `result` conditions address by JSON Pointer. Optional spec fields: `mode: "strict"`, `schedule: "sequential"`, `windowId`, `deadlineMs`, `defaults`, and `evidence: { success: "summary", failure: "scoped", maxInlineBytes }`. Specs are capped at 256 KiB.

### Capabilities response

`workflow_capabilities` returns `schemaVersions`, `modes`, `schedules`, `ops`, `conditions`, `tools` (the trusted `tool` allowlist), `authentication: { required, configured, minimumTokenBytes }`, `recovery` (`clientReconnect`, `sameProcessContinue: "undispatched_only"`, `restartHistory`, `automaticRestartReplay: false`, `unknownWriteReplay: false`), `journal`, `unsupported[]`, `coverage`, and `limits` (`activeRuns: 4`, `queuedRuns: 16`, `maxSteps: 100`, `maxSpecBytes: 262144`, `maxDeadlineMs: 300000`, plus live `quarantinedScopes` and retention-budget usage). When a workflow call returns `unauthorized`, check `authentication.configured` first: `false` means the host never configured a token (or configured one shorter than 32 bytes, which is ignored).

### Run report fields

`workflow_run`, `workflow_get`, `workflow_cancel`, and `workflow_resume` all return the run report:

| Field | Meaning |
|---|---|
| `runId`, `revision`, `cursor`, `checkpointId` | Identity, plus the values `workflow_resume` must echo back as `expectedRevision` / `checkpointId` |
| `status` | `queued`, `running`, `cancelling`, `paused`, `completed`, `failed`, `cancelled`, or `interrupted` (a run found in history after an app restart) |
| `reason` | Stable error code (table below) explaining a `paused`, `failed`, or `cancelled` status |
| `allowedNextActions` | Subset of `get`, `cancel`, `continue`, `reconcile`; only these are accepted next |
| `goalStatus` | `not_requested`, `passed`, `failed`, or `inconclusive` for the spec-level `goal` |
| `originalTestVerdict` | Verdict of the original run; reconciliation never rewrites it |
| `mayHaveEffects`, `resourceIsolation` | Whether a dispatched write may have landed, and whether its resources are `quarantined` |
| `steps[]`, `summary` | Per-step `outcome` objects (below) with `data`; `summary` counts `completedSteps` / `remainingSteps` |
| `blockedOutcome`, `blockedStep` | The outcome and step that stopped the run |
| `evidenceRefs[]`, `evidence`, `events` | Retained evidence ids (each readable via `evidenceId`), inline evidence, and the event log when included |
| `coverage` | `truncated`, `omittedFields`, `omittedEvidenceRefs`, `businessPersistence: "unobserved"`, `correlation: "state_only"`, `sources` |

Every outcome -- workflow steps and, since 0.15, legacy tools inside `batch_actions` -- separates `execution` (`not_dispatched`, `completed`, `failed`, `outcome_unknown`), `verification` (`not_requested`, `passed`, `failed`, `inconclusive`), and `effect` (`none`, `possible`, `confirmed`), alongside `data`, `error: { code, stage, message, retryableBeforeDispatch }`, `timing`, `dispatch` (with `requestId`), `evidenceRefs`, `coverage`, and `warnings`. Success means `execution: completed`, no `error`, and a passed or not-requested verification; business JSON never decides it. When a report exceeds its byte budget, evidence bodies, events, and error messages are dropped first and `coverage.truncated` is set -- identity, status, outcome facts, and evidence ids survive.

### Error codes

`error.code` and `reason` use these stable values:

| Code | Meaning / next move |
|---|---|
| `invalid_spec` | Spec or arguments rejected before dispatch (unknown field, bad range, `waitMs` > 30000). Fix and resubmit |
| `unauthorized` | Host has no token >= 32 bytes, or the supplied `authToken` does not match |
| `capability_unavailable`, `protocol_mismatch`, `unsupported_feature`, `unsupported_condition` | Plugin older than 0.15, page script out of sync, or an unsupported feature (`networkidle`, parallel schedule, unknown `op`, unknown resume intent) |
| `target_not_found`, `ambiguous_target`, `not_actionable` | Locator matched zero, several, or a disabled/hidden/read-only element. Narrow with `name`, `scope`, or `entity` |
| `target_changed`, `stale_ref` | The window vanished or the page identity changed between steps; a legacy `@eN` ref no longer resolves |
| `binding_missing`, `binding_type_mismatch` | `fromInput` / `fromStep` did not resolve, or resolved to the wrong JSON type |
| `precondition_failed`, `postcondition_failed`, `condition_timeout` | The last verified boundary no longer holds before dispatch; a required `expect`/`goal` failed; a `wait` or expectation ran out of time |
| `execution_failed` | The action itself threw; read `effect` to see whether it may still have landed |
| `outcome_unknown` | Dispatched, result lost: `effect: possible`, resources quarantined until the app restarts. Inspect, then `reconcile`; never replay |
| `run_key_conflict`, `run_not_found`, `run_expired` | Key reused with a different spec; unknown run id; run or evidence no longer retained in this app instance |
| `stale_checkpoint`, `resume_not_safe`, `app_instance_changed`, `resume_requires_inputs` | Resume rejected: re-read the report for the current `revision`/`checkpointId`; the run is still executing or has nothing to reconcile; the app restarted (history is read-only); continuing needs inputs that were not persisted |
| `cancel_requested`, `cancelled_before_dispatch` | Cancellation reasons, with or without a dispatched step |
| `resource_busy` | A conflicting operation holds the window resource (`retryableBeforeDispatch: true`; `quarantined: true` means an uncertain write holds it, and only an app restart releases that), or the 4-active / 16-queued workflow capacity is full |
| `persistence_unavailable`, `evidence_persistence_failed`, `capture_incomplete`, `observation_failed` | Journal storage unusable (workflows fail closed), evidence could not be stored, a result exceeded the retention budget, or the page observation failed |

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
- Typed execution/verification failures, including wait timeout and failed `act_and_verify`, fail the action and its success dependencies. Arbitrary business JSON containing `error` is preserved: only the legacy interaction tools with a genuine error-string contract (`webview_interact`, `webview_keyboard`, `webview_wait_for`, `webview_act_and_verify`, `webview_locator`, `webview_select_option`, `webview_scroll`) fail on a top-level `error` field. A per-action `timeoutMs` expiry yields `outcome_unknown` ("remote execution may continue"), not a plain failure. Every log entry carries the typed `outcome`, and failed outcomes retain their diagnostic data there.
- With `stopOnError: false`, sequential mode still runs the remaining actions in order after a failure -- only explicit `dependsOn` edges on the failed action skip their dependents. That is the "run all checks, report which failed" shape.
- Mix order and concurrency: `mode: "parallel"` plus `dependsOn` chains gives a DAG -- independent actions overlap while dependent ones wait.
- Batch screenshots default to `save: true`; a valid saved artifact replaces inline base64 in reports. Explicit `save: false` preserves legacy inline output. If saving fails or a usable artifact is unavailable, diagnostics/evidence are retained rather than silently discarded.
- Application-owned resource arbitration is shared with workflows and single tools. Conflicting operations return `resource_busy`; independent read-only diagnostics remain available during uncertain write isolation.
- Report-save failures return the business report plus `persistenceWarning`; `savedTo` is present only after a successful write. Do not repeat successful actions to repair report persistence.

---

## Setup & Info Tools

### bridge_status

Show the internal JS bridge: `bridge_port`, `workflowProtocolVersion` (`1` on plugins >= 0.15), `clients[]` (`windowId`, `url`, `title`, `ageMs` per connected webview), `pending` evals, and `fallbackAvailable` (whether the pre-dispatch eval+event fallback exists). No parameters. Call it first when a window seems unresponsive or before relying on `workflow_*` tools.

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
