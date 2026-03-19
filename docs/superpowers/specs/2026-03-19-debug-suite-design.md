# Debug Suite: Regex Filtering, File-Backed Logs, Event Monitoring, DOM Search

**Date:** 2026-03-19
**Status:** Approved
**Version:** v0.6.0

## Overview

Enhance tauri-connector's debugging capabilities for LLM agents with four interconnected features:

1. **Log enhancement** -- regex + level filtering on console logs
2. **File-backed persistence** -- JSONL log files that survive app restarts
3. **Event monitoring** -- wire up IPC capture + new event listener tool
4. **DOM regex search** -- regex-based element finding and snapshot search

All in-memory buffers (`VecDeque<LogEntry>`, `VecDeque<IpcEvent>`) are replaced with file-backed JSONL storage. The `regex` crate is added on the Rust side; JS-side filtering uses native `RegExp`.

---

## 1. File-Backed Log Storage

### Directory structure

```
{app_data_dir}/.tauri-connector/
  console.log     # webview console logs (JSONL)
  ipc.log         # IPC invocation logs (JSONL)
  events.log      # Tauri event logs (JSONL)
```

Resolved once at plugin init from `app.path().app_data_dir()` and stored as `log_dir: PathBuf` in `PluginState`. Directory created on first write.

### JSONL formats

**console.log:**
```json
{"level":"error","message":"fetch failed","timestamp":1710806400000,"window_id":"main"}
```

**ipc.log:**
```json
{"command":"get_user","args":{"id":42},"timestamp":1710806400000,"duration_ms":15}
```

**events.log:**
```json
{"event":"user:login","payload":{"user_id":42},"timestamp":1710806400000,"window_id":"main"}
```

### State changes

Remove from `PluginState`:
- `log_cache: Mutex<VecDeque<LogEntry>>`
- `ipc_events: Mutex<VecDeque<IpcEvent>>`

Add to `PluginState`:
- `log_dir: PathBuf`
- `console_writer: Mutex<BufWriter<File>>` -- opened at init, append mode
- `ipc_writer: Mutex<BufWriter<File>>` -- opened at init, append mode
- `event_writer: Mutex<BufWriter<File>>` -- opened at init, append mode
- `event_listeners: Mutex<Vec<String>>`

Keep:
- `ipc_monitor_active: Mutex<bool>`

### Concurrency

Each log file has a dedicated `Mutex<BufWriter<File>>` in `PluginState`, opened once at plugin init. All writes acquire the mutex, write JSONL line(s), and flush. All reads acquire the same mutex to prevent reading a partially-written line. This replaces the previous `VecDeque` mutexes with equivalent file-backed mutexes.

### Write path

- `push_logs` handler: acquires `console_writer` mutex, writes JSONL lines, flushes
- `push_ipc_event` handler: acquires `ipc_writer` mutex, writes JSONL lines, flushes
- `push_event` handler (new): acquires `event_writer` mutex, writes JSONL lines, flushes

Both `push_ipc_event` and `push_event` must be registered as `#[tauri::command]` in `lib.rs`'s `invoke_handler![]` macro so the bridge JS can call them via `plugin:connector|push_ipc_event` and `plugin:connector|push_event`.

### Read path

All read handlers acquire the corresponding writer mutex (to prevent concurrent write during read), use `BufReader` to read the file line by line, apply filters (level, regex, substring, since timestamp), collect matching entries, then return the last N matches. For large files, reads all lines and takes the tail -- acceptable given typical log volumes (hundreds to low thousands of entries per session).

### File-not-found handling

If a log file does not exist when a read handler is called (e.g., first run before any writes), return an empty collection: `{ count: 0, entries: [] }` (or `{ count: 0, logs: [] }` for `read_logs`). Never return an error for a missing log file.

---

## 2. Log Enhancement -- `read_logs` + `clear_logs`

