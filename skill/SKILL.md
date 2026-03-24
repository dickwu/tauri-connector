---
name: tauri-connector
description: "Deep inspection, interaction, and automation of Tauri v2 desktop apps. Use this skill whenever working with a Tauri app's UI -- clicking elements, filling forms, reading DOM state, taking screenshots, dragging elements, debugging console logs, testing user flows, or setting up tauri-connector in a new project. Also use when the user mentions admin/, front/, or tool/ desktop apps, asks about DOM snapshots, element refs, webview interaction, drag-and-drop, IPC debugging, or Tauri app testing. This skill is the bridge between Claude and any running Tauri desktop app."
---

# Tauri Connector

Inspect, interact with, and automate Tauri v2 desktop apps. The MCP server is embedded in the Tauri plugin -- it starts automatically when the app runs. No separate server process needed.

## How It Works

The plugin injects a JavaScript bridge into the Tauri webview. Claude sends commands (click, fill, snapshot, etc.) and the bridge executes them inside the running app's webview, returning results. There are three ways to send commands:

| Path | When to use |
|---|---|
| **MCP tools** (preferred) | When Claude has MCP access via `.mcp.json` -- tools appear as `webview_*`, `ipc_*`, etc. |
| **CLI** (`tauri-connector`) | When running shell commands -- supports `@eN` ref addressing from snapshots |
| **Bun scripts** (fallback) | When neither MCP nor CLI binary is available |

The app must be running for any of these to work. Verify with `lsof -i :9555 -P -n 2>/dev/null | grep LISTEN`.

## The Core Workflow: Snapshot → Interact → Verify

Almost every task follows this pattern:

1. **Snapshot** the DOM to see what's on screen and get ref IDs
2. **Interact** with elements using those refs (click, fill, drag, etc.)
3. **Verify** the result (re-snapshot, read logs, wait for element, screenshot)

Refs (like `@e5`, `@e12`) are stable handles assigned to interactive elements during a snapshot. They're the primary way to target elements -- more reliable than CSS selectors because the snapshot engine uses a multi-strategy fallback (CSS → ARIA role+name → tag+text) to re-resolve them even after DOM changes.

**After any action that changes the DOM (navigation, form submit, dialog open), take a new snapshot** -- old refs may point to stale or removed elements.

```bash
# Example: fill a login form
tauri-connector snapshot -i              # See refs: @e3=email, @e5=password, @e8=submit
tauri-connector fill @e3 "user@test.com"
tauri-connector fill @e5 "password123"
tauri-connector click @e8
tauri-connector wait --text "Welcome"    # Verify login succeeded
tauri-connector snapshot -i              # Get fresh refs for the new page
```

## Setup

For first-time setup in a Tauri project, read `skill/SETUP.md` in the tauri-connector repo. Key steps:

1. `tauri-plugin-connector = "0.7"` in `src-tauri/Cargo.toml`
2. Register plugin with `#[cfg(debug_assertions)]` guard
3. Add `"connector:default"` permission
4. Set `"withGlobalTauri": true` in `tauri.conf.json`
5. Add MCP URL `http://127.0.0.1:9556/sse` to `.mcp.json`

## CLI Reference

Install: `brew install dickwu/tap/tauri-connector` or `cargo build -p connector-cli --release`

### DOM & Inspection

```bash
tauri-connector snapshot -i                   # Interactive elements with refs (most useful)
tauri-connector snapshot -i -c                # Compact: only lines with refs
tauri-connector snapshot -i -s ".ant-form"    # Scope to a subtree
tauri-connector snapshot -i --mode accessibility   # ARIA roles/names
tauri-connector snapshot -i --mode structure        # Tags/classes only
tauri-connector snapshot -i --no-react --no-portals # Skip React/portal enrichment
tauri-connector snapshot -i -d 5 --max-elements 500 # Limit depth/count
tauri-connector dom                           # Cached DOM (pushed from frontend, no round-trip)
tauri-connector find "button.submit"          # Find by CSS
tauri-connector find "Submit" -s text         # Find by visible text
tauri-connector get title                     # Page title
tauri-connector get url                       # Current URL
tauri-connector get text @e7                  # Element text content
tauri-connector get html @e7                  # Element innerHTML
tauri-connector get value @e3                 # Input value
tauri-connector get attr @e5 href             # Attribute value
tauri-connector get box @e5                   # Bounding box {x, y, width, height}
tauri-connector get styles @e5                # All computed CSS styles
tauri-connector get count ".list-item"        # Count matching elements
tauri-connector pointed                       # Get Alt+Shift+Click captured element
```

