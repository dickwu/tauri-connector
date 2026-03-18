---
name: tauri-connector
description: "Interact with Tauri v2 desktop apps via tauri-connector. Use this skill when the user wants to: test Tauri UI, automate webview interactions, take DOM snapshots, click/hover/fill elements, inspect app state, read console logs, execute JS in webviews, debug Tauri desktop apps, or SET UP tauri-connector in a project. Also use when the user mentions admin/, front/, or tool/ desktop apps, or asks about DOM inspection, element interaction, or app testing. Provides automated setup, embedded MCP server, Rust CLI with ref-based addressing, and bun scripts as fallback."
---

# Tauri Connector

Deep inspection and interaction with Tauri v2 desktop apps. The **MCP server runs inside the plugin** -- starts automatically when the Tauri app runs.

## Setup

For first-time setup in a Tauri project, read `skill/SETUP.md` in the tauri-connector repo. Key steps: add `tauri-plugin-connector = "0.4"` to Cargo.toml, register plugin, add permissions, set `withGlobalTauri: true`, install `@zumer/snapdom` in frontend, add MCP URL to `.mcp.json`.

## Checking if the App is Running

The plugin writes a PID file to `target/.connector.json` when it starts. To check:

```bash
cat src-tauri/target/debug/.connector.json 2>/dev/null || cat target/debug/.connector.json 2>/dev/null
lsof -i :9555 -P -n 2>/dev/null | grep LISTEN
```

## CLI (Primary)

The `tauri-connector` CLI is the recommended way to interact with Tauri apps from the terminal.

```bash
# Homebrew (macOS/Linux)
brew install dickwu/tap/tauri-connector

# Or self-update if already installed
tauri-connector update

# Or build from source
cargo build -p connector-cli --release
```

### Quick Reference

