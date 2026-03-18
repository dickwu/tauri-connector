# tauri-connector

[![Crates.io](https://img.shields.io/crates/v/tauri-plugin-connector.svg)](https://crates.io/crates/tauri-plugin-connector)
[![License](https://img.shields.io/crates/l/tauri-plugin-connector.svg)](LICENSE)

A Tauri v2 plugin with **embedded MCP server** + Rust CLI for deep inspection and interaction with Tauri desktop applications. Drop-in replacement for `tauri-plugin-mcp-bridge` that **fixes the `__TAURI__ not available` bug** on macOS.

## The Problem

`tauri-plugin-mcp-bridge` injects JavaScript into the webview that relies on `window.__TAURI__` to send execution results back to Rust. On macOS with WKWebView, the injected scripts run in an isolated content world where `window.__TAURI__` doesn't exist -- causing all JS-based tools (execute_js, dom_snapshot, console logs) to time out.

## The Fix

tauri-connector uses a **dual-path JS execution** strategy:

1. **WS Bridge (primary)** -- A small JS client injected into the webview connects back to the plugin via `ws://127.0.0.1:{port}`. Scripts and results flow through this dedicated WebSocket channel.

2. **Eval+Event fallback** -- If the WS bridge times out (2s), the plugin falls back to injecting JS via Tauri's `window.eval()` and receiving results through Tauri's event system. This path requires `withGlobalTauri: true`.

The fallback is transparent -- callers get the same result regardless of which path succeeds. The **MCP server runs inside the plugin** -- when your Tauri app starts, it starts automatically.

```
Frontend JS (app context)
  |-- invoke('plugin:connector|push_dom') --> Rust state (cached DOM)
  |-- invoke('plugin:connector|push_logs') -> Rust state (cached logs)
  '-- WebSocket ws://127.0.0.1:9300 --------> Bridge (JS execution, path 1)

Plugin (Rust)
  |-- bridge.execute_js()
  |   |-- try WS bridge (2s timeout) --------> webview JS via WebSocket
  |   '-- fallback: window.eval() + event ---> webview JS via Tauri IPC
  |-- native screencapture (macOS) ----------> PNG/JPEG with resize
  '-- html2canvas fallback ------------------> foreignObjectRendering

Claude Code -------- SSE http://host:9556/sse -----> Embedded MCP server
                                                      |-- handlers (direct, in-process)
                                                      |-- bridge.execute_js() -> JS result
                                                      '-- state.get_dom() -> cached DOM

CLI (Rust) -------- WebSocket ws://host:9555 -----> Plugin WS server
```

## Components

| Component | Description |
|---|---|
| `plugin/` | Rust Tauri v2 plugin with **embedded MCP server** (`tauri-plugin-connector` on crates.io) |
| `crates/cli/` | Rust CLI binary with ref-based element addressing |
| `crates/mcp-server/` | Standalone Rust MCP server (alternative to embedded, connects via WebSocket) |
| `crates/client/` | Shared Rust WebSocket client library |

## Features

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

### CLI with Ref-Based Element Addressing

Inspired by [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser). Take a DOM snapshot with stable ref IDs, then interact with elements using those refs:

```bash
# Take snapshot -- assigns ref IDs to interactive elements
$ tauri-connector snapshot -i
- button "Add New" [ref=e113]
- heading "Task Centre" [level=1, ref=e103]
- switch [checked=false, ref=e104]
- textbox "Search..." [ref=e16]
- menuitem [ref=e8]
  - img "user" [ref=e10]

# Interact using refs (persist across CLI invocations)
$ tauri-connector click @e113         # Click "Add New"
$ tauri-connector fill @e16 "aspirin" # Fill search box
$ tauri-connector hover @e8           # Hover menu item
$ tauri-connector get text @e103      # Get "Task Centre"
$ tauri-connector press Enter         # Press key
$ tauri-connector logs -n 5           # Last 5 console logs
```

### Enhanced DOM Access

The plugin auto-pushes DOM snapshots from the frontend via Tauri's native IPC (`invoke()`), which works in the app's own JS context. The `get_cached_dom` tool returns this pre-cached, LLM-friendly snapshot instantly.

## Quick Start

> **Using Claude Code?** Install the skill for automated setup -- see [Claude Code Skill](#claude-code-skill-recommended) below.

### 1. Add the plugin

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-connector = "0.2"
```

### 2. Register it (debug-only)

```rust
// src-tauri/src/lib.rs -- place BEFORE .invoke_handler()
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

### 3. Add permission

```json
// src-tauri/capabilities/default.json -- add to permissions array
"connector:default"
```

### 4. Set `withGlobalTauri` (required)

```json
// src-tauri/tauri.conf.json
{ "app": { "withGlobalTauri": true } }
```

### 5. Configure Claude Code

```json
// .mcp.json -- the MCP server starts automatically with the app
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

### 6. Run

```bash
bun run tauri dev
```

Look for:
```
[connector][mcp] MCP ready for 'MyApp' -- url: http://0.0.0.0:9556/sse
[connector] Plugin ready for 'MyApp' (com.example.app) -- WS on 0.0.0.0:9555
```

The MCP server is now live. Claude Code connects automatically via the URL in `.mcp.json`.

## WebSocket API via Bun

Connect directly to the plugin WebSocket on port 9555 using `bun -e`. No build step or extra dependencies -- bun has native WebSocket support.

### Execute JavaScript

```bash
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'execute_js',
  script: '(() => ({ title: document.title, url: location.href }))()',
  window_id: 'main'
}));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 15000);
"
```

### Take Screenshot

```bash
bun -e "
const fs = require('fs');
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'screenshot',
  format: 'png', quality: 80, max_width: 1280, window_id: 'main'
}));
ws.onmessage = (e) => {
  const r = JSON.parse(e.data);
  if (r.result?.base64) {
    fs.writeFileSync('/tmp/screenshot.png', Buffer.from(r.result.base64, 'base64'));
    console.log('Saved /tmp/screenshot.png', r.result.width + 'x' + r.result.height);
  } else { console.log(r); }
  ws.close();
};
setTimeout(() => process.exit(1), 60000);
"
```

### DOM Snapshot / Click / Type

```bash
# Accessibility tree snapshot
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'dom_snapshot', snapshot_type: 'accessibility', window_id: 'main'
}));
ws.onmessage = (e) => { console.log(JSON.parse(e.data).result); ws.close(); };
setTimeout(() => process.exit(1), 15000);
"