### Interactions

```bash
tauri-connector click @e5                     # Click
tauri-connector click "button.submit"         # Click by CSS selector
tauri-connector dblclick @e5                  # Double-click
tauri-connector hover @e8                     # Hover (triggers dropdowns/tooltips)
tauri-connector focus @e3                     # Focus
tauri-connector fill @e3 "user@example.com"   # Clear input, set value, fire input+change
tauri-connector type @e3 "hello"              # Type char-by-char (fires keydown/keypress/keyup per char)
tauri-connector check @e10                    # Check checkbox
tauri-connector uncheck @e10                  # Uncheck checkbox
tauri-connector select @e6 "option1" "opt2"   # Select dropdown option(s)
tauri-connector scroll down 300               # Scroll page
tauri-connector scroll up 500 --selector ".list"  # Scroll element
tauri-connector scrollintoview @e20           # Scroll element into view (smooth, centered)
tauri-connector press Enter                   # Press key on focused element
tauri-connector press Tab                     # Navigate focus
```

### Drag and Drop

Simulates drag-and-drop with realistic paced intermediate events. Two strategies:

- **pointer** (default): `pointerdown` → paced `pointermove` → `pointerup` with mouse event mirrors. Works with dnd-kit, SortableJS, framer-motion, custom sliders, resize handles.
- **html5dnd**: `dragstart` → paced `dragenter`/`dragover` → `drop` + `dragend` with DataTransfer. Works with `draggable="true"` elements, react-beautiful-dnd.
- **auto** (default): Checks `el.draggable` -- uses html5dnd if true, pointer otherwise.

```bash
tauri-connector drag @e3 @e7                              # Drag by refs
tauri-connector drag "#item-3" "#item-1"                  # Drag by CSS selectors
tauri-connector drag @e5 "400,300"                        # Drag to pixel coordinates
tauri-connector drag @e3 @e7 --steps 20 --duration 500    # Slower, more intermediate events
tauri-connector drag "#card" ".list" --strategy pointer    # Force pointer strategy
tauri-connector drag "[draggable]" ".trash" --strategy html5dnd  # Force HTML5 DnD
```

| Flag | Default | Purpose |
|---|---|---|
| `--steps` | 10 | Intermediate move events (higher = smoother, some libs need >5 for threshold) |
| `--duration` | 300 | Total drag time in ms |
| `--strategy` | auto | `auto`, `pointer`, or `html5dnd` |

### Screenshot

```bash
tauri-connector screenshot /tmp/shot.png -m 1280    # PNG, max 1280px wide
tauri-connector screenshot /tmp/s.webp -f webp      # WebP format
tauri-connector screenshot /tmp/s.jpg -f jpeg -q 60 # JPEG, quality 60
```

Uses native `xcap` capture (pixel-accurate), falls back to `@zumer/snapdom` if unavailable.

### Logs & Debugging

```bash
tauri-connector logs -n 50                    # Last 50 console logs
tauri-connector logs -l error                 # Error level only
tauri-connector logs -l error,warn            # Multiple levels
tauri-connector logs -p "timeout|failed"      # Regex filter
tauri-connector state                         # App metadata, version, environment, windows
tauri-connector eval "document.title"         # Execute arbitrary JS
tauri-connector wait ".loaded"                # Wait for CSS selector to appear (default 5s)
tauri-connector wait --text "Success"         # Wait for text
tauri-connector wait ".modal" --timeout 10000 # Custom timeout
```

### IPC & Events

```bash
tauri-connector ipc exec greet -a '{"name":"world"}'   # Execute Tauri IPC command
tauri-connector ipc monitor                   # Start capturing IPC traffic
tauri-connector ipc captured                  # View captured IPC calls
tauri-connector ipc captured -f greet         # Filter by command name
tauri-connector ipc captured -p "user_\d+"    # Regex filter
tauri-connector ipc unmonitor                 # Stop capturing

tauri-connector emit my-event -p '{"foo":42}' # Emit custom Tauri event
tauri-connector events listen user:login,app:error  # Listen for specific events
tauri-connector events captured               # View captured events
tauri-connector events stop                   # Stop listening

tauri-connector clear all                     # Clear all log files
tauri-connector clear logs                    # Console logs only
```

### Window Management

