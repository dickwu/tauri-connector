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
  |-- xcap native capture (cross-platform) --> PNG/JPEG/WebP with resize
  '-- snapdom fallback ---------------------> DOM-to-image via @zumer/snapdom

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

### 20 Tools (MCP + CLI)

Every tool is available via both the embedded MCP server (for Claude Code) and the Rust CLI (for terminal use). The CLI uses ref-based element addressing inspired by [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser).

| Category | MCP Tool | CLI Command |
|---|---|---|
| JavaScript | `webview_execute_js` | `eval <script>` |
| DOM | `webview_dom_snapshot` | `snapshot [-i] [-c]` |
| DOM (cached) | `get_cached_dom` | `dom` |
| Elements | `webview_find_element` | `find <selector> [-s css\|xpath\|text]` |
| Styles | `webview_get_styles` | `get styles <@ref\|selector>` |
| Picker | `webview_get_pointed_element` | `pointed` |
| Select | `webview_select_element` | *(visual picker, not yet implemented)* |
| Interact | `webview_interact` | `click`, `dblclick`, `hover`, `focus`, `fill`, `type`, `check`, `uncheck`, `select`, `scroll`, `scrollintoview` |
| Keyboard | `webview_keyboard` | `press <key>` |
| Wait | `webview_wait_for` | `wait <selector> [--text] [--timeout]` |
| Screenshot | `webview_screenshot` | `screenshot <path> [-f png\|jpeg\|webp] [-m maxWidth]` |
| Windows | `manage_window` | `windows`, `resize <w> <h>` |
| State | `ipc_get_backend_state` | `state` |
| IPC | `ipc_execute_command` | `ipc exec <cmd> [-a '{...}']` |
| Monitor | `ipc_monitor` | `ipc monitor` / `ipc unmonitor` |
| Captured | `ipc_get_captured` | `ipc captured [-f filter]` |
| Events | `ipc_emit_event` | `emit <event> [-p '{...}']` |
| Logs | `read_logs` | `logs [-n 20] [-f filter]` |
| Setup | `get_setup_instructions` | `examples` |
| Devices | `list_devices` | *(info only)* |

### CLI Ref-Based Addressing

Take a DOM snapshot with stable ref IDs, then interact with elements using those refs:

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
tauri-plugin-connector = "0.4"
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

### 5. Install snapdom (screenshot fallback)

```bash
# In your frontend project
npm install @zumer/snapdom   # or: bun add @zumer/snapdom
```

If your project uses Vite/webpack, no extra setup needed. Otherwise expose on window:

```typescript
import { snapdom } from '@zumer/snapdom';
window.snapdom = snapdom;
```

### 6. Configure Claude Code

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

### 7. Run

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
# Homebrew (macOS/Linux)
brew install dickwu/tap/tauri-connector

# Or build from source
cargo build -p connector-cli --release
# Binary at target/release/tauri-connector
```

```bash
tauri-connector snapshot -i          # DOM snapshot with ref IDs
tauri-connector click @e5            # Click by ref
tauri-connector fill @e3 "query"     # Fill input
tauri-connector get text @e7         # Get text
tauri-connector press Enter          # Press key
tauri-connector screenshot /tmp/s.png -m 1280  # Screenshot
tauri-connector find "Submit" -s text          # Find elements
tauri-connector dom                  # Cached DOM from frontend
tauri-connector logs -n 10           # Console logs
tauri-connector state                # App metadata
tauri-connector resize 1024 768      # Resize window
tauri-connector ipc exec greet -a '{"name":"world"}'  # IPC command
tauri-connector ipc monitor          # Start IPC monitoring
tauri-connector ipc captured -f greet              # Get captured IPC
tauri-connector emit my-event -p '{"foo":42}'      # Emit event
tauri-connector pointed              # Alt+Shift+Click element info
```

Environment: `TAURI_CONNECTOR_HOST` (default `127.0.0.1`), `TAURI_CONNECTOR_PORT` (default `9555`).

## Claude Code Skill (Recommended)

Install the included skill to let Claude Code automatically set up and use tauri-connector.

### Install

```bash
mkdir -p ~/.claude/skills/tauri-connector
cp skill/SKILL.md ~/.claude/skills/tauri-connector/SKILL.md
cp skill/SETUP.md ~/.claude/skills/tauri-connector/SETUP.md
cp -r skill/scripts ~/.claude/skills/tauri-connector/scripts
```

### What It Does

Once installed, Claude will automatically:

- **Set up the plugin** in any Tauri project when asked
- **Use the CLI** for DOM snapshots and element interactions
- **Debug issues** using console logs, app state, and JS execution
- **Automate testing** with snapshot -> click/fill/verify workflows

> **For contributors:** The release workflow skill is at `.claude/skills/tauri-connector-release/SKILL.md` — it triggers automatically when you say "release" or "bump version" inside this repo.

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

1. **xcap native capture** (cross-platform): Uses the [xcap](https://github.com/nashaofu/xcap) crate for pixel-accurate window capture on Windows, macOS, and Linux. Matches the window by title, captures via native OS APIs, then resizes (`maxWidth`) and encodes to PNG/JPEG/WebP via the `image` crate. Runs on a blocking thread to avoid stalling the Tokio runtime.

2. **snapdom fallback**: When xcap is unavailable (e.g. Wayland without permissions, CI environments), falls back to [snapdom](https://github.com/zumerlab/snapdom) (`@zumer/snapdom`) — a fast DOM-to-image library that captures exactly what the web engine renders. Loaded via dynamic `import()` or `window.snapdom` global. No CDN dependency, works fully offline.

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
- Rust 2024 edition
- `@zumer/snapdom` in frontend (optional, for screenshot fallback when xcap is unavailable)

## License

MIT
