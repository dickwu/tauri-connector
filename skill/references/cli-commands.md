# CLI Command Reference

Complete reference for the `tauri-connector` CLI binary. Commands resolve the connection as `--host/--port` > `TAURI_CONNECTOR_*` env > nearby `.connector.json` > port scan.

Install: `brew install dickwu/tap/tauri-connector` or `cargo build -p connector-cli --release`

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin WebSocket host |
| `TAURI_CONNECTOR_PORT` | discovered | Plugin WebSocket port |
| `TAURI_CONNECTOR_PID_FILE` | | Explicit `.connector.json` path |
| `TAURI_CONNECTOR_APP_ID` | | Filter discovered instances by app identifier |

---

## Connection

```bash
tauri-connector status
tauri-connector status --json
tauri-connector --app-id com.example.app logs -l error
tauri-connector --window-id settings snapshot -i
tauri-connector --pid-file src-tauri/target/.connector.json snapshot -i
tauri-connector bridge
```

`status` lists live and stale candidates. `bridge` shows connected webviews, pending evals, and whether eval fallback is available. `--window-id` is global and scopes snapshots, refs, interactions, screenshots, logs, and window operations to a specific Tauri window label.

---

## Bundled Skills

The CLI embeds the current `tauri-connector` skill docs, including references.

```bash
tauri-connector skills list
tauri-connector skills get tauri-connector
tauri-connector skills get mcp-tools
tauri-connector skills path references/mcp-tools.md
```

`skills get` prints content to stdout. `skills path` materializes the bundled file under the CLI cache and prints the path; use it when an agent needs an actual filesystem path.

---

## DOM & Inspection

### snapshot

Take a DOM snapshot with optional ref IDs for interactive elements.

```bash
tauri-connector snapshot [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--interactive` | `-i` | false | Assign `@eN` refs to interactive elements |
| `--compact` | `-c` | false | Only show lines containing refs (subtree file markers are preserved) |
| `--depth` | `-d` | unlimited | Max tree depth |
| `--max-elements` | | unlimited | Max element count |
| `--max-tokens` | | 4000 | Token budget for inline output. Overflow spills to subtree files. `0` = unlimited |
| `--no-split` | | false | Disable subtree file splitting -- full inline output regardless of budget |
| `--selector` | `-s` | | CSS selector to scope subtree |
| `--mode` | | `ai` | `ai`, `accessibility`, `structure` |
| `--no-react` | | false | Skip React fiber enrichment |
| `--no-portals` | | false | Skip portal stitching |

Examples:
```bash
tauri-connector snapshot -i                        # Interactive elements with refs (default 4000-token budget)
tauri-connector snapshot -i -c                     # Compact: only ref lines + subtree markers
tauri-connector snapshot -i -s ".ant-form"         # Scope to subtree
tauri-connector snapshot -i --mode accessibility   # ARIA roles/names
tauri-connector snapshot -i --mode structure       # Tags/classes only
tauri-connector snapshot -i -d 5 --max-elements 500
tauri-connector snapshot -i --max-tokens 8000      # Raise inline budget
tauri-connector snapshot -i --max-tokens 0         # Unlimited (legacy behavior)
tauri-connector snapshot -i --no-split             # Full output, never write subtree files
```

When the budget fires, stderr prints the snapshot UUID plus each spilled subtree's label, estimated tokens, and absolute path under the app connector log directory's `snapshots/<uuid>/`.

### snapshots

Manage the snapshot session directory (subtree files from split snapshots).

```bash
tauri-connector snapshots list                     # Most recent 10 sessions (newest first)
tauri-connector snapshots read <uuid>              # Print layout.txt (default)
tauri-connector snapshots read <uuid> subtree-0.txt
tauri-connector snapshots read <uuid> refs.json    # Full ref map when allRefs spills to disk
tauri-connector snapshots read <uuid> meta.json    # Session metadata
```

`snapshots read` canonicalizes its path and refuses anything that escapes the session directory, so it is safe to drive from scripts.

### dom

Get the cached DOM pushed automatically from the frontend (no round-trip).

```bash
tauri-connector dom [--window-id <id>]
```

### find

Find elements by CSS selector, XPath, or visible text.

