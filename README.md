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

Claude Code -------- HTTP http://host:9556/mcp ----> Embedded MCP server
                                                      |-- handlers (direct, in-process)
                                                      |-- bridge.execute_js() -> JS result
                                                      '-- state.get_dom() -> cached DOM

CLI (Rust) -------- WebSocket ws://host:9555 -----> Plugin WS server
```

## Claude Code Skill (Recommended)

Install the skill to give Claude Code (and 30+ other AI agents) full debug and code review capabilities for Tauri apps.

### Install via skills.sh (easiest)

```bash
npx skills add dickwu/tauri-connector
```

This installs from [skills.sh](https://skills.sh) -- the agent skills directory. Works with Claude Code, Cursor, Windsurf, Codex, Gemini CLI, and more.

### Install manually

```bash
mkdir -p ~/.claude/skills/tauri-connector
cp -r skill/SKILL.md skill/SETUP.md skill/scripts skill/references \
  ~/.claude/skills/tauri-connector/
```

### What's Included

The skill provides a **debug & code review suite** with progressive disclosure:

| File | Purpose |
|---|---|
| `SKILL.md` | Main skill -- core workflow, debugging, code review, interaction reference |
| `SETUP.md` | Step-by-step setup guide for new Tauri projects |
| `scripts/` | 14 Bun TypeScript scripts for fallback WebSocket automation |
| `references/mcp-tools.md` | All 25 MCP tool parameter tables |
| `references/cli-commands.md` | Every CLI subcommand with flags and examples |
| `references/debug-playbook.md` | 10 step-by-step debug recipes (blank screen, silent clicks, form failures, slow IPC, drag issues, memory leaks, multi-window) |
| `references/code-review-playbook.md` | 9 code review workflows (visual regression, accessibility audit, component tree, IPC contract validation, DOM structure, event flow) |

### What It Enables

Once installed, Claude will automatically:

- **Debug Tauri apps** -- console errors, IPC monitoring, event capture, runtime state inspection
- **Review code changes** -- visual regression, accessibility audit, component tree review, IPC contract validation
- **Interact with the UI** -- click, fill, drag, type, scroll using ref-based addressing
- **Set up the plugin** in any Tauri v2 project when asked
- **Automate testing** with snapshot -> act -> verify workflows

> **For contributors:** The release workflow skill is at `.claude/skills/tauri-connector-release/SKILL.md` — it triggers automatically when you say "release" or "bump version" inside this repo.

## Components

| Component | Description |
|---|---|
| `plugin/` | Rust Tauri v2 plugin with **embedded MCP server** (`tauri-plugin-connector` on crates.io) |
| `crates/cli/` | Rust CLI binary with ref-based element addressing |
| `crates/mcp-server/` | Standalone Rust MCP server (alternative to embedded, connects via WebSocket) |
| `crates/client/` | Shared Rust WebSocket client library |

## Features

### 25 Tools (MCP + CLI) with Drag and Drop

Every tool is available via both the embedded MCP server (for Claude Code) and the Rust CLI (for terminal use). The CLI uses ref-based element addressing inspired by [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser).

| Category | MCP Tool | CLI Command |
|---|---|---|
| JavaScript | `webview_execute_js` | `eval <script>` |
| DOM | `webview_dom_snapshot` | `snapshot [-i] [-c] [--mode ai\|accessibility\|structure]` |
| DOM (cached) | `get_cached_dom` | `dom` |
| Elements | `webview_find_element` | `find <selector> [-s css\|xpath\|text]` |
| Styles | `webview_get_styles` | `get styles <@ref\|selector>` |
| Picker | `webview_get_pointed_element` | `pointed` |
| Select | `webview_select_element` | *(visual picker, not yet implemented)* |
| Interact | `webview_interact` | `click`, `dblclick`, `hover`, `drag`, `focus`, `fill`, `type`, `check`, `uncheck`, `select`, `scroll`, `scrollintoview` |
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
| Logs | `clear_logs` | `clear logs\|ipc\|events\|all` |
| Logs | `read_log_file` | *(via MCP only)* |
| Events | `ipc_listen` | `events listen\|captured\|stop` |
| Events | `event_get_captured` | `events captured [-p regex]` |
| DOM | `webview_search_snapshot` | *(via MCP only)* |
| Setup | `get_setup_instructions` | `examples` |
| Diagnostics | *(CLI only)* | `doctor [--json] [--no-runtime]` |
| Devices | `list_devices` | *(info only)* |

### CLI Ref-Based Addressing

Take a DOM snapshot with stable ref IDs, then interact with elements using those refs:

```bash
# Take snapshot -- assigns ref IDs, enriches with React component names, stitches portals
$ tauri-connector snapshot -i
- complementary [component=MainMenu]
  - menu [ref=e6, component=InheritableContextProvider]
    - menuitem [ref=e7, component=LegacyMenuItem]
      - img "calendar" [ref=e21]
  - main [component=MainMenu]
    - heading "Task Centre" [level=1, ref=e103]
    - textbox "Search..." [ref=e16]
    - combobox "Status" [ref=e50, component=InternalSelect, expanded=true]:
      - listbox "Status options" [portal]:        # stitched from document.body
        - option "Active" [selected, ref=e51]
        - option "Inactive" [ref=e52]

