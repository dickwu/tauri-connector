//! Application-owned leases. Unknown writes retain quarantine until explicit host resolution.

use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Late settlements kept for diagnostics and for results that beat quarantine.
const LATE_HISTORY: usize = 32;

#[derive(Clone, Default)]
pub struct ResourceArbiter {
    inner: Arc<Mutex<ArbiterState>>,
}

#[derive(Default)]
struct ArbiterState {
    held: BTreeMap<String, Held>,
    /// Requests whose page result arrived after the caller stopped waiting.
    late: VecDeque<LateRecord>,
}

struct Held {
    resources: Vec<String>,
    quarantine: Option<Quarantine>,
}

struct Quarantine {
    reason: String,
    request_id: Option<String>,
    since_ms: u64,
}

struct LateRecord {
    request_id: String,
    outcome: LateOutcome,
    released: bool,
    at_ms: u64,
}

/// What the page reported for a request after its caller timed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LateOutcome {
    /// The script finished; its effects are known to have completed.
    Succeeded,
    /// The script threw and the page proved nothing effect-capable ran.
    FailedWithoutEffect,
    /// The script threw after effects may have started.
    Failed,
    /// No result arrived (connection closed or the late window expired).
    Lost,
}

impl LateOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::FailedWithoutEffect => "failed_without_effect",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }

    /// Only a settled result with known effects can lift the isolation.
    pub fn releases_quarantine(self) -> bool {
        matches!(self, Self::Succeeded | Self::FailedWithoutEffect)
    }
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn describe(id: &str, held: &Held) -> Option<Value> {
    let quarantine = held.quarantine.as_ref()?;
    Some(json!({
        "id": id,
        "requestId": quarantine.request_id,
        "reason": quarantine.reason,
        "resources": held.resources,
        "sinceMs": quarantine.since_ms,
    }))
}

impl ArbiterState {
    fn record_late(&mut self, request_id: &str, outcome: LateOutcome, released: bool) {
        if self.late.len() >= LATE_HISTORY {
            self.late.pop_front();
        }
        self.late.push_back(LateRecord {
            request_id: request_id.to_string(),
            outcome,
            released,
            at_ms: now_ms(),
        });
    }
}

impl ResourceArbiter {
    fn lock(&self) -> std::sync::MutexGuard<'_, ArbiterState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn try_acquire(&self, mut resources: Vec<String>) -> Result<Lease, Value> {
        resources.sort();
        resources.dedup();
        let mut state = self.lock();
        let blockers: Vec<(&String, &Held)> = state
            .held
            .iter()
            .filter(|(_, entry)| {
                resources
                    .iter()
                    .any(|a| entry.resources.iter().any(|b| conflicts(a, b)))
            })
            .collect();
        if !blockers.is_empty() {
            let quarantines: Vec<Value> = blockers
                .iter()
                .filter_map(|(id, held)| describe(id, held))
                .collect();
            let quarantined = !quarantines.is_empty();
            let message = if quarantined {
                let ids: Vec<String> = quarantines
                    .iter()
                    .map(|q| {
                        format!(
                            "{} ({})",
                            q["requestId"]
                                .as_str()
                                .unwrap_or_else(|| q["id"].as_str().unwrap_or("?")),
                            q["reason"].as_str().unwrap_or("unknown")
                        )
                    })
                    .collect();
                format!(
                    "A previous operation with an unknown outcome quarantined this resource: {}. Check the app state, then run `tauri-connector resources release` (MCP: resources action=release) to lift it",
                    ids.join(", ")
                )
            } else {
                "A conflicting application operation holds this resource".to_string()
            };
            return Err(json!({
                "code": "resource_busy",
                "stage": "acquiring",
                "message": message,
                "quarantined": quarantined,
                "quarantines": quarantines,
                "retryableBeforeDispatch": true
            }));
        }
        let owner = uuid::Uuid::new_v4().to_string();
        state.held.insert(
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
        self.lock()
            .held
            .values()
            .filter(|h| h.quarantine.is_some())
            .count()
    }

