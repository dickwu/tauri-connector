---
name: tauri-connector
description: "Deep inspection, interaction, debugging, and code review for Tauri v2 desktop apps. Use this skill whenever: working with a Tauri app's UI (clicking, filling forms, reading DOM, screenshots, dragging elements); debugging console logs, IPC calls, or Tauri events; reviewing component trees, accessibility, or visual regressions; testing user flows or validating IPC contracts; setting up tauri-connector in a new project. Also triggers on: DOM snapshots, element refs, webview interaction, drag-and-drop, IPC debugging, Tauri app testing, visual regression, admin/ front/ or tool/ desktop apps, @eN ref syntax, or any mention of tauri-connector CLI or MCP tools. This is Claude's bridge to any running Tauri v2 desktop app -- if a Tauri app is involved, use this skill."
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# Tauri Connector -- Debug & Code Review Suite

Inspect, interact with, debug, and review Tauri v2 desktop apps. The MCP server is embedded in the Tauri plugin -- it starts automatically when the app launches. No separate server process needed.

## Architecture

The plugin injects a JavaScript bridge into each Tauri webview. Commands flow through three paths:

| Path | When to use |
|---|---|
| **MCP tools** (preferred) | Claude has MCP access via `.mcp.json` -- tools appear as `webview_*`, `ipc_*`, etc. |
| **CLI** (`tauri-connector`) | Shell commands with `@eN` ref addressing from snapshots |
| **Bun scripts** (fallback) | Neither MCP nor CLI binary available -- scripts at `skill/scripts/` |

Verify the app is running: `lsof -i :9555 -P -n 2>/dev/null | grep LISTEN`

Port layout:

| Range | Purpose |
|---|---|
| 9300--9400 | Internal bridge (plugin <-> webview JS) |
| 9555--9655 | External WebSocket (CLI + bun scripts) |
| 9556--9656 | Embedded MCP HTTP server: `/mcp` Streamable HTTP, `/sse` legacy HTTP+SSE |

## Core Loop: Debug Snapshot -> Act And Verify

Start with the high-level tools when debugging an unknown UI issue:

```bash
# MCP
debug_snapshot(includeDom: true, includeLogs: true, includeRuntime: true, includeScreenshot: true)
webview_act_and_verify(action: "click", selector: "@e5", waitForText: "Success", includeLogs: true, includeIpc: true, includeRuntime: true)

# CLI
tauri-connector debug snapshot --dom --logs --runtime --screenshot
tauri-connector act click @e5 --wait-text Success --logs --ipc --runtime
```

Fallback to the manual Snapshot -> Act -> Verify loop when you need finer control:

1. **Snapshot** the DOM to see what's on screen and get ref IDs
2. **Act** on elements using those refs (click, fill, drag, type, etc.)
3. **Verify** the result (re-snapshot, check logs, wait for element, screenshot)

Refs like `@e5`, `@e12` are stable handles assigned to interactive elements during a snapshot. The engine uses a multi-strategy fallback (CSS selector -> ARIA role+name -> tag+text content) to re-resolve them even after DOM changes. **Always re-snapshot after DOM-changing actions** -- old refs may point to stale or removed elements.

```bash
# MCP
webview_dom_snapshot(mode: "ai")                          # 1. Snapshot
webview_interact(action: "click", selector: "@e5")        # 2. Act
webview_wait_for(text: "Success", timeout: 5000)          # 3. Verify

# CLI
tauri-connector snapshot -i                               # 1. Snapshot (interactive refs)
tauri-connector click @e5                                 # 2. Act
tauri-connector wait --text "Success"                     # 3. Verify
```

---

## Debugging

### Console Errors

```bash
# Recent errors
read_logs(level: "error", lines: 100)
tauri-connector logs -l error -n 100

# Multi-level with regex
read_logs(level: "error,warn", pattern: "timeout|failed")
tauri-connector logs -l error,warn -p "timeout|failed"

# Historical logs (survive app restarts, stored as JSONL)
read_log_file(source: "console", level: "error", lines: 200, since: 1711900000000)
```

