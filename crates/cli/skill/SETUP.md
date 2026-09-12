# Tauri Connector Setup

For an existing app upgrade and live validation, use [references/upgrade-and-smoke.md](references/upgrade-and-smoke.md); preserve the app's existing feature names and account for shared build directories.

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
tauri-plugin-connector = { version = "0.16", optional = true }

[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]
```

> Mobile gotcha: if you also build for Android/iOS, scope the dep to desktop only:
> ```toml
> [target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]
> tauri-plugin-connector = { version = "0.16", optional = true }
> ```

### Alternative (legacy)

If `tauri-plugin-connector` is already a plain dependency, leave it; the legacy pattern uses the regular form:

```toml
[dependencies]
tauri-plugin-connector = "0.16"
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

Check `src-tauri/tauri.conf.json` for `"withGlobalTauri": true` under the `app` section. This is **required** for the eval+event fallback JS execution path and the auto-push DOM feature. (Since 0.15 the fallback is only used before a command is dispatched; a slow, failed, or lost response is never replayed through it.) If missing, add it:

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

If a client cannot reach the embedded HTTP server, register the standalone binary instead: `"command": "tauri-connector-mcp"` with an `env` block carrying `TAURI_CONNECTOR_HOST`, `TAURI_CONNECTOR_PORT` and, for workflows, `TAURI_CONNECTOR_WORKFLOW_TOKEN`. It forwards every call to the running app and never executes workflow steps itself.

## Step 6b: Enable application-owned workflows (optional, plugin >= 0.15)

The `workflow_*` MCP tools and `tauri-connector workflow` run known multi-step sequences inside the app. They stay locked until the host configures a workflow token of **at least 32 bytes** -- a shorter value is ignored as if no token were set. `workflow_capabilities` answers without a token and reports `authentication.configured`; `run`, `get`, `cancel` and `resume` return `unauthorized` until the token is configured on the host and supplied by the client.

Generate a token for the development session without printing it, then start the app and every client from the same environment:

```bash
export TAURI_CONNECTOR_WORKFLOW_TOKEN="$(openssl rand -hex 32)"
bun run tauri:dev                        # init() / ConnectorBuilder::new() read the variable
tauri-connector workflow capabilities    # expect "authentication": { "configured": true }
```

Alternatively pass a secret from the host's own configuration in code (never a literal in source), inside the same cfg gate as the plugin registration:

```rust
builder = builder.plugin(
    ConnectorBuilder::new()
        .workflow_token(token)   // >= 32 bytes
        .build()
);
```

Client side: the CLI and the standalone `tauri-connector-mcp` read `TAURI_CONNECTOR_WORKFLOW_TOKEN` from their environment (for the standalone server, add it to the `env` block of its `.mcp.json` entry); embedded MCP callers pass `authToken` as a tool argument next to `spec`. Keep the token out of the workflow `spec`, `inputs`, checked-in config, logs and prompts.

Run history is journaled to private files under the app's data directory. Windows currently fails closed with `persistence_unavailable` because private-file ACLs are not implemented; native macOS is validated, native Windows/Linux WebView validation is still outstanding.

## Step 7: Verify

Run the app:

- Feature-gated: `bun run tauri:dev` (which expands to `tauri dev --features dev-connector`).
- Legacy: `bun run tauri dev` (or `cargo tauri dev`).

Look for these log lines:

```
[connector][bridge] Internal bridge on port 9300
[connector][mcp] MCP ready for 'App Name' -- url: http://127.0.0.1:9556/mcp (/sse legacy)
[connector] Plugin ready for 'App Name' (com.app.id) -- WS on 127.0.0.1:9555
[connector] PID file: /path/to/src-tauri/target/.connector.json
```

The PID file enables the Rust CLI, standalone MCP server, and bun scripts to auto-discover ports without configuration. Run `tauri-connector status --json` to inspect the selected endpoint and stale candidates.

For a one-shot health check of the entire setup, run `tauri-connector doctor` in the project root. It auto-detects the active pattern and emits pattern-specific rows. Under the feature-gated pattern, expect:

```
✓ Cargo dependency: tauri-plugin-connector = "0.16" (optional, feature-gated)
✓ Plugin registered in src-tauri/src/lib.rs (cfg(feature = "dev-connector"))
✓ Permission "connector:default" in src-tauri/capabilities-dev/dev-connector.json
✓ [features] dev-connector = ["dep:tauri-plugin-connector"]
✓ Capability loaded at runtime via app.add_capability(include_str!("../capabilities-dev/..."))
```

The JSON form (`tauri-connector doctor --json`) exposes a top-level `setup_pattern` field with one of `"feature-gated" | "legacy" | "mixed" | "none"` so CI can branch on the active pattern. Use `--json` for CI, `--no-runtime` to skip the live probes.

Doctor also warns if an installed local `tauri-connector` skill doc differs from the CLI-bundled copy. Refresh docs with `tauri-connector skills get tauri-connector > ~/.codex/skills/tauri-connector/SKILL.md` or inspect embedded references with `tauri-connector skills list|get|path`.

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

For a different bind address, custom ports, disabling the embedded MCP, or a code-supplied workflow token, substitute the cfg gate matching your active pattern:

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(feature = "dev-connector")]   // or `cfg(debug_assertions)` for legacy
{
    builder = builder.plugin(
        ConnectorBuilder::new()
            .bind_address("127.0.0.1")   // default (localhost only); "0.0.0.0" exposes the connector to the network
            .port_range(9600, 9700)      // WS port range (default: 9555-9655)
            .mcp_port_range(9700, 9800)  // MCP port range (default: 9556-9656)
            .disable_mcp()               // skip the embedded MCP HTTP server (default: enabled)
            .workflow_token(token)       // enable workflow_* tools; >= 32 bytes, see Step 6b
            .build()
    );
}
```
