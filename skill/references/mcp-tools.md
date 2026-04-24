# MCP Tools Reference

Complete parameter tables for all 25 tauri-connector MCP tools. Configure in `.mcp.json`:

```json
{ "mcpServers": { "tauri-connector": { "url": "http://127.0.0.1:9556/sse" } } }
```

The standalone MCP server (`tauri-connector-mcp`) adds one additional tool: `driver_session`.

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

Poll for element or text to appear (100ms interval).

| Param | Type | Default | Description |
|---|---|---|---|
| `selector` | string | | CSS, XPath, or text to wait for |
| `strategy` | string | `css` | `css`, `xpath`, `text` |
| `text` | string | | Text content to wait for (alternative to selector) |
| `timeout` | number | 5000 | Timeout in ms |

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
    "allRefsPath": "<tmp>/tauri-connector-<pid>/snapshots/<uuid>/refs.json",
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
| `source` | string | yes | | `console`, `ipc`, `events` |
| `lines` | number | | 50 | Number of entries |
| `level` | string | | | Level filter (console only): `log`, `info`, `warn`, `error`, `debug` |
| `pattern` | string | | | Regex match |
| `since` | number | | | Epoch ms filter |
| `windowId` | string | | | Window filter (console only) |

### clear_logs

Clear log files.

| Param | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | `console`, `ipc`, `events`, `all` |

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