# Click an element
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'interact', action: 'click', selector: 'button.submit', window_id: 'main'
}));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 15000);
"

# Type text into focused element
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'keyboard', action: 'type', text: 'hello', window_id: 'main'
}));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 15000);
"
```

### App State / Logs / Windows

```bash
# App metadata (no bridge needed)
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({ id: '1', type: 'backend_state' }));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 5000);
"

# Console logs
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'console_logs', lines: 20, window_id: 'main'
}));
ws.onmessage = (e) => { console.log(JSON.parse(e.data)); ws.close(); };
setTimeout(() => process.exit(1), 5000);
"
```

### WS Command Reference

All commands use `{ id, type, ...params }` with snake_case types:

| Type | Key Params |
|---|---|
| `ping` | -- |
| `execute_js` | `script`, `window_id` |
| `screenshot` | `format`, `quality`, `max_width`, `window_id` |
| `dom_snapshot` | `snapshot_type`, `selector`, `window_id` |
| `find_element` | `selector`, `strategy`, `window_id` |
| `get_styles` | `selector`, `properties`, `window_id` |
| `interact` | `action`, `selector`, `strategy`, `x`, `y`, `window_id` |
| `keyboard` | `action`, `text`, `key`, `modifiers`, `window_id` |
| `wait_for` | `selector`, `strategy`, `text`, `timeout`, `window_id` |
| `window_list` / `window_info` / `window_resize` | `window_id`, `width`, `height` |
| `backend_state` | -- |
| `ipc_execute_command` | `command`, `args` |
| `ipc_monitor` | `action` |
| `ipc_get_captured` | `filter`, `limit` |
| `ipc_emit_event` | `event_name`, `payload` |
| `console_logs` | `lines`, `filter`, `window_id` |

## Rust CLI (Alternative)

A Rust CLI with ref-based element addressing is also available:

```bash
cargo build -p connector-cli --release
# Binary at target/release/tauri-connector
```

```bash
tauri-connector snapshot -i          # DOM snapshot with ref IDs
tauri-connector click @e5            # Click by ref
tauri-connector fill @e3 "query"     # Fill input
tauri-connector get text @e7         # Get text
tauri-connector press Enter          # Press key
tauri-connector logs -n 10           # Console logs
tauri-connector state                # App metadata
```

Environment: `TAURI_CONNECTOR_HOST` (default `127.0.0.1`), `TAURI_CONNECTOR_PORT` (default `9555`).

## Claude Code Skill (Recommended)

Install the included skill to let Claude Code automatically set up and use tauri-connector.

### Install

```bash
mkdir -p ~/.claude/skills/tauri-connector
cp skill/SKILL.md ~/.claude/skills/tauri-connector/SKILL.md
```

### What It Does

Once installed, Claude will automatically:

- **Set up the plugin** in any Tauri project when asked
- **Use the CLI** for DOM snapshots and element interactions
- **Debug issues** using console logs, app state, and JS execution
- **Automate testing** with snapshot -> click/fill/verify workflows

## MCP Server

### Embedded (Default)

The MCP server starts automatically inside the Tauri plugin when the app runs. Configure Claude Code with:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

No separate process, no Node.js, no install step. Just run your Tauri app.

### Standalone (Alternative)

A standalone Rust MCP binary is also available for cases where you can't modify the Tauri app:

```bash
cargo build -p connector-mcp-server --release
# Binary at target/release/tauri-connector-mcp
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

