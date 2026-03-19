---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, debug Tauri desktop apps, or SET UP tauri-connector in a project. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides automated setup, embedded MCP server, Rust CLI with ref-based addressing, and bun scripts as fallback."
---

# Tauri Connector

Deep inspection and interaction with Tauri v2 desktop apps. The **MCP server runs inside the plugin** -- starts automatically when the Tauri app runs.

## Setup

For first-time setup in a Tauri project, read `skill/SETUP.md` in the tauri-connector repo. Key steps: add `tauri-plugin-connector = "0.5"` to Cargo.toml, register plugin, add permissions, set `withGlobalTauri: true`, install `@zumer/snapdom` in frontend, add MCP URL to `.mcp.json`.

## Checking if the App is Running

The plugin writes a PID file to `target/.connector.json` when it starts. To check:

```bash
cat src-tauri/target/debug/.connector.json 2>/dev/null || cat target/debug/.connector.json 2>/dev/null
lsof -i :9555 -P -n 2>/dev/null | grep LISTEN
```

## CLI (Primary)

The `tauri-connector` CLI is the recommended way to interact with Tauri apps from the terminal.

```bash
# Homebrew (macOS/Linux)
brew install dickwu/tap/tauri-connector

# Or self-update if already installed
tauri-connector update

# Or build from source
cargo build -p connector-cli --release
```

### Quick Reference