```bash
tauri-connector find <selector> [-s <strategy>]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--strategy` | `-s` | `css` | `css`, `xpath`, `text` |

### get

Read properties from elements or the page.

```bash
tauri-connector get <property> [target] [extra]
```

| Property | Target | Extra | Description |
|---|---|---|---|
| `title` | | | Page title |
| `url` | | | Current URL |
| `text` | `@eN` or CSS | | Element text content |
| `html` | `@eN` or CSS | | Element innerHTML |
| `value` | `@eN` or CSS | | Input element value |
| `attr` | `@eN` or CSS | attr name | Attribute value |
| `box` | `@eN` or CSS | | Bounding box `{x, y, width, height}` |
| `styles` | `@eN` or CSS | | All computed CSS styles |
| `count` | CSS selector | | Count of matching elements |

Examples:
```bash
tauri-connector get title
tauri-connector get text @e7
tauri-connector get attr @e5 href
tauri-connector get count ".list-item"
tauri-connector get styles ".error-banner"
```

### pointed

Get the element captured by Alt+Shift+Click in the app.

```bash
tauri-connector pointed
```

---

## Interactions

### click

```bash
tauri-connector click <target>
```

`target` can be `@eN` ref or CSS selector.

### dblclick

```bash
tauri-connector dblclick <target>
```

### hover

```bash
tauri-connector hover <target>
```

### focus

```bash
tauri-connector focus <target>
```

### fill

Clear input and set value, firing `input` and `change` events.

```bash
tauri-connector fill <target> <text>
```

### type

Type text character-by-character, firing `keydown`/`keypress`/`keyup` per character.

```bash
tauri-connector type <target> <text>
```

### check / uncheck

Toggle checkbox state.

```bash
tauri-connector check <target>
tauri-connector uncheck <target>
```

### select

Select dropdown option(s) by value.

```bash
tauri-connector select <target> <values...>
```

### scroll

Scroll the page or a specific element.

```bash
tauri-connector scroll [direction] [amount] [--selector <sel>]
```

| Arg | Default | Description |
|---|---|---|
| `direction` | `down` | `up`, `down`, `left`, `right` |
| `amount` | `300` | Pixels to scroll |
| `--selector` | | Element to scroll (page if omitted) |

### scrollintoview

Scroll element into viewport (smooth, centered).

```bash
tauri-connector scrollintoview <target>
```

### press

Press a key on the currently focused element.

```bash
tauri-connector press <key>
```