    /// Reloaded unknown effects stay isolated; expiry is never a cancellation acknowledgement.
    pub fn restore_quarantine(&self, resources: Vec<String>) {
        self.lock().held.insert(
            uuid::Uuid::new_v4().to_string(),
            Held {
                resources,
                quarantine: Some(Quarantine {
                    reason: "interrupted".into(),
                    request_id: None,
                    since_ms: now_ms(),
                }),
            },
        );
    }

    /// Quarantined scopes plus recent late settlements, for `resources list`.
    pub fn list(&self) -> Value {
        let state = self.lock();
        let quarantined: Vec<Value> = state
            .held
            .iter()
            .filter_map(|(id, held)| describe(id, held))
            .collect();
        let late: Vec<Value> = state
            .late
            .iter()
            .rev()
            .map(|record| {
                json!({
                    "requestId": record.request_id,
                    "outcome": record.outcome.as_str(),
                    "releasedQuarantine": record.released,
                    "atMs": record.at_ms,
                })
            })
            .collect();
        json!({ "quarantined": quarantined, "lateSettlements": late })
    }

    /// Explicit host resolution. `target` matches a requestId or quarantine id;
    /// `None` releases every quarantine. Active (unquarantined) leases are untouched.
    pub fn release(&self, target: Option<&str>) -> Vec<Value> {
        let mut state = self.lock();
        let ids: Vec<String> = state
            .held
            .iter()
            .filter(|(id, held)| {
                held.quarantine.as_ref().is_some_and(|q| match target {
                    None => true,
                    Some(target) => {
                        id.as_str() == target || q.request_id.as_deref() == Some(target)
                    }
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter()
            .filter_map(|id| {
                let held = state.held.remove(id)?;
                describe(id, &held)
            })
            .collect()
    }

    /// A page result arrived after the caller gave up. A settled result with known
    /// effects lifts the matching quarantine; anything else is only recorded.
    pub fn settle_late(&self, request_id: &str, outcome: LateOutcome) -> bool {
        let mut state = self.lock();
        let matching: Vec<String> = state
            .held
            .iter()
            .filter(|(_, held)| {
                held.quarantine
                    .as_ref()
                    .is_some_and(|q| q.request_id.as_deref() == Some(request_id))
            })
            .map(|(id, _)| id.clone())
            .collect();
        let release = outcome.releases_quarantine() && !matching.is_empty();
        if release {
            for id in &matching {
                state.held.remove(id);
            }
        }
        state.record_late(request_id, outcome, release);
        release
    }
}

impl ArbiterState {
    /// A late result can win the race against the caller applying quarantine.
    fn settled_safely(&self, request_id: &str) -> bool {
        self.late
            .iter()
            .any(|r| r.request_id == request_id && r.outcome.releases_quarantine())
    }
}

impl Lease {
    pub fn quarantine(&self, reason: &str) {
        self.quarantine_request(reason, None);
    }

    /// Quarantine keyed by the bridge requestId so a late settlement can lift it.
    pub fn quarantine_request(&self, reason: &str, request_id: Option<&str>) {
        let mut state = self.arbiter.lock();
        if let Some(request_id) = request_id
            && state.settled_safely(request_id)
        {
            if let Some(record) = state
                .late
                .iter_mut()
                .rev()
                .find(|r| r.request_id == request_id)
            {
                record.released = true;
            }
            return;
        }
        if let Some(held) = state.held.get_mut(&self.owner) {
            held.quarantine = Some(Quarantine {
                reason: reason.into(),
                request_id: request_id.map(str::to_string),
                since_ms: now_ms(),
            });
        }
    }

    /// Quarantine after a response whose outcome is unknown, keeping its requestId.
    pub fn quarantine_response(&self, reason: &str, response: &crate::protocol::Response) {
        let request_id = response
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.dispatch.get("requestId"))
            .and_then(Value::as_str);
        self.quarantine_request(reason, request_id);
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.arbiter.lock();
        if state
            .held
            .get(&self.owner)
            .is_some_and(|h| h.quarantine.is_none())
        {
            state.held.remove(&self.owner);
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
        | "resources"
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
    #[test]
    fn resource_busy_names_the_quarantined_request_and_reason() {
        let arbiter = ResourceArbiter::default();
        let lease = arbiter.try_acquire(vec!["ui".into()]).unwrap();
        lease.quarantine_request("outcome_unknown", Some("req-1"));
        drop(lease);
        let error = arbiter
            .try_acquire(vec!["ui/window/main".into()])
            .err()
            .unwrap();
        assert_eq!(error["code"], "resource_busy");
        assert_eq!(error["quarantined"], true);
        assert_eq!(error["quarantines"][0]["requestId"], "req-1");
        assert_eq!(error["quarantines"][0]["reason"], "outcome_unknown");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("req-1 (outcome_unknown)"), "{message}");
        assert!(message.contains("resources release"), "{message}");
    }

    #[test]
    fn explicit_release_lifts_one_or_all_quarantines() {
        let arbiter = ResourceArbiter::default();
        for (scope, id) in [("ui/window/a", "req-a"), ("ui/window/b", "req-b")] {
            let lease = arbiter.try_acquire(vec![scope.into()]).unwrap();
            lease.quarantine_request("outcome_unknown", Some(id));
        }
        arbiter.restore_quarantine(vec!["backend".into()]);
        let listed = arbiter.list();
        assert_eq!(listed["quarantined"].as_array().unwrap().len(), 3);
        let released = arbiter.release(Some("req-a"));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0]["requestId"], "req-a");
        assert!(arbiter.try_acquire(vec!["ui/window/a".into()]).is_ok());
        assert!(arbiter.try_acquire(vec!["ui/window/b".into()]).is_err());
        assert!(arbiter.release(Some("missing")).is_empty());
        assert_eq!(arbiter.release(None).len(), 2);
        assert_eq!(arbiter.quarantined(), 0);
    }

    #[test]
    fn release_never_drops_an_active_lease() {
        let arbiter = ResourceArbiter::default();
        let active = arbiter.try_acquire(vec!["ui".into()]).unwrap();
        assert!(arbiter.release(None).is_empty());
        assert!(arbiter.try_acquire(vec!["ui".into()]).is_err());
        drop(active);
    }

    #[test]
    fn late_settlement_without_effect_lifts_quarantine_and_is_recorded() {
        let arbiter = ResourceArbiter::default();
        let lease = arbiter.try_acquire(vec!["ui".into()]).unwrap();
        lease.quarantine_request("outcome_unknown", Some("req-late"));
        drop(lease);
        assert!(!arbiter.settle_late("other", LateOutcome::Succeeded));
        assert!(!arbiter.settle_late("req-late", LateOutcome::Failed));
        assert_eq!(arbiter.quarantined(), 1);
        assert!(arbiter.settle_late("req-late", LateOutcome::FailedWithoutEffect));
        assert_eq!(arbiter.quarantined(), 0);
        let late = &arbiter.list()["lateSettlements"];
        assert_eq!(late[0]["requestId"], "req-late");
        assert_eq!(late[0]["outcome"], "failed_without_effect");
        assert_eq!(late[0]["releasedQuarantine"], true);
        assert_eq!(late[1]["outcome"], "failed");
    }

    #[test]
    fn late_success_lifts_quarantine_but_lost_results_keep_it() {
        let arbiter = ResourceArbiter::default();
        let lease = arbiter.try_acquire(vec!["ui".into()]).unwrap();
        lease.quarantine_request("outcome_unknown", Some("req-1"));
        drop(lease);
        assert!(!arbiter.settle_late("req-1", LateOutcome::Lost));
        assert_eq!(arbiter.quarantined(), 1);
        assert!(arbiter.settle_late("req-1", LateOutcome::Succeeded));
        assert!(arbiter.try_acquire(vec!["ui".into()]).is_ok());
    }

    #[test]
    fn a_late_result_that_beats_quarantine_prevents_it() {
        let arbiter = ResourceArbiter::default();
        let lease = arbiter.try_acquire(vec!["ui".into()]).unwrap();
        assert!(!arbiter.settle_late("req-race", LateOutcome::Succeeded));
        lease.quarantine_request("outcome_unknown", Some("req-race"));
        drop(lease);
        assert_eq!(arbiter.quarantined(), 0);
        assert_eq!(
            arbiter.list()["lateSettlements"][0]["releasedQuarantine"],
            true
        );
    }
}