```bash
# DOM & Inspection
tauri-connector snapshot -i                              # AI snapshot with refs (default)
tauri-connector snapshot -i --mode accessibility          # Accessibility tree only
tauri-connector snapshot -i --mode structure              # Structure tree only
tauri-connector snapshot -i --no-react                    # Skip React component names
tauri-connector snapshot -i --no-portals                  # Skip portal stitching
tauri-connector snapshot -i --max-depth 5                 # Limit tree depth
tauri-connector snapshot -i --max-elements 2000           # Limit element count
tauri-connector dom                                       # Cached DOM (pushed from frontend)
tauri-connector find "button"                             # Find elements by CSS
tauri-connector find "Submit" -s text                     # Find by text content
tauri-connector get text @e7                              # Get text content by ref
tauri-connector get title                                 # Page title
tauri-connector get url                                   # Current URL
tauri-connector pointed                                   # Alt+Shift+Click element info

# Interaction (use @eN refs from snapshot)
tauri-connector click @e5                      # Click by ref
tauri-connector click "button.submit"          # Click by CSS selector
tauri-connector dblclick @e5                   # Double-click
tauri-connector hover @e8                      # Hover (trigger dropdown/tooltip)
tauri-connector focus @e3                      # Focus element
tauri-connector fill @e3 "user@example.com"    # Clear and fill input
tauri-connector type @e3 "hello"               # Type character by character
tauri-connector check @e10                     # Check checkbox
tauri-connector uncheck @e10                   # Uncheck checkbox
tauri-connector select @e6 "option1" "option2" # Select options
tauri-connector scroll down 300                # Scroll page
tauri-connector scrollintoview @e20            # Scroll element into view

# Keyboard
tauri-connector press Enter                    # Press key
tauri-connector press Tab                      # Press Tab

# Screenshot
tauri-connector screenshot /tmp/shot.png -m 1280   # PNG, max 1280px wide
tauri-connector screenshot /tmp/s.webp -f webp     # WebP format

# IPC & Events
tauri-connector state                          # App backend state
tauri-connector ipc exec greet -a '{"name":"world"}'  # Execute IPC command
tauri-connector ipc monitor                    # Start IPC monitoring
tauri-connector ipc unmonitor                  # Stop IPC monitoring
tauri-connector ipc captured -f greet          # Get captured IPC traffic
tauri-connector emit my-event -p '{"foo":42}'  # Emit custom event

# Logs & Windows
tauri-connector logs -n 50                     # Last 50 console logs
tauri-connector logs -n 20 -f error            # Filtered logs
tauri-connector logs -n 10 -l error            # Error logs only (level filter)
tauri-connector logs -p "user_\\d+"            # Regex filter
tauri-connector windows                        # List windows
tauri-connector resize 1024 768                # Resize window

# Events
tauri-connector events listen user:login       # Listen for events
tauri-connector events captured                # Get captured events
tauri-connector events stop                    # Stop listening

# Clear
tauri-connector clear all                      # Clear all log files
tauri-connector clear logs                     # Clear console logs only

# Other
tauri-connector eval "document.title"          # Execute JavaScript
tauri-connector wait ".loaded"                 # Wait for element
tauri-connector wait --text "Success"          # Wait for text
tauri-connector --version                      # Show version
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WS port |

## Bun Scripts (Fallback)

If the CLI binary is not available, use the bun scripts at `~/opensource/tauri-connector/skill/scripts/`. Bun runs TypeScript natively with built-in WebSocket. Scripts auto-discover ports from the PID file.

```bash
SCRIPTS=~/opensource/tauri-connector/skill/scripts
```

```bash
bun run $SCRIPTS/state.ts                              # App metadata
bun run $SCRIPTS/eval.ts "document.title"              # Execute JS
bun run $SCRIPTS/screenshot.ts /tmp/shot.png 1280      # Screenshot
bun run $SCRIPTS/snapshot.ts                           # AI mode snapshot (default)
bun run $SCRIPTS/snapshot.ts accessibility                # Accessibility tree
bun run $SCRIPTS/snapshot.ts ai ".form"                   # Scoped to selector
bun run $SCRIPTS/find.ts "button"                      # Find elements by CSS
bun run $SCRIPTS/click.ts "button.submit"              # Click element
bun run $SCRIPTS/hover.ts ".menu-trigger"              # Hover
bun run $SCRIPTS/hover.ts ".menu-trigger" --off        # Hover-off (dismiss)
bun run $SCRIPTS/fill.ts "input.search" "query"        # Fill input
bun run $SCRIPTS/logs.ts 50                            # Console logs
bun run $SCRIPTS/windows.ts                            # List windows
bun run $SCRIPTS/wait.ts ".loaded"                     # Wait for element
```

Bun scripts also support `TAURI_CONNECTOR_HOST`, `TAURI_CONNECTOR_PORT`, and `TAURI_CONNECTOR_TIMEOUT` env vars, plus auto-discovery via PID file.

## MCP Server

The embedded MCP server starts automatically. Configure in `.mcp.json`:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

25 MCP tools with full CLI parity: `webview_execute_js`, `webview_screenshot`, `webview_dom_snapshot` (modes: ai/accessibility/structure, with portal stitching, React enrichment, and ref IDs), `get_cached_dom`, `webview_find_element` (strategies: css/xpath/text/regex; optional `target` window), `webview_search_snapshot`, `webview_get_styles`, `webview_get_pointed_element`, `webview_select_element`, `webview_interact`, `webview_keyboard`, `webview_wait_for`, `manage_window`, `ipc_get_backend_state`, `ipc_execute_command`, `ipc_monitor`, `ipc_get_captured` (supports `pattern` regex and `since` timestamp filters), `ipc_listen`, `ipc_emit_event`, `event_get_captured`, `read_logs` (supports `level` and `pattern` filters), `read_log_file`, `clear_logs`, `get_setup_instructions`, `list_devices`.

### Tool Parameter Details

#### `read_logs`

| Param | Type | Description |
|---|---|---|
| `lines` | number | Number of recent log entries to return (default 20) |
| `filter` | string | Legacy text filter on log messages |
| `level` | string | Filter by log level: `error`, `warn`, `info`, `debug` |
| `pattern` | string | Regex pattern to match against log messages |
| `window_id` | string | Target window (default `main`) |

#### `ipc_get_captured`

| Param | Type | Description |
|---|---|---|
| `filter` | string | Filter by IPC command name |
| `limit` | number | Max entries to return |
| `pattern` | string | Regex pattern to match against IPC payloads |
| `since` | string | ISO 8601 timestamp -- only return entries after this time |

#### `webview_find_element`

| Param | Type | Description |
|---|---|---|
| `selector` | string | CSS selector, XPath, text content, or regex pattern |
| `strategy` | string | One of: `css` (default), `xpath`, `text`, `regex` |
| `target` | string | Target window label (default `main`) |
| `window_id` | string | Alias for `target` |

#### `clear_logs`

Clears stored log data. Accepts a `source` param to target specific stores.

| Param | Type | Description |
|---|---|---|
| `source` | string | What to clear: `logs` (console), `ipc` (captured IPC), `events` (captured events), `all` (everything) |

#### `read_log_file`

Reads directly from the JSONL log file on disk with server-side filtering.

| Param | Type | Description |
|---|---|---|
| `source` | string | Log source file to read (default `console`) |
| `lines` | number | Number of lines to read from end of file |
| `level` | string | Filter by log level |
| `pattern` | string | Regex pattern filter |
| `since` | string | ISO 8601 timestamp -- only return entries after this time |
| `window_id` | string | Target window (default `main`) |

#### `ipc_listen`

Starts or stops listening for specific Tauri events in real time.

| Param | Type | Description |
|---|---|---|
| `action` | string | `start` to begin listening, `stop` to end |
| `events` | string[] | List of event names to listen for (used with `start`) |

#### `event_get_captured`

Returns events captured by an active `ipc_listen` session.

| Param | Type | Description |
|---|---|---|
| `event` | string | Filter by event name |
| `pattern` | string | Regex pattern to match against event payloads |
| `limit` | number | Max entries to return |
| `since` | string | ISO 8601 timestamp -- only return entries after this time |

#### `webview_search_snapshot`

Searches the most recent DOM snapshot for matching text or patterns.

| Param | Type | Description |
|---|---|---|
| `pattern` | string | Text or regex pattern to search for |
| `context` | number | Number of surrounding lines to include (default 2) |
| `mode` | string | Snapshot mode to search: `ai`, `accessibility`, `structure` |
| `window_id` | string | Target window (default `main`) |

## Common Workflows

### Understand Current Page

```bash
tauri-connector state
tauri-connector eval "(() => ({ title: document.title, url: location.href }))()"
tauri-connector snapshot -i
tauri-connector screenshot /tmp/page.png -m 1280
```

### Fill a Form

```bash
tauri-connector snapshot -i -s ".form"
tauri-connector fill @e3 "user@example.com"
tauri-connector click @e5
tauri-connector wait --text "Success"
```

### Hover to Reveal, Then Click

```bash
tauri-connector hover @e8                    # Trigger dropdown/tooltip
tauri-connector wait ".dropdown-menu"        # Wait for it to appear
tauri-connector click ".dropdown-item"       # Click revealed item
tauri-connector hover @e8                    # (Optional) hover-off to dismiss
```

### Debug

```bash
tauri-connector logs -n 50 -f error
tauri-connector state
tauri-connector eval "document.querySelector('.error')?.textContent"
```

### Debugging Workflow

Step-by-step recipe for diagnosing a Tauri app issue:

```bash
# 1. Start IPC monitor to capture backend traffic
tauri-connector ipc monitor