# Interact using refs (persist across CLI invocations)
$ tauri-connector click @e51          # Click "Active" option in portal
$ tauri-connector fill @e16 "aspirin" # Fill search box
$ tauri-connector hover @e7           # Hover menu item
$ tauri-connector drag @e51 @e52      # Drag element to target
$ tauri-connector get text @e103      # Get "Task Centre"
$ tauri-connector press Enter         # Press key
$ tauri-connector logs -n 5           # Last 5 console logs
```

### Unified Snapshot Engine (v0.5)

The DOM snapshot engine uses a single `window.__CONNECTOR_SNAPSHOT__()` function with three modes:

| Mode | Output | Use case |
|---|---|---|
| `ai` (default) | Role, name, ARIA states, `ref=eN` IDs, `component=Name`, portal stitching | Claude interaction |
| `accessibility` | Role, accessible name, ARIA states | Semantic understanding |
| `structure` | Tag, id, classes, data-testid | Layout debugging |

Key capabilities:
- **TreeWalker API** for ~78x faster traversal (no stack overflow on deep React trees)
- **Portal stitching** -- Ant Design modals/drawers/dropdowns are logically re-parented under their trigger via `aria-controls`/`aria-owns`
- **React fiber enrichment** -- Component names (`component=AppointmentModal`) via `__reactFiber$` on DOM nodes
- **Visibility pruning** -- `aria-hidden`, `display:none`, `visibility:hidden`, `role=presentation/none` automatically excluded
- **Virtual scroll detection** -- `rc-virtual-list-holder` containers annotated with visible item count
- **Token budgeting** -- `maxDepth`, `maxElements`, and token-aware splitting for graceful truncation on large DOMs

The plugin also auto-pushes DOM snapshots via Tauri IPC. The `get_cached_dom` tool returns this pre-cached snapshot instantly.

### Snapshot Budget Engine (v0.8+)

Large DOMs routinely blow past LLM context windows. The snapshot engine now budgets output by estimated tokens and spills overflow to on-disk **subtree files** so the inline response stays compact while the full tree remains reachable on demand.

- **Token estimation + section-atomic rendering** -- the walker estimates tokens per section and stops inlining once the budget is hit, emitting `{overflow: N subtrees, file=subtree-K.txt}` markers in place of the omitted content.
- **Repeating sibling collapse** -- runs of 5+ structurally identical siblings (same tag + role + ARIA state hash) are collapsed to 2 examples + a marker; the collapsed rows are written to a subtree file.
- **Subtree files** -- overflow content is written atomically to a PID-scoped session dir under the OS temp directory (`<tmp>/tauri-connector/<pid>/snapshots/<uuid>/subtree-N.txt`). Directories use `0700` permissions; writes are counter-based with `.tmp` + rename.
- **Auto-prune** -- old sessions are pruned by mtime with a per-window `Mutex` to avoid concurrent cleanup races; prune failures fall back to a bounded policy.
- **Search keeps full fidelity** -- `search_snapshot` / `webview_search_snapshot` match against the merged full-text (inline output plus all subtree contents), so filtered output never hides matches.
- **Defaults** -- MCP callers default to `max_tokens: 4000`; WebSocket / internal callers default to `0` (unlimited) for backward compatibility. Set `no_split: true` (or `--no-split` on the CLI) to disable file splitting entirely.

## Quick Start

> **Using Claude Code?** Install the skill for automated setup -- see [Claude Code Skill](#claude-code-skill-recommended) above.

### 1. Add the plugin (feature-gated, recommended)

The recommended pattern keeps `tauri-plugin-connector` and its transitive deps (xcap → libspa/pipewire on Linux, aws-sdk-s3, etc.) **out of release builds** entirely — they're never even compiled when the feature is off.

```toml
# src-tauri/Cargo.toml
[dependencies]
# ...

# Optional dep — only pulled when --features dev-connector is set.
tauri-plugin-connector = { version = "0.10", optional = true }