```bash
tauri-connector windows                       # List all windows
tauri-connector resize 1024 768               # Resize window
tauri-connector resize 800 600 --window-id settings  # Specific window
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WebSocket port |

## MCP Tools

25 tools. Configure in `.mcp.json`: `{ "mcpServers": { "tauri-connector": { "url": "http://127.0.0.1:9556/sse" } } }`

### Core Interaction Tools

**`webview_interact`** -- Perform gestures on elements.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `click`, `double-click`, `dblclick`, `focus`, `scroll`, `hover`, `hover-off`, `drag` |
| `selector` | string | | Source element (CSS/XPath/text) |
| `strategy` | string | | `css` (default), `xpath`, `text` |
| `x`, `y` | number | | Source coordinates (alternative to selector) |
| `direction` | string | | `up`/`down`/`left`/`right` (scroll only) |
| `distance` | number | | Scroll px, default 300 |
| `targetSelector` | string | | Drag target CSS selector |
| `targetX`, `targetY` | number | | Drag target coordinates |
| `steps` | number | | Drag intermediate events, default 10 |
| `durationMs` | number | | Drag duration ms, default 300 |
| `dragStrategy` | string | | `auto` (default), `pointer`, `html5dnd` |

**`webview_keyboard`** -- Type text or press keys.

| Param | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | `type` or `press` |
| `text` | string | | Text to type (for `type`) |
| `key` | string | | Key name (for `press`): Enter, Tab, Escape, etc. |
| `modifiers` | string[] | | `ctrl`, `shift`, `alt`, `meta` |

**`webview_wait_for`** -- Wait for element or text to appear. Polls every 100ms.

| Param | Type | Description |
|---|---|---|
| `selector` | string | CSS/XPath/text to wait for |
| `strategy` | string | `css`, `xpath`, `text` |
| `text` | string | Text content to wait for |
| `timeout` | number | Timeout ms, default 5000 |

### DOM & Inspection Tools

**`webview_dom_snapshot`** -- Get structured DOM. Mode `ai` (default) includes ref IDs, React component names, portal stitching.

| Param | Type | Default | Description |
|---|---|---|---|
| `mode` | string | `ai` | `ai`, `accessibility`, `structure` |
| `selector` | string | | CSS selector to scope subtree |
| `maxDepth` | number | unlimited | Max tree depth |
| `maxElements` | number | unlimited | Max element count |
| `reactEnrich` | boolean | true | Include React component names |
| `followPortals` | boolean | true | Stitch portals to triggers |
| `shadowDom` | boolean | false | Traverse shadow DOM |

**`webview_find_element`** -- Find elements by CSS, XPath, text, or regex.

| Param | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | Search query |
| `strategy` | string | | `css` (default), `xpath`, `text`, `regex` |
| `target` | string | | What regex matches: `text`, `class`, `id`, `attr`, `all` |

**`webview_search_snapshot`** -- Regex search over DOM snapshot with context lines.

| Param | Type | Required | Description |
|---|---|---|---|
| `pattern` | string | yes | Regex pattern |
| `context` | number | | Context lines, default 2, max 10 |
| `mode` | string | | `ai`, `accessibility`, `structure` |

**`get_cached_dom`** -- Get pre-cached DOM pushed from frontend (faster, no round-trip).

**`webview_get_styles`** -- Get computed CSS for a CSS-selected element. Pass `properties` array to filter.

**`webview_execute_js`** -- Execute arbitrary JavaScript. Use IIFE for return values: `"(() => { return value; })()"`.

**`webview_screenshot`** -- Native capture. Params: `format` (png/jpeg/webp), `quality` (0-100), `maxWidth`.

### IPC & Monitoring Tools

**`ipc_get_backend_state`** -- App name, version, debug/release, OS, arch, window list.

**`ipc_execute_command`** -- Call any Tauri IPC command: `{ "command": "greet", "args": {"name": "world"} }`.

**`ipc_monitor`** -- Start/stop IPC monitoring: `{ "action": "start" }` or `{ "action": "stop" }`.

**`ipc_get_captured`** -- Read captured IPC. Params: `filter` (substring), `pattern` (regex), `limit`, `since` (epoch ms).

**`ipc_emit_event`** -- Emit Tauri event: `{ "eventName": "my-event", "payload": {...} }`.

**`ipc_listen`** -- Start/stop event listeners: `{ "action": "start", "events": ["user:login", "app:error"] }`.

**`event_get_captured`** -- Read captured events. Params: `event` (exact name), `pattern` (regex), `limit`, `since`.

**`read_logs`** -- Console logs. Params: `lines` (default 50), `level` (comma-separated), `pattern` (regex).

**`read_log_file`** -- Historical JSONL logs. Params: `source` (console/ipc/events), `lines`, `level`, `pattern`, `since`.

**`clear_logs`** -- Clear log files. Param: `source` (console/ipc/events/all).

**`manage_window`** -- `{ "action": "list" }`, `{ "action": "info" }`, `{ "action": "resize", "width": 1024, "height": 768 }`.

## Common Workflows

### Explore an Unknown Page

```
1. webview_dom_snapshot (mode: "ai")        → see full element tree with refs
2. webview_screenshot                        → see what it looks like
3. webview_find_element (strategy: "text")  → find specific text/buttons
```

### Fill and Submit a Form

```
1. webview_dom_snapshot (selector: ".form") → see form fields with refs
2. webview_interact (action: "click", selector from snapshot) → focus field
3. webview_keyboard (action: "type", text: "value")  → fill each field
4. webview_interact (action: "click", selector: "submit button")
5. webview_wait_for (text: "Success")       → verify submission
```

### Drag to Reorder a List

```
1. webview_dom_snapshot → identify source and target refs
2. webview_interact (action: "drag", selector: "#item-3", targetSelector: "#item-1", steps: 15)
3. webview_dom_snapshot → verify new order
```

### Debug an Error

```
1. read_logs (level: "error")               → check console errors
2. ipc_monitor (action: "start")            → start capturing IPC
3. webview_interact (trigger the action)
4. ipc_get_captured                         → see what IPC calls happened
5. webview_dom_snapshot                     → inspect DOM state
6. read_logs (pattern: "specific-error")    → search for patterns
```

### Ant Design / React Apps

The snapshot engine understands React: it reads `__reactFiber$` internals to show component names, detects portals via `aria-controls`/`aria-owns` and stitches them to their triggers, and annotates virtual scroll containers. Example snapshot output:

```
- combobox "Status" [ref=e5, component=InternalSelect, expanded=true]:
  - listbox "Status options" [portal]:
    - option "Active" [selected]
    - option "Inactive"
