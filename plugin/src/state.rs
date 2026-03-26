//! Shared plugin state accessible from Tauri commands and WebSocket handlers.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Metadata for a referenced DOM element (shared with CLI crate).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefEntry {
    pub tag: String,
    pub role: Option<String>,
    pub name: String,
    pub selector: String,
    pub nth: Option<usize>,
}

/// Type alias for the ref map.
pub type RefMap = HashMap<String, RefEntry>;

/// Metadata about a snapshot capture.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotMeta {
    pub element_count: usize,
    pub truncated: bool,
    pub portal_count: usize,
    pub virtual_scroll_containers: usize,
}

/// Cached DOM snapshot pushed from the frontend via `invoke()`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DomEntry {
    pub window_id: String,
    pub html: String,
    pub text_content: String,
    pub snapshot: String,
    pub snapshot_mode: String,
    pub refs: RefMap,
    pub meta: SnapshotMeta,
    pub timestamp: u64,
    /// Merged full-text for search (skeleton + subtree content). Empty when no split.
    #[serde(default)]
    pub search_text: String,
    /// Snapshot session UUID if subtrees were written.
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

/// A captured console log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    pub timestamp: u64,
    pub window_id: String,
}

/// A captured IPC event (when monitoring is active).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub command: String,
    pub args: serde_json::Value,
    pub timestamp: u64,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

/// A captured frontend event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEntry {
    pub event: String,
    pub payload: serde_json::Value,
    pub timestamp: u64,
    pub window_id: String,
}

/// Shared mutable state for the plugin.
#[derive(Clone)]
pub struct PluginState {
    pub dom_cache: Arc<Mutex<std::collections::HashMap<String, DomEntry>>>,
    pub ipc_monitor_active: Arc<Mutex<bool>>,
    pub pointed_element: Arc<Mutex<Option<serde_json::Value>>>,
    pub log_dir: PathBuf,
    pub console_writer: Arc<Mutex<BufWriter<File>>>,
    pub ipc_writer: Arc<Mutex<BufWriter<File>>>,
    pub event_writer: Arc<Mutex<BufWriter<File>>>,
    pub event_listeners: Arc<Mutex<Vec<String>>>,
    pub snapshot_prune_lock: Arc<std::sync::Mutex<()>>,
}

impl PluginState {
    /// Create a new `PluginState` backed by JSONL files under `log_dir`.
    pub fn new(log_dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&log_dir)
            .map_err(|e| format!("failed to create log dir: {e}"))?;

        let open_append = |name: &str| -> Result<File, String> {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_dir.join(name))
                .map_err(|e| format!("failed to open {name}: {e}"))
        };

        let console_file = open_append("console.log")?;
        let ipc_file = open_append("ipc.log")?;
        let event_file = open_append("events.log")?;

        Ok(Self {
            dom_cache: Arc::new(Mutex::new(HashMap::new())),
            ipc_monitor_active: Arc::new(Mutex::new(false)),
            pointed_element: Arc::new(Mutex::new(None)),
            log_dir,
            console_writer: Arc::new(Mutex::new(BufWriter::new(console_file))),
            ipc_writer: Arc::new(Mutex::new(BufWriter::new(ipc_file))),
            event_writer: Arc::new(Mutex::new(BufWriter::new(event_file))),
            event_listeners: Arc::new(Mutex::new(Vec::new())),
            snapshot_prune_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    pub async fn push_dom(&self, entry: DomEntry) {
        let mut cache = self.dom_cache.lock().await;
        cache.insert(entry.window_id.clone(), entry);
    }

    pub async fn get_dom(&self, window_id: &str) -> Option<DomEntry> {
        let cache = self.dom_cache.lock().await;
        cache.get(window_id).cloned()
    }

    pub async fn push_logs(&self, entries: Vec<LogEntry>) {
        let mut writer = self.console_writer.lock().await;
        for entry in &entries {
            if let Ok(json) = serde_json::to_string(entry) {
                let _ = writeln!(*writer, "{json}");
            }
        }
        let _ = writer.flush();
    }

    pub async fn push_ipc_event(&self, event: IpcEvent) {
        let mut writer = self.ipc_writer.lock().await;
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = writeln!(*writer, "{json}");
        }
        let _ = writer.flush();
    }

    pub async fn push_event(&self, entry: EventEntry) {
        let mut writer = self.event_writer.lock().await;
        if let Ok(json) = serde_json::to_string(&entry) {
            let _ = writeln!(*writer, "{json}");
        }
        let _ = writer.flush();
    }

    pub async fn set_ipc_monitoring(&self, active: bool) {
        *self.ipc_monitor_active.lock().await = active;
    }

    #[allow(dead_code)]
    pub async fn is_ipc_monitoring(&self) -> bool {
        *self.ipc_monitor_active.lock().await
    }

    pub async fn set_pointed_element(&self, element: serde_json::Value) {
        *self.pointed_element.lock().await = Some(element);
    }

    pub async fn take_pointed_element(&self) -> Option<serde_json::Value> {
        self.pointed_element.lock().await.take()
    }
}

/// Flush and truncate the file behind a `BufWriter`, resetting it to empty.
#[allow(dead_code)]
pub fn clear_file(writer: &Mutex<BufWriter<File>>) {
    if let Ok(mut w) = writer.try_lock() {
        let _ = w.flush();
        let file = w.get_mut();
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
    }
}

/// Read a JSONL file, apply a text filter to each line, deserialize matches,
/// and return the last `limit` entries (tail semantics).
pub fn read_jsonl_filtered<T: DeserializeOwned>(
    path: &Path,
    filter_fn: impl Fn(&str) -> bool,
    limit: usize,
) -> Vec<T> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let reader = BufReader::new(file);
    let mut results: Vec<T> = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if line.is_empty() {
            continue;
        }

        if !filter_fn(&line) {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<T>(&line) {
            results.push(entry);
        }
    }

    // Return the last `limit` entries (tail)
    let skip = results.len().saturating_sub(limit);
    results.into_iter().skip(skip).collect()
}
