//! Application-owned leases. Unknown writes retain quarantine until explicit host resolution.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct ResourceArbiter {
    inner: Arc<Mutex<BTreeMap<String, Held>>>,
}

struct Held {
    resources: Vec<String>,
    quarantine: Option<String>,
}

pub struct Lease {
    owner: String,
    arbiter: ResourceArbiter,
}

fn conflicts(a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix(b).is_some_and(|s| s.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|s| s.starts_with('/'))
}

impl ResourceArbiter {
    pub fn try_acquire(&self, mut resources: Vec<String>) -> Result<Lease, Value> {
        resources.sort();
        resources.dedup();
        let mut held = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for entry in held.values() {
            if resources
                .iter()
                .any(|a| entry.resources.iter().any(|b| conflicts(a, b)))
            {
                return Err(
                    json!({"code":"resource_busy", "stage":"acquiring", "message":"A conflicting application operation holds this resource", "quarantined":entry.quarantine.is_some(), "retryableBeforeDispatch":true}),
                );
            }
        }
        let owner = uuid::Uuid::new_v4().to_string();
        held.insert(
            owner.clone(),
            Held {
                resources,
                quarantine: None,
            },
        );
        Ok(Lease {
            owner,
            arbiter: self.clone(),
        })
    }

    pub fn quarantined(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|h| h.quarantine.is_some())
            .count()
    }

    /// Reloaded unknown effects stay isolated; expiry is never a cancellation acknowledgement.
    pub fn restore_quarantine(&self, resources: Vec<String>) {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).insert(
            uuid::Uuid::new_v4().to_string(),
            Held {
                resources,
                quarantine: Some("interrupted".into()),
            },
        );
    }
}

impl Lease {
    pub fn quarantine(&self, reason: &str) {
        if let Some(held) = self
            .arbiter
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&self.owner)
        {
            held.quarantine = Some(reason.into());
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut held = self.arbiter.inner.lock().unwrap_or_else(|p| p.into_inner());
        if held
            .get(&self.owner)
            .is_some_and(|h| h.quarantine.is_none())
        {
            held.remove(&self.owner);
        }
    }
}

/// This table is trusted implementation metadata, never caller-provided metadata.
pub fn tool_resources(name: &str, args: &Value) -> Vec<String> {
    match name {
        "console_logs"
        | "read_logs"
        | "read_log_file"
        | "runtime_get_captured"
        | "event_get_captured"
        | "get_cached_dom"
        | "list_devices"
        | "get_setup_instructions"
        | "ipc_get_captured"
        | "ipc_get_events"
        | "ipc_get_backend_state"
        | "get_runtime_events"
        | "debug_get_runtime"
        | "debug_get_timeline"
        | "artifact_list"
        | "artifact_read"
        | "bridge_status"
        | "get_app_info"
        | "get_env_info"
        | "get_tauri_info"
        | "ping"
        | "workflow_capabilities" => Vec::new(),
        "webview_interact"
        | "webview_keyboard"
        | "webview_screenshot"
        | "webview_dom_snapshot"
        | "webview_wait_for"
        | "webview_find_element"
        | "webview_get_styles"
        | "webview_locator"
        | "webview_act_and_verify" => {
            let window = args
                .get("windowId")
                .or_else(|| args.get("window_id"))
                .and_then(Value::as_str)
                .unwrap_or("main");
            // These tools can run input handlers, autosave, annotations or native focus.
            vec![
                "backend".into(),
                "ui/global-focus".into(),
                format!("ui/window/{window}"),
            ]
        }
        _ => vec!["backend".into(), "ui".into()],
    }
}

pub async fn acquire(
    state: &crate::state::PluginState,
    name: &str,
    args: &Value,
) -> Result<Lease, Value> {
    let resources = tool_resources(name, args);
    if !resources.is_empty() {
        state.workflow.recover_before_write().await?;
    }
    state.workflow.resources.try_acquire(resources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_scope_conflicts_with_children_but_not_similar_prefixes() {
        assert!(conflicts("ui", "ui/window/main"));
        assert!(!conflicts("ui/window/main", "ui/window/main-two"));
        assert!(conflicts("backend", "backend/entity/task/123"));
    }

    #[test]
    fn leases_release_on_drop_and_cannot_reenter_with_caller_owner() {
        let arbiter = ResourceArbiter::default();
        let first = arbiter.try_acquire(vec!["ui/window/main".into()]).unwrap();
        assert!(arbiter.try_acquire(vec!["ui/window/main".into()]).is_err());
        drop(first);
        assert!(arbiter.try_acquire(vec!["ui/window/main".into()]).is_ok());
    }

    #[test]
    fn quarantine_survives_drop_without_blocking_diagnostics() {
        let arbiter = ResourceArbiter::default();
        let first = arbiter
            .try_acquire(vec!["ui".into(), "backend".into()])
            .unwrap();
        first.quarantine("outcome_unknown");
        drop(first);
        assert!(arbiter.try_acquire(vec!["ui/window/main".into()]).is_err());
        assert!(arbiter.try_acquire(Vec::new()).is_ok());
    }

    #[test]
    fn atomic_multi_resource_failure_does_not_leak_partial_lease() {
        let arbiter = ResourceArbiter::default();
        let first = arbiter.try_acquire(vec!["backend".into()]).unwrap();
        assert!(
            arbiter
                .try_acquire(vec!["ui".into(), "backend".into()])
                .is_err()
        );
        assert!(arbiter.try_acquire(vec!["ui".into()]).is_ok());
        drop(first);
    }

    #[test]
    fn unknown_tools_and_screenshots_take_exclusive_scopes() {
        assert_eq!(
            tool_resources("unknown", &serde_json::json!({})),
            vec!["backend", "ui"]
        );
        assert!(
            tool_resources("webview_screenshot", &serde_json::json!({}))
                .contains(&"ui/window/main".into())
        );
        assert!(tool_resources("console_logs", &serde_json::json!({})).is_empty());
    }
}
