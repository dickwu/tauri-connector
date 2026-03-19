# Debug Suite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace in-memory log/IPC buffers with file-backed JSONL, add regex + level filtering, wire up IPC/event capture, add DOM regex search — 5 new MCP tools.

**Architecture:** File-backed `Mutex<BufWriter<File>>` writers replace `VecDeque` buffers. Regex filtering via `regex` crate (Rust) and `RegExp` (JS). IPC interception wraps `__TAURI_INTERNALS__.invoke()`. Event listeners registered via bridge JS.

**Tech Stack:** Rust (regex, serde_json, std::io), Tauri v2, JavaScript (injected bridge code), clap (CLI)

**Spec:** `docs/superpowers/specs/2026-03-19-debug-suite-design.md`

---

## File Structure

| File | Role | Action |
|------|------|--------|
| `plugin/Cargo.toml` | Dependencies | Modify: add `regex` |
| `plugin/src/state.rs` | Plugin state + file I/O | Modify: replace deques with file writers, add helpers |
| `plugin/src/handlers.rs` | All command handlers | Modify: update existing, add 6 new handlers |
| `plugin/src/mcp_tools.rs` | MCP tool defs + dispatch | Modify: update schemas, add 5 new tools |
| `plugin/src/protocol.rs` | WS message types | Modify: add 5 new Command variants |
| `plugin/src/server.rs` | WS routing | Modify: route new commands |
| `plugin/src/bridge.rs` | JS injection | Modify: IPC wrapper, event listener scripts |
| `plugin/src/lib.rs` | Plugin init + IPC commands | Modify: init log_dir, register commands, update PID file |
| `crates/mcp-server/src/tools.rs` | Standalone MCP tool defs | Modify: mirror tool schema updates |
| `crates/cli/src/main.rs` | CLI entry + clap defs | Modify: add events/clear subcommands, log flags |
| `crates/cli/src/commands.rs` | CLI command impls | Modify: update logs, add events/clear |
| `skill/scripts/logs.ts` | Bun log script | Modify: add level/pattern params |
| `skill/scripts/events.ts` | Bun event script | Create |
| `skill/SKILL.md` | Usage guide | Modify: add new tool docs |
| `README.md` | Project readme | Modify: update tables and examples |

---

### Task 1: Add regex dependency and update state.rs (file-backed storage)

**Files:**
- Modify: `plugin/Cargo.toml:28-42`
- Modify: `plugin/src/state.rs` (full rewrite of storage layer)

- [ ] **Step 1: Add regex crate to Cargo.toml**

In `plugin/Cargo.toml`, add after the `axum` line (line 42):

```toml
regex = { version = "1", default-features = false, features = ["std", "perf"] }
```

- [ ] **Step 2: Rewrite state.rs — replace VecDeque with file writers**

Replace `plugin/src/state.rs` contents. Key changes:
- Remove `log_cache: Arc<Mutex<VecDeque<LogEntry>>>` and `ipc_events: Arc<Mutex<VecDeque<IpcEvent>>>`
- Remove `MAX_LOG_ENTRIES` and `MAX_IPC_EVENTS` constants
- Add `log_dir: PathBuf`, `console_writer: Arc<Mutex<BufWriter<File>>>`, `ipc_writer: Arc<Mutex<BufWriter<File>>>`, `event_writer: Arc<Mutex<BufWriter<File>>>`, `event_listeners: Arc<Mutex<Vec<String>>>`
- Add `pub error: Option<String>` to `IpcEvent` struct (for capturing IPC errors during monitoring)
- Add `EventEntry` struct (event, payload, timestamp, window_id)
- Add `PluginState::new(log_dir: PathBuf)` constructor that creates the dir and opens files
- Add `append_jsonl<T: Serialize>(&self, writer: &Mutex<BufWriter<File>>, entry: &T)` helper
- Add `read_jsonl_filtered<T: DeserializeOwned>(path: &Path, filter_fn, limit) -> Vec<T>` helper
- Update `push_logs()` to write JSONL to console_writer
- Remove `get_logs()` — replaced by file-reading in handlers
- Update `push_ipc_event()` — remove `#[allow(dead_code)]`, write to ipc_writer
- Remove `get_ipc_events()` — replaced by file-reading in handlers
- Add `push_event(entry: EventEntry)` — write to event_writer
- Add `clear_file(writer: &Mutex<BufWriter<File>>)` — truncate via `set_len(0)` + seek
- Keep `push_dom`, `get_dom`, `set_ipc_monitoring`, `is_ipc_monitoring`, `set_pointed_element`, `take_pointed_element` unchanged
- Keep `LogEntry`, `IpcEvent`, `DomEntry`, `RefEntry`, `SnapshotMeta`, `RefMap` structs (LogEntry and IpcEvent still used as serialization types)

```rust
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
```

Note: `Seek` and `SeekFrom` are imported in `handlers.rs` (where `clear_logs` lives), not here.

The `PluginState::new()` constructor:

```rust
pub fn new(log_dir: PathBuf) -> Result<Self, String> {
    fs::create_dir_all(&log_dir)
        .map_err(|e| format!("Failed to create log dir: {e}"))?;

    let open = |name: &str| -> Result<Arc<Mutex<BufWriter<File>>>, String> {
        let path = log_dir.join(name);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Failed to open {name}: {e}"))?;
        Ok(Arc::new(Mutex::new(BufWriter::new(file))))
    };

    Ok(Self {
        dom_cache: Arc::new(Mutex::new(HashMap::new())),
        log_dir,
        console_writer: open("console.log")?,
        ipc_writer: open("ipc.log")?,
        event_writer: open("events.log")?,
        ipc_monitor_active: Arc::new(Mutex::new(false)),
        event_listeners: Arc::new(Mutex::new(Vec::new())),
        pointed_element: Arc::new(Mutex::new(None)),
    })
}
```

The `push_logs` method writes JSONL:

```rust
pub async fn push_logs(&self, entries: Vec<LogEntry>) {
    let mut writer = self.console_writer.lock().await;
    for entry in &entries {
        if let Ok(line) = serde_json::to_string(entry) {
            let _ = writeln!(writer, "{line}");
        }
    }
    let _ = writer.flush();
}
```