- list [virtual-scroll, visible=8]:
  - option "Item 1" [ref=e10]
```

Use `snapshot -i -s ".ant-form"` to scope to a specific form subtree.

## Bun Scripts (Fallback)

When neither MCP nor CLI is available. Requires `bun` runtime. Scripts at `~/opensource/tauri-connector/skill/scripts/`.

```bash
SCRIPTS=~/opensource/tauri-connector/skill/scripts
bun run $SCRIPTS/snapshot.ts                           # AI snapshot
bun run $SCRIPTS/click.ts "button.submit"              # Click
bun run $SCRIPTS/hover.ts ".menu-trigger"              # Hover
bun run $SCRIPTS/hover.ts ".menu-trigger" --off        # Hover-off
bun run $SCRIPTS/drag.ts "#item" ".drop-zone"          # Drag
bun run $SCRIPTS/drag.ts "#item" "400,300" --steps 15  # Drag to coords
bun run $SCRIPTS/fill.ts "input" "query"               # Fill input
bun run $SCRIPTS/screenshot.ts /tmp/shot.png 1280      # Screenshot
bun run $SCRIPTS/find.ts "button"                      # Find elements
bun run $SCRIPTS/logs.ts 50                            # Console logs
bun run $SCRIPTS/eval.ts "document.title"              # Execute JS
bun run $SCRIPTS/wait.ts ".loaded"                     # Wait for element
bun run $SCRIPTS/state.ts                              # App metadata
bun run $SCRIPTS/windows.ts                            # List windows
bun run $SCRIPTS/events.ts listen user:login           # Listen for events
```

## Troubleshooting

**Connection Refused** -- App isn't running or plugin isn't loaded. Check: `lsof -i :9555 | grep LISTEN`. Start with `bun run tauri dev`.

**Stale PID File** -- If the app crashed: `rm target/debug/.connector.json`.

**Port Conflict** -- Use `ConnectorBuilder::new().port_range(9600, 9700)` in Rust, or set `TAURI_CONNECTOR_PORT=9600`.

**Refs Not Found** -- DOM changed since last snapshot. Re-run `snapshot -i` to get fresh refs.

**Drag Not Working** -- Try explicit `--strategy pointer` or `--strategy html5dnd`. Increase `--steps` (some libs need >5px movement to trigger). Increase `--duration` for timing-sensitive implementations.
