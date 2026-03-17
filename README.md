# tauri-connector

[![Crates.io](https://img.shields.io/crates/v/tauri-plugin-connector.svg)](https://crates.io/crates/tauri-plugin-connector)
[![License](https://img.shields.io/crates/l/tauri-plugin-connector.svg)](LICENSE)

A Tauri v2 plugin + MCP server + CLI for deep inspection and interaction with Tauri desktop applications. Drop-in replacement for `tauri-plugin-mcp-bridge` that **fixes the `__TAURI__ not available` bug** on macOS.

## The Problem

`tauri-plugin-mcp-bridge` injects JavaScript into the webview that relies on `window.__TAURI__` to send execution results back to Rust. On macOS with WKWebView, the injected scripts run in an isolated content world where `window.__TAURI__` doesn't exist -- causing all JS-based tools (execute_js, dom_snapshot, console logs) to time out.

## The Fix

tauri-connector uses an **internal WebSocket bridge** instead of Tauri's IPC layer. A small JS client injected into the webview connects back to the plugin via `ws://127.0.0.1:{port}`. Results flow through this dedicated channel, completely bypassing the content world isolation issue.

```
Frontend JS (app context)
  |-- invoke('plugin:connector|push_dom') --> Rust state (cached DOM)
  |-- invoke('plugin:connector|push_logs') -> Rust state (cached logs)
  '-- WebSocket ws://127.0.0.1:9300 --------> Bridge (JS execution)

MCP Server (stdio)                    CLI
  '-- WebSocket ws://host:9555 -----> Plugin server <----- WebSocket
       |-- handlers (window, IPC, backend)
       |-- bridge.execute_js() -> WebSocket -> JS result
       '-- state.get_dom() -> cached DOM (instant)
```

## Components

| Component | Description |
|---|---|
| `plugin/` | Rust Tauri v2 plugin (`tauri-plugin-connector` on crates.io) |
| `server/` | TypeScript MCP server for Claude Code / AI agents |
| `cli/` | Command-line interface with ref-based element addressing |

## Features

### 18 MCP Tools

| Category | Tools |
|---|---|
| Session | `driver_session` |
| JavaScript | `webview_execute_js` |
| DOM | `webview_dom_snapshot`, `get_cached_dom` |
| Elements | `webview_find_element`, `webview_get_styles`, `webview_get_pointed_element` |
| Interaction | `webview_interact`, `webview_keyboard`, `webview_wait_for` |
| Screenshot | `webview_screenshot` |
| Windows | `manage_window` |
| IPC | `ipc_get_backend_state`, `ipc_execute_command`, `ipc_monitor`, `ipc_get_captured`, `ipc_emit_event` |
| Logs | `read_logs` |

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

### 1. Add the Tauri Plugin

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-connector = "0.1"
```

Or from git:

```toml
tauri-plugin-connector = { git = "https://github.com/dickwu/tauri-connector" }
```

### 2. Register the Plugin

```rust
// src-tauri/src/lib.rs
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

The plugin only runs in debug builds -- it won't affect production.

### 3. Add Permissions

```json
// src-tauri/capabilities/default.json
{
  "permissions": [
    "connector:default"
  ]
}
```

### 4. Set `withGlobalTauri` (recommended)

```json
// src-tauri/tauri.conf.json
{
  "app": {
    "withGlobalTauri": true
  }
}
```

This enables the auto-push DOM feature. The core JS execution works without it.

### 5. Run Your App

```bash
bun run tauri dev
```

You should see:

```
[connector][bridge] Internal bridge on port 9300
[connector] Plugin initialized for 'Your App' (com.your.app) on 0.0.0.0:9555
[connector][server] Listening on 0.0.0.0:9555
[connector][bridge] Webview client connected from 127.0.0.1:xxxxx
```

## CLI Usage

### Install

```bash
cd cli
bun install  # or npm install
```

### Commands

```bash
# Run via tsx (development)
npx tsx src/index.ts <command>

# Or build and run
bun run build
node dist/index.js <command>
```

### Snapshot

```bash
snapshot                        # Full DOM tree with refs
snapshot -i                     # Interactive elements only
snapshot -c                     # Compact (strip wrappers)
snapshot -i -c                  # Interactive + compact (best for LLM)
snapshot -d 3                   # Max depth 3
snapshot -s ".main-content"     # Scope to selector
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
click @e5                       # Click element
click "#submit-btn"             # Click by CSS selector
dblclick @e3                    # Double-click
hover @e2                       # Hover
focus @e4                       # Focus
fill @e4 "hello@example.com"    # Clear + fill input
type @e4 "search query"         # Type character by character
check @e6                       # Check checkbox
uncheck @e6                     # Uncheck checkbox
select @e7 "Option A" "Option B" # Select option(s)
scrollintoview @e10             # Scroll element into view
```

### Keyboard

```bash
press Enter                     # Press key
press Tab
press Escape
```

### Scroll

```bash
scroll down 500                 # Scroll page down 500px
scroll up 300                   # Scroll page up
scroll left 200                 # Scroll horizontally
scroll down 300 --selector @e5  # Scroll within element
```

### Getters

```bash
get title                       # Page title
get url                         # Current URL
get text @e3                    # Text content
get html @e3                    # Inner HTML
get value @e4                   # Input value
get attr @e1 href               # Attribute value
get box @e5                     # Bounding box {x, y, width, height}
get styles @e5                  # All computed styles
get count ".list-item"          # Element count by selector
```

### Wait

```bash
wait ".loading-complete"        # Wait for element
wait 2000                       # Wait for duration (ms)
wait --text "Success"           # Wait for text to appear
wait --timeout 10000 ".slow"    # Custom timeout
```

### Other

```bash
eval "document.title"           # Run arbitrary JS
logs                            # Console logs (last 20)
logs -n 50                      # Last 50 logs
logs -f "error"                 # Filter logs
state                           # App metadata (name, version, Tauri version)
windows                         # List all windows
help                            # Full help
```

### Ref Persistence

Refs from `snapshot` are saved to `/tmp/tauri-connector-refs.json` and persist across CLI invocations. Run `snapshot` again to refresh refs after DOM changes.

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin port |

## Claude Code Skill (Recommended)

The easiest way to use tauri-connector with Claude Code is to install the included skill. It teaches Claude how to use all CLI commands, MCP tools, and common workflows automatically.

### Install the Skill

```bash
# Copy to your Claude Code skills directory
cp -r skill/SKILL.md ~/.claude/skills/tauri-connector/SKILL.md
```

Or create the directory first if it doesn't exist:

```bash
mkdir -p ~/.claude/skills/tauri-connector
cp skill/SKILL.md ~/.claude/skills/tauri-connector/SKILL.md
```

### What the Skill Provides

Once installed, Claude Code will automatically use tauri-connector when you ask it to:

- "Test the login flow in the tool app"
- "Click the Add New button"
- "What's on the current page?"
- "Fill in the search box with 'aspirin'"
- "Check the console logs for errors"
- "Take a DOM snapshot"

The skill covers:

- **CLI workflow**: `snapshot` -> ref-based interactions (`click @e5`, `fill @e3 "text"`)
- **WebSocket API**: Direct connection for scripts and automation
- **MCP server**: Tool definitions for AI agent integration
- **Common workflows**: Form filling, navigation testing, debugging
- **Troubleshooting**: Connection issues, stale refs, port conflicts

### Sharing the Skill

The skill file is included in the repo at `skill/SKILL.md`. Others can install it by cloning the repo and copying the file to their `~/.claude/skills/tauri-connector/` directory.

## MCP Server Setup

### Install

```bash
cd server
bun install
bun run build
```

### Configure Claude Code

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "node",
      "args": ["/path/to/tauri-connector/server/dist/index.js"],
      "env": {
        "TAURI_CONNECTOR_HOST": "127.0.0.1",
        "TAURI_CONNECTOR_PORT": "9555"
      }
    }
  }
}
```

Or with `tsx` for development:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "command": "npx",
      "args": ["tsx", "/path/to/tauri-connector/server/src/index.ts"]
    }
  }
}
```

## Plugin Configuration

### Custom Bind Address

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")  // localhost only (default: 0.0.0.0)
            .port_range(8000, 8100)     // custom port range (default: 9555-9655)
            .build()
    );
}
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

