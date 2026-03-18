---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, debug Tauri desktop apps, or SET UP tauri-connector in a project. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides automated setup, embedded MCP server, bun scripts for WebSocket interaction."
---

# Tauri Connector

Deep inspection and interaction with Tauri v2 desktop apps. The **MCP server runs inside the plugin** -- starts automatically when the Tauri app runs.

## Setup

For first-time setup in a Tauri project, read `skill/SETUP.md` in the tauri-connector repo. Key steps: add `tauri-plugin-connector = "0.3"` to Cargo.toml, register plugin, add permissions, set `withGlobalTauri: true`, install `@zumer/snapdom` in frontend, add MCP URL to `.mcp.json`.

## Checking if the App is Running

The plugin writes a PID file to `target/.connector.json` when it starts. The bun scripts auto-discover ports from this file. To check manually:

```bash
# Check if PID file exists (look in the Tauri project's target dir)
cat src-tauri/target/debug/.connector.json 2>/dev/null || cat target/debug/.connector.json 2>/dev/null

# Or check the port directly
lsof -i :9555 -P -n 2>/dev/null | grep LISTEN
```

If the app is already running in another terminal, the bun scripts connect directly -- no need to start a new instance.

## Bun Scripts

Ready-to-run scripts at `~/opensource/tauri-connector/skill/scripts/`. Bun runs TypeScript natively with built-in WebSocket. Scripts auto-discover ports from the PID file.

```bash
SCRIPTS=~/opensource/tauri-connector/skill/scripts
```

### Quick Reference

