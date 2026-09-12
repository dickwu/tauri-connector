//! Authorized, process-local IPC observation. Capture never mutates workflow history.
mod redact;
mod store;
use crate::{bridge::Bridge, state::PluginState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use store::Store;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureOptions {
    #[serde(default = "metadata")]
    pub result_policy: String,
    #[serde(default = "metadata")]
    pub argument_policy: String,
    #[serde(default)]
    pub follow_pages: bool,
    #[serde(default)]
    pub commands: Vec<String>,
}
fn metadata() -> String {
    "metadata".into()
}
impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            result_policy: metadata(),
            argument_policy: metadata(),
            follow_pages: false,
            commands: Vec::new(),
        }
    }
}
impl CaptureOptions {
    fn validate(&self) -> Result<(), Value> {
        if !matches!(self.result_policy.as_str(), "metadata" | "preview")
            || !matches!(self.argument_policy.as_str(), "metadata" | "preview")
            || self.commands.len() > 50
            || self.commands.iter().any(|c| c.is_empty() || c.len() > 256)
        {
            return Err(error(
                "invalid_argument",
                "Invalid capture policy or command filter",
            ));
        }
        Ok(())
    }
    fn matches(&self, command: &Value) -> bool {
        self.commands.is_empty()
            || command
                .as_str()
                .is_some_and(|c| self.commands.iter().any(|filter| filter == c))
    }
}
#[derive(Default)]
pub struct CaptureService {
    store: Mutex<Store>,
    operations: tokio::sync::Mutex<()>,
    maintenance_started: AtomicBool,
    wake: Arc<tokio::sync::Notify>,
}
impl Drop for CaptureService {
    fn drop(&mut self) {
        self.wake.notify_one();
    }
}
impl CaptureService {
    fn start_maintenance(
        self: &Arc<Self>,
        workflow: &Arc<crate::workflow::WorkflowService>,
        bridge: &Bridge,
    ) {
        self.wake.notify_one();
        if self.maintenance_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        let wake = self.wake.clone();
        let workflow = workflow.clone();
        let bridge = bridge.clone();
        tokio::spawn(async move {
            loop {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let serial = service.operations.lock().await;
                let jobs = {
                    let mut store = service.store.lock().unwrap_or_else(|e| e.into_inner());
                    store.authorize_domains(|g| workflow.inspection_authorized(g));
                    store.expire();
                    store.take_cleanup()
                };
                for job in jobs {
                    expire_page_leases(&bridge, job).await;
                }
                let next = {
                    let mut store = service.store.lock().unwrap_or_else(|e| e.into_inner());
                    store.expire();
                    store.next_deadline()
                };
                if next.is_none() {
                    service.maintenance_started.store(false, Ordering::Release);
                    return;
                }
                let deadline = next.unwrap().min(Instant::now() + Duration::from_secs(1));
                drop(serial);
                drop(service);
                tokio::select! {_=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>{},_=wake.notified()=>{}}
            }
        });
    }
    pub fn set_preview_commands(&self, commands: Vec<String>) {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .preview_commands = commands
            .into_iter()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .take(50)
            .collect();
    }
    pub fn set_preview_paths(&self, paths: Vec<String>) {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .preview_paths = paths
            .into_iter()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .take(50)
            .collect();
    }
    pub fn ingest(
        &self,
        window: &str,
        payload: Value,
        workflow: &crate::workflow::WorkflowService,
    ) -> Value {
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.expire();
        store.authorize_domains(|generation| workflow.inspection_authorized(generation));
        let accepted = payload
            .get("sourceId")
            .and_then(Value::as_str)
            .is_some_and(|source| store.accepts_source(window, source));
        if accepted {
            store.ingest(window, payload);
        }
        self.wake.notify_one();
        json!({"accepted":accepted,"continueCapture":accepted})
    }
    pub fn window_closed(&self, window: &str) {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .interrupt_window(window);
        self.wake.notify_one();
    }
    pub fn revoke_all(&self) {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .authorize_domains(|_| false);
        self.wake.notify_one();
    }
    /// Called after an observed navigation/window lifecycle change. Never replays business IPC.
    pub async fn page_changed(&self, window: &str, state: &PluginState, bridge: &Bridge) {
        let _serial = self.operations.lock().await;
        {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            store.authorize_domains(|g| state.workflow.inspection_authorized(g));
            store.expire();
            store.interrupt_window(window);
        }
        if !self
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .has_follow_session(window)
        {
            return;
        }
        if state.workflow.recover_before_inspection().await.is_err() {
            return;
        }
        let Ok(_lease) = state
            .workflow
            .resources
            .try_acquire(vec!["backend".into(), format!("ui/window/{window}")])
        else {
            return;
        };
        if let Ok(context) = bridge.runtime_context(window, 2000).await {
            let (source, config) = {
                let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                let source = store.source(window, json!(context));
                store.follow_source(window, &source);
                let config = store.config(&source);
                (source, config)
            };
            if config["enabled"] == true {
                let domain = self
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .active_domain(&source);
                let reply = apply(bridge, window, config, &|| {
                    domain.is_some_and(|g| state.workflow.inspection_authorized(g))
                })
                .await;
                let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(ref acknowledgement) = reply {
                    store.note_source_hook(&source, acknowledgement);
                }
                store.acknowledge_source(&source, reply.is_ok());
            }
        }
    }
}
async fn expire_page_leases(bridge: &Bridge, job: Value) {
    let Some(window) = job["windowId"].as_str() else {
        return;
    };
    let script = format!(
        "((job)=>{{const c=window.__CONNECTOR_CAPTURE__;if(!c)return {{cleaned:true}};const s=c.status();if(s.sourceId!==job.sourceId||Object.keys(job.context).some(key=>s.context?.[key]!==job.context[key]))return {{cleaned:true,sourceChanged:true}};return c.expireLeases(job.sessionIds);}})({})",
        crate::runtime::json(&job)
    );
    // Only remove already-installed leases. Never install runtime, reacquire wrappers,
    // await an application invocation, or replay an operation during cleanup.
    let _ = bridge
        .execute_js_for_window_with_request_id(
            &script,
            500,
            window,
            &uuid::Uuid::new_v4().to_string(),
        )
        .await;
}
pub(crate) fn error(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message,"stage":"inspection","retryableBeforeDispatch":false})
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
async fn apply(
    bridge: &Bridge,
    window: &str,
    mut config: Value,
    authorize: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Value, Value> {
    let expected = config["context"].clone();
    let desired = config["enabled"] == true;
    config["action"] = json!("configure");
    config["selfTest"] = json!(desired);
    let result = bridge
        .execute_runtime_authorized(
            window,
            "ipcCapture",
            config,
            2000,
            &uuid::Uuid::new_v4().to_string(),
            authorize,
        )
        .await
        .map_err(|_| {
            error(
                "observation_failed",
                "Capture hook did not acknowledge configuration",
            )
        })?;
    if result["applied"] != json!(desired) || result["context"] != expected {
        return Err(error(
            "observation_failed",
            "Capture hook acknowledgement does not match execution context",
        ));
    }
    Ok(result)
}
pub async fn handle(
    tool: &str,
    args: &Value,
    state: &PluginState,
    bridge: &Bridge,
) -> Result<Value, Value> {
    let requested_at = Instant::now();
    let domain = state.workflow.inspection_authorize(args)?;
    let allowed = if tool == "ipc_query" {
        &[
            "captureSessionId",
            "cursor",
            "invocationId",
            "phase",
            "limit",
            "maxBytes",
            "authToken",
        ][..]
    } else {
        &[
            "action",
            "windowId",
            "options",
            "captureSessionId",
            "authToken",
        ][..]
    };
    if !args.is_object()
        || args
            .as_object()
            .unwrap()
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(error("invalid_argument", "Unknown IPC capture argument"));
    }
    if tool == "ipc_query" {
        for field in ["limit", "maxBytes"] {
            if args.get(field).is_some_and(|v| v.as_u64().is_none()) {
                return Err(error(
                    "invalid_argument",
                    "Query bounds must be positive integers",
                ));
            }
        }
        for field in ["cursor", "invocationId"] {
            if args
                .get(field)
                .is_some_and(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
            {
                return Err(error("invalid_argument", "Invalid capture query filter"));
            }
        }
        if args
            .get("phase")
            .is_some_and(|v| !matches!(v.as_str(), Some("started" | "succeeded" | "failed")))
        {
            return Err(error("invalid_argument", "Invalid capture phase"));
        }
        let id = required(args, "captureSessionId")?;
        let mut store = state
            .capture
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.expire();
        store.authorize_domains(|g| state.workflow.inspection_authorized(g));
        let result = store.query(id, domain, args)?;
        if !state.workflow.inspection_authorized(domain) {
            return Err(error("unauthorized", "Capture authorization was revoked"));
        }
        return Ok(result);
    }
    if tool != "ipc_capture" {
        return Err(error("unsupported_feature", "Unknown capture operation"));
    }
    let action = required(args, "action")?;
    let _serial = state.capture.operations.lock().await;
    if !state.workflow.inspection_authorized(domain) {
        return Err(error("unauthorized", "Capture authorization was revoked"));
    }
    let result = match action {
        "start" => {
            if args.get("captureSessionId").is_some() {
                return Err(error(
                    "invalid_argument",
                    "Start does not accept captureSessionId",
                ));
            }
            let window = args
                .get("windowId")
                .map(|_| required(args, "windowId"))
                .transpose()?
                .unwrap_or("main");
            let options: CaptureOptions =
                serde_json::from_value(args.get("options").cloned().unwrap_or(json!({})))
                    .map_err(|_| error("invalid_argument", "Invalid capture options"))?;
            options.validate()?;
            state.workflow.recover_before_inspection().await?;
            let provisional = {
                let mut store = state
                    .capture
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                store.expire();
                if store.reservation.is_none() {
                    Some(state.workflow.reserve_inspection_memory(16 * 1024 * 1024)?)
                } else {
                    None
                }
            };
            // Hook installation is a UI mutation and cannot cross unknown-write quarantine.
            let _lease = state
                .workflow
                .resources
                .try_acquire(vec!["backend".into(), format!("ui/window/{window}")])?;
            let context = json!(
                bridge
                    .runtime_context(window, 2000)
                    .await
                    .map_err(|_| error(
                        "runtime_unavailable",
                        "Cannot establish capture execution context"
                    ))?
            );
            if !state.workflow.inspection_authorized(domain) {
                return Err(error("unauthorized", "Capture authorization was revoked"));
            }
            let (id, config) = {
                let mut store = state
                    .capture
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                store.expire();
                store.authorize_domains(|g| state.workflow.inspection_authorized(g));
                let source = store.source(window, context);
                let charge = if store.reservation.is_none() {
                    Some(match provisional {
                        Some(charge) => charge,
                        None => state.workflow.reserve_inspection_memory(16 * 1024 * 1024)?,
                    })
                } else {
                    None
                };
                let id = store.start_at(domain, window, options, &source, requested_at)?;
                if store.reservation.is_none() {
                    store.reservation = charge;
                }
                let config = store.config(&source);
                (id, config)
            };
            state.capture.start_maintenance(&state.workflow, bridge);
            let applied = apply(bridge, window, config, &|| {
                state.workflow.inspection_authorized(domain)
            })
            .await;
            if !state.workflow.inspection_authorized(domain) {
                state.capture.revoke_all();
                return Err(error("unauthorized", "Capture authorization was revoked"));
            }
            match applied {
                Ok(ref acknowledgement) => {
                    let mut store = state
                        .capture
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    store.expire();
                    store.note_hook(&id, acknowledgement);
                    store.acknowledge(&id, true);
                    store.status(&id, domain)
                }
                Err(cause) => {
                    let rollback = {
                        let mut store = state
                            .capture
                            .store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let config = store.stop_config(&id, domain).ok().flatten();
                        store.remove(&id);
                        config
                    };
                    if let Some(mut config) = rollback {
                        config["drain"] = json!(true);
                        let enabled = config["enabled"] == true;
                        let _ = apply(bridge, window, config, &|| {
                            !enabled || state.workflow.inspection_authorized(domain)
                        })
                        .await;
                    }
                    Err(cause)
                }
            }
        }
        "status" | "stop" => {
            if args.get("windowId").is_some() || args.get("options").is_some() {
                return Err(error(
                    "invalid_argument",
                    "Status and stop only accept captureSessionId",
                ));
            }
            let id = required(args, "captureSessionId")?;
            let window = state
                .capture
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .window(id, domain)?;
            if action == "stop" {
                let config = state
                    .capture
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stop_config(id, domain)?;
                let acknowledgement = if let Some(config) = config {
                    let enabled = config["enabled"] == true;
                    Some(
                        apply(bridge, &window, config, &|| {
                            !enabled || state.workflow.inspection_authorized(domain)
                        })
                        .await,
                    )
                } else {
                    None
                };
                let mut store = state
                    .capture
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let cleanup = acknowledgement
                    .as_ref()
                    .is_none_or(|r| r.as_ref().is_ok_and(|v| v["drainConfirmed"] == true));
                if !cleanup {
                    let missing = acknowledgement
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|v| v["queuedEvents"].as_u64())
                        .unwrap_or(0)
                        .saturating_add(1);
                    store.note_gap(id, missing);
                }
                let mut result = store.stop(id, domain)?;
                result["hookCleanupAcknowledged"] = json!(cleanup);
                Ok(result)
            } else {
                let probe = bridge
                    .execute_runtime_authorized(
                        &window,
                        "ipcCapture",
                        json!({"action":"status"}),
                        2000,
                        &uuid::Uuid::new_v4().to_string(),
                        &|| state.workflow.inspection_authorized(domain),
                    )
                    .await;
                let mut store = state
                    .capture
                    .store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let current = store.status(id, domain)?;
                let matches = probe
                    .as_ref()
                    .is_ok_and(|p| p["context"] == current["context"] && p["applied"] == true);
                if current["desired"] == true && !matches {
                    store.interrupt_window(&window);
                }
                let mut result = store.status(id, domain)?;
                result["hookStatus"] =
                    probe.unwrap_or_else(|_| json!({"applied":null,"coverage":"unavailable"}));
                Ok(result)
            }
        }
        _ => Err(error(
            "invalid_argument",
            "Capture action must be start, status, or stop",
        )),
    };
    state.capture.wake.notify_one();
    if !state.workflow.inspection_authorized(domain) {
        state.capture.revoke_all();
        return Err(error("unauthorized", "Capture authorization was revoked"));
    }
    result
}
fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or_else(|| {
            error(
                "invalid_argument",
                "Required capture identifier is missing or invalid",
            )
        })
}

#[cfg(test)]
mod service_tests;