The `read_jsonl_filtered` function:

```rust
pub fn read_jsonl_filtered<T: DeserializeOwned>(
    path: &Path,
    filter_fn: impl Fn(&str) -> bool,
    limit: usize,
) -> Vec<T> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(), // file-not-found → empty
    };
    let reader = BufReader::new(file);
    let mut matched: Vec<T> = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() { continue; }
        if !filter_fn(&line) { continue; }
        if let Ok(entry) = serde_json::from_str::<T>(&line) {
            matched.push(entry);
        }
    }
    // Take last N (tail)
    if matched.len() > limit {
        matched.drain(..matched.len() - limit);
    }
    matched
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -30`
Expected: Compilation errors in `handlers.rs` and `lib.rs` due to removed methods (expected — we fix those in next tasks)

- [ ] **Step 4: Commit**

```bash
git add plugin/Cargo.toml plugin/src/state.rs
git commit -m "refactor(state): replace in-memory deques with file-backed JSONL writers"
```

---

### Task 2: Update lib.rs — init log_dir, register new IPC commands, update PID file

**Files:**
- Modify: `plugin/src/lib.rs:29-393`

- [ ] **Step 1: Add new IPC payload structs and commands**

After `PointedElementPayload` (line 145), add:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushIpcEventPayload {
    command: String,
    #[serde(default)]
    args: serde_json::Value,
    timestamp: u64,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

#[tauri::command]
async fn push_ipc_event(
    app: AppHandle,
    payload: PushIpcEventPayload,
) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state.push_ipc_event(state::IpcEvent {
        command: payload.command,
        args: payload.args,
        timestamp: payload.timestamp,
        duration_ms: payload.duration_ms,
        error: payload.error,
    }).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushEventPayload {
    event: String,
    #[serde(default)]
    payload: serde_json::Value,
    timestamp: u64,
    #[serde(default = "default_main")]
    window_id: String,
}

#[tauri::command]
async fn push_event(
    app: AppHandle,
    payload: PushEventPayload,
) -> Result<(), String> {
    let state = app.state::<PluginState>();
    state.push_event(state::EventEntry {
        event: payload.event,
        payload: payload.payload,
        timestamp: payload.timestamp,
        window_id: payload.window_id,
    }).await;
    Ok(())
}
```

- [ ] **Step 2: Update invoke_handler to register new commands**

Change `lib.rs:226-230`:

```rust
.invoke_handler(tauri::generate_handler![
    push_dom,
    push_logs,
    set_pointed_element,
    push_ipc_event,
    push_event,
])
```

- [ ] **Step 3: Init PluginState with log_dir instead of Default**

In the `.setup()` closure (line 232), replace `PluginState::default()` with:

```rust
let log_dir = app.path().app_data_dir()
    .unwrap_or_else(|_| std::env::temp_dir())
    .join(".tauri-connector");

let plugin_state = match PluginState::new(log_dir.clone()) {
    Ok(s) => s,
    Err(e) => {
        eprintln!("[connector] Failed to init log dir: {e}");
        PluginState::new(std::env::temp_dir().join(".tauri-connector"))
            .expect("temp dir should work")
    }
};
```

The `app` in the setup closure is `&AppHandle` — `app.path()` is available via `Manager` trait.

- [ ] **Step 4: Add log_dir to PID file JSON**

In `write_pid_file()`, add a `log_dir` parameter and include it in the JSON output:

```rust
fn write_pid_file(
    ws_port: u16,
    mcp_port: Option<u16>,
    bridge_port: u16,
    app_name: &str,
    app_id: &str,
    log_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
```

Add to the `serde_json::json!` block:
```rust
"log_dir": log_dir.to_string_lossy(),
```

Update the call site to pass `&log_dir`.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -30`
Expected: Remaining errors in handlers.rs (signatures changed) — fixed in Task 3.

- [ ] **Step 6: Commit**

```bash
git add plugin/src/lib.rs
git commit -m "feat(lib): init file-backed log dir, register push_ipc_event and push_event commands"
```

---

### Task 3: Update handlers.rs — rewrite console_logs, ipc_get_captured, add new handlers

**Files:**
- Modify: `plugin/src/handlers.rs:625-696`

- [ ] **Step 1: Add regex import at top of handlers.rs**

```rust
use std::io::{Seek, SeekFrom};
use regex::Regex;
use crate::state::EventEntry;
```

- [ ] **Step 2: Rewrite `console_logs` handler (line 681)**

Replace the current `console_logs` function with file-backed read + regex/level filtering:

```rust
pub async fn console_logs(
    id: &str,
    lines: usize,
    filter: Option<&str>,
    pattern: Option<&str>,
    level: Option<&str>,
    window_id: &str,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("console.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let levels: Option<Vec<String>> = level.map(|l| {
        l.split(',').map(|s| s.trim().to_lowercase()).collect()
    });
    let filter_lower = filter.map(|f| f.to_lowercase());

    let wid = window_id.to_string();
    let _writer = state.console_writer.lock().await; // prevent concurrent writes
    let entries = crate::state::read_jsonl_filtered::<crate::state::LogEntry>(
        &path,
        |line| {
            // Level filter (cheapest)
            if let Some(ref lvls) = levels {
                let has_level = lvls.iter().any(|l| {
                    line.contains(&format!("\"level\":\"{}\"", l))
                });
                if !has_level { return false; }
            }
            // Window ID filter (on raw string, before deserialization)
            if !line.contains(&format!("\"window_id\":\"{}\"", wid)) {
                return false;
            }
            // Regex or substring on message
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            if let Some(ref f) = filter_lower {
                return line.to_lowercase().contains(f);
            }
            true
        },
        lines,
    );
    drop(_writer);

    match serde_json::to_value(&entries) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": entries.len(), "logs": v }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}
```

- [ ] **Step 3: Rewrite `ipc_get_captured` handler (line 643)**

Replace with file-backed read + regex/filter/since:

```rust
pub async fn ipc_get_captured(
    id: &str,
    filter: Option<&str>,
    pattern: Option<&str>,
    limit: usize,
    since: Option<u64>,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("ipc.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let filter_lower = filter.map(|f| f.to_lowercase());

    let _writer = state.ipc_writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<crate::state::IpcEvent>(
        &path,
        |line| {
            if let Some(ts) = since {
                // Quick check: extract timestamp from line
                if let Some(pos) = line.find("\"timestamp\":") {
                    let rest = &line[pos + 12..];
                    if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next() {
                        if let Ok(t) = val.parse::<u64>() {
                            if t < ts { return false; }
                        }
                    }
                }
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            if let Some(ref f) = filter_lower {
                return line.to_lowercase().contains(f);
            }
            true
        },
        limit,
    );
    drop(_writer);

    match serde_json::to_value(&entries) {
        Ok(v) => Response::success(
            id.to_string(),
            serde_json::json!({ "count": entries.len(), "events": v }),
        ),
        Err(e) => Response::error(id.to_string(), format!("Serialization error: {e}")),
    }
}
```

- [ ] **Step 4: Add `clear_logs` handler**

```rust
pub async fn clear_logs(
    id: &str,
    source: &str,
    state: &PluginState,
) -> Response {
    let clear = |writer: &Arc<tokio::sync::Mutex<std::io::BufWriter<std::fs::File>>>| async {
        let mut w = writer.lock().await;
        let _ = w.flush(); // flush stale buffer BEFORE truncating
        let file = w.get_mut();
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
    };

    match source {
        "console" => clear(&state.console_writer).await,
        "ipc" => clear(&state.ipc_writer).await,
        "events" => clear(&state.event_writer).await,
        "all" => {
            clear(&state.console_writer).await;
            clear(&state.ipc_writer).await;
            clear(&state.event_writer).await;
        }
        _ => return Response::error(id.to_string(), format!("Unknown source: {source}. Use: console, ipc, events, all")),
    }

    Response::success(id.to_string(), serde_json::json!({ "cleared": true, "source": source }))
}
```

- [ ] **Step 5: Add `read_log_file` handler**

```rust
pub async fn read_log_file(
    id: &str,
    source: &str,
    lines: usize,
    level: Option<&str>,
    pattern: Option<&str>,
    since: Option<u64>,
    window_id: Option<&str>,
    state: &PluginState,
) -> Response {
    let (path, writer) = match source {
        "console" => (state.log_dir.join("console.log"), &state.console_writer),
        "ipc" => (state.log_dir.join("ipc.log"), &state.ipc_writer),
        "events" => (state.log_dir.join("events.log"), &state.event_writer),
        _ => return Response::error(id.to_string(), format!("Unknown source: {source}")),
    };

    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };
    let levels: Option<Vec<String>> = if source == "console" {
        level.map(|l| l.split(',').map(|s| s.trim().to_lowercase()).collect())
    } else {
        None
    };

    let _w = writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<serde_json::Value>(
        &path,
        |line| {
            if let Some(ts) = since {
                if let Some(pos) = line.find("\"timestamp\":") {
                    let rest = &line[pos + 12..];
                    if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next() {
                        if let Ok(t) = val.parse::<u64>() {
                            if t < ts { return false; }
                        }
                    }
                }
            }
            if let Some(ref lvls) = levels {
                let has_level = lvls.iter().any(|l| {
                    line.contains(&format!("\"level\":\"{}\"", l))
                });
                if !has_level { return false; }
            }
            if let Some(ref wid) = window_id {
                if source != "ipc" && !line.contains(&format!("\"window_id\":\"{}\"", wid)) {
                    return false;
                }
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            true
        },
        lines,
    );
    drop(_w);

    Response::success(
        id.to_string(),
        serde_json::json!({ "source": source, "count": entries.len(), "entries": entries }),
    )
}
```

- [ ] **Step 6: Add `ipc_listen` handler**

```rust
pub async fn ipc_listen(
    id: &str,
    action: &str,
    events: Option<&[String]>,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    match action {
        "start" => {
            let Some(event_names) = events else {
                return Response::error(id.to_string(), "events parameter required for start");
            };
            let mut listeners = state.event_listeners.lock().await;
            let new_events: Vec<&String> = event_names.iter()
                .filter(|e| !listeners.contains(e))
                .collect();

            if new_events.is_empty() {
                return Response::success(id.to_string(), serde_json::json!({
                    "listening": *listeners,
                    "added": 0,
                }));
            }

            let events_js: Vec<String> = new_events.iter().map(|e| {
                format!(
                    "window.__TAURI__.event.listen('{}', function(ev) {{\
                        var ipc = window.__CONNECTOR_ORIG_INVOKE__ || window.__TAURI_INTERNALS__.invoke;\
                        ipc('plugin:connector|push_event', {{\
                            payload: {{ event: '{}', payload: ev.payload, timestamp: Date.now(), windowId: ev.windowLabel || 'main' }}\
                        }}).catch(function(){{}});\
                    }}).then(function(unlisten) {{\
                        window.__CONNECTOR_EVENT_LISTENERS__ = window.__CONNECTOR_EVENT_LISTENERS__ || {{}};\
                        window.__CONNECTOR_EVENT_LISTENERS__['{}'] = unlisten;\
                    }});",
                    e, e, e
                )
            }).collect();

            let script = events_js.join("\n");
            match bridge.execute_js(&script, 5_000).await {
                Ok(_) => {
                    for e in &new_events {
                        listeners.push((*e).clone());
                    }
                    Response::success(id.to_string(), serde_json::json!({
                        "listening": *listeners,
                        "added": new_events.len(),
                    }))
                }
                Err(e) => Response::error(id.to_string(), format!("Failed to register listeners: {e}")),
            }
        }
        "stop" => {
            let script = r#"(function() {
                var listeners = window.__CONNECTOR_EVENT_LISTENERS__ || {};
                Object.values(listeners).forEach(function(unlisten) {
                    if (typeof unlisten === 'function') unlisten();
                });
                window.__CONNECTOR_EVENT_LISTENERS__ = {};
            })()"#;
            let _ = bridge.execute_js(script, 5_000).await;

            let mut listeners = state.event_listeners.lock().await;
            listeners.clear();
            Response::success(id.to_string(), serde_json::json!({ "listening": [], "stopped": true }))
        }
        _ => Response::error(id.to_string(), format!("Unknown action: {action}. Use: start, stop")),
    }
}
```

- [ ] **Step 7: Add `event_get_captured` handler**

```rust
pub async fn event_get_captured(
    id: &str,
    event: Option<&str>,
    pattern: Option<&str>,
    limit: usize,
    since: Option<u64>,
    state: &PluginState,
) -> Response {
    let path = state.log_dir.join("events.log");
    let re = match pattern {
        Some(p) => match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
        },
        None => None,
    };

    let _w = state.event_writer.lock().await;
    let entries = crate::state::read_jsonl_filtered::<serde_json::Value>(
        &path,
        |line| {
            if let Some(ts) = since {
                if let Some(pos) = line.find("\"timestamp\":") {
                    let rest = &line[pos + 12..];
                    if let Some(val) = rest.split(|c: char| !c.is_ascii_digit()).next() {
                        if let Ok(t) = val.parse::<u64>() {
                            if t < ts { return false; }
                        }
                    }
                }
            }
            if let Some(ev) = event {
                if !line.contains(&format!("\"event\":\"{}\"", ev)) {
                    return false;
                }
            }
            if let Some(ref re) = re {
                return re.is_match(line);
            }
            true
        },
        limit,
    );
    drop(_w);

    Response::success(
        id.to_string(),
        serde_json::json!({ "count": entries.len(), "entries": entries }),
    )
}
```

- [ ] **Step 8: Add `search_snapshot` handler**

```rust
pub async fn search_snapshot(
    id: &str,
    pattern: &str,
    context: usize,
    mode: &str,
    window_id: &str,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    let context = context.min(10); // cap at 10

    // Check cached snapshot first (< 10s old)
    let snapshot_text = {
        let cache = state.dom_cache.lock().await;
        if let Some(entry) = cache.get(window_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if entry.snapshot_mode == mode && now - entry.timestamp < 10 {
                Some(entry.snapshot.clone())
            } else {
                None
            }
        } else {
            None
        }
    };

    let snapshot = match snapshot_text {
        Some(s) => s,
        None => {
            let script = format!(
                "JSON.stringify(window.__CONNECTOR_SNAPSHOT__({{ mode: '{}' }}).snapshot)",
                mode
            );
            match bridge.execute_js(&script, 15_000).await {
                Ok(val) => {
                    let s = val.as_str().unwrap_or("").to_string();
                    if s.is_empty() {
                        return Response::error(id.to_string(), "Snapshot returned empty — page may still be loading");
                    }
                    s
                }
                Err(e) => return Response::error(id.to_string(), format!("Snapshot failed: {e}")),
            }
        }
    };

    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return Response::error(id.to_string(), format!("Invalid regex: {e}")),
    };

    let lines: Vec<&str> = snapshot.lines().collect();
    let mut matches: Vec<serde_json::Value> = Vec::new();
    let mut last_end: usize = 0; // for merging overlapping contexts

    for (i, line) in lines.iter().enumerate() {
        if re.is_match(line) {
            let ctx_start = i.saturating_sub(context);
            let ctx_end = (i + context + 1).min(lines.len());

            // Merge with previous if overlapping
            let actual_start = if ctx_start < last_end { last_end } else { ctx_start };
            last_end = ctx_end;

            if actual_start < ctx_end {
                let ctx_lines: Vec<&str> = lines[actual_start..ctx_end].to_vec();
                matches.push(serde_json::json!({
                    "line": i + 1,
                    "content": line,
                    "context": ctx_lines,
                }));
            } else {
                // Fully overlapping with previous — just add the match line
                matches.push(serde_json::json!({
                    "line": i + 1,
                    "content": line,
                    "context": [],
                }));
            }
        }
    }

    Response::success(id.to_string(), serde_json::json!({
        "matches": matches,
        "total": matches.len(),
        "pattern": pattern,
    }))
}
```

- [ ] **Step 9: Update `ipc_monitor` handler to set JS flag via bridge**

Update `ipc_monitor` (line 625) to also inject JS:

```rust
pub async fn ipc_monitor(
    id: &str,
    action: &str,
    state: &PluginState,
    bridge: &crate::bridge::Bridge,
) -> Response {
    match action {
        "start" => {
            state.set_ipc_monitoring(true).await;
            let _ = bridge.execute_js("window.__CONNECTOR_IPC_MONITOR__ = true", 2_000).await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": true }))
        }
        "stop" => {
            state.set_ipc_monitoring(false).await;
            let _ = bridge.execute_js("window.__CONNECTOR_IPC_MONITOR__ = false", 2_000).await;
            Response::success(id.to_string(), serde_json::json!({ "monitoring": false }))
        }
        _ => Response::error(id.to_string(), format!("Unknown action: {action}. Use: start, stop")),
    }
}
```

- [ ] **Step 10: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -40`
Expected: Errors in mcp_tools.rs/server.rs due to changed handler signatures — fixed in Tasks 4-5.

- [ ] **Step 11: Commit**

```bash
git add plugin/src/handlers.rs
git commit -m "feat(handlers): file-backed log read/write, regex filtering, 6 new handlers"
```

---

### Task 4: Update protocol.rs and server.rs — new WS message types

**Files:**
- Modify: `plugin/src/protocol.rs:155-163`
- Modify: `plugin/src/server.rs:118-196`

- [ ] **Step 1: Add new Command variants to protocol.rs**

After `ConsoleLogs` (line 156), add:

```rust
    // --- New Debug Suite Commands ---
    ClearLogs {
        #[serde(default = "default_all")]
        source: String,
    },
    ReadLogFile {
        source: String,
        #[serde(default = "default_read_lines")]
        lines: usize,
        #[serde(default)]
        level: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        window_id: Option<String>,
    },
    IpcListen {
        action: String,
        #[serde(default)]
        events: Option<Vec<String>>,
    },
    EventGetCaptured {
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default = "default_ipc_limit")]
        limit: usize,
        #[serde(default)]
        since: Option<u64>,
    },
    SearchSnapshot {
        pattern: String,
        #[serde(default = "default_context")]
        context: usize,
        #[serde(default = "default_snapshot_type")]
        mode: String,
        #[serde(default = "default_window")]
        window_id: String,
    },