### `read_logs` -- updated parameters

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `lines` | number | 50 | Max entries to return (tail) |
| `level` | string | null | Filter by level: `"error"`, `"warn"`, `"error,warn"` etc. Comma-separated, case-insensitive |
| `pattern` | string | null | Regex on `message` field. Uses Rust `regex::Regex` crate. Invalid patterns return an error response. |
| `filter` | string | null | Plain substring match on `message` (backward compat). If both `filter` and `pattern` are set, `pattern` wins. |
| `windowId` | string | `"main"` | Filter by window_id field |

Filter pipeline order: level check (cheapest) -> regex/substring on message -> window_id match -> take last N.

### `clear_logs` -- new tool

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `source` | string | `"all"` | Which log to clear: `"console"`, `"ipc"`, `"events"`, `"all"` |

Acquires the corresponding writer mutex(es), truncates the file(s) via `file.set_len(0)` + seek to start, then releases. This guarantees no partial writes during truncation. Returns `{ cleared: true, source: "all" }`.

### `read_log_file` -- new tool

For reading historical logs even after app restart.

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `source` | string | required | Which log: `"console"`, `"ipc"`, `"events"` |
| `lines` | number | 100 | Max entries from tail |
| `level` | string | null | Level filter (console only). Silently ignored for `"ipc"` and `"events"` sources. |
| `pattern` | string | null | Regex on any field (serialized line) |
| `since` | number (u64) | null | Timestamp floor -- only entries after this epoch ms. Deserialized as `u64` in Rust (not `f64`) to avoid floating-point precision loss. |
| `windowId` | string | null | Filter by window_id (console and events only). Silently ignored for `"ipc"` source. |

Returns `{ source, count, entries: [...] }`.

---

## 3. Event Monitoring -- IPC capture + `ipc_listen`

### Wire up IPC interception

In `bridge.rs` init JS, wrap `window.__TAURI_INTERNALS__.invoke()`:

```javascript
const _origInvoke = window.__TAURI_INTERNALS__.invoke;
window.__TAURI_INTERNALS__.invoke = async function(cmd, args, options) {
  const t0 = Date.now();
  try {
    const result = await _origInvoke.call(this, cmd, args, options);
    if (window.__CONNECTOR_IPC_MONITOR__) {
      _origInvoke.call(this, 'plugin:connector|push_ipc_event', {
        payload: { command: cmd, args: args || {}, timestamp: t0, durationMs: Date.now() - t0 }
      });
    }
    return result;
  } catch(e) {
    if (window.__CONNECTOR_IPC_MONITOR__) {
      _origInvoke.call(this, 'plugin:connector|push_ipc_event', {
        payload: { command: cmd, args: args || {}, timestamp: t0, durationMs: Date.now() - t0, error: e.message }
      });
    }
    throw e;
  }
};
```

Note: Uses `_origInvoke` for the push call to avoid infinite recursion. Excludes `plugin:connector|*` commands to avoid self-monitoring noise.

- `push_ipc_event` handler: appends to `{log_dir}/ipc.log`
- `ipc_monitor start`: sets `window.__CONNECTOR_IPC_MONITOR__ = true` via bridge JS + flips Rust `ipc_monitor_active` flag
- `ipc_monitor stop`: unsets both
- Remove `#[allow(dead_code)]` from `push_ipc_event` and `is_ipc_monitoring`

### `ipc_get_captured` -- updated parameters

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `filter` | string | null | Substring on command (backward compat) |
| `pattern` | string | null | Regex across the full serialized entry |
| `limit` | number | 100 | Max entries |
| `since` | number | null | Timestamp floor |

Reads from `ipc.log` file with filtering.

### `ipc_listen` -- new tool

Registers JS `listen()` for specific Tauri event names and buffers captures.

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `action` | string | required | `"start"` or `"stop"` |
| `events` | string[] | required for start | Event names to listen for, e.g. `["user:login", "appointment:update"]` |

