---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, debug Tauri desktop apps, or SET UP tauri-connector in a project. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides automated setup, MCP tools, and CLI with ref-based element addressing."
---

# Tauri Connector

Deep inspection and interaction with Tauri v2 desktop apps. Fixes the `__TAURI__ not available` bug by using an internal WebSocket bridge instead of Tauri's IPC layer.

## When to Use

- Setting up tauri-connector in a Tauri project
- Testing UI flows in Tauri desktop apps
- Automating webview interactions (click, hover, fill, type, scroll)
- Taking DOM snapshots for understanding page structure
- Reading console logs from the webview
- Executing JavaScript in the webview context
- Inspecting app metadata, window state, IPC commands
- Debugging desktop app behavior

## Automated Setup

When the user asks to set up tauri-connector in a Tauri project, follow these steps automatically. Detect the project by looking for `src-tauri/` directory and `tauri.conf.json`.

### Step 1: Add Cargo dependency

Check `src-tauri/Cargo.toml`. If `tauri-plugin-connector` is not present, add it:

```toml
[dependencies]
tauri-plugin-connector = "0.1"
```

### Step 2: Register the plugin

Check `src-tauri/src/lib.rs` or `src-tauri/src/main.rs` for the `tauri::Builder` chain. Add the plugin registration wrapped in `#[cfg(debug_assertions)]` so it only runs in dev builds:

```rust
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

Place this BEFORE the `.invoke_handler()` call and AFTER the initial builder creation.

### Step 3: Add permissions

Check `src-tauri/capabilities/default.json` (or the main capabilities file). Add `"connector:default"` to the `permissions` array:

```json
{
  "permissions": [
    "connector:default"
  ]
}
```

### Step 4: Verify `withGlobalTauri`

Check `src-tauri/tauri.conf.json` for `"withGlobalTauri": true` under the `app` section. If missing, add it — this enables the auto-push DOM feature:

```json
{
  "app": {
    "withGlobalTauri": true
  }
}
```

### Step 5: Verify

Run the app with `bun run tauri dev` (or `cargo tauri dev`). Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector] Plugin initialized for 'App Name' (com.app.id) on 0.0.0.0:9555
[connector][server] Listening on 0.0.0.0:9555
[connector][bridge] Webview client connected from 127.0.0.1:xxxxx
```

### Custom Configuration

For localhost-only access or custom ports:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")  // default: 0.0.0.0
            .port_range(9600, 9700)     // default: 9555-9655
            .build()
    );
}
```

## CLI Usage

The CLI is at `~/opensource/tauri-connector/cli/`. Run commands with:

```bash
CLI="npx tsx ~/opensource/tauri-connector/cli/src/index.ts"
```

### Workflow: Snapshot then Interact

```bash
# 1. Take snapshot to get ref IDs
$CLI snapshot -i

# Output:
# - button "Add New" [ref=e5]
# - textbox "Search" [ref=e3]
# - heading "Dashboard" [level=1, ref=e7]

