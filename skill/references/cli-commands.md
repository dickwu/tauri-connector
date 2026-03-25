# CLI Command Reference

Complete reference for the `tauri-connector` CLI binary. All commands connect to `TAURI_CONNECTOR_HOST:TAURI_CONNECTOR_PORT` (default `127.0.0.1:9555`).

Install: `brew install dickwu/tap/tauri-connector` or `cargo build -p connector-cli --release`

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `TAURI_CONNECTOR_HOST` | `127.0.0.1` | Plugin WebSocket host |
| `TAURI_CONNECTOR_PORT` | `9555` | Plugin WebSocket port |

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
| `--compact` | `-c` | false | Only show lines containing refs |
| `--depth` | `-d` | unlimited | Max tree depth |
| `--max-elements` | | unlimited | Max element count |
| `--selector` | `-s` | | CSS selector to scope subtree |
| `--mode` | | `ai` | `ai`, `accessibility`, `structure` |
| `--no-react` | | false | Skip React fiber enrichment |
| `--no-portals` | | false | Skip portal stitching |

Examples:
```bash
tauri-connector snapshot -i                        # Interactive elements with refs
tauri-connector snapshot -i -c                     # Compact: only ref lines
tauri-connector snapshot -i -s ".ant-form"         # Scope to subtree
tauri-connector snapshot -i --mode accessibility   # ARIA roles/names
tauri-connector snapshot -i --mode structure       # Tags/classes only
tauri-connector snapshot -i -d 5 --max-elements 500
```

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

## Screenshot

```bash
tauri-connector screenshot <output-path> [FLAGS]
```

| Flag | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | `png` | `png`, `jpeg`, `webp` |
| `--quality` | `-q` | 80 | JPEG/WebP quality (0-100) |
| `--max-width` | `-m` | | Max width in pixels (maintains aspect ratio) |
| `--window-id` | | | Target window |

Examples:
```bash
tauri-connector screenshot /tmp/shot.png -m 1280
tauri-connector screenshot /tmp/s.webp -f webp
tauri-connector screenshot /tmp/s.jpg -f jpeg -q 60
```

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

Wait for a CSS selector or text content to appear (polls every 100ms).

```bash
tauri-connector wait [selector] [FLAGS]
```

| Flag | Default | Description |
|---|---|---|
| `--text` | | Text to wait for |
| `--timeout` | 5000 | Timeout in ms |

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