**Start**: Injects JS via bridge that calls `window.__TAURI__.event.listen(eventName, callback)` for each event. Callback calls `_origInvoke('plugin:connector|push_event', { payload: { event, payload, timestamp, window_id } })`. Stores unlisten handles on `window.__CONNECTOR_EVENT_LISTENERS__`.

**Stop**: Injects JS via bridge that calls all unlisten functions from `window.__CONNECTOR_EVENT_LISTENERS__`, clears that object, then clears `event_listeners` in Rust state. If the webview has reloaded (e.g., after HMR or app restart), the JS unlisten handles are already gone -- the injected stop script is a no-op on the JS side, and we just clear the Rust state.

**Double-start semantics**: Calling `start` with new events accumulates -- existing listeners are kept and new ones are added. Duplicate event names are deduplicated (no double-registration). Calling `start` with an already-listened event is a no-op for that event.

### `event_get_captured` -- new tool (renamed from `event_get_captured` for consistency with `ipc_get_captured`)

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `event` | string | null | Filter by event name (exact match) |
| `pattern` | string | null | Regex across the full serialized entry |
| `limit` | number | 100 | Max entries |
| `since` | number | null | Timestamp floor |

Reads from `events.log` file with filtering. Returns `{ count, entries: [...] }`.

Note on `pattern` semantics: For both `ipc_get_captured` and `event_get_captured`, the `pattern` regex is matched against the full serialized JSONL line. This is intentionally broader than `filter` (which matches command/event name only) -- it lets the LLM search across args, payloads, error messages, and any field. The `filter` param remains command/event-name-scoped for backward compatibility.

---

## 4. DOM Regex Search

### `webview_find_element` -- new `regex` strategy

Add `"regex"` as fourth strategy alongside `"css"`, `"xpath"`, `"text"`.

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `selector` | string | required | Regex pattern (when strategy is "regex") |
| `strategy` | string | `"css"` | `"css"`, `"xpath"`, `"text"`, `"regex"` |
| `target` | string | `"text"` | What regex matches: `"text"`, `"class"`, `"id"`, `"attr"`, `"all"` |
| `windowId` | string | `"main"` | |

JS implementation uses `document.createTreeWalker` + `new RegExp(selector, 'i')`. Matches against the specified `target` field of each element. Returns same shape as existing strategies: `[{ tag, id, className, text, rect, visible }]`.

`target` meanings:
- `"text"`: match against `el.textContent.trim()`
- `"class"`: match against `el.className`
- `"id"`: match against `el.id`
- `"attr"`: match against space-separated `name=value` pairs (e.g., `"id=patient_123 aria-live=true lang=en"`)
- `"all"`: match against `el.outerHTML`

### `webview_search_snapshot` -- new tool

Searches the rendered AI snapshot string with regex, returning matched lines with context.

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `pattern` | string | required | Regex pattern |
| `context` | number | 2 | Lines of context before/after each match |
| `mode` | string | `"ai"` | Snapshot mode to search |
| `windowId` | string | `"main"` | |

**How it works:**
1. If a cached snapshot exists in `PluginState.dom_cache` with matching mode and is less than 10s old, use it. Otherwise take a fresh snapshot via `window.__CONNECTOR_SNAPSHOT__({ mode })`.
2. Splits snapshot string by newline
3. Applies `new RegExp(pattern, 'gi')` line by line
4. Returns matches with surrounding context lines and line numbers. Adjacent/overlapping context windows are merged into a single block (grep-style).

`context` is capped at 10 (values above 10 are silently clamped).

Returns:
```json
{
  "matches": [
    { "line": 42, "content": "- textbox \"patient_123\" [ref=e16]", "context": ["...", "..."] }
  ],
  "total": 3,
  "pattern": "patient_\\d+"
}
```

---

## 5. CLI Updates

### `logs` command