### IPC Debugging

Monitor all `invoke()` calls to find failing commands, unexpected args, or slow responses:

```bash
# 1. Start monitoring
ipc_monitor(action: "start")
tauri-connector ipc monitor

# 2. Trigger the action in the app

# 3. Check captured calls (each entry has: command, args, duration_ms, error)
ipc_get_captured(pattern: "user_\\d+", limit: 20)
tauri-connector ipc captured -p "user_\d+" -l 20

# 4. Test a specific command directly
ipc_execute_command(command: "greet", args: {"name": "test"})
tauri-connector ipc exec greet -a '{"name":"test"}'

# 5. Stop monitoring
ipc_monitor(action: "stop")
tauri-connector ipc unmonitor
```

### Event Debugging

Monitor Tauri app-level events (not DOM events):

```bash
# Listen for specific events
ipc_listen(action: "start", events: ["user:login", "app:error", "state:update"])
tauri-connector events listen user:login,app:error,state:update

# Trigger actions, then check captured events
event_get_captured(pattern: "error", limit: 50)
tauri-connector events captured -p "error" -l 50

# Stop listening
ipc_listen(action: "stop")
tauri-connector events stop
```

### Visual Debugging

```bash
# Native pixel-accurate screenshot (xcap, falls back to snapdom)
webview_screenshot(format: "png", maxWidth: 1280, save: true, nameHint: "debug")
tauri-connector screenshot --name-hint debug -m 1280

# Annotated vision map: labels [N] map to @eN refs from the latest ai snapshot
webview_dom_snapshot(mode: "ai")
webview_screenshot(format: "png", annotate: true, save: true, nameHint: "map")
tauri-connector snapshot -i && tauri-connector screenshot --annotate --name-hint map

# DOM snapshot shows full element tree with refs
webview_dom_snapshot(mode: "ai")
tauri-connector snapshot -i

# Search the snapshot for patterns
webview_search_snapshot(pattern: "error|warning", context: 3)
```

### Runtime State Inspection

```bash
# App metadata: name, version, debug/release, OS, arch, window list
ipc_get_backend_state()
tauri-connector state

# Execute arbitrary JS for runtime inspection
webview_execute_js(script: "(() => { return window.__APP_STATE__ })()")
tauri-connector eval "JSON.stringify(window.__APP_STATE__)"

# Check element computed styles
webview_get_styles(selector: ".error-banner", properties: ["display", "color", "visibility"])
tauri-connector get styles ".error-banner"

# Get specific element properties
tauri-connector get text @e7        # Text content
tauri-connector get value @e3       # Input value
tauri-connector get attr @e5 href   # Attribute
tauri-connector get box @e5         # Bounding box
tauri-connector get count ".item"   # Count matching elements
```

### Full Debug Recipe

When investigating a bug, use `debug_snapshot` first to collect app/bridge state, DOM, logs, runtime captures, and optional screenshot in one call. For a failing interaction, use `webview_act_and_verify` to mark, act, wait, and collect log/IPC/runtime diffs. If the verdict is inconclusive, fall back to the manual loop:

1. `debug_snapshot(includeDom: true, includeLogs: true, includeRuntime: true)`
2. `webview_act_and_verify(action: "...", selector: "@eN", waitForText: "...", includeLogs: true, includeIpc: true, includeRuntime: true)`
3. Manual fallback: `webview_dom_snapshot` -> `ipc_monitor(start)` -> action -> `read_logs` / `runtime_get_captured` / `ipc_get_captured` -> `webview_screenshot` -> `ipc_monitor(stop)`

For more recipes: read `skill/references/debug-playbook.md`

---

## Code Review

### Visual Regression Check

Compare before/after screenshots to catch unintended visual changes:

1. `webview_screenshot(format: "png", maxWidth: 1280)` -- capture current state
2. Apply the code change, rebuild the app
3. Screenshot again -- compare side by side for regressions

### Accessibility Audit

Use accessibility mode to review ARIA roles, names, and semantic structure:

```bash
webview_dom_snapshot(mode: "accessibility")
tauri-connector snapshot -i --mode accessibility
```

Check for: missing labels on interactive elements, incorrect ARIA roles, broken focus order, form fields without associated labels, missing alt text.

### Component Tree Review

React apps get component names extracted from fiber internals:

```bash
webview_dom_snapshot(mode: "ai", reactEnrich: true, followPortals: true)
tauri-connector snapshot -i
```

The snapshot shows React component names, stitches portals to their triggers, and annotates virtual scroll containers:

```
- combobox "Status" [ref=e5, component=InternalSelect, expanded=true]:
  - listbox "Status options" [portal]:
    - option "Active" [selected]
    - option "Inactive"
- list [virtual-scroll, visible=8]:
  - option "Item 1" [ref=e10]
```

### IPC Contract Validation

Verify that UI actions trigger correct IPC commands with expected arguments:

1. `ipc_monitor(action: "start")`
2. Walk through the user flow step by step
3. `ipc_get_captured()` -- verify each command name, args shape, and response
4. Check for: unexpected commands, missing required args, error responses, excessive duplicate calls

### DOM Structure Review

Scope snapshots to specific components for focused review:

```bash
webview_dom_snapshot(selector: ".ant-form", mode: "ai")
tauri-connector snapshot -i -s ".ant-form"

# Search DOM for patterns (data-testid coverage, class conventions, etc.)
webview_search_snapshot(pattern: "data-testid", context: 2)
```

### Event Flow Verification

Verify correct event sequences after user actions:

1. `ipc_listen(action: "start", events: ["state:update", "ui:refresh", "data:saved"])`
2. Perform the action being reviewed
3. `event_get_captured()` -- verify events fired in correct order with expected payloads

For more workflows: read `skill/references/code-review-playbook.md`

---

## Interaction Reference

### Click, Fill, Type

```bash
# MCP
webview_interact(action: "click", selector: "@e5")
webview_interact(action: "click", selector: "button.submit", strategy: "css")
webview_interact(action: "double-click", selector: "@e3")
webview_interact(action: "focus", selector: "#email")
webview_keyboard(action: "type", text: "user@example.com")
webview_keyboard(action: "press", key: "Enter")
webview_keyboard(action: "press", key: "a", modifiers: ["ctrl"])

# CLI
tauri-connector click @e5
tauri-connector dblclick @e3
tauri-connector focus @e3
tauri-connector fill @e3 "user@example.com"    # Clear + set value + fire input/change
tauri-connector type @e3 "hello"               # Char-by-char with key events
tauri-connector check @e10                     # Check checkbox
tauri-connector uncheck @e10                   # Uncheck checkbox
tauri-connector select @e6 "option1" "opt2"    # Select dropdown
tauri-connector press Enter
tauri-connector scroll down 300 --selector ".list"
tauri-connector scrollintoview @e20
```

### Drag and Drop

Three strategies: `auto` (default checks `el.draggable`), `pointer`, `html5dnd`.

```bash
# MCP
webview_interact(action: "drag", selector: "@e3", targetSelector: "@e7", steps: 15, durationMs: 500)
webview_interact(action: "drag", selector: "#item", targetX: 400, targetY: 300, dragStrategy: "pointer")

# CLI
tauri-connector drag @e3 @e7 --steps 15 --duration 500
tauri-connector drag "#card" ".drop-zone" --strategy html5dnd
tauri-connector drag @e5 "400,300"
```

- **pointer**: `pointerdown` -> paced `pointermove` -> `pointerup`. Works with dnd-kit, SortableJS, custom sliders, resize handles.
- **html5dnd**: `dragstart` -> `dragenter`/`dragover` -> `drop` + `dragend`. Works with `draggable="true"`, react-beautiful-dnd.
- Increase `steps` (>5) if the library needs movement threshold. Increase `durationMs` for timing-sensitive libs.

### Wait and Find