# 2. Interact using refs
$CLI click @e5            # Click "Add New"
$CLI fill @e3 "query"     # Fill search box
$CLI get text @e7         # Get heading text
$CLI press Enter          # Press key
$CLI hover @e2            # Hover element
```

### Snapshot Options

```bash
$CLI snapshot              # Full DOM tree with refs
$CLI snapshot -i           # Interactive elements only (best for LLM)
$CLI snapshot -c           # Compact mode
$CLI snapshot -i -c        # Interactive + compact (most concise)
$CLI snapshot -s ".content" # Scope to CSS selector
```

### Element Interactions

```bash
$CLI click @e5             # Click
$CLI dblclick @e3          # Double-click
$CLI hover @e2             # Hover
$CLI focus @e4             # Focus
$CLI fill @e4 "text"       # Clear and fill input
$CLI type @e4 "text"       # Type character by character
$CLI check @e6             # Check checkbox
$CLI uncheck @e6           # Uncheck
$CLI select @e7 "Option"   # Select dropdown option
$CLI scrollintoview @e10   # Scroll element into view
```

### Keyboard and Scroll

```bash
$CLI press Enter           # Press key
$CLI press Tab
$CLI scroll down 500       # Scroll page
$CLI scroll up 300
```

### Get Information

```bash
$CLI get title             # Page title
$CLI get url               # Current URL
$CLI get text @e3          # Element text content
$CLI get html @e3          # Inner HTML
$CLI get value @e4         # Input value
$CLI get attr @e1 href     # Attribute
$CLI get box @e5           # Bounding box
$CLI get count ".item"     # Element count
```

### Wait and Debug

```bash
$CLI wait ".loaded"        # Wait for element
$CLI wait --text "Success" # Wait for text
$CLI logs                  # Console logs
$CLI logs -n 50 -f "error" # Filtered logs
$CLI state                 # App metadata
$CLI windows               # Window list
```

### Ref Format

Three formats accepted: `@e1`, `ref=e1`, or `e1`. Refs persist across CLI invocations in `/tmp/tauri-connector-refs.json`. Run `snapshot` again to refresh after DOM changes.

## WebSocket API

Connect directly to the plugin on port 9555 for automation:

```python
import asyncio, websockets, json

async def test():
    async with websockets.connect('ws://127.0.0.1:9555') as ws:
        await ws.send(json.dumps({
            'id': '1', 'type': 'execute_js',
            'script': '(() => document.title)()',
            'window_id': 'main'
        }))
        print(await ws.recv())

asyncio.run(test())
```

### Command Types

```
ping, execute_js, screenshot, dom_snapshot, get_cached_dom,
find_element, get_styles, interact, keyboard, wait_for,
window_list, window_info, window_resize, backend_state,
ipc_execute_command, ipc_monitor, ipc_get_captured,
ipc_emit_event, console_logs, select_element, get_pointed_element
```

## MCP Server

Configure in Claude Code settings for direct MCP tool access:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "npx",
      "args": ["tsx", "~/opensource/tauri-connector/server/src/index.ts"],
      "env": {
        "TAURI_CONNECTOR_HOST": "127.0.0.1",
        "TAURI_CONNECTOR_PORT": "9555"
      }
    }
  }
}
```

## Common Workflows

### Understand Current Page

```bash
$CLI snapshot -i -c          # Get interactive elements
$CLI get title               # Page title
$CLI get url                 # Current URL
$CLI logs -n 10              # Recent console output
```

### Fill a Form

```bash
$CLI snapshot -i             # Find form fields
$CLI fill @e3 "John"        # Fill name
$CLI fill @e4 "john@ex.com" # Fill email
$CLI click @e7              # Click submit
$CLI wait --text "Success"  # Wait for confirmation
```

### Navigate and Test

```bash
$CLI snapshot -i             # See current page
$CLI click @e12              # Click nav item
$CLI snapshot -i             # See new page
$CLI get text @e5            # Verify content
```

### Debug an Issue

```bash
$CLI logs -f "error"         # Check for errors
$CLI state                   # App version/environment
$CLI snapshot -s ".error-panel"  # Scope to error area
```

## Troubleshooting

### Connection Refused
App isn't running or plugin isn't loaded. Run `bun run tauri dev` and check for `[connector]` logs.

### Refs Not Working
Refs expire after DOM changes. Run `snapshot` again to refresh.

### Port Conflict
Set `TAURI_CONNECTOR_PORT=9600` or use `ConnectorBuilder::new().port_range(9600, 9700)`.

## Source

- Plugin: `~/opensource/tauri-connector/plugin/` (crates.io: `tauri-plugin-connector`)
- MCP Server: `~/opensource/tauri-connector/server/`
- CLI: `~/opensource/tauri-connector/cli/`
- GitHub: https://github.com/dickwu/tauri-connector
