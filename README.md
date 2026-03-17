# tauri-connector

A Tauri v2 plugin + MCP server for deep inspection and interaction with Tauri desktop applications. Drop-in replacement for `tauri-plugin-mcp-bridge` that **fixes the `__TAURI__ not available` bug** on macOS.

## The Problem

`tauri-plugin-mcp-bridge` injects JavaScript into the webview that relies on `window.__TAURI__` to send execution results back to Rust. On macOS with WKWebView, the injected scripts run in an isolated content world where `window.__TAURI__` doesn't exist — causing all JS-based tools (execute_js, dom_snapshot, console logs) to time out.

## The Fix

tauri-connector uses an **internal WebSocket bridge** instead of Tauri's IPC layer. A small JS client injected into the webview connects back to the plugin via `ws://127.0.0.1:{port}`. Results flow through this dedicated channel, completely bypassing the content world isolation issue.

```
Frontend JS (app context)
  |-- invoke('plugin:connector|push_dom') --> Rust state (cached DOM)
  |-- invoke('plugin:connector|push_logs') -> Rust state (cached logs)
  '-- WebSocket ws://127.0.0.1:9300 --------> Bridge (JS execution)

MCP Server (stdio)
  '-- WebSocket ws://host:9555 -------------> Plugin server
       |-- handlers (window, IPC, backend)
       |-- bridge.execute_js() -> WebSocket -> JS result
       '-- state.get_dom() -> cached DOM (instant)
```

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

### Enhanced DOM Access

The plugin auto-pushes DOM snapshots from the frontend via Tauri's native IPC (`invoke()`), which works in the app's own JS context. The `get_cached_dom` tool returns this pre-cached, LLM-friendly snapshot instantly.

## Quick Start

### 1. Add the Tauri Plugin

In your `src-tauri/Cargo.toml`:

```toml
[dependencies]
tauri-plugin-connector = { git = "https://github.com/dickwu/tauri-connector", branch = "main" }
```

Or for local development:

```toml
[dependencies]
tauri-plugin-connector = { path = "/path/to/tauri-connector/plugin" }
```

### 2. Register the Plugin

In `src-tauri/src/lib.rs` or `src-tauri/src/main.rs`:

```rust
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

The plugin only runs in debug builds — it won't affect production.

### 3. Add Permissions

In `src-tauri/capabilities/default.json`:

```json
{
  "permissions": [
    "connector:default"
  ]
}
```

### 4. Set `withGlobalTauri` (recommended)

In `src-tauri/tauri.conf.json`:

```json
{
  "app": {
    "withGlobalTauri": true
  }
}
```

This enables the auto-push DOM feature. The core JS execution works without it.

### 5. Install the MCP Server

```bash
cd server
bun install   # or npm install
bun run build # or npx tsc
```

### 6. Configure Claude Code

Add to your Claude Code MCP settings (`.claude/settings.json` or similar):

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

Or use `bun` / `tsx` for development:

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

### 7. Run Your Tauri App

```bash
bun run tauri dev
# or
cargo tauri dev
```

You should see in the console:

```
[connector][bridge] Internal bridge on port 9300
[connector] Plugin initialized for 'Your App' (com.your.app) on 0.0.0.0:9555
[connector][server] Listening on 0.0.0.0:9555
[connector][bridge] Webview client connected from 127.0.0.1:xxxxx
```

## Configuration

### Custom Bind Address

By default, the plugin listens on `0.0.0.0` (all interfaces). For localhost-only:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")
            .build()
    );
}
```

### Custom Port Range

