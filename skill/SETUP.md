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

## Step 0b: Choose the registration pattern

There are two supported patterns. **Default to the feature-gated pattern unless the user explicitly asks for the legacy form.**

| Criterion | Feature-gated (recommended) | Legacy (`cfg(debug_assertions)`) |
|---|---|---|
| Plugin compiled in release `tauri build`? | **No** — dep is `optional` and the cargo feature is off | Yes (stripped at link time, but still pulled into the dep graph) |
| Heavy transitive deps (xcap, libspa/pipewire, aws-sdk-s3) in release? | **No** | Yes — compiled, then mostly DCE'd |
| Capability JSON loaded by `tauri build`? | **No** — lives outside the `capabilities/` glob | Risk of leaking unless you delete the file before shipping |
| Needs a separate dev script? | Yes — `bun run tauri:dev` runs `tauri dev --features dev-connector` | No — plain `tauri dev` works |
| Suggested when… | Project ships releases or cares about supply-chain hygiene | Hobby project / always-on dev builds / quickest setup |

`tauri-connector doctor` recognizes both patterns. On a legacy setup it emits a non-blocking warn nudging migration. The steps below cover both — feature-gated first, legacy as **Alternative** sub-blocks.

## Step 1: Add the Cargo dependency

### Feature-gated (recommended)

In `src-tauri/Cargo.toml`, add the dep as `optional` and declare a `dev-connector` cargo feature:

```toml
[dependencies]
# ...
tauri-plugin-connector = { version = "0.10", optional = true }

[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]
```

> Mobile gotcha: if you also build for Android/iOS, scope the dep to desktop only:
> ```toml
> [target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]
> tauri-plugin-connector = { version = "0.10", optional = true }
> ```

### Alternative (legacy)

If `tauri-plugin-connector` is already a plain dependency, leave it; the legacy pattern uses the regular form:

```toml
[dependencies]
tauri-plugin-connector = "0.10"
```

## Step 2: Register the plugin

### Feature-gated (recommended)

Wrap the plugin registration in `cfg(feature = "dev-connector")`. Place it BEFORE `.invoke_handler(...)` and AFTER the initial builder creation. Define a module-level constant for the dev capability so it can be used by the runtime registration in Step 3:

```rust
// src-tauri/src/lib.rs
#[cfg(feature = "dev-connector")]
const DEV_CONNECTOR_CAPABILITY: &str =
    include_str!("../capabilities-dev/dev-connector.json");

pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(feature = "dev-connector")]
    {
        builder = builder.plugin(tauri_plugin_connector::init());
    }

    builder
        .setup(|app| {
            #[cfg(feature = "dev-connector")]
            app.add_capability(DEV_CONNECTOR_CAPABILITY)
                .map_err(|e| format!("dev-connector capability: {e}"))?;
            Ok(())
        })
        .invoke_handler(/* ... */)
        .run(/* ... */);
}
```

### Alternative (legacy)

```rust
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

## Step 3: Add the connector permission

### Feature-gated (recommended)

Create `src-tauri/capabilities-dev/dev-connector.json` (a NEW directory, **outside** `capabilities/`):

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "dev-connector",
  "description": "Permissions for tauri-plugin-connector dev tooling. Lives outside capabilities/ so tauri-build's default ./capabilities/**/* glob does NOT auto-load it. Registered at runtime via app.add_capability(include_str!(...)) gated on cfg(feature = \"dev-connector\").",
  "windows": ["main"],
  "permissions": ["connector:default"]
}
```

The runtime registration (`app.add_capability(...)`) added in Step 2 is what loads this file when `--features dev-connector` is on. Plain `tauri build` skips the feature, never sees the capability — release builds no longer have to delete the file.

### Alternative (legacy)

Edit `src-tauri/capabilities/default.json` (or any existing capability JSON) and add `"connector:default"` to the `permissions` array:

```json
{
  "permissions": [
    "connector:default"
  ]
}
```

## Step 3b: Add the dev script (feature-gated only)

In `package.json`, add a script that flips the cargo feature on for `tauri dev`:

```json
{
  "scripts": {
    "tauri:dev": "tauri dev --features dev-connector"
  }
}
```

From now on, `bun run tauri:dev` (or `cargo tauri dev --features dev-connector`) compiles the plugin in. Plain `bun run tauri dev` / `tauri build` skip the feature entirely.

## Step 4: Verify `withGlobalTauri` (REQUIRED, both patterns)

Check `src-tauri/tauri.conf.json` for `"withGlobalTauri": true` under the `app` section. This is **required** for the eval+event fallback JS execution path and the auto-push DOM feature. If missing, add it:

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
      "url": "http://127.0.0.1:9556/sse"
    }
  }
}
```

The MCP server is embedded in the plugin -- no separate command or install needed.

## Step 7: Verify

Run the app:

- Feature-gated: `bun run tauri:dev` (which expands to `tauri dev --features dev-connector`).
- Legacy: `bun run tauri dev` (or `cargo tauri dev`).

Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector][mcp] MCP ready for 'App Name' -- url: http://0.0.0.0:9556/sse
[connector] Plugin ready for 'App Name' (com.app.id) -- WS on 0.0.0.0:9555
[connector] PID file: /path/to/src-tauri/target/debug/.connector.json
```

For a one-shot health check of the entire setup, run `tauri-connector doctor` in the project root. It auto-detects the active pattern and emits pattern-specific rows. Under the feature-gated pattern, expect:

```
✓ Cargo dependency: tauri-plugin-connector = "0.10" (optional, feature-gated)
✓ Plugin registered in src-tauri/src/lib.rs (cfg(feature = "dev-connector"))
✓ Permission "connector:default" in src-tauri/capabilities-dev/dev-connector.json
✓ [features] dev-connector = ["dep:tauri-plugin-connector"]
✓ Capability loaded at runtime via app.add_capability(include_str!("../capabilities-dev/..."))
```

The JSON form (`tauri-connector doctor --json`) exposes a top-level `setup_pattern` field with one of `"feature-gated" | "legacy" | "mixed" | "none"` so CI can branch on the active pattern. Use `--json` for CI, `--no-runtime` to skip the live probes.

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

For localhost-only access, custom ports, or disabling the embedded MCP — substitute the cfg gate matching your active pattern:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(feature = "dev-connector")]   // or `cfg(debug_assertions)` for legacy
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