```bash
bun run $SCRIPTS/state.ts                              # App metadata
bun run $SCRIPTS/eval.ts "document.title"              # Execute JS
bun run $SCRIPTS/screenshot.ts /tmp/shot.png 1280      # Screenshot (path, max_width)
bun run $SCRIPTS/snapshot.ts                           # DOM accessibility tree
bun run $SCRIPTS/snapshot.ts structure                  # DOM structure tree
bun run $SCRIPTS/snapshot.ts accessibility ".sidebar"   # Scoped snapshot
bun run $SCRIPTS/find.ts "button"                      # Find elements by CSS
bun run $SCRIPTS/find.ts "Submit" text                 # Find by text content
bun run $SCRIPTS/click.ts "button.submit"              # Click element
bun run $SCRIPTS/click.ts "Add New" text               # Click by text
bun run $SCRIPTS/hover.ts ".menu-trigger"              # Hover (show dropdown/tooltip)
bun run $SCRIPTS/hover.ts ".menu-trigger" --off        # Hover-off (dismiss)
bun run $SCRIPTS/hover.ts "Settings" text              # Hover by text content
bun run $SCRIPTS/fill.ts "input.search" "query"        # Focus + type into input
bun run $SCRIPTS/logs.ts 50                            # Last 50 console logs
bun run $SCRIPTS/logs.ts 20 error                      # Filtered logs
bun run $SCRIPTS/windows.ts                            # List windows
bun run $SCRIPTS/windows.ts main                       # Window info
bun run $SCRIPTS/wait.ts ".loaded"                     # Wait for element
bun run $SCRIPTS/wait.ts "Success" --text              # Wait for text
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | auto from PID file, or `9555` | Plugin WS port |
| `TAURI_CONNECTOR_TIMEOUT` | `15000` | Default timeout (ms) |

### Port Discovery

Scripts resolve the WS port in this order:
1. `TAURI_CONNECTOR_PORT` env var (explicit override)
2. `target/.connector.json` PID file (auto-discovery from nearby `target/` dirs)
3. Default `9555`

### Inline Alternative

For one-off commands without the scripts, use `bun -e`:

```bash
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({ id: '1', type: 'backend_state' }));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 5000);
"
```

## WS Command Types

All commands use `{ id, type, ...params }` with snake_case types:

| Type | Key Params |
|---|---|
| `execute_js` | `script`, `window_id` |
| `screenshot` | `format` (png/jpeg/webp), `quality`, `max_width`, `window_id` |
| `dom_snapshot` | `snapshot_type`, `selector`, `window_id` |
| `find_element` | `selector`, `strategy`, `window_id` |
| `interact` | `action` (click/double-click/focus/scroll/hover/hover-off), `selector`, `strategy`, `x`, `y`, `window_id` |
| `keyboard` | `action`, `text`, `key`, `modifiers`, `window_id` |
| `wait_for` | `selector`, `strategy`, `text`, `timeout`, `window_id` |
| `backend_state` | -- |
| `console_logs` | `lines`, `filter`, `window_id` |
| `window_list` / `window_info` | `window_id` |
| `ipc_execute_command` | `command`, `args` |
| `ipc_emit_event` | `event_name`, `payload` |

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

20 MCP tools: `webview_execute_js`, `webview_screenshot`, `webview_dom_snapshot`, `get_cached_dom`, `webview_find_element`, `webview_get_styles`, `webview_get_pointed_element`, `webview_select_element`, `webview_interact`, `webview_keyboard`, `webview_wait_for`, `manage_window`, `ipc_get_backend_state`, `ipc_execute_command`, `ipc_monitor`, `ipc_get_captured`, `ipc_emit_event`, `read_logs`, `get_setup_instructions`, `list_devices`.

## Common Workflows

### Understand Current Page

```bash
bun run $SCRIPTS/state.ts
bun run $SCRIPTS/eval.ts "(() => ({ title: document.title, url: location.href }))()"
bun run $SCRIPTS/snapshot.ts accessibility
bun run $SCRIPTS/screenshot.ts /tmp/page.png
```

### Fill a Form

```bash
bun run $SCRIPTS/snapshot.ts accessibility ".form"
bun run $SCRIPTS/fill.ts "input[name=email]" "user@example.com"
bun run $SCRIPTS/click.ts "button[type=submit]"
bun run $SCRIPTS/wait.ts "Success" --text
```

### Hover to Reveal, Then Click

```bash
# 1. Hover to trigger dropdown/tooltip/submenu
bun run $SCRIPTS/hover.ts ".menu-trigger"
# 2. Wait for revealed element to appear
bun run $SCRIPTS/wait.ts ".dropdown-menu"
# 3. Click the revealed item
bun run $SCRIPTS/click.ts ".dropdown-item"
# 4. (Optional) Dismiss by hovering off
bun run $SCRIPTS/hover.ts ".menu-trigger" --off
```

Hover fires the full pointer+mouse event sequence (pointerover, pointerenter, mouseover, mouseenter, pointermove, mousemove) with proper coordinates. Works with Ant Design, MUI, Headless UI, Radix, and any framework using JS event listeners.

### Debug

```bash
bun run $SCRIPTS/logs.ts 50 error
bun run $SCRIPTS/state.ts
bun run $SCRIPTS/eval.ts "document.querySelector('.error')?.textContent"
```

## Screenshot

The `webview_screenshot` tool uses a two-tier approach:

1. **xcap** (primary): Native cross-platform window capture via the `xcap` crate. Captures actual rendered pixels on Windows, macOS, and Linux.
2. **snapdom** (fallback): If xcap fails, falls back to `@zumer/snapdom` in the frontend. Requires the package to be installed (see SETUP.md Step 5).

Supported formats: `png` (default), `jpeg`, `webp`. Use `max_width` to resize for smaller payloads.

```bash
bun run $SCRIPTS/screenshot.ts /tmp/shot.png 1280          # PNG, max 1280px wide
bun run $SCRIPTS/screenshot.ts /tmp/shot.webp 800           # WebP, max 800px wide
```

## Troubleshooting

### Connection Refused
App isn't running or plugin isn't loaded. Check: `lsof -i :9555 | grep LISTEN`. Start with `bun run tauri dev`.

### Stale PID File
If the app crashed, the PID file may be stale. Scripts verify the PID is alive and ignore dead entries. Delete manually if needed: `rm target/debug/.connector.json`.

### Port Conflict
Use `ConnectorBuilder::new().port_range(9600, 9700)` or set `TAURI_CONNECTOR_PORT=9600`.
