# Tauri Connector Setup

Step-by-step guide to add tauri-connector to a Tauri v2 project. Detect the project by looking for `src-tauri/` directory and `tauri.conf.json`.

## Step 0: Install the CLI (macOS/Linux)

```bash
# Homebrew (recommended)
brew install dickwu/tap/tauri-connector

# Or self-update if already installed
tauri-connector update

# Or build from source
cargo build -p connector-cli --release
```

This installs both `tauri-connector` (CLI) and `tauri-connector-mcp` (standalone MCP server).

## Step 1: Add Cargo dependency

Check `src-tauri/Cargo.toml`. If `tauri-plugin-connector` is not present, add it:

```toml
[dependencies]
tauri-plugin-connector = "0.10"
```

## Step 2: Register the plugin

Check `src-tauri/src/lib.rs` or `src-tauri/src/main.rs` for the `tauri::Builder` chain. Add the plugin registration wrapped in `#[cfg(debug_assertions)]` so it only runs in dev builds:

```rust
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

Place this BEFORE the `.invoke_handler()` call and AFTER the initial builder creation.

## Step 3: Add permissions

Check `src-tauri/capabilities/default.json` (or the main capabilities file). Add `"connector:default"` to the `permissions` array:

```json
{
  "permissions": [
    "connector:default"
  ]
}
```

## Step 4: Verify `withGlobalTauri` (REQUIRED)

Check `src-tauri/tauri.conf.json` for `"withGlobalTauri": true` under the `app` section. This is **required** for the eval+event fallback JS execution path and auto-push DOM feature. If missing, add it:

```json
{
  "app": {
    "withGlobalTauri": true
  }
}
```

## Step 5: Install snapdom (screenshot fallback)

The screenshot tool uses `xcap` for native window capture (cross-platform). When `xcap` is unavailable (e.g. Wayland without permissions, or CI environments), it falls back to `snapdom` — a fast DOM-to-image library that captures exactly what the web engine renders.

Install in your frontend project:

```bash
# npm
npm install @zumer/snapdom

# bun
bun add @zumer/snapdom

# pnpm
pnpm add @zumer/snapdom
```

**Option A (recommended):** If your project uses a bundler (Vite, webpack, etc.), no extra setup needed — the plugin uses dynamic `import('@zumer/snapdom')` automatically.

**Option B (global):** If dynamic import doesn't work in your setup, expose snapdom on `window` in your app's entry point:

```typescript
import { snapdom } from '@zumer/snapdom';
window.snapdom = snapdom;
```

## Step 6: Configure Claude Code

Add to `.mcp.json` in the project root:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/mcp"
    }
  }
}
```

The MCP server is embedded in the plugin -- no separate command or install needed.

## Step 7: Verify

Run the app with `bun run tauri dev` (or `cargo tauri dev`). Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector][mcp] MCP ready for 'App Name' -- url: http://127.0.0.1:9556/mcp (/sse legacy)
[connector] Plugin ready for 'App Name' (com.app.id) -- WS on 127.0.0.1:9555
[connector] PID file: /path/to/src-tauri/target/.connector.json
```

The PID file enables the Rust CLI, standalone MCP server, and bun scripts to auto-discover ports without configuration. Run `tauri-connector status --json` to inspect the selected endpoint and stale candidates.

For a one-shot health check of the entire setup, run `tauri-connector doctor` in the project root. It walks every file above, probes the running WS/MCP ports, and prints a Fix line for anything missing. Use `--json` for CI, `--no-runtime` to skip the live probes.

## Step 8: Auto-detect hook (optional)

Install a Claude Code hook that automatically detects when your Tauri app is running and signals available connector tools on every prompt:

```bash
tauri-connector hook install
```

This writes a lightweight `UserPromptSubmit` hook to `.claude/settings.local.json`. It checks for the `.connector.json` PID file and outputs available tools — zero noise when the app isn't running.

To remove:

```bash
tauri-connector hook remove
```

## Custom Configuration

For remote access, custom ports, or disabling the embedded MCP:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(debug_assertions)]
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("0.0.0.0")     // remote debug opt-in; default is 127.0.0.1
            .port_range(9600, 9700)      // WS port range (default: 9555-9655)
            .mcp_port_range(9700, 9800)  // MCP port range (default: 9556-9656)
            .build()
    );
}
```