```bash
# DOM & Inspection
tauri-connector snapshot -i                    # DOM snapshot with ref IDs (interactive only)
tauri-connector snapshot -i -c                 # Compact snapshot
tauri-connector dom                            # Cached DOM (pushed from frontend)
tauri-connector find "button"                  # Find elements by CSS
tauri-connector find "Submit" -s text          # Find by text content
tauri-connector get text @e7                   # Get text content by ref
tauri-connector get title                      # Page title
tauri-connector get url                        # Current URL
tauri-connector pointed                        # Alt+Shift+Click element info

# Interaction (use @eN refs from snapshot)
tauri-connector click @e5                      # Click by ref
tauri-connector click "button.submit"          # Click by CSS selector
tauri-connector dblclick @e5                   # Double-click
tauri-connector hover @e8                      # Hover (trigger dropdown/tooltip)
tauri-connector focus @e3                      # Focus element
tauri-connector fill @e3 "user@example.com"    # Clear and fill input
tauri-connector type @e3 "hello"               # Type character by character
tauri-connector check @e10                     # Check checkbox
tauri-connector uncheck @e10                   # Uncheck checkbox
tauri-connector select @e6 "option1" "option2" # Select options
tauri-connector scroll down 300                # Scroll page
tauri-connector scrollintoview @e20            # Scroll element into view

# Keyboard
tauri-connector press Enter                    # Press key
tauri-connector press Tab                      # Press Tab

# Screenshot
tauri-connector screenshot /tmp/shot.png -m 1280   # PNG, max 1280px wide
tauri-connector screenshot /tmp/s.webp -f webp     # WebP format

# IPC & Events
tauri-connector state                          # App backend state
tauri-connector ipc exec greet -a '{"name":"world"}'  # Execute IPC command
tauri-connector ipc monitor                    # Start IPC monitoring
tauri-connector ipc unmonitor                  # Stop IPC monitoring
tauri-connector ipc captured -f greet          # Get captured IPC traffic
tauri-connector emit my-event -p '{"foo":42}'  # Emit custom event

# Logs & Windows
tauri-connector logs -n 50                     # Last 50 console logs
tauri-connector logs -n 20 -f error            # Filtered logs
tauri-connector windows                        # List windows
tauri-connector resize 1024 768                # Resize window

# Other
tauri-connector eval "document.title"          # Execute JavaScript
tauri-connector wait ".loaded"                 # Wait for element
tauri-connector wait --text "Success"          # Wait for text
tauri-connector --version                      # Show version
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WS port |

## Bun Scripts (Fallback)

If the CLI binary is not available, use the bun scripts at `~/opensource/tauri-connector/skill/scripts/`. Bun runs TypeScript natively with built-in WebSocket. Scripts auto-discover ports from the PID file.

```bash
SCRIPTS=~/opensource/tauri-connector/skill/scripts
```

```bash
bun run $SCRIPTS/state.ts                              # App metadata
bun run $SCRIPTS/eval.ts "document.title"              # Execute JS
bun run $SCRIPTS/screenshot.ts /tmp/shot.png 1280      # Screenshot
bun run $SCRIPTS/snapshot.ts                           # DOM accessibility tree
bun run $SCRIPTS/find.ts "button"                      # Find elements by CSS
bun run $SCRIPTS/click.ts "button.submit"              # Click element
bun run $SCRIPTS/hover.ts ".menu-trigger"              # Hover
bun run $SCRIPTS/hover.ts ".menu-trigger" --off        # Hover-off (dismiss)
bun run $SCRIPTS/fill.ts "input.search" "query"        # Fill input
bun run $SCRIPTS/logs.ts 50                            # Console logs
bun run $SCRIPTS/windows.ts                            # List windows
bun run $SCRIPTS/wait.ts ".loaded"                     # Wait for element
```

Bun scripts also support `TAURI_CONNECTOR_HOST`, `TAURI_CONNECTOR_PORT`, and `TAURI_CONNECTOR_TIMEOUT` env vars, plus auto-discovery via PID file.

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

20 MCP tools with full CLI parity: `webview_execute_js`, `webview_screenshot`, `webview_dom_snapshot`, `get_cached_dom`, `webview_find_element`, `webview_get_styles`, `webview_get_pointed_element`, `webview_select_element`, `webview_interact`, `webview_keyboard`, `webview_wait_for`, `manage_window`, `ipc_get_backend_state`, `ipc_execute_command`, `ipc_monitor`, `ipc_get_captured`, `ipc_emit_event`, `read_logs`, `get_setup_instructions`, `list_devices`.

## Common Workflows

### Understand Current Page

```bash
tauri-connector state
tauri-connector eval "(() => ({ title: document.title, url: location.href }))()"
tauri-connector snapshot -i
tauri-connector screenshot /tmp/page.png -m 1280
```

### Fill a Form

```bash
tauri-connector snapshot -i -s ".form"
tauri-connector fill @e3 "user@example.com"
tauri-connector click @e5
tauri-connector wait --text "Success"
```

### Hover to Reveal, Then Click

```bash
tauri-connector hover @e8                    # Trigger dropdown/tooltip
tauri-connector wait ".dropdown-menu"        # Wait for it to appear
tauri-connector click ".dropdown-item"       # Click revealed item
tauri-connector hover @e8                    # (Optional) hover-off to dismiss
```

### Debug

```bash
tauri-connector logs -n 50 -f error
tauri-connector state
tauri-connector eval "document.querySelector('.error')?.textContent"
```

### IPC Debugging

```bash
tauri-connector ipc monitor                  # Start capturing
# ... interact with the app ...
tauri-connector ipc captured                 # See all captured IPC calls
tauri-connector ipc captured -f greet        # Filter by command name
tauri-connector ipc unmonitor                # Stop capturing
```

## Screenshot

The `webview_screenshot` tool / `screenshot` CLI command uses a two-tier approach:

1. **xcap** (primary): Native cross-platform window capture via the `xcap` crate.
2. **snapdom** (fallback): Falls back to `@zumer/snapdom` in the frontend.

Supported formats: `png` (default), `jpeg`, `webp`. Use `-m` / `maxWidth` to resize.

## Troubleshooting

### Connection Refused
App isn't running or plugin isn't loaded. Check: `lsof -i :9555 | grep LISTEN`. Start with `bun run tauri dev`.

### Stale PID File
If the app crashed, the PID file may be stale. Delete manually: `rm target/debug/.connector.json`.

### Port Conflict
Use `ConnectorBuilder::new().port_range(9600, 9700)` or set `TAURI_CONNECTOR_PORT=9600`.
