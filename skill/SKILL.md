---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, debug Tauri desktop apps, or SET UP tauri-connector in a project. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides automated setup, embedded MCP server, Rust CLI with ref-based element addressing."
---

# Tauri Connector

Deep inspection and interaction with Tauri v2 desktop apps. Fixes the `__TAURI__ not available` bug by using a dual-path JS execution strategy: WebSocket bridge (primary) with Tauri eval+event fallback. The **MCP server runs inside the plugin** -- starts automatically when the Tauri app runs.

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

### Step 4: Verify `withGlobalTauri` (REQUIRED)

Check `src-tauri/tauri.conf.json` for `"withGlobalTauri": true` under the `app` section. This is **required** for the eval+event fallback JS execution path and auto-push DOM feature. If missing, add it:

```json
{
  "app": {
    "withGlobalTauri": true
  }
}
```

### Step 5: Configure Claude Code

Add to `.mcp.json` in the project root:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

The MCP server is embedded in the plugin -- no separate command or install needed.

### Step 6: Verify

Run the app with `bun run tauri dev` (or `cargo tauri dev`). Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector][mcp] SSE server listening on 0.0.0.0:9556
[connector][mcp] MCP ready for 'App Name' -- url: http://0.0.0.0:9556/sse
[connector] Plugin ready for 'App Name' (com.app.id) -- WS on 0.0.0.0:9555
```

### Custom Configuration

For localhost-only access, custom ports, or disabling the embedded MCP:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")   // default: 0.0.0.0
            .port_range(9600, 9700)      // WS port range (default: 9555-9655)
            .mcp_port_range(9700, 9800)  // MCP port range (default: 9556-9656)
            .build()
    );
}
```

## CLI Usage

The CLI is a Rust binary. Build it from the tauri-connector repo:

```bash
cargo build -p connector-cli --release
# Binary at target/release/tauri-connector
```

Or run directly during development:

```bash
cargo run -p connector-cli -- <command>
```

Set an alias for convenience:

```bash
alias tc="~/opensource/tauri-connector/target/release/tauri-connector"
```

### Workflow: Snapshot then Interact

```bash
tc snapshot -i

# Output:
# - button "Add New" [ref=e5]
# - textbox "Search" [ref=e3]
# - heading "Dashboard" [level=1, ref=e7]

tc click @e5            # Click "Add New"
tc fill @e3 "query"     # Fill search box
tc get text @e7         # Get heading text
tc press Enter          # Press key
tc hover @e2            # Hover element
```

### Snapshot Options

```bash
tc snapshot              # Full DOM tree with refs
tc snapshot -i           # Interactive elements only (best for LLM)
tc snapshot -c           # Compact mode
tc snapshot -i -c        # Interactive + compact (most concise)
tc snapshot -s ".content" # Scope to CSS selector
```

### Element Interactions

```bash
tc click @e5             # Click
tc dblclick @e3          # Double-click
tc hover @e2             # Hover
tc focus @e4             # Focus
tc fill @e4 "text"       # Clear and fill input
tc type @e4 "text"       # Type character by character
tc check @e6             # Check checkbox
tc uncheck @e6           # Uncheck
tc select @e7 "Option"   # Select dropdown option
tc scrollintoview @e10   # Scroll element into view
```

### Keyboard and Scroll

```bash
tc press Enter           # Press key
tc press Tab
tc scroll down 500       # Scroll page
tc scroll up 300
```

### Get Information

```bash
tc get title             # Page title
tc get url               # Current URL
tc get text @e3          # Element text content
tc get html @e3          # Inner HTML
tc get value @e4         # Input value
tc get attr @e1 href     # Attribute
tc get box @e5           # Bounding box
tc get count ".item"     # Element count
```

### Wait and Debug

```bash
tc wait ".loaded"        # Wait for element
tc wait --text "Success" # Wait for text
tc logs                  # Console logs
tc logs -n 50 -f "error" # Filtered logs
tc state                 # App metadata
tc windows               # Window list
```