```

Add new default functions:

```rust
fn default_all() -> String { "all".to_string() }
fn default_read_lines() -> usize { 100 }
fn default_context() -> usize { 2 }
```

Also update `ConsoleLogs` to add `level` and `pattern` fields:

```rust
ConsoleLogs {
    #[serde(default = "default_lines")]
    lines: usize,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default = "default_window")]
    window_id: String,
},
```

And update `IpcGetCaptured` to add `pattern` and `since`:

```rust
IpcGetCaptured {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default = "default_ipc_limit")]
    limit: usize,
    #[serde(default)]
    since: Option<u64>,
},
```

Update `FindElement` to add `target`:

```rust
FindElement {
    selector: String,
    #[serde(default = "default_strategy")]
    strategy: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default = "default_window")]
    window_id: String,
},
```

- [ ] **Step 2: Update server.rs handle_command to route new variants**

Update `ConsoleLogs` match arm (line 192):

```rust
Command::ConsoleLogs { lines, filter, level, pattern, window_id } => {
    handlers::console_logs(&id, lines, filter.as_deref(), pattern.as_deref(), level.as_deref(), &window_id, state).await
}
```

Update `IpcGetCaptured` match arm (line 184):

```rust
Command::IpcGetCaptured { filter, pattern, limit, since } => {
    handlers::ipc_get_captured(&id, filter.as_deref(), pattern.as_deref(), limit, since, state).await
}
```

Update `IpcMonitor` match arm (line 183) — now takes bridge:

```rust
Command::IpcMonitor { action } => handlers::ipc_monitor(&id, &action, state, bridge).await,
```

Add new match arms:

```rust
// Debug Suite
Command::ClearLogs { source } => {
    handlers::clear_logs(&id, &source, state).await
}
Command::ReadLogFile { source, lines, level, pattern, since, window_id } => {
    handlers::read_log_file(&id, &source, lines, level.as_deref(), pattern.as_deref(), since, window_id.as_deref(), state).await
}
Command::IpcListen { action, events } => {
    handlers::ipc_listen(&id, &action, events.as_deref(), state, bridge).await
}
Command::EventGetCaptured { event, pattern, limit, since } => {
    handlers::event_get_captured(&id, event.as_deref(), pattern.as_deref(), limit, since, state).await
}
Command::SearchSnapshot { pattern, context, mode, window_id } => {
    handlers::search_snapshot(&id, &pattern, context, &mode, &window_id, state, bridge).await
}
```

Note: `handle_command` and `Server::run` need to pass `bridge` to `handle_command`. Update the function signature and call site.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -30`