Add flags:
- `--level <level>` / `-l <level>`: filter by log level
- `--pattern <regex>` / `-p <regex>`: regex filter on message

### New `events` subcommand

```bash
tauri-connector events listen user:login,appointment:update   # start listening
tauri-connector events captured [-p <regex>] [--since <ts>]   # get captured events
tauri-connector events stop                                    # stop listening
```

### `ipc captured` updates

Add flags:
- `--pattern <regex>` / `-p <regex>`: regex filter
- `--since <timestamp>`: timestamp floor

### `clear` subcommand

```bash
tauri-connector clear logs          # clear console.log
tauri-connector clear ipc           # clear ipc.log
tauri-connector clear events        # clear events.log
tauri-connector clear all           # clear all
```

---

## 6. Documentation Updates

### `skill/SKILL.md`

- New "Debugging Workflow" section with step-by-step recipe
- Updated tool parameter tables for `read_logs`, `ipc_get_captured`
- New tool docs: `clear_logs`, `read_log_file`, `ipc_listen`, `event_get_captured`, `webview_search_snapshot`
- Updated `webview_find_element` with `regex` strategy and `target` param

### `README.md`

- Tool table: add 5 new tools (20 -> 25), update existing descriptions
- WS Command Reference: add new WS message types
- Update "Console Log Capture" section for file-backed storage
- CLI section: add new flags and subcommands
- Architecture diagram: note file persistence path

### `skill/scripts/`

- `logs.ts`: add `--level` and `--pattern` params to WS request
- New `events.ts`: bun script for event listening and captured event retrieval

---

## File Change Inventory

| File | Changes |
|------|---------|
| `plugin/Cargo.toml` | Add `regex = { version = "1", default-features = false, features = ["std", "perf"] }` |
| `plugin/src/state.rs` | Remove `log_cache`, `ipc_events` deques; add `log_dir`, `event_listeners`, file I/O helpers (`append_jsonl`, `read_jsonl_filtered`) |
| `plugin/src/handlers.rs` | Update `console_logs`, `ipc_get_captured`; add `clear_logs`, `read_log_file`, `ipc_listen`, `event_get_captured`, `push_event`, `search_snapshot` |
| `plugin/src/mcp_tools.rs` | Register 5 new tools, update schemas with `level`/`pattern`/`since`/`target` params |
| `plugin/src/bridge.rs` | IPC invoke wrapper JS, event listener injection JS, `push_event` IPC command |
| `plugin/src/lib.rs` | Init `log_dir` from `app.path().app_data_dir()`, open writer files, register new IPC commands (`push_ipc_event`, `push_event`) in `invoke_handler![]`, add `log_dir` to `.connector.json` PID file |
| `plugin/src/protocol.rs` | New message types: `ClearLogs`, `ReadLogFile`, `IpcListen`, `GetCapturedEvents`, `SearchSnapshot` |
| `plugin/src/server.rs` | Route new WS message types to handlers |
| `crates/mcp-server/src/tools.rs` | Mirror all tool schema updates and new tool definitions |
| `crates/cli/src/main.rs` | Add `events` and `clear` subcommands to clap |
| `crates/cli/src/commands.rs` | `--level`, `--pattern` on `logs`; new `events`, `clear` command implementations |
| `skill/SKILL.md` | New tool docs, debugging workflow, updated parameter tables |
| `skill/scripts/logs.ts` | Add level + pattern params |
| `skill/scripts/events.ts` | New bun script for event operations |
| `README.md` | Tool table, WS ref, architecture, CLI examples |

## Backward Compatibility

- `filter` param on `read_logs` and `ipc_get_captured` still works (substring match). `pattern` takes precedence when both set.
- Existing WS message types (`console_logs`, `ipc_get_captured`, `ipc_monitor`, `ipc_emit_event`) still work with existing params.
- In-memory reads are replaced by file reads -- callers see the same response shape, just sourced from disk instead of memory.