### Ref Format

Three formats accepted: `@e1`, `ref=e1`, or `e1`. Refs persist across CLI invocations in `/tmp/tauri-connector-refs.json`. Run `snapshot` again to refresh after DOM changes.

## MCP Server

### Embedded (Default -- Recommended)

The MCP server runs inside the plugin. No setup beyond adding the URL to `.mcp.json`:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

Start the Tauri app and the MCP server is live. Tool calls go directly to the plugin handlers -- zero overhead.

### Standalone (Alternative)

If you can't modify the Tauri app, use the standalone Rust MCP binary:

```bash
cargo build -p connector-mcp-server --release
```

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "tauri-connector-mcp",
      "env": {
        "TAURI_CONNECTOR_HOST": "127.0.0.1",
        "TAURI_CONNECTOR_PORT": "9555"
      }
    }
  }
}
```

### 20 MCP Tools

| Category | Tools |
|---|---|
| JavaScript | `webview_execute_js` |
| DOM | `webview_dom_snapshot`, `get_cached_dom` |
| Elements | `webview_find_element`, `webview_get_styles`, `webview_get_pointed_element`, `webview_select_element` |
| Interaction | `webview_interact`, `webview_keyboard`, `webview_wait_for` |
| Screenshot | `webview_screenshot` |
| Windows | `manage_window` |
| IPC | `ipc_get_backend_state`, `ipc_execute_command`, `ipc_monitor`, `ipc_get_captured`, `ipc_emit_event` |
| Logs | `read_logs` |
| Setup | `get_setup_instructions`, `list_devices` |

## WebSocket API

Connect directly to the plugin WS on port 9555 for custom automation:

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

## Screenshot

Take screenshots of the Tauri webview via the `webview_screenshot` MCP tool or WS API:

```bash
# Via WebSocket (type: "screenshot")
ws.send(JSON.stringify({
  id: "1", type: "screenshot",
  format: "png", quality: 80,
  max_width: 1280, window_id: "main"
}));
```

The screenshot uses a tiered approach:
1. **Native screencapture** (macOS) -- uses window position/size, supports resize and format conversion. Requires Screen Recording permission.
2. **html2canvas fallback** -- dynamically injects html2canvas with `foreignObjectRendering: true` for modern CSS support. No app dependencies needed.

The MCP tool returns image content directly. The WS API returns base64-encoded image data in the result.

## Common Workflows

### Understand Current Page

```bash
tc snapshot -i -c          # Get interactive elements
tc get title               # Page title
tc get url                 # Current URL
tc logs -n 10              # Recent console output
```

### Fill a Form

```bash
tc snapshot -i             # Find form fields
tc fill @e3 "John"        # Fill name
tc fill @e4 "john@ex.com" # Fill email
tc click @e7              # Click submit
tc wait --text "Success"  # Wait for confirmation
```

### Navigate and Test

```bash
tc snapshot -i             # See current page
tc click @e12              # Click nav item
tc snapshot -i             # See new page
tc get text @e5            # Verify content
```

### Debug an Issue

```bash
tc logs -f "error"         # Check for errors
tc state                   # App version/environment
tc snapshot -s ".error-panel"  # Scope to error area
```

## Troubleshooting

### Connection Refused
App isn't running or plugin isn't loaded. Run `bun run tauri dev` and check for `[connector]` logs.

### Refs Not Working
Refs expire after DOM changes. Run `snapshot` again to refresh.

### Port Conflict
Use `ConnectorBuilder::new().port_range(9600, 9700).mcp_port_range(9700, 9800)`.

## Source

- Plugin + Embedded MCP: `~/opensource/tauri-connector/plugin/`
- Rust CLI: `~/opensource/tauri-connector/crates/cli/`
- Standalone MCP: `~/opensource/tauri-connector/crates/mcp-server/`
- Shared Client: `~/opensource/tauri-connector/crates/client/`
- GitHub: https://github.com/dickwu/tauri-connector