- [ ] **Step 4: Commit**

```bash
git add plugin/src/protocol.rs plugin/src/server.rs
git commit -m "feat(protocol+server): add 5 new WS command types, update existing with regex/level"
```

---

### Task 5: Update mcp_tools.rs — register new tools, update schemas

**Files:**
- Modify: `plugin/src/mcp_tools.rs:63-420`

- [ ] **Step 1: Update `call_tool` dispatch**

Update `"read_logs"` match arm (line 235):

```rust
"read_logs" => {
    let lines = num_arg(args, "lines").unwrap_or(50.0) as usize;
    let filter = str_arg(args, "filter");
    let pattern = str_arg(args, "pattern");
    let level = str_arg(args, "level");
    let wid = window_id(args);
    handlers::console_logs(id, lines, filter.as_deref(), pattern.as_deref(), level.as_deref(), &wid, state).await
}
```

Update `"ipc_monitor"` (line 218) — pass bridge:

```rust
"ipc_monitor" => {
    let action = str_arg(args, "action").unwrap_or_default();
    handlers::ipc_monitor(id, &action, state, bridge).await
}
```

Update `"ipc_get_captured"` (line 223):

```rust
"ipc_get_captured" => {
    let filter = str_arg(args, "filter");
    let pattern = str_arg(args, "pattern");
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let since = num_arg(args, "since").map(|n| n as u64);
    handlers::ipc_get_captured(id, filter.as_deref(), pattern.as_deref(), limit, since, state).await
}
```

