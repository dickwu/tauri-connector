---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, or debug Tauri desktop apps. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides both MCP tools and CLI with ref-based element addressing."
---

# Tauri Connector — Interact with Tauri Desktop Apps

This skill enables deep inspection and interaction with Tauri v2 desktop applications using `tauri-connector`. It fixes the `__TAURI__ not available` bug in `tauri-plugin-mcp-bridge` by using an internal WebSocket bridge for JS execution.

## When to Use

- Testing UI flows in Tauri desktop apps
- Automating webview interactions (click, hover, fill, type, scroll)
- Taking DOM snapshots for understanding page structure
- Reading console logs from the webview
- Executing JavaScript in the webview context
- Inspecting app metadata, window state, IPC commands
- Debugging desktop app behavior
- Any time the user mentions interacting with a running Tauri app

## Prerequisites

The Tauri app must have `tauri-plugin-connector` installed and be running in dev mode:

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-connector = "0.1"
```

```rust
// src-tauri/src/lib.rs
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

```json
// src-tauri/capabilities/default.json
{ "permissions": ["connector:default"] }
```

When running, you should see:
```
[connector] Plugin initialized for 'App Name' on 0.0.0.0:9555
[connector][bridge] Webview client connected
```

## Method 1: CLI (Recommended for Interactive Use)

The CLI provides ref-based element addressing inspired by agent-browser. Located at `~/opensource/tauri-connector/cli/`.

### Workflow: Snapshot → Interact

```bash
CLI="npx tsx ~/opensource/tauri-connector/cli/src/index.ts"

# 1. Take snapshot to get ref IDs
$CLI snapshot -i

# Output:
# - button "Add New" [ref=e5]
# - textbox "Search" [ref=e3]
# - heading "Dashboard" [level=1, ref=e7]

# 2. Interact using refs
$CLI click @e5            # Click "Add New"
$CLI fill @e3 "query"     # Fill search box
$CLI get text @e7         # Get heading text → "Dashboard"
$CLI press Enter          # Press key
$CLI hover @e2            # Hover element
```

### All CLI Commands

#### Snapshot
```bash
$CLI snapshot              # Full DOM tree with refs
$CLI snapshot -i           # Interactive elements only (best for LLM)
$CLI snapshot -c           # Compact mode
$CLI snapshot -i -c        # Interactive + compact (most concise)
$CLI snapshot -s ".content" # Scope to selector
```

#### Element Interactions
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

#### Keyboard & Scroll
```bash
$CLI press Enter           # Press key
$CLI press Tab
$CLI scroll down 500       # Scroll page
$CLI scroll up 300
```

#### Get Information
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

#### Wait & Debug
```bash
$CLI wait ".loaded"        # Wait for element
$CLI wait --text "Success" # Wait for text
$CLI logs                  # Console logs
$CLI logs -n 50 -f "error" # Filtered logs
$CLI state                 # App metadata
$CLI windows               # Window list
$CLI eval "document.title" # Run JS
```

### Ref Format

Three formats accepted: `@e1`, `ref=e1`, or `e1`. Refs persist across CLI invocations in `/tmp/tauri-connector-refs.json`. Run `snapshot` again to refresh after DOM changes.

## Method 2: WebSocket (For Scripts & Automation)

Connect directly to the plugin's WebSocket on port 9555:

```python
import asyncio, websockets, json

async def test():
    async with websockets.connect('ws://127.0.0.1:9555') as ws:
        # Execute JS
        await ws.send(json.dumps({
            'id': '1', 'type': 'execute_js',
            'script': '(() => document.title)()',
            'window_id': 'main'
        }))
        print(await ws.recv())

        # Get backend state
        await ws.send(json.dumps({'id': '2', 'type': 'backend_state'}))
        print(await ws.recv())

asyncio.run(test())
```

### WebSocket Command Types

```
ping, execute_js, screenshot, dom_snapshot, get_cached_dom,
find_element, get_styles, interact, keyboard, wait_for,
window_list, window_info, window_resize, backend_state,
ipc_execute_command, ipc_monitor, ipc_get_captured,
ipc_emit_event, console_logs, select_element, get_pointed_element
```

## Method 3: MCP Server (For Claude Code Integration)

Configure in Claude Code settings:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "npx",
      "args": ["tsx", "/Users/gwddeveloper/opensource/tauri-connector/server/src/index.ts"],
      "env": {
        "TAURI_CONNECTOR_HOST": "127.0.0.1",
        "TAURI_CONNECTOR_PORT": "9555"
      }
    }
  }
}
```

Then use MCP tools directly: `driver_session`, `webview_execute_js`, `webview_dom_snapshot`, `manage_window`, etc.

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
$CLI eval "localStorage.getItem('token')"  # Check state
$CLI snapshot -s ".error-panel"  # Scope to error area
```

## Troubleshooting

### Connection Refused
The Tauri app isn't running or the plugin isn't loaded. Start with `bun run tauri dev` and check for `[connector]` logs.

### Refs Not Working
Refs expire after DOM changes. Run `snapshot` again to refresh.

### Port Conflict
Set custom port: `TAURI_CONNECTOR_PORT=9600` or use `ConnectorBuilder::new().port_range(9600, 9700)` in Rust.

## Source Code

- Plugin: `~/opensource/tauri-connector/plugin/` (Rust, published on crates.io)
- MCP Server: `~/opensource/tauri-connector/server/` (TypeScript)
- CLI: `~/opensource/tauri-connector/cli/` (TypeScript)
- GitHub: https://github.com/dickwu/tauri-connector