Keys: `Enter`, `Tab`, `Escape`, `Backspace`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Home`, `End`, `PageUp`, `PageDown`, `Delete`, `Space`, `F1`-`F12`, or any single character.

---

## Drag and Drop

```bash
tauri-connector drag <source> <target> [FLAGS]
```

`source` and `target` can be `@eN` refs, CSS selectors, or `"x,y"` pixel coordinates.

| Flag | Default | Description |
|---|---|---|
| `--steps` | 10 | Intermediate move events (higher = smoother, some libs need >5) |
| `--duration` | 300 | Total drag time in ms |
| `--strategy` | `auto` | `auto`, `pointer`, `html5dnd` |

Strategy details:
- **auto**: Checks `el.draggable` -- uses `html5dnd` if true, `pointer` otherwise
- **pointer**: `pointerdown` -> paced `pointermove` -> `pointerup` with mouse event mirrors. Works with dnd-kit, SortableJS, framer-motion, custom sliders, resize handles.
- **html5dnd**: `dragstart` -> paced `dragenter`/`dragover` -> `drop` + `dragend` with DataTransfer. Works with `draggable="true"`, react-beautiful-dnd.

Examples:
```bash
tauri-connector drag @e3 @e7                              # Refs
tauri-connector drag "#item-3" "#item-1"                  # CSS selectors
tauri-connector drag @e5 "400,300"                        # To pixel coords
tauri-connector drag @e3 @e7 --steps 20 --duration 500    # Slower, smoother
tauri-connector drag "#card" ".list" --strategy pointer    # Force pointer
tauri-connector drag "[draggable]" ".trash" --strategy html5dnd
```

---

## Application-owned workflows

```bash
tauri-connector workflow capabilities
tauri-connector workflow run <JSON_FILE_OR_INLINE_OR_STDIN_DASH> --wait-ms 30000
tauri-connector workflow get <runId> --cursor 0 --include evidence
tauri-connector workflow get <runId> --evidence-id <evidenceRef> --offset 0
tauri-connector workflow cancel <runId>
tauri-connector workflow resume <runId> --expected-revision 7 --checkpoint-id <checkpointId> --intent reconcile
```

Set `TAURI_CONNECTOR_WORKFLOW_TOKEN` to the trusted host token (at least 32 bytes). Capability lookup does not require a token. `run` reads a v1 spec and forwards it unchanged to the application; it never runs workflow steps in the CLI. The spec's `windowId` controls execution. `--wait-ms` ranges from 0 to 30000 (default 1000); it limits response waiting while app execution continues under its original deadline.

The JSON response includes `runId` and the current status. Exit code `0` means completed success or successful capability lookup; `1` means failure, cancellation, invalid input or transport error; `2` means queued, running, paused, interrupted or unknown. Never interpret a nonzero code as proof that no effect occurred. Query `get` or reuse the same `runKey` and identical spec after a lost submission response.

`get --evidence-id` reads one retained reference as an `evidencePage` with JSON text `content`, byte `offset`, `nextOffset`, and `totalBytes`. Follow the returned UTF-8 byte `nextOffset` to read further chunks. `--offset` requires `--evidence-id`; omitted offset starts at zero. `nextOffset: null` marks the final chunk. Paging does not dispatch business actions.

Resume requires all three flags and `--intent continue|reconcile`. Continue is allowed only for an undispatched paused step in the same app instance with remaining deadline. Reconcile only rechecks an available postcondition; it cannot replay an uncertain action or turn the original failure into a passing test. After app restart, retained runs are read-only history. See `references/mcp-tools.md` for spec/locator/expression/condition fields and limits.

## Batch Actions

```bash
tauri-connector batch <SPEC> [FLAGS]
```

Run several MCP tool calls from one JSON spec -- sequentially, in parallel, or
DAG-ordered via `dependsOn` -- and print the run report with one log entry per
action (status, timing, result/error). `SPEC` is inline JSON, a path to a JSON
file, or `-` for stdin. Actions use MCP tool names and arguments (see
`references/mcp-tools.md` -> `batch_actions` for the full spec format); a bare
JSON array is shorthand for `{ "actions": [...] }`.

| Flag | Default | Description |
|---|---|---|
| `--mode` | `sequential` | `sequential` or `parallel` (overrides the spec) |
| `--continue-on-error` | off | Keep running the remaining actions after a failure (`stopOnError: false`); only explicit `dependsOn` edges on the failed action skip |
| `--max-parallel` | unlimited | Cap on concurrently running actions |
| `--save` | | Write the run report as pretty JSON to this file |

Examples:
```bash
# Sequential flow: click, wait, then read errors
tauri-connector batch '{
  "actions": [
    { "id": "open", "tool": "webview_interact", "args": { "action": "click", "selector": "@e5" } },
    { "id": "settle", "tool": "webview_wait_for", "args": { "selector": ".modal", "timeout": 5000 } },
    { "tool": "read_logs", "args": { "level": "error" } }
  ]
}'

# Parallel evidence collection, report saved to a file
tauri-connector batch flow.json --mode parallel --save report.json

# Bare-array shorthand from stdin
echo '[ { "tool": "bridge_status" }, { "tool": "ipc_get_backend_state" } ]' \
  | tauri-connector batch -