Update `"webview_find_element"` (line 116) — pass target:

```rust
"webview_find_element" => {
    let selector = str_arg(args, "selector").unwrap_or_default();
    let strategy = str_arg(args, "strategy").unwrap_or_else(|| "css".to_string());
    let target = str_arg(args, "target");
    let wid = window_id(args);
    handlers::find_element(id, &selector, &strategy, target.as_deref(), &wid, bridge).await
}
```

Add 5 new dispatch arms before the `_ =>` fallback:

```rust
"clear_logs" => {
    let source = str_arg(args, "source").unwrap_or_else(|| "all".to_string());
    handlers::clear_logs(id, &source, state).await
}

"read_log_file" => {
    let source = str_arg(args, "source").unwrap_or_else(|| "console".to_string());
    let lines = num_arg(args, "lines").unwrap_or(100.0) as usize;
    let level = str_arg(args, "level");
    let pattern = str_arg(args, "pattern");
    let since = num_arg(args, "since").map(|n| n as u64);
    let wid = str_arg(args, "windowId");
    handlers::read_log_file(id, &source, lines, level.as_deref(), pattern.as_deref(), since, wid.as_deref(), state).await
}

"ipc_listen" => {
    let action = str_arg(args, "action").unwrap_or_default();
    let events: Option<Vec<String>> = args.get("events")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    handlers::ipc_listen(id, &action, events.as_deref(), state, bridge).await
}

"event_get_captured" => {
    let event = str_arg(args, "event");
    let pattern = str_arg(args, "pattern");
    let limit = num_arg(args, "limit").unwrap_or(100.0) as usize;
    let since = num_arg(args, "since").map(|n| n as u64);
    handlers::event_get_captured(id, event.as_deref(), pattern.as_deref(), limit, since, state).await
}

"webview_search_snapshot" => {
    let pattern = str_arg(args, "pattern").unwrap_or_default();
    let context = num_arg(args, "context").unwrap_or(2.0) as usize;
    let mode = str_arg(args, "mode").unwrap_or_else(|| "ai".to_string());
    let wid = window_id(args);
    handlers::search_snapshot(id, &pattern, context, &mode, &wid, state, bridge).await
}
```