```rust
ConnectorBuilder::new()
    .port_range(8000, 8100)
    .build()
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | MCP server connects to this host |
| `TAURI_CONNECTOR_PORT` | `9555` | MCP server connects to this port |

## Tool Reference

### `driver_session`

Start, stop, or check the connection to a Tauri app.

```json
{ "action": "start", "host": "127.0.0.1", "port": 9555 }
```

### `webview_execute_js`

Execute JavaScript in the webview. Use IIFE syntax for return values.

```json
{ "script": "(() => { return document.title; })()" }
```

Supports `async`/`await`:

```json
{ "script": "(async () => { return await fetch('/api').then(r => r.json()); })()" }
```

### `webview_dom_snapshot`

Get a structured DOM snapshot. Two types available:

- `accessibility` -- roles, names, ARIA states (good for understanding UI semantics)
- `structure` -- tag names, IDs, CSS classes (good for CSS selectors)

```json
{ "type": "accessibility" }
{ "type": "structure", "selector": ".main-content" }
```

### `get_cached_dom`

Get the DOM snapshot that was auto-pushed from the frontend via Tauri IPC. Faster than `webview_dom_snapshot` since it reads cached data without JS execution.

Returns: `html`, `text_content`, `accessibility_tree`, `structure_tree`, `timestamp`.

```json
{ "windowId": "main" }
```

### `webview_find_element`

Find elements by CSS selector, XPath, or text content.

```json
{ "selector": ".ant-btn-primary", "strategy": "css" }
{ "selector": "//button[@type='submit']", "strategy": "xpath" }
{ "selector": "Save Changes", "strategy": "text" }
```

Returns element metadata: tag, id, className, text, bounding rect, visibility.

### `webview_get_styles`

Get computed CSS styles for an element.

```json
{ "selector": ".header", "properties": ["background-color", "font-size", "padding"] }
```

Omit `properties` to get all computed styles.

### `webview_interact`

Perform UI interactions.

```json
{ "action": "click", "selector": "#submit-btn" }
{ "action": "scroll", "selector": ".content", "direction": "down", "distance": 300 }
{ "action": "hover", "selector": ".menu-item" }
{ "action": "focus", "selector": "input[name='email']" }
{ "action": "click", "x": 100, "y": 200 }
```

Actions: `click`, `double-click`, `focus`, `scroll`, `hover`.

### `webview_keyboard`

Type text or press keys.

```json
{ "action": "type", "text": "hello@example.com" }
{ "action": "press", "key": "Enter" }
{ "action": "press", "key": "a", "modifiers": ["ctrl"] }
```

### `webview_wait_for`

Wait for an element or text to appear.

```json
{ "selector": ".loading-complete", "timeout": 10000 }
{ "text": "Successfully saved", "timeout": 5000 }
```

### `webview_get_pointed_element`

Get metadata for the element the user Alt+Shift+Clicked. The bridge JS registers a global listener that captures element info on Alt+Shift+Click.

### `manage_window`

List, inspect, or resize windows.

```json
{ "action": "list" }
{ "action": "info", "windowId": "main" }
{ "action": "resize", "windowId": "main", "width": 1280, "height": 720 }
```

### `ipc_get_backend_state`

Get app metadata: name, identifier, version, Tauri version, OS, architecture, window list.

### `ipc_execute_command`

Execute any Tauri IPC command (same as calling `invoke()` from the frontend).

```json
{ "command": "get_all_accounts", "args": {} }
```

### `ipc_monitor` / `ipc_get_captured`

Start monitoring IPC traffic, then retrieve captured events.

```json
{ "action": "start" }
{ "filter": "account", "limit": 50 }
```

### `ipc_emit_event`

Emit a custom Tauri event for testing.

```json
{ "eventName": "deep-link-received", "payload": "myapp://test" }
```

### `read_logs`

Read captured console logs (log, warn, error, info, debug).

```json
{ "lines": 20, "filter": "error" }
```

## Frontend Integration (Optional)

For enhanced DOM access, your frontend can proactively push DOM snapshots to the plugin:

```typescript
import { invoke } from '@tauri-apps/api/core';

// Push current DOM state for LLM consumption
async function pushDom() {
  await invoke('plugin:connector|push_dom', {
    payload: {
      windowId: 'main',
      html: document.body.innerHTML,
      textContent: document.body.innerText,
      accessibilityTree: '', // your custom tree builder
      structureTree: '',     // your custom tree builder
    }
  });
}

// Push on route changes
window.addEventListener('popstate', () => setTimeout(pushDom, 1000));
```

The bridge JS also auto-pushes DOM on page load and significant mutations when `window.__TAURI_INTERNALS__` is available.

## Migrating from tauri-plugin-mcp-bridge

1. Replace the Cargo dependency:

```diff
- tauri-plugin-mcp-bridge = "0.10"
+ tauri-plugin-connector = { git = "https://github.com/dickwu/tauri-connector" }
```

2. Update plugin registration:

```diff
- builder = builder.plugin(tauri_plugin_mcp_bridge::init());
+ builder = builder.plugin(tauri_plugin_connector::init());
```

3. Update capabilities:

```diff
- "mcp-bridge:default"
+ "connector:default"
```

4. Update your MCP server config to use `tauri-connector-mcp` instead of `mcp-server-tauri`, pointing to port `9555`.

## Project Structure

```
tauri-connector/
|-- plugin/                    # Rust Tauri v2 plugin
|   |-- Cargo.toml
|   |-- build.rs
|   |-- src/
|   |   |-- lib.rs             # Plugin entry + Tauri IPC commands
|   |   |-- bridge.rs          # Internal WebSocket bridge (the fix)
|   |   |-- server.rs          # External WebSocket server
|   |   |-- handlers.rs        # All command handlers
|   |   |-- protocol.rs        # Message types
|   |   '-- state.rs           # Shared state (DOM cache, logs, IPC)
|   |-- js/bridge.js           # Placeholder (real JS injected at runtime)
|   '-- permissions/
|       '-- default.toml
|-- server/                    # TypeScript MCP server
|   |-- package.json
|   |-- tsconfig.json
|   '-- src/
|       |-- index.ts           # MCP tool definitions
|       '-- client.ts          # WebSocket client
'-- README.md
```

## How It Works

### Internal WebSocket Bridge

1. On plugin setup, an internal WebSocket server starts on `127.0.0.1:9300-9400`
2. A JavaScript bridge client is injected into the webview via `WebviewWindow`
3. The bridge connects to the internal WebSocket — this runs in the page's main JS context, not an isolated content world
4. When the MCP server requests JS execution, the plugin sends the script through the internal WebSocket
5. The bridge evaluates the script and sends results back through the same WebSocket
6. No dependency on `window.__TAURI__` for result delivery

### Console Log Capture

The bridge intercepts `console.log/warn/error/info/debug` calls, storing them in a ring buffer. Logs are accessible via the `read_logs` tool or pushed to Rust via `invoke()`.

### Element Picker

Alt+Shift+Click on any element captures its metadata (tag, id, classes, attributes, bounding rect). Retrieved via `webview_get_pointed_element`.

## Requirements

- Tauri v2.x
- Rust 2024 edition
- Node.js 18+ (for MCP server)

## License

MIT
