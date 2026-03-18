# Tauri Connector Setup

Step-by-step guide to add tauri-connector to a Tauri v2 project. Detect the project by looking for `src-tauri/` directory and `tauri.conf.json`.

## Step 1: Add Cargo dependency

Check `src-tauri/Cargo.toml`. If `tauri-plugin-connector` is not present, add it:

```toml
[dependencies]
tauri-plugin-connector = "0.2"
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

## Step 5: Configure Claude Code

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

## Step 6: Verify

Run the app with `bun run tauri dev` (or `cargo tauri dev`). Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector][mcp] MCP ready for 'App Name' -- url: http://0.0.0.0:9556/sse
[connector] Plugin ready for 'App Name' (com.app.id) -- WS on 0.0.0.0:9555
[connector] PID file: /path/to/src-tauri/target/debug/.connector.json
```

The PID file enables bun scripts to auto-discover ports without configuration.

## Custom Configuration

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