# 2. Listen for specific frontend events
tauri-connector events listen user:login app:error

# 3. Trigger the action you want to debug (interact with the app)
tauri-connector click @e5

# 4. Check logs with level filter for errors/warnings
tauri-connector logs -n 30 -l error
tauri-connector logs -p "timeout|failed"

# 5. Search the DOM snapshot for relevant state
tauri-connector snapshot -i
# then search within the snapshot:
# (use webview_search_snapshot MCP tool with pattern)

# 6. Review captured IPC and events
tauri-connector ipc captured
tauri-connector events captured

# 7. Clean up when done
tauri-connector ipc unmonitor
tauri-connector events stop
tauri-connector clear all
```

### Ant Design / React Apps

```bash
# Full AI snapshot — portals stitched, components named
tauri-connector snapshot -i
# Output shows portals as logical children of triggers:
# - combobox "Status" [ref=e5, component=InternalSelect, expanded=true]:
#   - listbox "Status options" [portal]:
#     - option "Active" [selected]
#     - option "Inactive"

# Virtual scroll containers annotated:
# - list [virtual-scroll, visible=8]:
#   - option "Item 1" [ref=e10]

# Scope snapshot to a specific form
tauri-connector snapshot -i -s ".ant-form"
```

### IPC Debugging

```bash
tauri-connector ipc monitor                  # Start capturing
# ... interact with the app ...
tauri-connector ipc captured                 # See all captured IPC calls
tauri-connector ipc captured -f greet        # Filter by command name
tauri-connector ipc unmonitor                # Stop capturing
```

## Screenshot

The `webview_screenshot` tool / `screenshot` CLI command uses a two-tier approach:

1. **xcap** (primary): Native cross-platform window capture via the `xcap` crate.
2. **snapdom** (fallback): Falls back to `@zumer/snapdom` in the frontend.

Supported formats: `png` (default), `jpeg`, `webp`. Use `-m` / `maxWidth` to resize.

## Troubleshooting

### Connection Refused
App isn't running or plugin isn't loaded. Check: `lsof -i :9555 | grep LISTEN`. Start with `bun run tauri dev`.

### Stale PID File
If the app crashed, the PID file may be stale. Delete manually: `rm target/debug/.connector.json`.

### Port Conflict
Use `ConnectorBuilder::new().port_range(9600, 9700)` or set `TAURI_CONNECTOR_PORT=9600`.