- [ ] **Step 2: Update `tool_definitions` — update existing schemas and add new tools**

Update `read_logs` tool def (line 398):

```rust
tool_def("read_logs",
    "Read console logs. Supports level filtering (error,warn) and regex patterns on messages.",
    json!({ "type": "object", "properties": {
        "lines": { "type": "number", "description": "Max entries to return (default 50)" },
        "filter": { "type": "string", "description": "Substring match on message (backward compat)" },
        "pattern": { "type": "string", "description": "Regex pattern on message (overrides filter)" },
        "level": { "type": "string", "description": "Filter by level: error, warn, info, log, debug. Comma-separated." },
        "windowId": { "type": "string" }
    } })
),
```

Update `ipc_get_captured` tool def (line 384):

```rust
tool_def("ipc_get_captured",
    "Retrieve captured IPC traffic. Supports regex pattern and timestamp filtering.",
    json!({ "type": "object", "properties": {
        "filter": { "type": "string", "description": "Substring match on command name" },
        "pattern": { "type": "string", "description": "Regex on full entry (overrides filter)" },
        "limit": { "type": "number" },
        "since": { "type": "number", "description": "Only entries after this epoch ms" }
    } })
),
```

Update `webview_find_element` tool def (line 302):

```rust
tool_def("webview_find_element",
    "Find elements by CSS, XPath, text, or regex pattern",
    json!({ "type": "object", "properties": {
        "selector": { "type": "string" },
        "strategy": { "type": "string", "enum": ["css", "xpath", "text", "regex"] },
        "target": { "type": "string", "enum": ["text", "class", "id", "attr", "all"], "description": "What regex matches against (regex strategy only)" },
        "windowId": { "type": "string" }
    }, "required": ["selector"] })
),
```

Add 5 new tool defs before the closing `]`:

```rust
tool_def("clear_logs",
    "Clear log files. Specify source: console, ipc, events, or all.",
    json!({ "type": "object", "properties": {
        "source": { "type": "string", "enum": ["console", "ipc", "events", "all"], "default": "all" }
    } })
),
tool_def("read_log_file",
    "Read historical log files (persisted across app restarts). Supports regex and timestamp filtering.",
    json!({ "type": "object", "properties": {
        "source": { "type": "string", "enum": ["console", "ipc", "events"] },
        "lines": { "type": "number", "description": "Max entries from tail (default 100)" },
        "level": { "type": "string", "description": "Level filter (console only)" },
        "pattern": { "type": "string", "description": "Regex on serialized entry" },
        "since": { "type": "number", "description": "Epoch ms floor" },
        "windowId": { "type": "string" }
    }, "required": ["source"] })
),
tool_def("ipc_listen",
    "Listen for Tauri events. Start captures events to events.log, stop removes all listeners.",
    json!({ "type": "object", "properties": {
        "action": { "type": "string", "enum": ["start", "stop"] },
        "events": { "type": "array", "items": { "type": "string" }, "description": "Event names to listen for" }
    }, "required": ["action"] })
),
tool_def("event_get_captured",
    "Retrieve captured Tauri events from events.log. Filter by event name, regex, or timestamp.",
    json!({ "type": "object", "properties": {
        "event": { "type": "string", "description": "Filter by event name (exact)" },
        "pattern": { "type": "string", "description": "Regex on full entry" },
        "limit": { "type": "number" },
        "since": { "type": "number", "description": "Epoch ms floor" }
    } })
),
tool_def("webview_search_snapshot",
    "Search DOM snapshot with regex. Returns matched lines with context. Uses cached snapshot if fresh (<10s).",
    json!({ "type": "object", "properties": {
        "pattern": { "type": "string" },
        "context": { "type": "number", "description": "Lines of context (default 2, max 10)" },
        "mode": { "type": "string", "enum": ["ai", "accessibility", "structure"] },
        "windowId": { "type": "string" }
    }, "required": ["pattern"] })
),
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add plugin/src/mcp_tools.rs
git commit -m "feat(mcp): register 5 new tools, update read_logs/ipc_get_captured/find_element schemas"
```

---

### Task 6: Update bridge.rs — IPC interception wrapper and _origInvoke

**Files:**
- Modify: `plugin/src/bridge.rs:928-947`

- [ ] **Step 1: Inject IPC invoke wrapper after console intercept, before autoPushLogs**

After the `window.__CONNECTOR_LOGS__ = consoleLogs;` line (around line 284) and before the `connect()` function, add the IPC invoke wrapper:

```javascript
  // === IPC Invoke Wrapper (for monitoring) ===
  if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
    const _origInvoke = window.__TAURI_INTERNALS__.invoke;
    window.__CONNECTOR_ORIG_INVOKE__ = _origInvoke;
    window.__TAURI_INTERNALS__.invoke = async function(cmd, args, options) {{
      if (cmd.startsWith('plugin:connector|')) {{
        return _origInvoke.call(this, cmd, args, options);
      }}
      const t0 = Date.now();
      try {{
        const result = await _origInvoke.call(this, cmd, args, options);
        if (window.__CONNECTOR_IPC_MONITOR__) {{
          _origInvoke.call(this, 'plugin:connector|push_ipc_event', {{
            payload: {{ command: cmd, args: args || {{}}, timestamp: t0, durationMs: Date.now() - t0 }}
          }}).catch(function(){{}});
        }}
        return result;
      }} catch(e) {{
        if (window.__CONNECTOR_IPC_MONITOR__) {{
          _origInvoke.call(this, 'plugin:connector|push_ipc_event', {{
            payload: {{ command: cmd, args: args || {{}}, timestamp: t0, durationMs: Date.now() - t0, error: e.message }}
          }}).catch(function(){{}});
        }}
        throw e;
      }}
    }};
  }}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -10`