```

- The global `--window-id` becomes the default `windowId` for actions that
  don't set one in their own `args`.
- Typed execution and verification failures fail the action. Arbitrary business JSON containing `error` remains data. Report-save failures preserve the business report with `persistenceWarning`.
- Exits non-zero when any action fails or is skipped, after printing the
  report.
- `batch_actions`, `driver_session`, and `workflow_*` lifecycle tools are rejected inside a batch.
- Batch screenshots default to saved artifacts with compact report references. Explicit `save: false` retains legacy inline base64.

---

## Screenshot

```bash
tauri-connector screenshot [output-path] [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | `png` | `png`, `jpeg`, `webp` |
| `--quality` | `-q` | 80 | JPEG/WebP quality (0-100) |
| `--max-width` | `-m` | | Max width in pixels (maintains aspect ratio) |
| `--selector` | `-s` | | CSS selector or `@eN` ref for element-scoped capture |
| `--overwrite` | | false | Allow replacing an existing output path |
| `--output-dir` | | connector artifact dir | Directory for auto-generated names |
| `--name-hint` | | `full` | Slug included in generated filenames |
| `--annotate` | | false | Overlay numbered labels for `@eN` refs from the latest `ai` snapshot |

Examples:
```bash
tauri-connector screenshot --name-hint login-before -m 1280  # Auto artifact path
tauri-connector screenshot /tmp/shot.png                     # If it exists, writes a unique sibling
tauri-connector screenshot /tmp/shot.png --overwrite          # Explicitly replace that path
tauri-connector screenshot --selector @e5 --name-hint submit  # Element capture
tauri-connector snapshot -i && tauri-connector screenshot --annotate --name-hint map
tauri-connector screenshot /tmp/s.webp -f webp
tauri-connector screenshot /tmp/s.jpg -f jpeg -q 60
```

The command prints the final resolved path and `sha256` as JSON. Annotated screenshots also include `annotations`, `snapshotId`, and `refsPath`; artifact manifest rows include `path`, `sha256`, `refsPath`, `snapshotId`, `windowId`, `selector`, `width`, and `height`. Do not assume a reused requested path is the latest capture unless `--overwrite` was used.

### artifacts

List, inspect, prune, and compare screenshot artifacts from the connector manifest.

```bash
tauri-connector artifacts list --kind screenshot
tauri-connector artifacts show shot_...
tauri-connector artifacts show shot_... --base64
tauri-connector artifacts compare shot_before shot_after --threshold 0.01
tauri-connector artifacts prune --keep 50
tauri-connector artifacts prune --keep 50 --manifest-only
```

`prune` removes older manifest entries and deletes their files by default. Use `--manifest-only` to rewrite only the registry.

---

## Logs & Debugging

### logs

Read console logs from the webview.

```bash
tauri-connector logs [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--lines` | `-n` | 50 | Number of entries |
| `--filter` | `-f` | | Substring match |
| `--level` | `-l` | | Comma-separated: `log`, `info`, `warn`, `error`, `debug` |
| `--pattern` | `-p` | | Regex match |

### runtime

Read runtime-level frontend captures: window errors, unhandled promise rejections, network failures/statuses, navigation changes, and resource load failures.

```bash
tauri-connector runtime -n 100 --kind network --pattern "500|timeout"
tauri-connector runtime -l error --since-mark mark_...
tauri-connector clear runtime
```

### debug

Create marks and collect bundled debug context.

```bash
tauri-connector debug mark before-login-click
tauri-connector debug snapshot --dom --screenshot --logs --ipc --runtime
tauri-connector debug snapshot --runtime --since-mark mark_...
```

### act

Perform an action, wait for a visible result, and collect fresh evidence in one call.

```bash
tauri-connector act click @e5 --wait-text Success --screenshot --logs --ipc --runtime
tauri-connector act fill @e3 "user@example.com" --wait-selector ".valid" --dom
tauri-connector act press Enter --wait-selector ".submitted" --runtime
```

### eval

Execute arbitrary JavaScript and print the result.

```bash
tauri-connector eval <script>
```

### state

Print app metadata: name, version, debug/release, OS, arch, window list.

```bash
tauri-connector state
```

### wait

Wait for selector state, text content, URL glob, load state, or a JavaScript condition (polls every 100ms). Multiple conditions are combined.

```bash
tauri-connector wait [selector] [FLAGS]
```

| Flag | Default | Description |
|---|---|---|
| `--text` | | Text to wait for |
| `--url` | | Glob pattern matched against `location.href` |
| `--load-state` | | `domcontentloaded` or `load`; `networkidle` returns `unsupported_condition` |
| `--fn` | | JavaScript expression/function/body that returns truthy |
| `--state` | `attached` | Selector state: `attached`, `detached`, `visible`, `hidden` |
| `--timeout` | 5000 | Timeout in ms |

Examples:
```bash
tauri-connector wait ".loaded" --state visible --timeout 10000
tauri-connector wait --url "**/settings*" --load-state load
tauri-connector wait --fn "window.__APP_READY__ === true"
tauri-connector wait ".toast" --state hidden
```

### locator

Find by semantic locator and optionally act on the matched element. Use this when `@eN` refs are unavailable or stale.

```bash
tauri-connector locator --role button --name "Save" --action click
tauri-connector locator --label Email --action fill --value user@example.com
tauri-connector locator --text "Sign in" --exact --first
tauri-connector locator --test-id submit --action click
```

| Flag | Description |
|---|---|
| `--role`, `--text`, `--label`, `--placeholder`, `--alt`, `--title`, `--test-id` | Primary locator |
| `--name` | Accessible-name filter |
| `--exact` | Exact text/name match |
| `--first`, `--last`, `--nth` | Match selection |
| `--action` | `click`, `fill`, `type`, `hover`, `focus`, `check`, `uncheck`, `text` |
| `--value` | Value for `fill` or `type` |

### clear

Clear log files.

```bash
tauri-connector clear <target>
```

`target`: `logs`, `ipc`, `events`, `all`

---

## IPC Commands

### ipc exec

Execute a Tauri IPC command.

```bash
tauri-connector ipc exec <command> [-a <json-args>]
```

### ipc monitor / unmonitor

Start or stop IPC call monitoring.

```bash
tauri-connector ipc monitor
tauri-connector ipc unmonitor
```

### ipc captured

View captured IPC calls.

```bash
tauri-connector ipc captured [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--filter` | `-f` | | Substring match on command |
| `--pattern` | `-p` | | Regex match |
| `--since` | | | Epoch ms filter |
| `--limit` | `-l` | 50 | Max entries |

---

## Event Commands

### emit

Emit a Tauri event.

```bash
tauri-connector emit <event-name> [-p <json-payload>]
```

### events listen

Start listening for specific Tauri events.

```bash
tauri-connector events listen <events>
```

`events`: comma-separated event names (e.g., `user:login,app:error`)

### events captured

View captured events.

```bash
tauri-connector events captured [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--pattern` | `-p` | | Regex match |
| `--since` | | | Epoch ms filter |
| `--limit` | `-l` | 50 | Max entries |

### events stop

Stop listening for events.

```bash
tauri-connector events stop
```

---

## Window Management

### windows

List all open windows.

```bash
tauri-connector windows
```

### resize

Resize a window.

```bash
tauri-connector resize <width> <height> [--window-id <id>]
```

---

## Utility

### update

Check for or install CLI updates from GitHub Releases.

```bash
tauri-connector update [--check]
```

### examples

Print usage examples.

```bash
tauri-connector examples
```

### doctor

Diagnose the current project's tauri-connector setup. Walks `src-tauri/`, the
frontend `package.json`, `.mcp.json`, and the runtime `.connector.json` PID
file, then probes the WebSocket + MCP ports. Each missing piece is reported
with a concrete Fix line, and `--json` includes a top-level `fixes` array for
CI/reporting. Exits non-zero when one or more required checks fail.

```bash
tauri-connector doctor                     # full checklist
tauri-connector doctor --no-runtime        # skip live WS/MCP probes
tauri-connector doctor --json              # machine-readable output
```

What it verifies:
- `tauri-plugin-connector` in `src-tauri/Cargo.toml`
- plugin registered via `tauri_plugin_connector::init()` (or `ConnectorBuilder`) in `lib.rs`/`main.rs`
- `"connector:default"` permission in any file under `src-tauri/capabilities/`
- `app.withGlobalTauri: true` in `src-tauri/tauri.conf.json`
- `@zumer/snapdom` in root `package.json` (any dependency bucket)
- `.mcp.json` registers `tauri-connector`
- `.connector.json` PID file + live WS ping + MCP Streamable HTTP initialize POST
- PID liveness, runtime metadata/log_dir, JSONL log files, bridge status, runtime/artifact/debug WS commands
- MCP Streamable HTTP lifecycle: initialize 200, notification 202 empty body, ping 200, GET /mcp 405, DELETE 204
- `.claude/` auto-detect hook installation (optional)