[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]
```

> Mobile gotcha: if you also build for Android/iOS, scope the dep to desktop:
> `[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]`

### 2. Register it behind the cargo feature

```rust
// src-tauri/src/lib.rs -- place BEFORE .invoke_handler()
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
            // Register the dev capability at runtime so plain `tauri build`
            // (without --features dev-connector) does not need any dev-only
            // capability JSON in `capabilities/`.
            #[cfg(feature = "dev-connector")]
            app.add_capability(DEV_CONNECTOR_CAPABILITY)
                .map_err(|e| format!("dev-connector capability: {e}"))?;
            Ok(())
        })
        .invoke_handler(/* ... */)
        .run(/* ... */);
}
```

### 3. Drop the dev capability JSON outside `capabilities/`

`tauri-build`'s default glob is `./capabilities/**/*`, so anything under that directory is auto-loaded. Keeping the dev capability one level over (`capabilities-dev/`) means a release `tauri build` will not see it — no need to delete the file before shipping.

```json
// src-tauri/capabilities-dev/dev-connector.json (NEW directory)
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "dev-connector",
  "description": "Permissions for tauri-plugin-connector dev tooling. Registered at runtime via app.add_capability(...) gated on cfg(feature = \"dev-connector\").",
  "windows": ["main"],
  "permissions": ["connector:default"]
}
```

### 4. Wire up the dev script + `withGlobalTauri`

```json
// package.json — flip the feature on for `tauri:dev` only
{
  "scripts": {
    "tauri:dev": "tauri dev --features dev-connector"
  }
}
```

```json
// src-tauri/tauri.conf.json — required for the eval+event JS fallback path
{ "app": { "withGlobalTauri": true } }
```

`bun run tauri:dev` (or `cargo tauri dev --features dev-connector`) compiles the plugin in. Plain `tauri build` skips the feature, leaves the dep uncompiled, and never sees the dev capability.

#### Alternative: legacy `cfg(debug_assertions)` pattern

If you don't want a separate dev script and don't mind the plugin (and its transitive crates) being **compiled** for release — they're stripped at link time by dead-code elimination, but still pulled into the dependency graph — the original `cfg(debug_assertions)` form still works:

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-connector = "0.10"
```

```rust
// src-tauri/src/lib.rs
#[cfg(debug_assertions)]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
```

```json
// src-tauri/capabilities/default.json — add to permissions array
"connector:default"
```