- [ ] **Step 3: Commit**

```bash
git add plugin/src/bridge.rs
git commit -m "feat(bridge): add IPC invoke wrapper for monitoring with self-exclusion guard"
```

---

### Task 7: Update find_element handler — add regex strategy

**Files:**
- Modify: `plugin/src/handlers.rs:92-168`

- [ ] **Step 1: Update `find_element` signature and add regex strategy**

Update the function to accept `target` parameter and add a `"regex"` match arm:

```rust
pub async fn find_element(
    id: &str,
    selector: &str,
    strategy: &str,
    target: Option<&str>,
    _window_id: &str,
    bridge: &Bridge,
) -> Response {
```

Add before the `_ =>` (CSS) arm in the strategy match:

```rust
        "regex" => {
            let tgt = target.unwrap_or("text");
            let match_expr = match tgt {
                "class" => "el.className || ''",
                "id" => "el.id || ''",
                "attr" => "Array.from(el.attributes).map(a => a.name + '=' + a.value).join(' ')",
                "all" => "el.outerHTML",
                _ => "(el.textContent || '').trim()",
            };
            format!(
                r#"(() => {{
                    const re = new RegExp("{pat}", "i");
                    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
                    const elements = [];
                    while (walker.nextNode()) {{
                        const el = walker.currentNode;
                        const val = {match_expr};
                        if (re.test(val)) {{
                            const rect = el.getBoundingClientRect();
                            elements.push({{
                                tag: el.tagName.toLowerCase(),
                                id: el.id || null,
                                className: el.className || null,
                                text: (el.textContent || '').trim().substring(0, 200),
                                rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
                                visible: rect.width > 0 && rect.height > 0
                            }});
                        }}
                    }}
                    return {{ count: elements.length, elements }};
                }})()"#,
                pat = selector.replace('"', r#"\""#),
                match_expr = match_expr,
            )
        }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p tauri-plugin-connector 2>&1 | head -10`
Expected: PASS (or minor signature mismatches to fix in call sites)

- [ ] **Step 3: Commit**

```bash
git add plugin/src/handlers.rs
git commit -m "feat(handlers): add regex strategy to find_element with target param"
```

---

### Task 8: Update standalone MCP server (crates/mcp-server/src/tools.rs)

**Files:**
- Modify: `crates/mcp-server/src/tools.rs:1-300`

- [ ] **Step 1: Update tool_definitions to mirror plugin changes**

Mirror all tool schema changes from Task 5:
- Update `read_logs` with `pattern`, `level` params
- Update `ipc_get_captured` with `pattern`, `since` params
- Update `webview_find_element` with `regex` strategy, `target` param
- Add 5 new tool defs: `clear_logs`, `read_log_file`, `ipc_listen`, `event_get_captured`, `webview_search_snapshot`

- [ ] **Step 2: Update `call_tool` dispatch**

For `handle_read_logs`, `handle_ipc_get_captured` — extract new params and pass them in the WS JSON:

```rust
"read_logs" => {
    let mut cmd = json!({
        "type": "console_logs",
        "lines": num_arg(args, "lines").unwrap_or(50.0) as usize,
        "window_id": window_id(args),
    });
    if let Some(f) = str_arg(args, "filter") { cmd["filter"] = json!(f); }
    if let Some(p) = str_arg(args, "pattern") { cmd["pattern"] = json!(p); }
    if let Some(l) = str_arg(args, "level") { cmd["level"] = json!(l); }
    handle_generic(client, cmd).await
}
```

Add new handlers that forward to the plugin WS:

```rust
"clear_logs" => handle_generic(client, json!({
    "type": "clear_logs",
    "source": str_arg(args, "source").unwrap_or_else(|| "all".to_string()),
})).await,
"read_log_file" => { /* forward all params */ },
"ipc_listen" => { /* forward action + events */ },
"event_get_captured" => { /* forward all params */ },
"webview_search_snapshot" => { /* forward all params */ },
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p connector-mcp-server 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add crates/mcp-server/src/tools.rs
git commit -m "feat(mcp-server): mirror debug suite tool schemas and add 5 new tool dispatchers"
```

---

### Task 9: Update CLI — add flags and new subcommands

**Files:**
- Modify: `crates/cli/src/main.rs:160-258`
- Modify: `crates/cli/src/commands.rs:397-442`

- [ ] **Step 1: Update `Logs` command in main.rs to add `--level` and `--pattern`**

```rust
Logs {
    #[arg(short = 'n', long, default_value_t = 20)]
    lines: usize,
    #[arg(short, long)]
    filter: Option<String>,
    #[arg(short, long)]
    level: Option<String>,
    #[arg(short, long)]
    pattern: Option<String>,
},
```

- [ ] **Step 2: Add `Events` and `Clear` subcommands**

After the `Emit` variant:

```rust
/// Listen for and retrieve Tauri events
Events {
    #[command(subcommand)]
    action: EventCommands,
},
/// Clear log files
Clear {
    /// What to clear: logs, ipc, events, all
    target: String,
},
```

```rust
#[derive(Subcommand)]
enum EventCommands {
    /// Start listening for events
    Listen {
        /// Comma-separated event names
        events: String,
    },
    /// Get captured events
    Captured {
        #[arg(short, long)]
        pattern: Option<String>,
        #[arg(long)]
        since: Option<u64>,
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
    },
    /// Stop listening
    Stop,
}
```

- [ ] **Step 3: Update `IpcCommands::Captured` to add pattern/since**

```rust
Captured {
    #[arg(short, long)]
    filter: Option<String>,
    #[arg(short, long)]
    pattern: Option<String>,
    #[arg(long)]
    since: Option<u64>,
    #[arg(short, long, default_value_t = 100)]
    limit: usize,
},
```

- [ ] **Step 4: Wire up new commands in main match**

```rust
Commands::Logs { lines, filter, level, pattern } => {
    commands::logs(&client, lines, filter.as_deref(), level.as_deref(), pattern.as_deref()).await
}
Commands::Events { action } => match action {
    EventCommands::Listen { events } => {
        commands::event_listen(&client, &events).await
    }
    EventCommands::Captured { pattern, since, limit } => {
        commands::event_captured(&client, pattern.as_deref(), since, limit).await
    }
    EventCommands::Stop => {
        commands::event_stop(&client).await
    }
},
Commands::Clear { target } => {
    commands::clear_logs(&client, &target).await
}
```

