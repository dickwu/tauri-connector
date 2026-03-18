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

## CLI Usage

### Install (build from source)

```bash
cargo build -p connector-cli --release
# Binary at target/release/tauri-connector
```

### Commands

```bash
tauri-connector <command> [args...]
tauri-connector --help               # List all commands
tauri-connector examples             # Detailed help with examples
```

### Snapshot

```bash
tauri-connector snapshot                        # Full DOM tree with refs
tauri-connector snapshot -i                     # Interactive elements only
tauri-connector snapshot -c                     # Compact (strip wrappers)
tauri-connector snapshot -i -c                  # Interactive + compact (best for LLM)
tauri-connector snapshot -d 3                   # Max depth 3
tauri-connector snapshot -s ".main-content"     # Scope to selector
```

Output format (similar to agent-browser):

```
- role "accessible-name" [attr1, attr2, ref=eN]
```

Example:

```
- navigation
  - link "Home" [ref=e1]
  - link "Products" [ref=e2]
- main
  - heading "Dashboard" [level=1, ref=e3]
  - textbox "Search" [required, ref=e4]: current value
  - button "Submit" [ref=e5]
  - switch [checked=false, ref=e6]
```

### Interactions

All commands accept `@eN` refs or CSS selectors:

```bash
tauri-connector click @e5                       # Click element
tauri-connector dblclick @e3                    # Double-click
tauri-connector hover @e2                       # Hover
tauri-connector focus @e4                       # Focus
tauri-connector fill @e4 "hello@example.com"    # Clear + fill input
tauri-connector type @e4 "search query"         # Type character by character
tauri-connector check @e6                       # Check checkbox
tauri-connector uncheck @e6                     # Uncheck checkbox
tauri-connector select @e7 "Option A" "Option B" # Select option(s)
tauri-connector scrollintoview @e10             # Scroll element into view
```

### Keyboard

```bash
tauri-connector press Enter
tauri-connector press Tab
tauri-connector press Escape
```

### Scroll

```bash
tauri-connector scroll down 500                 # Scroll page down 500px
tauri-connector scroll up 300                   # Scroll page up
tauri-connector scroll left 200                 # Scroll horizontally
tauri-connector scroll down 300 --selector @e5  # Scroll within element
```

### Getters

```bash
tauri-connector get title                       # Page title
tauri-connector get url                         # Current URL
tauri-connector get text @e3                    # Text content
tauri-connector get html @e3                    # Inner HTML
tauri-connector get value @e4                   # Input value
tauri-connector get attr @e1 href               # Attribute value
tauri-connector get box @e5                     # Bounding box {x, y, width, height}
tauri-connector get styles @e5                  # All computed styles
tauri-connector get count ".list-item"          # Element count by selector
```

### Wait

```bash
tauri-connector wait ".loading-complete"        # Wait for element
tauri-connector wait --text "Success"           # Wait for text to appear
tauri-connector wait --timeout 10000 ".slow"    # Custom timeout
```

### Other

```bash
tauri-connector eval "document.title"           # Run arbitrary JS
tauri-connector logs                            # Console logs (last 20)
tauri-connector logs -n 50                      # Last 50 logs
tauri-connector logs -f "error"                 # Filter logs
tauri-connector state                           # App metadata
tauri-connector windows                         # List all windows
```

### Ref Persistence

Refs from `snapshot` are saved to `/tmp/tauri-connector-refs.json` and persist across CLI invocations. Run `snapshot` again to refresh refs after DOM changes.

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WebSocket port |

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
|-- skill/                      # Claude Code skill
|   '-- SKILL.md
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
