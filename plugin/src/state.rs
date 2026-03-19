//! Shared plugin state accessible from Tauri commands and WebSocket handlers.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Maximum number of cached log entries.
const MAX_LOG_ENTRIES: usize = 1000;
/// Maximum number of captured IPC events.
const MAX_IPC_EVENTS: usize = 500;

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
}

/// Shared mutable state for the plugin.
#[derive(Clone)]
pub struct PluginState {
    pub dom_cache: Arc<Mutex<std::collections::HashMap<String, DomEntry>>>,
    pub log_cache: Arc<Mutex<VecDeque<LogEntry>>>,
    pub ipc_monitor_active: Arc<Mutex<bool>>,
    pub ipc_events: Arc<Mutex<VecDeque<IpcEvent>>>,
    pub pointed_element: Arc<Mutex<Option<serde_json::Value>>>,
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            dom_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
            log_cache: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_ENTRIES))),
            ipc_monitor_active: Arc::new(Mutex::new(false)),
            ipc_events: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_IPC_EVENTS))),
            pointed_element: Arc::new(Mutex::new(None)),
        }
    }
}

impl PluginState {
    pub async fn push_dom(&self, entry: DomEntry) {
        let mut cache = self.dom_cache.lock().await;
        cache.insert(entry.window_id.clone(), entry);
    }

    pub async fn get_dom(&self, window_id: &str) -> Option<DomEntry> {
        let cache = self.dom_cache.lock().await;
        cache.get(window_id).cloned()
    }

    pub async fn push_logs(&self, entries: Vec<LogEntry>) {
        let mut cache = self.log_cache.lock().await;
        for entry in entries {
            if cache.len() >= MAX_LOG_ENTRIES {
                cache.pop_front();
            }
            cache.push_back(entry);
        }
    }

    pub async fn get_logs(&self, lines: usize, filter: Option<&str>) -> Vec<LogEntry> {
        let cache = self.log_cache.lock().await;
        let iter = cache.iter().rev();

        let filtered: Vec<LogEntry> = if let Some(f) = filter {
            let f_lower = f.to_lowercase();
            iter.filter(|e| e.message.to_lowercase().contains(&f_lower))
                .take(lines)
                .cloned()
                .collect()
        } else {
            iter.take(lines).cloned().collect()
        };

        filtered.into_iter().rev().collect()
    }

    #[allow(dead_code)]
    pub async fn push_ipc_event(&self, event: IpcEvent) {
        let mut events = self.ipc_events.lock().await;
        if events.len() >= MAX_IPC_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }

    pub async fn get_ipc_events(
        &self,
        filter: Option<&str>,
        limit: usize,
    ) -> Vec<IpcEvent> {
        let events = self.ipc_events.lock().await;
        let iter = events.iter().rev();

        let filtered: Vec<IpcEvent> = if let Some(f) = filter {
            let f_lower = f.to_lowercase();
            iter.filter(|e| e.command.to_lowercase().contains(&f_lower))
                .take(limit)
                .cloned()
                .collect()
        } else {
            iter.take(limit).cloned().collect()
        };

        filtered.into_iter().rev().collect()
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
