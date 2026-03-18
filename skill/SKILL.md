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
tauri-plugin-connector = "0.2"
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

## Bun Scripts

Ready-to-run scripts in `skill/scripts/`. No build step -- bun runs TypeScript natively with built-in WebSocket support.

Set `SCRIPTS` for convenience:

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
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WS port |
| `TAURI_CONNECTOR_TIMEOUT` | `15000` | Default timeout (ms) |

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

### WS Command Types Reference

All commands use `{ id, type, ...params }`. The `type` field uses snake_case:

| Type | Required Params | Optional Params |
|---|---|---|
| `ping` | -- | -- |
| `execute_js` | `script` | `window_id` |
| `screenshot` | -- | `format`, `quality`, `max_width`, `window_id` |
| `dom_snapshot` | -- | `snapshot_type`, `selector`, `window_id` |
| `get_cached_dom` | -- | `window_id` |
| `find_element` | `selector` | `strategy`, `window_id` |
| `get_styles` | `selector` | `properties`, `window_id` |
| `interact` | `action` | `selector`, `strategy`, `x`, `y`, `direction`, `distance`, `window_id` |
| `keyboard` | `action` | `text`, `key`, `modifiers`, `window_id` |
| `wait_for` | -- | `selector`, `strategy`, `text`, `timeout`, `window_id` |
| `window_list` | -- | -- |
| `window_info` | -- | `window_id` |
| `window_resize` | `width`, `height` | `window_id` |
| `backend_state` | -- | -- |
| `ipc_execute_command` | `command` | `args` |
| `ipc_monitor` | `action` | -- |
| `ipc_get_captured` | -- | `filter`, `limit` |
| `ipc_emit_event` | `event_name` | `payload` |
| `console_logs` | -- | `lines`, `filter`, `window_id` |

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

## Common Workflows

### Understand Current Page

```bash
bun run $SCRIPTS/state.ts                                          # App info
bun run $SCRIPTS/eval.ts "(() => ({ title: document.title, url: location.href }))()"
bun run $SCRIPTS/snapshot.ts accessibility                         # Full a11y tree
bun run $SCRIPTS/screenshot.ts /tmp/page.png                       # Visual capture
```

### Fill a Form

```bash
bun run $SCRIPTS/snapshot.ts accessibility ".form"                 # Find fields
bun run $SCRIPTS/fill.ts "input[name=email]" "user@example.com"    # Fill email
bun run $SCRIPTS/fill.ts "input[name=name]" "John"                 # Fill name
bun run $SCRIPTS/click.ts "button[type=submit]"                    # Submit
bun run $SCRIPTS/wait.ts "Success" --text                          # Confirm
```

### Debug an Issue

```bash
bun run $SCRIPTS/logs.ts 50 error                                  # Error logs
bun run $SCRIPTS/state.ts                                          # App version
bun run $SCRIPTS/eval.ts "document.querySelector('.error')?.textContent"
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