`tauri-connector doctor` accepts both patterns; on the legacy form it emits a non-blocking warn nudging you toward the feature-gated layout.

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
      "url": "http://127.0.0.1:9556/mcp"
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
[connector][mcp] MCP ready for 'MyApp' -- url: http://127.0.0.1:9556/mcp (/sse legacy)
[connector] Plugin ready for 'MyApp' (com.example.app) -- WS on 0.0.0.0:9555
```

The MCP server is now live. Claude Code connects automatically via the URL in `.mcp.json`.

### 8. Verify with `doctor` (v0.9+)

`tauri-connector doctor` auto-detects whether the project is using the **feature-gated** or **legacy** registration pattern, then walks the current project and confirms every setup step above. It inspects `src-tauri/Cargo.toml` (including the `[features]` block), the plugin registration in `lib.rs`/`main.rs` (accepts either `cfg(feature = "dev-connector")` or `cfg(debug_assertions)`), both `src-tauri/capabilities/*.json` and `src-tauri/capabilities-dev/*.json`, `src-tauri/tauri.conf.json`, the frontend `package.json`, the root `.mcp.json`, and the live `.connector.json` PID file (plus WS/MCP port probes) -- each missing piece is reported with a copy-pasteable `Fix:` snippet. It exits non-zero when any required check fails, so it drops straight into CI.

```bash
tauri-connector doctor                 # full checklist (text)
tauri-connector doctor --no-runtime    # skip live WS/MCP probes (offline / CI)
tauri-connector doctor --json          # machine-readable output
```

The `--json` payload includes a top-level `setup_pattern` field with one of `"feature-gated" | "legacy" | "mixed" | "none"` so CI can branch on the active pattern:

```bash
$ tauri-connector doctor --no-runtime --json | jq '.setup_pattern, .summary'
"feature-gated"
{ "ok": 9, "fail": 0, "warn": 1, "passed": true }
```

Sections reported:

| Section | Checks |
|---|---|
| Environment | CLI version, working directory, Tauri v2 project detection |
| Plugin Setup | `tauri-plugin-connector` Cargo dep (with `(optional, feature-gated)` tag when applicable), plugin registered (cites the matched cfg gate), `connector:default` permission in `capabilities/` or `capabilities-dev/`, `app.withGlobalTauri: true`, `@zumer/snapdom` in `package.json`, `.mcp.json` entry; under feature-gated/mixed: also `[features] dev-connector` declared and runtime `app.add_capability(include_str!(...))`. Legacy setups receive a non-blocking warn nudging migration. |
| Runtime | `.connector.json` PID file, PID alive, WS ping on `ws_port`, MCP TCP probe on `mcp_port` |
| Integration | `.claude/` auto-detect hook install status (optional) |

Example output for the feature-gated pattern (all green):

```
tauri-connector doctor v0.10.1

Plugin Setup
  ✓ Cargo dependency: tauri-plugin-connector = "0.10" (optional, feature-gated)
  ✓ Plugin registered in src-tauri/src/lib.rs (cfg(feature = "dev-connector"))
  ✓ Permission "connector:default" in src-tauri/capabilities-dev/dev-connector.json
  ✓ app.withGlobalTauri: true
  ✓ Frontend dependency: @zumer/snapdom
  ✓ .mcp.json registers tauri-connector (http://127.0.0.1:9556/mcp)
  ✓ [features] dev-connector = ["dep:tauri-plugin-connector"]
  ✓ Capability loaded at runtime via app.add_capability(include_str!("../capabilities-dev/..."))
```

Example output for a legacy setup (passes, with the migration nudge):

```
Plugin Setup
  ✓ Cargo dependency: tauri-plugin-connector = "0.10"
  ✓ Plugin registered in src-tauri/src/lib.rs (cfg(debug_assertions))
  ✓ Permission "connector:default" in src-tauri/capabilities/default.json
  ✓ app.withGlobalTauri: true
  ✓ Frontend dependency: @zumer/snapdom
  ✓ .mcp.json registers tauri-connector (http://127.0.0.1:9556/mcp)
  ! Using legacy debug_assertions gate — consider migrating to --features dev-connector
      Fix: 1. tauri-plugin-connector = { version = "0.10", optional = true }
           2. [features] dev-connector = ["dep:tauri-plugin-connector"]
           3. replace cfg(debug_assertions) with cfg(feature = "dev-connector")
           4. move connector:default to capabilities-dev/dev-connector.json
           5. register at runtime: app.add_capability(include_str!(...))
           6. "tauri:dev": "tauri dev --features dev-connector"
```

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
# AI snapshot (default mode -- includes refs, component names, portal stitching)
bun -e "
const ws = new WebSocket('ws://127.0.0.1:9555');
ws.onopen = () => ws.send(JSON.stringify({
  id: '1', type: 'dom_snapshot', mode: 'ai', window_id: 'main'
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
| `dom_snapshot` | `mode` (ai/accessibility/structure), `selector`, `max_depth`, `max_elements`, `max_tokens`, `no_split`, `react_enrich`, `follow_portals`, `shadow_dom`, `window_id` |
| `find_element` | `selector`, `strategy`, `window_id` |
| `get_styles` | `selector`, `properties`, `window_id` |
| `interact` | `action`, `selector`, `strategy`, `x`, `y`, `target_selector`, `target_x`, `target_y`, `steps`, `duration_ms`, `drag_strategy`, `window_id` |
| `keyboard` | `action`, `text`, `key`, `modifiers`, `window_id` |
| `wait_for` | `selector`, `strategy`, `text`, `timeout`, `window_id` |
| `window_list` / `window_info` / `window_resize` | `window_id`, `width`, `height` |
| `backend_state` | -- |
| `ipc_execute_command` | `command`, `args` |
| `ipc_monitor` | `action` |
| `ipc_get_captured` | `filter`, `limit`, `pattern`, `since` |
| `ipc_emit_event` | `event_name`, `payload` |
| `console_logs` | `lines`, `filter`, `level`, `pattern`, `since`, `window_id` |
| `clear_logs` | `source` |
| `read_log_file` | `source`, `lines`, `level`, `pattern`, `since`, `window_id` |
| `ipc_listen` | `action`, `events` |
| `event_get_captured` | `event`, `pattern`, `limit`, `since` |
| `search_snapshot` | `pattern`, `context`, `mode`, `window_id` |

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
tauri-connector snapshot -i          # AI snapshot with refs + component names
tauri-connector snapshot -i --mode accessibility  # Accessibility tree only
tauri-connector snapshot -i --no-react            # Skip React enrichment
tauri-connector snapshot -i --no-portals          # Skip portal stitching
tauri-connector snapshot -i --max-elements 2000   # Limit output size
tauri-connector snapshot -i --max-tokens 4000     # Token budget (default 4000, 0=unlimited)
tauri-connector snapshot -i --no-split            # Disable subtree file splitting
tauri-connector snapshots list                    # List recent snapshot sessions
tauri-connector snapshots read <uuid>             # Read layout.txt from a session
tauri-connector snapshots read <uuid> subtree-0.txt  # Read a specific subtree file
tauri-connector click @e5            # Click by ref
tauri-connector fill @e3 "query"     # Fill input
tauri-connector drag @e3 @e7         # Drag element to target
tauri-connector drag @e5 "400,300" --strategy pointer  # Drag to coordinates
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
tauri-connector logs -n 10 -l error              # Error logs only
tauri-connector logs -p "user_\\d+"              # Regex filter
tauri-connector events listen user:login         # Listen for events
tauri-connector events captured                  # Get captured events
tauri-connector events stop                      # Stop listening
tauri-connector clear all                        # Clear all log files
```

Environment: `TAURI_CONNECTOR_HOST` (default `127.0.0.1`), `TAURI_CONNECTOR_PORT` (default `9555`).

## MCP Server

### Embedded (Default)

The MCP server starts automatically inside the Tauri plugin when the app runs. Configure Claude Code with:

```json
{
  "mcpServers": {
    "tauri-connector": {
      "url": "http://127.0.0.1:9556/mcp"
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

Wrap the builder in whichever cfg gate matches your setup pattern (`cfg(feature = "dev-connector")` for the recommended feature-gated layout, or `cfg(debug_assertions)` for the legacy alternative):

```rust
use tauri_plugin_connector::ConnectorBuilder;

#[cfg(feature = "dev-connector")]
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

// The bridge auto-pushes DOM snapshots on page load and significant mutations.
// For manual push (e.g. after a custom state change):
const result = window.__CONNECTOR_SNAPSHOT__({ mode: 'ai', maxElements: 5000 });
await invoke('plugin:connector|push_dom', {
  payload: {
    windowId: 'main',
    html: document.body.innerHTML.substring(0, 500000),
    textContent: document.body.innerText.substring(0, 200000),
    snapshot: result.snapshot,
    snapshotMode: 'ai',
    refs: JSON.stringify(result.refs),
    meta: JSON.stringify(result.meta),
  }
});
```

The bridge JS auto-pushes DOM on page load and significant mutations (5s debounce) when `window.__TAURI_INTERNALS__` is available.

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
|-- skill/                      # Claude Code skill -- debug & code review suite
|   |-- SKILL.md                # Main skill (debug + code review + interaction)
|   |-- SETUP.md                # Setup instructions for new projects
|   |-- scripts/                # Bun scripts for WS interaction (fallback)
|   |   |-- connector.ts        # Shared helper (auto-discovers ports via PID file)
|   |   |-- state.ts, eval.ts, screenshot.ts, snapshot.ts
|   |   |-- click.ts, drag.ts, fill.ts, find.ts, hover.ts, wait.ts
|   |   '-- logs.ts, events.ts, windows.ts
|   '-- references/             # Progressive disclosure reference files
|       |-- mcp-tools.md        # All 25 MCP tool parameter tables
|       |-- cli-commands.md     # Full CLI command reference
|       |-- debug-playbook.md   # 10 debug recipes
|       '-- code-review-playbook.md  # 9 code review workflows
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

1. Plugin starts a streamable HTTP MCP server on port 9556 (configurable)
2. New clients use `POST /mcp` for JSON-RPC and can open `GET /mcp` for a stream
3. Legacy clients can still use `GET /sse` plus `POST /message`
4. Handlers call the bridge and plugin state directly -- zero WebSocket overhead

### Console Log Capture

The bridge intercepts `console.log/warn/error/info/debug`, storing entries in file-backed JSONL storage at `{app_data_dir}/.tauri-connector/console.log`. Logs persist across app restarts. Accessible via `read_logs`, `read_log_file`, or auto-pushed to Rust via `invoke()`.

### Ref System

The unified snapshot engine assigns sequential ref IDs (`e0`, `e1`, ...) to interactive elements (buttons, links, inputs, checkboxes, etc.) and elements with `onclick`, `tabindex`, or `cursor:pointer`. Three ref formats are accepted: `@e1`, `ref=e1`, or `e1`. Refs are persisted to disk and used across subsequent CLI invocations until the next `snapshot` refreshes them. The ref resolution uses a three-strategy fallback: CSS selector, then role+name text matching, then `[role="..."]` attribute matching.

## Requirements

- Tauri v2.x
- Rust 2024 edition
- [Bun](https://bun.sh/) (for skill scripts / WebSocket API examples)
- `@zumer/snapdom` in frontend (optional, for screenshot fallback when xcap is unavailable)

## License

MIT