## Plugin Configuration

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")  // localhost only (default: 0.0.0.0)
            .port_range(8000, 8100)     // WS port range (default: 9555-9655)
            .mcp_port_range(8100, 8200) // MCP port range (default: 9556-9656)
            .build()
    );
}

// Or disable the embedded MCP server:
ConnectorBuilder::new()
    .disable_mcp()
    .build()
```

## Frontend Integration (Optional)

Push DOM snapshots from your frontend for instant LLM access:

```typescript
import { invoke } from '@tauri-apps/api/core';

await invoke('plugin:connector|push_dom', {
  payload: {
    windowId: 'main',
    html: document.body.innerHTML,
    textContent: document.body.innerText,
    accessibilityTree: '',
    structureTree: '',
  }
});
```

The bridge JS also auto-pushes DOM on page load and significant mutations when `window.__TAURI_INTERNALS__` is available.

### Alt+Shift+Click Element Picker

Alt+Shift+Click any element in the app to capture its metadata. Retrieve via `webview_get_pointed_element` MCP tool.

## Project Structure

```
tauri-connector/
|-- Cargo.toml                  # Workspace root
|-- plugin/                     # Rust Tauri v2 plugin (crates.io)
|   |-- Cargo.toml
|   '-- src/
|       |-- lib.rs              # Plugin entry + Tauri IPC commands
|       |-- bridge.rs           # Internal WebSocket bridge (the fix)
|       |-- server.rs           # External WebSocket server (for CLI)
|       |-- mcp.rs              # Embedded MCP SSE server
|       |-- mcp_tools.rs        # MCP tool definitions + dispatch
|       |-- handlers.rs         # All command handlers
|       |-- protocol.rs         # Message types
|       '-- state.rs            # Shared state (DOM cache, logs, IPC)
|-- crates/
|   |-- client/                 # Shared Rust WebSocket client
|   |   '-- src/lib.rs
|   |-- mcp-server/             # Standalone MCP server (alternative)
|   |   '-- src/
|   |       |-- main.rs         # Stdio JSON-RPC loop
|   |       |-- protocol.rs     # JSON-RPC types
|   |       '-- tools.rs        # Tool definitions + dispatch
|   '-- cli/                    # Rust CLI binary
|       '-- src/
|           |-- main.rs         # Clap CLI entry point
|           |-- commands.rs     # Command implementations
|           '-- snapshot.rs     # Ref system + DOM snapshot builder
|-- skill/                      # Claude Code skill + bun scripts
|   |-- SKILL.md                # Usage guide (loaded by Claude Code)
|   |-- SETUP.md                # Setup instructions
|   '-- scripts/                # Bun scripts for WS interaction
|       |-- connector.ts        # Shared helper (auto-discovers ports via PID file)
|       |-- state.ts, eval.ts, screenshot.ts, snapshot.ts
|       |-- click.ts, fill.ts, find.ts, wait.ts
|       '-- logs.ts, windows.ts
|-- LICENSE
'-- README.md
```

## How It Works

### JS Execution (Dual Path)

The bridge uses two execution paths for maximum reliability:

1. **WS Bridge (primary, 2s timeout)**: Internal WebSocket on `127.0.0.1:9300-9400`. Bridge JS injected into the webview connects back, executes scripts via `AsyncFunction`, and returns results through the WebSocket. Uses `tokio::select!` for multiplexed read/write on a single stream.

2. **Eval+Event fallback**: If the WS bridge times out, the plugin injects JS via Tauri's `window.eval()` and receives results through Tauri's event system (`plugin:event|emit`). Requires `withGlobalTauri: true`. Handles double-serialized event payloads automatically.

The fallback is transparent -- `bridge.execute_js()` returns the same result regardless of which path succeeded.

### Screenshot

The `webview_screenshot` tool uses a tiered approach:

1. **Native screencapture** (macOS): Uses `screencapture -R` with Retina-aware window position/size. Supports resize (`maxWidth`) and format conversion (PNG/JPEG) via the `image` crate. Requires Screen Recording permission.

2. **html2canvas fallback**: Dynamically injects [html2canvas](https://html2canvas.hertzen.com/) from CDN with `foreignObjectRendering: true` for modern CSS support (including `lab()`, `oklch()` colors). No app dependencies needed. Returns MCP image content type directly.

### PID File Auto-Discovery

When the plugin starts, it writes `target/.connector.json` with all port info:

```json
{ "pid": 12345, "ws_port": 9555, "mcp_port": 9556, "bridge_port": 9300, "app_name": "MyApp", "app_id": "com.example.app" }
```

The bun scripts in `skill/scripts/` auto-discover this file, verify the PID is alive, and connect without any env vars. If the Tauri app is already running in another terminal, the scripts connect directly -- no need to start a new instance.

### Embedded MCP Server

1. Plugin starts an SSE HTTP server on port 9556 (configurable)
2. Claude Code connects via `GET /sse` and receives an SSE event stream
3. Tool calls arrive via `POST /message` with JSON-RPC bodies
4. Handlers call the bridge and plugin state directly -- zero WebSocket overhead

### Console Log Capture

The bridge intercepts `console.log/warn/error/info/debug`, storing entries in a ring buffer (500 max). Accessible via `read_logs` or auto-pushed to Rust via `invoke()`.

### Ref System

The CLI's `snapshot` command assigns sequential ref IDs (`e1`, `e2`, ...) to interactive and content elements based on their ARIA roles. Three ref formats are accepted: `@e1`, `ref=e1`, or `e1`. Refs are persisted to disk and used across subsequent CLI invocations until the next `snapshot` refreshes them.

## Requirements

- Tauri v2.x
- Rust 2021+ edition

## License

MIT