```bash
# MCP
webview_wait_for(selector: ".loaded", timeout: 10000)
webview_wait_for(text: "Success", strategy: "text")
webview_wait_for(url: "**/settings*", loadState: "load", timeout: 10000)
webview_locator(role: "button", name: "Save", action: "click")
webview_find_element(selector: "Submit", strategy: "text")
webview_find_element(selector: "error|warning", strategy: "regex", target: "class")

# CLI
tauri-connector wait ".loaded" --state visible --timeout 10000
tauri-connector wait --text "Success"
tauri-connector wait --url "**/settings*" --load-state load
tauri-connector locator --role button --name Save --action click
tauri-connector find "Submit" -s text
```

### Windows

```bash
manage_window(action: "list")
manage_window(action: "resize", width: 1024, height: 768)
tauri-connector windows
tauri-connector resize 1024 768 --window-id settings
```

---

## Snapshot Budget & Subtree Files

For complex apps, DOM snapshots can exceed AI tool result limits. The snapshot engine automatically manages output size:

- **Default behavior**: `maxTokens: 4000` over MCP -- large DOMs return a layout skeleton (inline) plus `file=subtree-K.txt` markers pointing at on-disk subtree files.
- **WebSocket / Bun / internal callers**: default `maxTokens: 0` (full output) for backward compatibility. Pass `max_tokens` explicitly to opt in.
- **Unlimited on MCP**: set `maxTokens: 0` or `noSplit: true` to restore legacy behavior.
- **Subtree files**: written atomically under `<log_dir>/snapshots/<snapshotId>/` (`0700` dir on unix). The active `log_dir` is exposed in `.connector.json` and backend/debug state; if it cannot be initialized, the plugin falls back to a temp `.tauri-connector` directory. `meta.subtreeFiles[].path` gives the absolute path; `allRefsPath` points to `refs.json` when the ref map also spills.
- **Reading subtrees**: use the `Read` tool on the `path` field, or the CLI's `snapshots read <uuid> <file>` (canonicalized -- rejects path traversal). Old sessions are auto-pruned by mtime (keeps newest 5).
- **Search stays complete**: `webview_search_snapshot` matches against merged full text (skeleton + every subtree), so filters never hide inside spilled content.
- **Repeating siblings**: runs of 5+ siblings with the same tag + role + ARIA state collapse to 2 examples + a marker; the collapsed rows are written to a subtree file whenever the budget is active.
- **`--compact` / `-c`** on the CLI keeps lines containing refs _plus_ subtree markers, so you never lose a pointer to spilled content.

```bash
# MCP -- default 4000-token budget (splits if needed)
webview_dom_snapshot(mode: "ai")

# MCP -- raise the budget for a big page
webview_dom_snapshot(mode: "ai", maxTokens: 8000)

# MCP -- unlimited (legacy behavior)
webview_dom_snapshot(mode: "ai", maxTokens: 0)
webview_dom_snapshot(mode: "ai", noSplit: true)

# MCP -- search across spilled subtrees (context=3 lines)
webview_search_snapshot(pattern: "submit|confirm", context: 3)

# CLI -- with default budget
tauri-connector snapshot -i

# CLI -- larger budget, or full output
tauri-connector snapshot -i --max-tokens 8000
tauri-connector snapshot -i --no-split

# CLI -- list/read snapshot sessions
tauri-connector snapshots list
tauri-connector snapshots read <uuid>                 # layout.txt (default)
tauri-connector snapshots read <uuid> subtree-0.txt
tauri-connector snapshots read <uuid> refs.json

# Bun -- opt in to budgeting (default is unlimited over WS)
bun run $SCRIPTS/snapshot.ts ai --max-tokens 4000
bun run $SCRIPTS/snapshot.ts ai --no-split
```

---

## Ant Design / React Apps

The snapshot engine reads `__reactFiber$` internals to show component names, detects portals via `aria-controls`/`aria-owns` and stitches them to their triggers, and annotates virtual scroll containers.

Scope to Ant Design components:

```bash
webview_dom_snapshot(selector: ".ant-modal-content")   # Modal
webview_dom_snapshot(selector: ".ant-drawer-body")     # Drawer
webview_dom_snapshot(selector: ".ant-form")            # Form
webview_dom_snapshot(selector: ".ant-table-wrapper")   # Table
```

## Bun Script Fallback

When MCP and CLI are unavailable. Requires `bun` runtime:

```bash
SCRIPTS=<tauri-connector-repo>/skill/scripts
bun run $SCRIPTS/snapshot.ts              # DOM snapshot with refs
bun run $SCRIPTS/click.ts "button.submit" # Click element
bun run $SCRIPTS/fill.ts "input" "value"  # Fill input
bun run $SCRIPTS/drag.ts "@e3" "@e7"      # Drag between refs
bun run $SCRIPTS/hover.ts ".trigger"      # Hover (--off to leave)
bun run $SCRIPTS/logs.ts 50               # Console logs
bun run $SCRIPTS/screenshot.ts /tmp/s.png # Screenshot
bun run $SCRIPTS/eval.ts "document.title" # Execute JS
bun run $SCRIPTS/find.ts "button"         # Find elements
bun run $SCRIPTS/wait.ts ".loaded"        # Wait for selector
bun run $SCRIPTS/state.ts                 # App metadata
bun run $SCRIPTS/windows.ts              # List windows
bun run $SCRIPTS/events.ts listen user:login  # Listen for events
```

## Setup

For first-time setup in a Tauri v2 project, read `skill/SETUP.md`. The skill defaults to the **feature-gated** pattern (cleaner release builds; legacy `cfg(debug_assertions)` still supported as Alternative). Summary:

1. `tauri-plugin-connector = { version = "0.12", optional = true }` in `src-tauri/Cargo.toml`
2. Declare the cargo feature: `[features] dev-connector = ["dep:tauri-plugin-connector"]`
3. Register the plugin with `#[cfg(feature = "dev-connector")]` guard
4. Drop the dev capability JSON at `src-tauri/capabilities-dev/dev-connector.json` (outside the default `capabilities/` glob), and register it at runtime via `app.add_capability(include_str!("../capabilities-dev/dev-connector.json"))` inside the same `cfg(feature = "dev-connector")`
5. Set `"withGlobalTauri": true` in `tauri.conf.json`
6. Install `@zumer/snapdom` for screenshot fallback
7. Add `"tauri:dev": "tauri dev --features dev-connector"` to `package.json`
8. Add `"url": "http://127.0.0.1:9556/mcp"` to `.mcp.json`

For the legacy alternative, swap step 1 to `tauri-plugin-connector = "0.12"`, drop step 2, replace step 3 with `#[cfg(debug_assertions)]`, replace step 4 with `"connector:default"` in `src-tauri/capabilities/default.json`, and skip step 7. `tauri-connector doctor` accepts both — it auto-detects the active pattern.

CLI install: `brew install dickwu/tap/tauri-connector`

### Verify setup with `doctor` (v0.11+)

Before troubleshooting a broken connection, DOM bridge timeout, or missing MCP tools, run `tauri-connector doctor` from the project root. It validates every setup step in one pass and prints a concrete `Fix:` line for anything missing or misconfigured -- faster than walking `.mcp.json`, `tauri.conf.json`, capabilities, etc. by hand.

```bash
tauri-connector doctor                 # full checklist (text)
tauri-connector doctor --no-runtime    # skip live WS/MCP probes (offline / CI)
tauri-connector doctor --json          # machine-readable output (exit code 0/1)
```

The `--json` payload includes a top-level `setup_pattern` field with one of `"feature-gated" | "legacy" | "mixed" | "none"` — branch on this in CI to apply pattern-specific gates without re-parsing the section list.

What it verifies:

| Section | Checks |
|---|---|
| Environment | CLI version, working directory, Tauri v2 project detection (walks up to find `src-tauri/tauri.conf.json`) |
| Plugin Setup | `tauri-plugin-connector` in `src-tauri/Cargo.toml` (with `(optional, feature-gated)` tag when applicable); plugin registered via `init()` / `ConnectorBuilder` in `lib.rs`/`main.rs` (cites the matched cfg gate); `"connector:default"` in `src-tauri/capabilities/*.json` **or** `src-tauri/capabilities-dev/*.json`; `app.withGlobalTauri: true`; `@zumer/snapdom` in `package.json`; `.mcp.json` registers `/mcp`. Under feature-gated/mixed: also verifies `[features] dev-connector`, runtime `app.add_capability(include_str!(...))`, and a package script that passes `--features dev-connector`. Legacy setups receive a non-blocking warn nudging migration. |
| Runtime | `.connector.json` PID file under `target/`, PID alive, runtime metadata/log_dir/log files, WebSocket ping on `ws_port`, bridge status, runtime/artifact/debug command availability, and MCP Streamable HTTP lifecycle (`initialize`, notification 202, ping, GET 405, DELETE) on `mcp_port` |
| Integration | `.claude/` auto-detect hook install status (optional), local skill doc freshness against the CLI-bundled skill |

Exit code is non-zero when any required check fails, so `doctor --json` drops cleanly into CI or pre-commit. The JSON payload includes a top-level `fixes` array with every warning/failure remediation. Use `--no-runtime` when the Tauri app isn't running (offline setup validation).

First move when something looks wrong: `tauri-connector doctor`. Second move: read the `Fix:` line.

### Bundled skill docs

The CLI embeds this skill and references so agents can load version-matched docs before guessing command syntax:

```bash
tauri-connector skills list
tauri-connector skills get tauri-connector
tauri-connector skills get mcp-tools
tauri-connector skills path references/mcp-tools.md
```

## Deep Reference

For full parameter tables and extended workflows:

| File | Contents |
|---|---|
| `skill/references/mcp-tools.md` | MCP tool parameter tables with types and defaults |
| `skill/references/cli-commands.md` | Every CLI subcommand with all flags and examples |
| `skill/references/debug-playbook.md` | Step-by-step recipes for common debug scenarios |
| `skill/references/code-review-playbook.md` | Code review workflow recipes and checklists |

## Troubleshooting

Run `tauri-connector doctor` first -- it catches most of the issues below in one pass and prints the exact fix.

| Problem | Fix |
|---|---|
| Any setup problem | `tauri-connector doctor` -- prints a `Fix:` line for each missing/misconfigured piece |
| `Permission connector:default not found` in release `tauri build` | The connector capability JSON is being loaded by `tauri-build`'s default `./capabilities/**/*` glob. Migrate to the feature-gated layout: move it to `src-tauri/capabilities-dev/dev-connector.json` and register it at runtime via `app.add_capability(include_str!(...))` inside `cfg(feature = "dev-connector")`. Re-run `tauri-connector doctor`. |
| `tauri build` still compiles the plugin / pulls xcap, aws-sdk-s3 | Plugin is gated on `cfg(debug_assertions)` (legacy). Migrate to `cfg(feature = "dev-connector")` with `optional = true` so the dep is skipped entirely when the feature is off. Doctor's legacy nudge has the full migration checklist. |
| Connection refused | App not running or plugin not loaded. Check: `lsof -i :9555 \| grep LISTEN` |
| Stale PID file | App crashed. Delete: `rm target/debug/.connector.json` |
| Port conflict | Use `ConnectorBuilder::new().port_range(9600, 9700)` or set `TAURI_CONNECTOR_PORT=9600` |
| Refs not found | DOM changed since snapshot. Re-run snapshot for fresh refs |
| Drag not working | Try explicit `--strategy pointer` or `html5dnd`. Increase `--steps` (>5) and `--duration` |
| Screenshot blank | Install `@zumer/snapdom` for DOM-based fallback capture |
| No MCP tools | Verify `.mcp.json` has `"url": "http://127.0.0.1:9556/mcp"` and app is running |
| Bridge not connecting | Check `withGlobalTauri: true` in tauri.conf.json. Bridge auto-reconnects every 1s |
| Logs empty | Console interception starts on bridge connect. Ensure plugin is registered before app loads |