Alt+Shift+Click any element in the app to capture its metadata. Retrieve via `webview_get_pointed_element` MCP tool or `get pointed` CLI command.

## Project Structure

```
tauri-connector/
|-- plugin/                    # Rust Tauri v2 plugin (crates.io)
|   |-- Cargo.toml
|   |-- build.rs
|   '-- src/
|       |-- lib.rs             # Plugin entry + Tauri IPC commands
|       |-- bridge.rs          # Internal WebSocket bridge (the fix)
|       |-- server.rs          # External WebSocket server
|       |-- handlers.rs        # All 18 command handlers
|       |-- protocol.rs        # Message types
|       '-- state.rs           # Shared state (DOM cache, logs, IPC)
|-- server/                    # TypeScript MCP server
|   |-- package.json
|   '-- src/
|       |-- index.ts           # 18 MCP tool definitions
|       '-- client.ts          # WebSocket client
|-- cli/                       # CLI with ref-based addressing
|   |-- package.json
|   '-- src/
|       |-- index.ts           # Command dispatcher + handlers
|       |-- client.ts          # WebSocket client
|       '-- snapshot.ts        # Ref system + DOM snapshot builder
|-- skill/                     # Claude Code skill
|   '-- SKILL.md               # Auto-triggers on Tauri app interaction
|-- LICENSE
'-- README.md
```

## How It Works

### Internal WebSocket Bridge

1. Plugin starts an internal WebSocket on `127.0.0.1:9300-9400`
2. Bridge JS is injected into the webview via `WebviewWindow`
3. The bridge connects to the internal WebSocket from the page's main JS context
4. JS execution requests flow through this WebSocket channel
5. Results return through the same channel -- no `window.__TAURI__` needed

### Console Log Capture

The bridge intercepts `console.log/warn/error/info/debug`, storing entries in a ring buffer (500 max). Accessible via `read_logs` or auto-pushed to Rust via `invoke()`.

### Ref System

The CLI's `snapshot` command assigns sequential ref IDs (`e1`, `e2`, ...) to interactive and content elements based on their ARIA roles. Three ref formats are accepted: `@e1`, `ref=e1`, or `e1`. Refs are persisted to disk and used across subsequent CLI invocations until the next `snapshot` refreshes them.

## Requirements

- Tauri v2.x
- Rust 2024 edition
- Node.js 18+ / Bun (for MCP server and CLI)

## License

MIT