Update IPC captured:
```rust
IpcCommands::Captured { filter, pattern, since, limit } => {
    commands::ipc_captured(&client, filter.as_deref(), pattern.as_deref(), since, limit).await
}
```

- [ ] **Step 5: Rewrite `logs()` and `ipc_captured()`, add new command functions in commands.rs**

Rewrite `logs()` to use WS `console_logs` message instead of JS execution:

```rust
pub async fn logs(
    client: &ConnectorClient,
    lines: usize,
    filter: Option<&str>,
    level: Option<&str>,
    pattern: Option<&str>,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "console_logs", "lines": lines, "window_id": "main" });
    if let Some(f) = filter { cmd["filter"] = json!(f); }
    if let Some(l) = level { cmd["level"] = json!(l); }
    if let Some(p) = pattern { cmd["pattern"] = json!(p); }

    let result = client.send(cmd).await?;
    if let Some(logs) = result.get("logs").and_then(|v| v.as_array()) {
        for entry in logs {
            let ts = entry.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
            let lvl = entry.get("level").and_then(|v| v.as_str()).unwrap_or("LOG");
            let msg = entry.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let secs = ts / 1000;
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            println!("{h:02}:{m:02}:{s:02} {:<5} {msg}", lvl.to_uppercase());
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    }
    Ok(())
}
```

Rewrite `ipc_captured()` to pass new params:

```rust
pub async fn ipc_captured(
    client: &ConnectorClient,
    filter: Option<&str>,
    pattern: Option<&str>,
    since: Option<u64>,
    limit: usize,
) -> Result<(), String> {
    let mut cmd = json!({ "type": "ipc_get_captured", "limit": limit });
    if let Some(f) = filter { cmd["filter"] = json!(f); }
    if let Some(p) = pattern { cmd["pattern"] = json!(p); }
    if let Some(s) = since { cmd["since"] = json!(s); }
    let result = client.send(cmd).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}
```

Add:

```rust
pub async fn event_listen(client: &ConnectorClient, events: &str) -> Result<(), String> {
    let event_list: Vec<String> = events.split(',').map(|s| s.trim().to_string()).collect();
    let result = client.send(json!({
        "type": "ipc_listen",
        "action": "start",
        "events": event_list,
    })).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}

pub async fn event_captured(
    client: &ConnectorClient,
    pattern: Option<&str>,
    since: Option<u64>,
    limit: usize,
) -> Result<(), String> { /* forward to event_get_captured WS type */ }

pub async fn event_stop(client: &ConnectorClient) -> Result<(), String> { /* send ipc_listen stop */ }

pub async fn clear_logs(client: &ConnectorClient, target: &str) -> Result<(), String> {
    let source = match target {
        "logs" => "console",
        "ipc" => "ipc",
        "events" => "events",
        "all" => "all",
        _ => return Err(format!("Unknown target: {target}. Use: logs, ipc, events, all")),
    };
    let result = client.send(json!({ "type": "clear_logs", "source": source })).await?;
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    Ok(())
}
```

- [ ] **Step 6: Verify full workspace compiles**

Run: `cargo check --workspace 2>&1 | tail -5`
Expected: All crates compile successfully.

- [ ] **Step 7: Commit**

```bash
git add crates/cli/src/main.rs crates/cli/src/commands.rs
git commit -m "feat(cli): add --level/--pattern flags to logs, events/clear subcommands"
```

---

### Task 10: Update bun scripts

**Files:**
- Modify: `skill/scripts/logs.ts`
- Create: `skill/scripts/events.ts`

- [ ] **Step 1: Update logs.ts to add level and pattern params**

Read `skill/scripts/logs.ts` and add `--level` and `--pattern` CLI flags. Pass them in the WS JSON as `level` and `pattern` fields alongside existing `lines` and `filter`.

- [ ] **Step 2: Create events.ts bun script**

Create `skill/scripts/events.ts` with:
- `listen <event1,event2,...>` — sends `ipc_listen start`
- `captured [--pattern regex] [--since ts]` — sends `event_get_captured`
- `stop` — sends `ipc_listen stop`

Follow the pattern from `skill/scripts/logs.ts` using the shared `connector.ts` helper.

- [ ] **Step 3: Commit**

```bash
git add skill/scripts/logs.ts skill/scripts/events.ts
git commit -m "feat(scripts): update logs.ts with level/pattern, add events.ts bun script"
```

---

### Task 11: Update documentation (skill/SKILL.md + README.md)

**Files:**
- Modify: `skill/SKILL.md`
- Modify: `README.md`

- [ ] **Step 1: Update skill/SKILL.md**

Add new sections:
- Updated `read_logs` tool doc with `level`, `pattern` params
- New tool docs: `clear_logs`, `read_log_file`, `ipc_listen`, `event_get_captured`, `webview_search_snapshot`
- Updated `webview_find_element` with `regex` strategy and `target` param
- "Debugging Workflow" recipe section

- [ ] **Step 2: Update README.md**

- Tool table: 20 → 25 tools
- WS Command Reference: add `clear_logs`, `read_log_file`, `ipc_listen`, `event_get_captured`, `search_snapshot`
- Update "Console Log Capture" section for file-backed JSONL storage at `{app_data_dir}/.tauri-connector/`
- CLI examples: add `--level`, `--pattern`, `events`, `clear` commands
- Architecture diagram: add file persistence path

- [ ] **Step 3: Commit**

```bash
git add skill/SKILL.md README.md
git commit -m "docs: update SKILL.md and README.md for debug suite v0.6.0"
```

---

### Task 12: Final build verification and version bump

**Files:**
- Modify: All workspace `Cargo.toml` files (version bump)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: All crates build successfully.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 3: Version bump to 0.6.0**

Update version in all 4 workspace `Cargo.toml` files from `0.5.0` to `0.6.0`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: bump to v0.6.0"
```
