use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use connector_client::outcome::{
    EffectStatus, ExecutionOutcome, ExecutionStatus, VerificationStatus, WorkflowError,
};
use connector_client::workflow::{WorkflowSpec, resolve_condition, resolve_step};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, OnceCell, Semaphore};

use super::{
    journal::{Journal, JournalRecord},
    resources::ResourceArbiter,
};
use crate::{
    bridge::{Bridge, BridgeError},
    state::PluginState,
};

const MAX_RUNS: usize = 1000;
const MAX_LIVE_RUNS: usize = 20;

type BackendFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, WorkflowError>> + Send + 'a>>;

trait Backend: Send + Sync {
    fn call<'a>(
        &'a self,
        window: &'a str,
        request: Value,
        timeout_ms: u64,
        request_id: String,
    ) -> BackendFuture<'a>;
}

struct AppBackend {
    bridge: Bridge,
    app: Option<tauri::AppHandle>,
    state: PluginState,
    principal: String,
}

impl Backend for AppBackend {
    fn call<'a>(
        &'a self,
        window: &'a str,
        request: Value,
        timeout_ms: u64,
        request_id: String,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            self.state
                .workflow
                .authorize(&json!({"authToken":self.principal}))
                .map_err(|_| {
                    WorkflowError::new(
                        "unauthorized",
                        "preparing",
                        "Workflow authorization was revoked",
                    )
                })?;
            if request["cmd"] == "tool" {
                let name = request["tool"].as_str().unwrap_or_default();
                if !allowed_tool(name) {
                    return Err(WorkflowError::new(
                        "unauthorized",
                        "preparing",
                        "Tool is not in the host workflow allowlist",
                    ));
                }
                let response = crate::mcp_tools::dispatch_raw(
                    name,
                    &request["args"],
                    &self.bridge,
                    self.app.as_ref(),
                    &self.state,
                )
                .await;
                return match response.payload {
                    crate::protocol::ResponsePayload::Success { result } => {
                        Ok(json!({"ok":true,"dispatched":false,"data":result}))
                    }
                    crate::protocol::ResponsePayload::Error { .. } => Err(WorkflowError::new(
                        "execution_failed",
                        "executing",
                        "Allowed diagnostic tool failed",
                    )),
                };
            }
            if let Some(app) = &self.app {
                use tauri::Manager;
                let webview = app.get_webview_window(window).ok_or_else(|| {
                    WorkflowError::new(
                        "target_changed",
                        "preparing",
                        "Target window is unavailable",
                    )
                })?;
                let current = webview.url().map_err(|_| {
                    WorkflowError::new(
                        "unauthorized",
                        "preparing",
                        "Cannot validate the current WebView origin",
                    )
                })?;
                let config = app.config();
                let bundled = (current.scheme() == "tauri"
                    && current.host_str() == Some("localhost"))
                    || (matches!(current.scheme(), "http" | "https")
                        && current.host_str() == Some("tauri.localhost"));
                let development = config
                    .build
                    .dev_url
                    .as_ref()
                    .is_some_and(|url| url.origin() == current.origin());
                let configured = config.app.windows.iter().any(|window| match &window.url {
                    tauri::WebviewUrl::External(url) => url.origin() == current.origin(),
                    tauri::WebviewUrl::CustomProtocol(url) => {
                        url.scheme() == current.scheme() && url.host_str() == current.host_str()
                    }
                    _ => false,
                });
                if !bundled && !development && !configured {
                    return Err(WorkflowError::new(
                        "unauthorized",
                        "preparing",
                        "Current origin is outside the host configuration",
                    ));
                }
            }
            // Poll cadence belongs to the host condition loop. It is not a
            // page identity field and must not enter the runtime handshake.
            let mut request = request;
            if let Some(context) = request.get_mut("context").and_then(Value::as_object_mut) {
                context.remove("pollIntervalMs");
            }
            self.bridge
                .execute_runtime_authorized(
                    window,
                    "workflow",
                    request,
                    timeout_ms,
                    &request_id,
                    &|| {
                        self.state
                            .workflow
                            .authorize(&json!({"authToken":self.principal}))
                            .is_ok()
                    },
                )
                .await
                .map_err(|error| match error {
                    BridgeError::NotDispatched { .. } => WorkflowError::new(
                        "target_not_found",
                        "preparing",
                        "WebView request was not dispatched",
                    ),
                    BridgeError::ExecutionFailed { .. } => WorkflowError::new(
                        "execution_failed",
                        "executing",
                        "WebView execution failed",
                    ),
                    BridgeError::DispatchedOutcomeUnknown { .. } => WorkflowError::new(
                        "outcome_unknown",
                        "executing",
                        "WebView result is unknown; do not replay the operation",
                    ),
                })
        })
    }
}

fn allowed_tool(name: &str) -> bool {
    matches!(name, "bridge_status" | "ipc_get_backend_state")
}

struct Run {
    spec: Option<WorkflowSpec>,
    report: Value,
    outcomes: BTreeMap<String, ExecutionOutcome>,
    contexts: HashMap<String, Value>,
    index: usize,
    started: Instant,
    cancel: Arc<AtomicBool>,
    notify: Arc<Notify>,
    executing: bool,
    run_key_hash: String,
    spec_hash: String,
    principal_hash: String,
    secrets: Vec<String>,
    events: Vec<Value>,
    evidence: BTreeMap<String, Value>,
    reservations: Vec<super::budget::Reservation>,
}

#[derive(Default)]
struct Registry {
    by_key: HashMap<String, String>,
    runs: HashMap<String, Arc<Mutex<Run>>>,
}

pub struct WorkflowService {
    pub resources: ResourceArbiter,
    directory: PathBuf,
    token: std::sync::Mutex<Option<String>>,
    authorization_generation: AtomicU64,
    app_instance_id: String,
    journal: OnceCell<Result<Journal, String>>,
    registry: Mutex<Registry>,
    slots: Arc<Semaphore>,
    storage_valid: AtomicBool,
    memory: super::budget::MemoryBudget,
}

impl WorkflowService {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            resources: ResourceArbiter::default(),
            directory,
            token: std::sync::Mutex::new(
                std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN")
                    .ok()
                    .filter(|s| s.len() >= 32),
            ),
            authorization_generation: AtomicU64::new(1),
            app_instance_id: crate::identity::app_instance_id().to_owned(),
            journal: OnceCell::new(),
            registry: Mutex::new(Registry::default()),
            slots: Arc::new(Semaphore::new(4)),
            storage_valid: AtomicBool::new(true),
            memory: super::budget::MemoryBudget::default(),
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        let mut current = self.token.lock().unwrap_or_else(|p| p.into_inner());
        let next = token.filter(|s| s.len() >= 32);
        if *current != next {
            self.authorization_generation.fetch_add(1, Ordering::SeqCst);
            *current = next;
        }
    }

    /// Inspection objects are scoped to the host credential generation, never
    /// to a transport connection or a copy of the credential in page state.
    pub(crate) fn inspection_authorize(&self, args: &Value) -> Result<u64, Value> {
        let generation = self.authorization_generation.load(Ordering::SeqCst);
        self.authorize(args)?;
        if !self.inspection_authorized(generation) {
            return Err(error(
                "unauthorized",
                "Inspection authorization was revoked",
            ));
        }
        Ok(generation)
    }

    pub(crate) fn inspection_authorized(&self, generation: u64) -> bool {
        let token = self.token.lock().unwrap_or_else(|p| p.into_inner());
        token.is_some() && self.authorization_generation.load(Ordering::SeqCst) == generation
    }

    /// Hydrate persisted unknown effects before changing the UI. Fresh Windows
    /// hosts can inspect in memory; an existing durable-history directory still
    /// fails closed under the journal's platform storage rules.
    pub(crate) async fn recover_before_inspection(&self) -> Result<(), Value> {
        if cfg!(unix) || self.directory.exists() {
            self.recover_before_write().await
        } else {
            Ok(())
        }
    }

    pub(crate) fn reserve_inspection_memory(
        &self,
        bytes: usize,
    ) -> Result<super::budget::Reservation, Value> {
        self.memory
            .try_reserve(bytes)
            .map_err(|_| error("resource_busy", "Shared retention memory budget exhausted"))
    }

    pub fn disable_storage(&self) {
        self.storage_valid.store(false, Ordering::SeqCst);
    }

    pub async fn recover_before_write(&self) -> Result<(), Value> {
        if !self.storage_valid.load(Ordering::SeqCst) {
            return Err(error(
                "persistence_unavailable",
                "Stable application storage is unavailable",
            ));
        }
        if self.directory.exists()
            || self
                .token
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
        {
            self.store().await?;
        }
        Ok(())
    }

    pub(crate) fn authorize(&self, args: &Value) -> Result<String, Value> {
        let token = self.token.lock().unwrap_or_else(|p| p.into_inner());
        let expected = token.as_deref().ok_or_else(|| {
            error(
                "unauthorized",
                "Workflow access requires a host-configured token of at least 32 bytes",
            )
        })?;
        let supplied = args
            .get("authToken")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut difference = expected.len() ^ supplied.len();
        for (a, b) in expected.bytes().zip(supplied.bytes()) {
            difference |= (a ^ b) as usize;
        }
        if difference != 0 {
            return Err(error(
                "unauthorized",
                "Workflow credential is missing or invalid",
            ));
        }
        Ok(expected.to_string())
    }

    async fn store(&self) -> Result<&Journal, Value> {
        if !self.storage_valid.load(Ordering::SeqCst) {
            return Err(error(
                "persistence_unavailable",
                "Stable application storage is unavailable",
            ));
        }
        self.journal
            .get_or_init(|| async {
                let directory = self.directory.clone();
                let (journal, history) =
                    tokio::task::spawn_blocking(move || Journal::open(&directory))
                        .await
                        .map_err(|_| "Journal initialization task failed".to_string())?
                        .map_err(|e| e.to_string())?;
                let mut latest = HashMap::new();
                for record in history {
                    latest.insert(record.run_id.clone(), record);
                }
                let mut registry = self.registry.lock().await;
                for record in latest.into_values() {
                    let mut report = record.report;
                    let prior_status = report["status"]
                        .as_str()
                        .unwrap_or("interrupted")
                        .to_string();
                    let interrupted =
                        !matches!(prior_status.as_str(), "completed" | "failed" | "cancelled");
                    if interrupted {
                        report["status"] = json!("interrupted");
                        report["reason"] = json!("app_instance_changed");
                        report["allowedNextActions"] = json!(["get", "reconcile"]);
                    }
                    if report["resourceIsolation"] == "quarantined"
                        || report["blockedOutcome"]["effect"] == "possible"
                        || (interrupted && report["mayHaveEffects"].as_bool().unwrap_or(true))
                    {
                        self.resources
                            .restore_quarantine(vec!["backend".into(), "ui".into()]);
                        report["resourceIsolation"] = json!("quarantined");
                    }
                    report["recoveredFromJournal"] = json!(true);
                    let reservation = self
                        .memory
                        .try_reserve(
                            estimated_bytes(&report)
                                .saturating_mul(4)
                                .saturating_add(8192),
                        )
                        .map_err(|_| {
                            "Historical run retention exceeds the host memory budget".to_string()
                        })?;
                    registry
                        .by_key
                        .insert(record.run_key_hash.clone(), record.run_id.clone());
                    registry.runs.insert(
                        record.run_id,
                        Arc::new(Mutex::new(Run {
                            spec: None,
                            report,
                            outcomes: BTreeMap::new(),
                            contexts: HashMap::new(),
                            index: 0,
                            started: Instant::now(),
                            cancel: Arc::new(AtomicBool::new(false)),
                            notify: Arc::new(Notify::new()),
                            executing: false,
                            run_key_hash: record.run_key_hash,
                            spec_hash: record.spec_hash,
                            principal_hash: record.principal_hash,
                            secrets: Vec::new(),
                            events: Vec::new(),
                            evidence: BTreeMap::new(),
                            reservations: vec![reservation],
                        })),
                    );
                }
                Ok(journal)
            })
            .await
            .as_ref()
            .map_err(|_| {
                error(
                    "persistence_unavailable",
                    "Workflow journal is unavailable; new operations are disabled",
                )
            })
    }

    async fn find(&self, run_id: &str, principal_hash: &str) -> Result<Arc<Mutex<Run>>, Value> {
        let run = self
            .registry
            .lock()
            .await
            .runs
            .get(run_id)
            .cloned()
            .ok_or_else(|| error("run_not_found", "Unknown workflow run"))?;
        if run.lock().await.principal_hash != principal_hash {
            return Err(error("run_not_found", "Unknown workflow run"));
        }
        Ok(run)
    }

    async fn checkpoint(&self, run: &mut Run, event: &str) -> Result<(), Value> {
        let revision = run.report["revision"].as_u64().unwrap_or(0) + 1;
        run.report["revision"] = json!(revision);
        run.report["cursor"] = json!(revision);
        run.report["checkpointId"] = json!(uuid::Uuid::new_v4().to_string());
        let status = run.report["status"].clone();
        run.events
            .push(json!({"seq":revision,"event":event,"status":status}));
        if run.events.len() > 256 {
            run.events.remove(0);
            run.report["coverage"]["truncated"] = json!(true);
        }
        let mut persisted = super::security::journal_report(&run.report, &run.secrets);
        // Journal history is diagnostic-only: never persist value bindings or input-bearing evidence.
        persisted.as_object_mut().unwrap().remove("steps");
        if let Some(outcome) = persisted
            .get_mut("blockedOutcome")
            .and_then(Value::as_object_mut)
        {
            outcome.remove("data");
        }
        let record = JournalRecord {
            run_id: run.report["runId"].as_str().unwrap().into(),
            revision,
            event_seq: revision,
            event: event.into(),
            app_instance_id: self.app_instance_id.clone(),
            spec_hash: run.spec_hash.clone(),
            run_key_hash: run.run_key_hash.clone(),
            principal_hash: run.principal_hash.clone(),
            report: persisted,
        };
        let result = self.store().await?.append(record).await;
        if result.is_err() {
            run.report["persistenceWarning"] =
                json!("Execution result retained in memory; durable recovery is degraded");
            return Err(error(
                "persistence_unavailable",
                "Journal acknowledgement failed; no further actions will be dispatched",
            ));
        }
        Ok(())
    }

    async fn create(
        self: &Arc<Self>,
        spec: WorkflowSpec,
        principal: &str,
        backend: Arc<dyn Backend>,
    ) -> Result<Arc<Mutex<Run>>, Value> {
        let journal = self.store().await?;
        for step in &spec.steps {
            let value = serde_json::to_value(step)
                .map_err(|_| error("invalid_spec", "Cannot encode step"))?;
            if value["op"] == "tool" && !allowed_tool(value["tool"].as_str().unwrap_or_default()) {
                return Err(error(
                    "unauthorized",
                    "Tool is not in the workflow allowlist",
                ));
            }
        }
        let normalized = serde_json::to_value(&spec)
            .map_err(|_| error("invalid_spec", "Cannot encode workflow"))?;
        let canonical = spec
            .canonical_value()
            .map_err(|_| error("invalid_spec", "Cannot normalize workflow"))?;
        let spec_hash = journal
            .fingerprint(&canonical)
            .map_err(|_| error("persistence_unavailable", "Cannot fingerprint workflow"))?;
        let principal_hash = journal
            .fingerprint(&json!({"principal":principal}))
            .map_err(|_| error("persistence_unavailable", "Cannot fingerprint principal"))?;
        let run_key_hash = journal
            .fingerprint(&json!({"principal":principal,"logicalKey":spec.run_key}))
            .map_err(|_| error("persistence_unavailable", "Cannot fingerprint run identity"))?;
        let mut registry = self.registry.lock().await;
        if let Some(run_id) = registry.by_key.get(&run_key_hash) {
            let existing = registry.runs[run_id].clone();
            if existing.lock().await.spec_hash != spec_hash {
                return Err(error(
                    "run_key_conflict",
                    "runKey is already bound to a different effective specification",
                ));
            }
            return Ok(existing);
        }
        if registry.runs.len() >= MAX_RUNS {
            return Err(error(
                "resource_busy",
                "Run retention capacity reached; existing identities are preserved",
            ));
        }
        let mut active = 0;
        for run in registry.runs.values() {
            if run.lock().await.executing {
                active += 1;
            }
        }
        if active >= MAX_LIVE_RUNS {
            return Err(error(
                "resource_busy",
                "Active and queued workflow capacity reached",
            ));
        }
        let reservation = self
            .memory
            .try_reserve(
                estimated_bytes(&normalized)
                    .saturating_mul(4)
                    .saturating_add(131072),
            )
            .map_err(|_| {
                error(
                    "resource_busy",
                    "Host run retention budget is full; existing identities are preserved",
                )
            })?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let mut secrets = vec![principal.to_string()];
        collect_strings(&normalized["inputs"], &mut secrets);
        for step in normalized["steps"].as_array().into_iter().flatten() {
            collect_expression_secrets(step, &mut secrets);
        }
        collect_expression_secrets(&normalized["goal"], &mut secrets);
        let report = json!({"runId":run_id,"appInstanceId":self.app_instance_id,"buildId":env!("CARGO_PKG_VERSION"),"revision":0,"cursor":0,"status":"queued","originalTestVerdict":"not_requested","goalStatus":if spec.goal.is_some(){"inconclusive"}else{"not_requested"},"recoveryOccurred":false,"mayHaveEffects":false,"summary":{"completedSteps":0,"remainingSteps":spec.steps.len()},"steps":[],"allowedNextActions":["get","cancel"],"evidenceRefs":[],"coverage":{"sources":{"dom":"not_enabled","frontendRuntime":"not_enabled","rustTrace":"not_enabled","businessState":"not_enabled"},"businessPersistence":"unobserved","correlation":"state_only","truncated":false}});
        let run = Arc::new(Mutex::new(Run {
            spec: Some(spec),
            report,
            outcomes: BTreeMap::new(),
            contexts: HashMap::new(),
            index: 0,
            started: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
            executing: true,
            run_key_hash: run_key_hash.clone(),
            spec_hash,
            principal_hash,
            secrets,
            events: Vec::new(),
            evidence: BTreeMap::new(),
            reservations: vec![reservation],
        }));
        registry.by_key.insert(run_key_hash, run_id.clone());
        registry.runs.insert(run_id, run.clone());
        drop(registry);
        {
            let mut entry = run.lock().await;
            if self.checkpoint(&mut entry, "run_created").await.is_err() {
                entry.report["status"] = json!("failed");
                entry.report["reason"] = json!("persistence_unavailable");
                entry.executing = false;
                return Ok(run.clone());
            }
        }
        let manager = self.clone();
        let task_run = run.clone();
        tokio::spawn(async move {
            manager.drive(task_run, backend).await;
        });
        Ok(run)
    }
}

fn error(code: &str, message: &str) -> Value {
    json!({"code":code,"stage":"workflow","message":message,"retryableBeforeDispatch":false})
}

/// Conservative accounting for retained JSON trees and map allocation overhead.
/// This is an admission budget, not a process resident-set measurement.
fn estimated_bytes(value: &Value) -> usize {
    let children = match value {
        Value::String(s) => s.capacity(),
        Value::Array(a) => a.iter().fold(
            a.capacity().saturating_mul(std::mem::size_of::<Value>()),
            |total, v| total.saturating_add(estimated_bytes(v)),
        ),
        Value::Object(o) => o.iter().fold(0usize, |total, (key, v)| {
            total
                .saturating_add(key.capacity())
                .saturating_add(96)
                .saturating_add(estimated_bytes(v))
        }),
        _ => 0,
    };
    std::mem::size_of::<Value>().saturating_add(children)
}

fn collect_strings(value: &Value, values: &mut Vec<String>) {
    match value {
        Value::String(s) if !s.is_empty() => values.push(s.clone()),
        Value::Number(n) => values.push(n.to_string()),
        Value::Array(a) => {
            for v in a {
                collect_strings(v, values)
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_strings(v, values)
            }
        }
        _ => {}
    }
}

fn collect_expression_secrets(value: &Value, secrets: &mut Vec<String>) {
    if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            if matches!(key.as_str(), "value" | "expected" | "key") {
                if value.is_string() || value.is_number() {
                    collect_strings(value, secrets);
                } else if let Some(literal) = value.get("literal") {
                    collect_strings(literal, secrets);
                }
            } else if key == "args" {
                collect_strings(value, secrets);
            }
            if value.is_object() {
                collect_expression_secrets(value, secrets);
            }
            if let Some(array) = value.as_array() {
                for child in array {
                    collect_expression_secrets(child, secrets);
                }
            }
        }
    }
}

fn terminal_actions(status: &str, unknown: bool) -> Vec<&'static str> {
    if unknown {
        return vec!["get", "reconcile", "cancel"];
    }
    match status {
        "paused" => vec!["get", "continue", "cancel"],
        "queued" | "running" | "cancelling" => vec!["get", "cancel"],
        _ => vec!["get"],
    }
}

impl WorkflowService {
    async fn drive(self: Arc<Self>, run: Arc<Mutex<Run>>, backend: Arc<dyn Backend>) {
        let (spec, started, cancel, notify) = {
            let entry = run.lock().await;
            (
                entry.spec.clone().unwrap(),
                entry.started,
                entry.cancel.clone(),
                entry.notify.clone(),
            )
        };
        let deadline = started + Duration::from_millis(spec.deadline_ms);
        let slot = tokio::select! {
            slot=self.slots.clone().acquire_owned()=>slot.ok(),
            _=notify.notified()=>None,
            _=tokio::time::sleep_until(deadline.into())=>None,
        };
        let Some(_slot) = slot else {
            self.stop(
                &run,
                if cancel.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "failed"
                },
                "condition_timeout",
                None,
            )
            .await;
            return;
        };
        // Unknown input handlers may invoke the backend and alter focus. This conservative
        // lease covers the entire segment and is inherited by internal tool execution.
        let lease = match self
            .resources
            .try_acquire(vec!["backend".into(), "ui".into()])
        {
            Ok(lease) => lease,
            Err(_) => {
                self.stop(
                    &run,
                    "paused",
                    "resource_busy",
                    Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                        "resource_busy",
                        "acquiring",
                        "Another operation owns the required resources",
                    ))),
                )
                .await;
                return;
            }
        };
        {
            let mut entry = run.lock().await;
            entry.report["status"] = json!("running");
            if self.checkpoint(&mut entry, "run_started").await.is_err() {
                entry.executing = false;
                entry.report["status"] = json!("paused");
                entry.report["reason"] = json!("persistence_unavailable");
                return;
            }
        }
        // A continuation rechecks the last verified boundary before touching the
        // remaining plan. Completed actions are never executed a second time.
        let previous = {
            let entry = run.lock().await;
            if entry.report["recoveryOccurred"] == true && entry.index > 0 {
                spec.steps[entry.index - 1]
                    .expect
                    .as_ref()
                    .map(|condition| {
                        let window = spec.steps[entry.index - 1]
                            .window_id
                            .clone()
                            .unwrap_or_else(|| spec.window_id.clone());
                        (
                            window.clone(),
                            resolve_condition(condition, &spec.inputs, &entry.outcomes),
                            entry.contexts.get(&window).cloned().unwrap_or(Value::Null),
                            results_data(&entry.outcomes),
                        )
                    })
            } else {
                None
            }
        };
        if let Some((window, condition, context, results)) = previous {
            let checked = match condition {
                Ok(condition) => {
                    observe_condition(
                        &*backend,
                        &window,
                        &serde_json::to_value(condition).unwrap(),
                        &context,
                        &results,
                        (Instant::now() + Duration::from_millis(spec.defaults.locator_timeout_ms))
                            .min(deadline),
                        &cancel,
                        &notify,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            if checked.is_err() {
                self.stop(
                    &run,
                    "paused",
                    "precondition_failed",
                    Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                        "precondition_failed",
                        "preparing",
                        "The last verified boundary no longer holds",
                    ))),
                )
                .await;
                return;
            }
        }
        loop {
            let index = run.lock().await.index;
            if index >= spec.steps.len() {
                break;
            }
            if cancel.load(Ordering::SeqCst) {
                self.stop(&run, "cancelled", "cancelled_before_dispatch", None)
                    .await;
                return;
            }
            if Instant::now() >= deadline {
                self.stop(&run, "failed", "condition_timeout", None).await;
                return;
            }
            let step_started = Instant::now();
            let step_deadline = (step_started
                + Duration::from_millis(
                    spec.steps[index]
                        .timeout_ms
                        .unwrap_or(spec.defaults.step_timeout_ms),
                ))
            .min(deadline);
            let resolved = {
                let entry = run.lock().await;
                resolve_step(&spec.steps[index], &spec.inputs, &entry.outcomes)
            };
            let step = match resolved {
                Ok(s) => s,
                Err(e) => {
                    self.stop(
                        &run,
                        "paused",
                        &e.code.clone(),
                        Some(ExecutionOutcome::not_dispatched(e)),
                    )
                    .await;
                    return;
                }
            };
            let window = step
                .window_id
                .as_deref()
                .unwrap_or(&spec.window_id)
                .to_string();
            let mut step_value = serde_json::to_value(&step).unwrap();
            let operation = step_value["op"].as_str().unwrap().to_string();
            let writing = matches!(operation.as_str(), "click" | "fill" | "type" | "press");
            let observation_id = uuid::Uuid::new_v4().to_string();
            let request_id = uuid::Uuid::new_v4().to_string();
            let mut context = {
                let entry = run.lock().await;
                entry.contexts.get(&window).cloned().unwrap_or_else(
                    || json!({"appInstanceId":self.app_instance_id,"windowId":window}),
                )
            };
            let prepare = json!({"cmd":"prepare","observationId":observation_id,"context":context,"step":step_value,"condition":step_value.get("expect"),"timeoutMs":remaining_ms(step_deadline)});
            let prepared = bounded_call(
                &*backend,
                &window,
                prepare,
                step_deadline,
                uuid::Uuid::new_v4().to_string(),
                false,
            )
            .await;
            let prepared = match prepared {
                Ok(v) if v["ok"] == true && v["ready"] == true => v,
                Ok(v) => {
                    let e = page_error(&v, "observation_failed");
                    cleanup(&*backend, &window, &observation_id, &context).await;
                    self.stop(
                        &run,
                        "paused",
                        &e.code.clone(),
                        Some(ExecutionOutcome::not_dispatched(e)),
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    cleanup(&*backend, &window, &observation_id, &context).await;
                    self.stop(
                        &run,
                        "paused",
                        &e.code.clone(),
                        Some(ExecutionOutcome::not_dispatched(e)),
                    )
                    .await;
                    return;
                }
            };
            if context.get("pageEpoch").is_some()
                && context["pageEpoch"] != prepared["context"]["pageEpoch"]
            {
                cleanup(&*backend, &window, &observation_id, &context).await;
                self.stop(
                    &run,
                    "paused",
                    "target_changed",
                    Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                        "target_changed",
                        "preparing",
                        "Page identity changed",
                    ))),
                )
                .await;
                return;
            }
            context = prepared["context"].clone();
            context["pollIntervalMs"] = json!(spec.defaults.poll_interval_ms);
            {
                let mut entry = run.lock().await;
                entry.contexts.insert(window.clone(), context.clone());
                entry.report["coverage"]["sources"]["dom"] = json!("available");
                entry.report["blockedStep"] = json!(step.id);
                entry.report["dispatchContext"] = super::security::dispatch_metadata(&context);
                entry.report["dispatchContext"]["requestId"] = json!(request_id);
                entry.report["dispatchContext"]["windowId"] = json!(window);
                entry.report["dispatchContext"]["attempt"] = json!(1);
                if self.checkpoint(&mut entry, "step_prepared").await.is_err() {
                    drop(entry);
                    cleanup(&*backend, &window, &observation_id, &context).await;
                    self.stop(
                        &run,
                        "paused",
                        "persistence_unavailable",
                        Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                            "persistence_unavailable",
                            "preparing",
                            "Cannot persist step preparation",
                        ))),
                    )
                    .await;
                    return;
                }
            }
            if cancel.load(Ordering::SeqCst) || Instant::now() >= step_deadline {
                cleanup(&*backend, &window, &observation_id, &context).await;
                self.stop(
                    &run,
                    if cancel.load(Ordering::SeqCst) {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    "cancelled_before_dispatch",
                    None,
                )
                .await;
                return;
            }
            if writing {
                let mut entry = run.lock().await;
                entry.report["mayHaveEffects"] = json!(true);
                if self
                    .checkpoint(&mut entry, "step_dispatch_intent")
                    .await
                    .is_err()
                {
                    drop(entry);
                    cleanup(&*backend, &window, &observation_id, &context).await;
                    self.stop(
                        &run,
                        "paused",
                        "persistence_unavailable",
                        Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                            "persistence_unavailable",
                            "dispatching",
                            "Dispatch intent is not durable",
                        ))),
                    )
                    .await;
                    return;
                }
            }
            if cancel.load(Ordering::SeqCst) || Instant::now() >= step_deadline {
                cleanup(&*backend, &window, &observation_id, &context).await;
                self.stop(
                    &run,
                    if cancel.load(Ordering::SeqCst) {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    "cancelled_before_dispatch",
                    Some(ExecutionOutcome::not_dispatched(WorkflowError::new(
                        "cancelled_before_dispatch",
                        "dispatching",
                        "Dispatch stopped at deadline or cancellation boundary",
                    ))),
                )
                .await;
                return;
            }
            let execution_started = Instant::now();
            let outcome = if operation == "wait" {
                let results = results_data(&run.lock().await.outcomes);
                match wait_condition(
                    &*backend,
                    &window,
                    &step_value["condition"],
                    &context,
                    &observation_id,
                    &results,
                    step_deadline,
                    &cancel,
                    &notify,
                )
                .await
                {
                    Ok(data) => ExecutionOutcome::completed(data, EffectStatus::None),
                    Err(e) => ExecutionOutcome::failed(e, EffectStatus::None),
                }
            } else {
                let request = if operation == "tool" {
                    json!({"cmd":"tool","tool":step_value["tool"],"args":step_value["args"]})
                } else {
                    json!({"cmd":"execute","observationId":observation_id,"context":context,"step":step_value,"timeoutMs":remaining_ms(step_deadline)})
                };
                let locator_deadline = (Instant::now()
                    + Duration::from_millis(spec.defaults.locator_timeout_ms))
                .min(step_deadline);
                let response = loop {
                    if cancel.load(Ordering::SeqCst) {
                        break Ok(
                            json!({"ok":false,"dispatched":false,"error":{"code":"cancel_requested","stage":"preparing","message":"Cancellation requested before dispatch"}}),
                        );
                    }
                    let response = bounded_call(
                        &*backend,
                        &window,
                        request.clone(),
                        step_deadline,
                        request_id.clone(),
                        writing,
                    )
                    .await;
                    let may_requery = response.as_ref().is_ok_and(|value| {
                        value["ok"] == false
                            && value["dispatched"] == false
                            && matches!(
                                value["error"]["code"].as_str(),
                                Some("target_not_found" | "not_actionable")
                            )
                    });
                    if !may_requery || Instant::now() >= locator_deadline {
                        break response;
                    }
                    tokio::select! {
                        _=tokio::time::sleep(Duration::from_millis(spec.defaults.poll_interval_ms.min(remaining_ms(locator_deadline))))=>{},
                        _=notify.notified()=>{},
                    }
                };
                match response {
                    Ok(value) if value["ok"] == true && value["dispatched"] == writing => {
                        let mut out = ExecutionOutcome::completed(
                            value["data"].clone(),
                            if writing {
                                EffectStatus::Confirmed
                            } else {
                                EffectStatus::None
                            },
                        );
                        out.effect_scope = value
                            .get("effectScope")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        out
                    }
                    Ok(value) => {
                        let e = page_error(&value, "protocol_mismatch");
                        if value["dispatched"] == true {
                            ExecutionOutcome::failed(
                                e,
                                if writing {
                                    EffectStatus::Possible
                                } else {
                                    EffectStatus::None
                                },
                            )
                        } else if value["dispatched"] == false {
                            ExecutionOutcome::not_dispatched(e)
                        } else if writing {
                            ExecutionOutcome::unknown(WorkflowError::new(
                                "protocol_mismatch",
                                "executing",
                                "Page response did not establish dispatch state",
                            ))
                        } else {
                            ExecutionOutcome::failed(e, EffectStatus::None)
                        }
                    }
                    Err(e) if e.code == "outcome_unknown" => {
                        let mut out = ExecutionOutcome::unknown(e);
                        if !writing {
                            out.effect = EffectStatus::None;
                        }
                        out
                    }
                    Err(e)
                        if matches!(
                            e.code.as_str(),
                            "target_not_found" | "target_changed" | "unauthorized"
                        ) =>
                    {
                        ExecutionOutcome::not_dispatched(e)
                    }
                    Err(e) => ExecutionOutcome::failed(
                        e,
                        if writing {
                            EffectStatus::Possible
                        } else {
                            EffectStatus::None
                        },
                    ),
                }
            };
            let mut outcome = outcome;
            outcome.timing = json!({"preparingMs":execution_started.duration_since(step_started).as_millis(),"executionMs":execution_started.elapsed().as_millis()});
            let reservation = self
                .memory
                .try_reserve(estimated_bytes(&outcome.data).saturating_mul(4));
            match reservation {
                Ok(reservation) => run.lock().await.reservations.push(reservation),
                Err(_) => {
                    outcome.data = Value::Null;
                    outcome.error = Some(WorkflowError::new(
                        "capture_incomplete",
                        "retaining",
                        "Result exceeded the remaining host retention budget",
                    ));
                    outcome.verification = VerificationStatus::Inconclusive;
                    outcome.coverage = json!({"truncated":true});
                }
            }
            outcome.dispatch = super::security::dispatch_metadata(&context);
            outcome.dispatch["requestId"] = json!(request_id);
            outcome.dispatch["windowId"] = json!(window);
            outcome.dispatch["dispatched"] =
                json!(writing && outcome.execution != ExecutionStatus::NotDispatched);
            outcome.coverage["correlation"] = json!("state_only");
            outcome.coverage["businessPersistence"] = json!("unobserved");
            // Preserve the completion result before verification or optional evidence work.
            {
                let mut entry = run.lock().await;
                entry.report["blockedOutcome"] = serde_json::to_value(&outcome).unwrap();
                if self
                    .checkpoint(&mut entry, "step_execution_result")
                    .await
                    .is_err()
                {
                    drop(entry);
                    cleanup(&*backend, &window, &observation_id, &context).await;
                    if outcome.effect != EffectStatus::None && writing {
                        lease.quarantine("outcome_unknown");
                        run.lock().await.report["resourceIsolation"] = json!("quarantined");
                    }
                    self.stop(&run, "paused", "persistence_unavailable", Some(outcome))
                        .await;
                    return;
                }
            }
            let verification_started = Instant::now();
            if outcome.execution == ExecutionStatus::Completed
                && outcome.error.is_none()
                && step.expect.is_some()
            {
                let mut results = results_data(&run.lock().await.outcomes);
                results.insert(step.id.clone(), outcome.data.clone());
                let expectation = step_value.take()["expect"].clone();
                match wait_condition(
                    &*backend,
                    &window,
                    &expectation,
                    &context,
                    &observation_id,
                    &results,
                    step_deadline,
                    &cancel,
                    &notify,
                )
                .await
                {
                    Ok(_) => outcome.verification = VerificationStatus::Passed,
                    Err(mut e) => {
                        if e.code == "condition_timeout" {
                            e.code = "postcondition_failed".into();
                        }
                        outcome.verification = if e.code == "cancel_requested" {
                            VerificationStatus::Inconclusive
                        } else {
                            VerificationStatus::Failed
                        };
                        outcome.error = Some(e);
                    }
                }
            }
            outcome.timing["verificationMs"] = json!(verification_started.elapsed().as_millis());
            outcome.timing["totalMs"] = json!(step_started.elapsed().as_millis());
            if !outcome.is_success(step.expect.is_some()) {
                if writing
                    && (outcome.effect == EffectStatus::Possible
                        || (outcome.execution == ExecutionStatus::Completed
                            && matches!(
                                outcome.verification,
                                VerificationStatus::Failed | VerificationStatus::Inconclusive
                            )))
                {
                    lease.quarantine("outcome_unknown");
                    run.lock().await.report["resourceIsolation"] = json!("quarantined");
                }
                let evidence_started = Instant::now();
                let grace = spec.defaults.failure_evidence_grace_ms;
                let evidence_bytes = step
                    .evidence
                    .as_ref()
                    .unwrap_or(&spec.evidence)
                    .max_inline_bytes
                    .min(2048);
                let evidence_request = json!({"cmd":"evidence","context":context,"observationId":observation_id,"target":serde_json::to_value(&step).unwrap().get("target"),"maxBytes":evidence_bytes});
                let evidence = if grace == 0 {
                    None
                } else {
                    tokio::time::timeout(
                        Duration::from_millis(grace),
                        backend.call(
                            &window,
                            evidence_request,
                            grace,
                            uuid::Uuid::new_v4().to_string(),
                        ),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok)
                };
                if let Some(value) = evidence {
                    let mut entry = run.lock().await;
                    let id = uuid::Uuid::new_v4().to_string();
                    // Only structural details are retained; raw DOM values/text are not evidence-safe.
                    entry.evidence.insert(id.clone(),json!({"captureKind":"scoped_snapshot","context":super::security::dispatch_metadata(&context),"available":value["ok"]==true,"details":{"elements":value["elements"],"scope":value["scope"],"truncated":value["truncated"]}}));
                    entry.report["evidenceRefs"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(id));
                    outcome.evidence_refs.push(id);
                }
                outcome.timing["evidenceMs"] = json!(evidence_started.elapsed().as_millis());
                cleanup(&*backend, &window, &observation_id, &context).await;
                let reason = outcome
                    .error
                    .as_ref()
                    .map(|e| e.code.as_str())
                    .unwrap_or("execution_failed")
                    .to_string();
                self.stop(
                    &run,
                    if cancel.load(Ordering::SeqCst) {
                        "cancelled"
                    } else {
                        "paused"
                    },
                    &reason,
                    Some(outcome),
                )
                .await;
                return;
            }
            cleanup(&*backend, &window, &observation_id, &context).await;
            {
                let mut entry = run.lock().await;
                entry.outcomes.insert(step.id.clone(), outcome.clone());
                entry.index += 1;
                entry.report["steps"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":step.id,"status":"succeeded","outcome":outcome}));
                entry.report["summary"] = json!({"completedSteps":entry.index,"remainingSteps":spec.steps.len()-entry.index});
                entry
                    .report
                    .as_object_mut()
                    .unwrap()
                    .remove("blockedOutcome");
                entry.report.as_object_mut().unwrap().remove("blockedStep");
                if self
                    .checkpoint(&mut entry, "step_verification_result")
                    .await
                    .is_err()
                {
                    entry.executing = false;
                    entry.report["status"] = json!("paused");
                    entry.report["reason"] = json!("persistence_unavailable");
                    return;
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            self.stop(&run, "cancelled", "cancel_requested", None).await;
            return;
        }
        if let Some(goal) = &spec.goal {
            let (condition, context, results) = {
                let entry = run.lock().await;
                (
                    resolve_condition(goal, &spec.inputs, &entry.outcomes),
                    entry.contexts.get(&spec.window_id).cloned().unwrap_or_else(
                        || json!({"appInstanceId":self.app_instance_id,"windowId":spec.window_id}),
                    ),
                    results_data(&entry.outcomes),
                )
            };
            let verdict = match condition {
                Ok(condition) => observe_condition(
                    &*backend,
                    &spec.window_id,
                    &serde_json::to_value(condition).unwrap(),
                    &context,
                    &results,
                    deadline,
                    &cancel,
                    &notify,
                )
                .await
                .map(|_| ()),
                Err(e) => Err(e),
            };
            if let Err(e) = verdict {
                run.lock().await.report["goalStatus"] = json!("failed");
                self.stop(&run, "failed", &e.code, None).await;
                return;
            }
            run.lock().await.report["goalStatus"] = json!("passed");
        }
        self.stop(&run, "completed", "", None).await;
    }

    async fn stop(
        &self,
        run: &Arc<Mutex<Run>>,
        status: &str,
        reason: &str,
        outcome: Option<ExecutionOutcome>,
    ) {
        let mut entry = run.lock().await;
        entry.executing = false;
        entry.report["status"] = json!(status);
        if !reason.is_empty() {
            entry.report["reason"] = json!(reason);
        }
        let unknown = outcome
            .as_ref()
            .is_some_and(|o| o.execution == ExecutionStatus::OutcomeUnknown);
        if let Some(outcome) = outcome {
            entry.report["blockedOutcome"] = serde_json::to_value(outcome).unwrap();
        }
        let old = entry.report["originalTestVerdict"]
            .as_str()
            .unwrap_or("not_requested");
        if !matches!(old, "failed" | "inconclusive") {
            entry.report["originalTestVerdict"] = json!(if status == "completed" {
                "passed"
            } else if unknown || status == "cancelled" {
                "inconclusive"
            } else {
                "failed"
            });
        }
        let blocked_dispatched = entry.report["blockedOutcome"]["execution"]
            .as_str()
            .is_some_and(|s| s != "not_dispatched");
        entry.report["allowedNextActions"] = json!(if status == "paused" && blocked_dispatched {
            vec!["get", "reconcile", "cancel"]
        } else {
            terminal_actions(status, unknown)
        });
        let _ = self.checkpoint(&mut entry, &format!("run_{status}")).await;
        entry.notify.notify_waiters();
    }
}

fn remaining_ms(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn page_error(value: &Value, fallback: &str) -> WorkflowError {
    WorkflowError::new(
        value["error"]["code"].as_str().unwrap_or(fallback),
        value["error"]["stage"].as_str().unwrap_or("executing"),
        value["error"]["message"]
            .as_str()
            .unwrap_or("Page operation failed"),
    )
}
fn results_data(outcomes: &BTreeMap<String, ExecutionOutcome>) -> BTreeMap<String, Value> {
    outcomes
        .iter()
        .map(|(k, v)| (k.clone(), v.data.clone()))
        .collect()
}

async fn bounded_call(
    backend: &dyn Backend,
    window: &str,
    request: Value,
    deadline: Instant,
    request_id: String,
    writing: bool,
) -> Result<Value, WorkflowError> {
    if Instant::now() >= deadline {
        return Err(WorkflowError::new(
            "condition_timeout",
            "preparing",
            "No remaining execution budget",
        ));
    }
    tokio::time::timeout_at(
        deadline.into(),
        backend.call(window, request, remaining_ms(deadline), request_id),
    )
    .await
    .unwrap_or_else(|_| {
        Err(WorkflowError::new(
            if writing {
                "outcome_unknown"
            } else {
                "condition_timeout"
            },
            "executing",
            "Operation exceeded its remaining deadline",
        ))
    })
}

#[allow(clippy::too_many_arguments)]
async fn observe_condition(
    backend: &dyn Backend,
    window: &str,
    condition: &Value,
    context: &Value,
    results: &BTreeMap<String, Value>,
    deadline: Instant,
    cancel: &AtomicBool,
    notify: &Notify,
) -> Result<Value, WorkflowError> {
    let id = uuid::Uuid::new_v4().to_string();
    let prepared=bounded_call(backend,window,json!({"cmd":"prepare","observationId":id,"context":context,"condition":condition,"results":results,"timeoutMs":remaining_ms(deadline)}),deadline,uuid::Uuid::new_v4().to_string(),false).await;
    let result = match prepared {
        Ok(value) if value["ok"] == true && value["ready"] == true => {
            wait_condition(
                backend, window, condition, context, &id, results, deadline, cancel, notify,
            )
            .await
        }
        Ok(value) => Err(page_error(&value, "observation_failed")),
        Err(error) => Err(error),
    };
    cleanup(backend, window, &id, context).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn wait_condition(
    backend: &dyn Backend,
    window: &str,
    condition: &Value,
    context: &Value,
    observation_id: &str,
    results: &BTreeMap<String, Value>,
    deadline: Instant,
    cancel: &AtomicBool,
    notify: &Notify,
) -> Result<Value, WorkflowError> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(WorkflowError::new(
                "cancel_requested",
                "waiting",
                "Cancellation requested",
            ));
        }
        if Instant::now() >= deadline {
            return Err(WorkflowError::new(
                "condition_timeout",
                "waiting",
                "Condition did not become true within the remaining deadline",
            ));
        }
        let request = json!({"cmd":"condition","condition":condition,"context":context,"observationId":observation_id,"results":results});
        let response = tokio::select! {
            value=bounded_call(backend,window,request,deadline,uuid::Uuid::new_v4().to_string(),false)=>value?,
            _=notify.notified()=>return Err(WorkflowError::new("cancel_requested","waiting","Cancellation requested")),
        };
        if response["ok"] != true {
            return Err(page_error(&response, "observation_failed"));
        }
        if response["satisfied"] == true {
            return Ok(response);
        }
        tokio::select! {
            _=tokio::time::sleep(Duration::from_millis(remaining_ms(deadline).min(context["pollIntervalMs"].as_u64().unwrap_or(100))))=>{},
            _=notify.notified()=>return Err(WorkflowError::new("cancel_requested","waiting","Cancellation requested")),
        }
    }
}

async fn cleanup(backend: &dyn Backend, window: &str, observation_id: &str, context: &Value) {
    let _ = tokio::time::timeout(
        Duration::from_millis(250),
        backend.call(
            window,
            json!({"cmd":"cleanup","observationId":observation_id,"context":context}),
            250,
            uuid::Uuid::new_v4().to_string(),
        ),
    )
    .await;
}

pub async fn call(
    name: &str,
    args: &Value,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Result<Value, Value> {
    let manager = &state.workflow;
    if name == "workflow_capabilities" {
        if !args.is_object()
            || args
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key != "windowId")
            || args
                .get("windowId")
                .is_some_and(|v| v.as_str().is_none_or(str::is_empty))
        {
            return Err(error(
                "invalid_spec",
                "Capabilities accepts only an optional nonempty windowId",
            ));
        }
        let mut capabilities = json!({"schemaVersions":[1],"modes":["strict"],"schedules":["sequential"],"ops":["click","fill","type","press","wait","query","tool"],"conditions":["element","valueEquals","textContains","attributeEquals","result","all","any"],"tools":["bridge_status","ipc_get_backend_state"],"authentication":{"required":true,"configured":manager.token.lock().unwrap_or_else(|p|p.into_inner()).is_some(),"minimumTokenBytes":32},"recovery":{"clientReconnect":true,"sameProcessContinue":"undispatched_only","restartHistory":cfg!(unix),"automaticRestartReplay":false,"unknownWriteReplay":false},"journal":{"durability":"sync_data","privateStorage":cfg!(unix),"dedupRetention":"no_eviction_until_host_archive","maxRuns":MAX_RUNS},"unsupported":["ref","parallel","arbitraryJavaScript","unknownIpc","transitionConditions","businessPersistence","automaticRollback","stabilityMs"],"coverage":{"correlation":"state_only","businessPersistence":"unobserved","dom":"light_dom","nativeInput":false},"limits":{"activeRuns":4,"queuedRuns":16,"quarantinedScopes":manager.resources.quarantined(),"maxSteps":100,"maxSpecBytes":262144,"maxDeadlineMs":300000,"retentionBudgetBytes":manager.memory.limit_bytes(),"retentionChargedBytes":manager.memory.used_bytes()}});
        capabilities["inspection"] = crate::capabilities::inspection();
        return Ok(capabilities);
    }
    let principal = manager.authorize(args)?;
    let journal = manager.store().await?;
    let principal_hash = journal
        .fingerprint(&json!({"principal":principal}))
        .map_err(|_| error("persistence_unavailable", "Cannot read principal"))?;
    let backend: Arc<dyn Backend> = Arc::new(AppBackend {
        bridge: bridge.clone(),
        app: app.cloned(),
        state: state.clone(),
        principal: principal.clone(),
    });
    let allowed: &[&str] = match name {
        "workflow_run" => &["spec", "waitMs", "authToken"],
        "workflow_get" => &[
            "runId",
            "cursor",
            "include",
            "evidenceId",
            "offset",
            "authToken",
        ],
        "workflow_cancel" => &["runId", "authToken"],
        "workflow_resume" => &[
            "runId",
            "expectedRevision",
            "checkpointId",
            "intent",
            "authToken",
        ],
        _ => return Err(error("unsupported_feature", "Unknown workflow operation")),
    };
    if !args.is_object()
        || args
            .as_object()
            .unwrap()
            .keys()
            .any(|k| !allowed.contains(&k.as_str()))
    {
        return Err(error("invalid_spec", "Unknown operation argument"));
    }
    let wait_ms = if name == "workflow_run" {
        args.get("waitMs")
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n <= 30000)
                    .ok_or_else(|| error("invalid_spec", "waitMs must be 0..30000"))
            })
            .transpose()?
            .unwrap_or(1000)
    } else {
        0
    };
    if name == "workflow_resume"
        && (args
            .get("expectedRevision")
            .and_then(Value::as_u64)
            .is_none()
            || args
                .get("checkpointId")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty))
    {
        return Err(error(
            "invalid_spec",
            "Resume requires a revision and nonempty checkpointId",
        ));
    }
    let run = if name == "workflow_run" {
        let spec =
            WorkflowSpec::parse(&args["spec"]).map_err(|e| serde_json::to_value(e).unwrap())?;
        manager.create(spec, &principal, backend.clone()).await?
    } else {
        let id = args
            .get("runId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("invalid_spec", "runId is required"))?;
        manager.find(id, &principal_hash).await?
    };
    if name == "workflow_run" {
        let until = Instant::now() + Duration::from_millis(wait_ms);
        while run.lock().await.executing && Instant::now() < until {
            tokio::time::sleep(
                Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
    if name == "workflow_cancel" {
        let mut entry = run.lock().await;
        if entry.spec.is_none() {
            return Err(error(
                "app_instance_changed",
                "Historical runs are read-only after restart",
            ));
        }
        if !entry.cancel.swap(true, Ordering::SeqCst) {
            if entry.executing {
                entry.report["status"] = json!("cancelling");
            } else if entry.report["status"] != "completed" {
                entry.report["status"] = json!("cancelled");
            }
            entry.report["cancellation"] =
                json!({"requested":true,"inFlight":entry.executing,"rollback":false});
            let _ = manager.checkpoint(&mut entry, "cancel_requested").await;
            entry.notify.notify_waiters();
        }
    }
    if name == "workflow_resume" {
        resume_run(manager, &run, args, backend).await?;
    }
    let entry = run.lock().await;
    let mut report = entry.report.clone();
    let budget = entry
        .spec
        .as_ref()
        .map(|s| s.evidence.max_inline_bytes)
        .unwrap_or(16384);
    if name == "workflow_get" {
        if let Some(evidence_id) = args.get("evidenceId") {
            let id = evidence_id
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| error("invalid_spec", "evidenceId must be a nonempty identifier"))?;
            let evidence = entry.evidence.get(id).ok_or_else(|| {
                error(
                    "run_expired",
                    "Evidence is not retained in this application instance",
                )
            })?;
            let mut wrapper = json!({"evidence":{}});
            wrapper["evidence"][id] = evidence.clone();
            let safe = super::security::public_report(&wrapper, &entry.secrets);
            let content = serde_json::to_string(&safe["evidence"][id]).unwrap();
            let offset = args
                .get("offset")
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| error("invalid_spec", "offset must be a UTF-8 byte offset"))
                })
                .transpose()?
                .unwrap_or(0);
            let offset = usize::try_from(offset)
                .map_err(|_| error("invalid_spec", "Evidence offset is out of range"))?;
            if offset > content.len() || !content.is_char_boundary(offset) {
                return Err(error(
                    "invalid_spec",
                    "Evidence offset is not a UTF-8 character boundary",
                ));
            }
            let mut end = content
                .len()
                .min(offset.saturating_add((budget / 12).clamp(32, 1024)));
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            return Ok(
                json!({"runId":entry.report["runId"],"revision":entry.report["revision"],"status":entry.report["status"],"evidencePage":{"id":id,"offset":offset,"content":&content[offset..end],"nextOffset":if end<content.len(){Some(end)}else{None},"totalBytes":content.len()}}),
            );
        }
        if args.get("offset").is_some() {
            return Err(error("invalid_spec", "offset requires evidenceId"));
        }
        let cursor = args
            .get("cursor")
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| error("invalid_spec", "cursor must be an event sequence"))
            })
            .transpose()?
            .unwrap_or(0);
        report["events"] = json!(
            entry
                .events
                .iter()
                .filter(|v| v["seq"].as_u64().unwrap_or(0) > cursor)
                .collect::<Vec<_>>()
        );
        if let Some(include) = args.get("include") {
            let includes = include
                .as_array()
                .ok_or_else(|| error("invalid_spec", "include must be an array"))?;
            if includes
                .iter()
                .any(|v| !matches!(v.as_str(), Some("steps" | "events" | "evidence")))
            {
                return Err(error("invalid_spec", "Unknown include projection"));
            }
            if includes.iter().any(|v| v == "evidence") {
                report["evidence"] = serde_json::to_value(&entry.evidence).unwrap();
            }
        }
    }
    if let Some(spec) = &entry.spec {
        for (index, step) in report["steps"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let policy = spec
                .steps
                .get(index)
                .and_then(|s| s.evidence.as_ref())
                .unwrap_or(&spec.evidence);
            if step.to_string().len() > policy.max_inline_bytes
                && let Some(outcome) = step.get_mut("outcome").and_then(Value::as_object_mut)
            {
                outcome.remove("data");
                outcome.entry("coverage").or_insert(json!({}))["truncated"] = json!(true);
            }
        }
        if let Some(policy) = spec
            .steps
            .get(entry.index)
            .and_then(|s| s.evidence.as_ref())
            && report["blockedOutcome"].to_string().len() > policy.max_inline_bytes
            && let Some(outcome) = report
                .get_mut("blockedOutcome")
                .and_then(Value::as_object_mut)
        {
            outcome.remove("data");
            outcome.entry("coverage").or_insert(json!({}))["truncated"] = json!(true);
        }
    }
    Ok(super::output::project(
        super::security::public_report(&report, &entry.secrets),
        budget,
    ))
}

async fn resume_run(
    manager: &Arc<WorkflowService>,
    run: &Arc<Mutex<Run>>,
    args: &Value,
    backend: Arc<dyn Backend>,
) -> Result<(), Value> {
    let intent = args
        .get("intent")
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_spec", "Resume intent is required"))?;
    if !matches!(intent, "continue" | "reconcile") {
        return Err(error("unsupported_feature", "Unsupported resume intent"));
    }
    {
        let mut entry = run.lock().await;
        if args.get("expectedRevision") != entry.report.get("revision")
            || args.get("checkpointId") != entry.report.get("checkpointId")
        {
            return Err(error("stale_checkpoint", "Run revision/checkpoint changed"));
        }
        if entry.executing {
            return Err(error("resume_not_safe", "Run is still executing"));
        }
        if entry.spec.is_none() {
            return Err(error(
                "app_instance_changed",
                "Historical run is read-only after application restart",
            ));
        }
        if intent == "continue" {
            if entry.cancel.load(Ordering::SeqCst)
                || entry.report["status"] != "paused"
                || entry.report["blockedOutcome"]["execution"] != "not_dispatched"
            {
                return Err(error(
                    "resume_not_safe",
                    "Only undispatched paused steps can continue",
                ));
            }
            if entry.started.elapsed().as_millis()
                >= entry.spec.as_ref().unwrap().deadline_ms as u128
            {
                return Err(error(
                    "resume_not_safe",
                    "Original run deadline has expired",
                ));
            }
            entry.executing = true;
            entry.report["recoveryOccurred"] = json!(true);
            entry.report["status"] = json!("queued");
            if manager
                .checkpoint(&mut entry, "continue_requested")
                .await
                .is_err()
            {
                entry.executing = false;
                return Err(error(
                    "persistence_unavailable",
                    "Cannot persist continuation",
                ));
            }
        } else {
            entry.report["recoveryOccurred"] = json!(true);
            manager
                .checkpoint(&mut entry, "reconciliation_requested")
                .await?;
        }
    }
    if intent == "continue" {
        let manager = manager.clone();
        let next = run.clone();
        tokio::spawn(async move {
            manager.drive(next, backend).await;
        });
    } else {
        reconcile(manager, run, &*backend).await?;
    }
    Ok(())
}

async fn reconcile(
    manager: &WorkflowService,
    run: &Arc<Mutex<Run>>,
    backend: &dyn Backend,
) -> Result<(), Value> {
    let (window, condition, context, results, cancel, notify) = {
        let entry = run.lock().await;
        let spec = entry.spec.as_ref().unwrap();
        let step = spec
            .steps
            .get(entry.index)
            .ok_or_else(|| error("resume_not_safe", "No blocked step to reconcile"))?;
        let window = step
            .window_id
            .clone()
            .unwrap_or_else(|| spec.window_id.clone());
        let condition = step.expect.as_ref().ok_or_else(|| {
            error(
                "resume_not_safe",
                "No read-only postcondition is available; inspect diagnostics",
            )
        })?;
        let condition = resolve_condition(condition, &spec.inputs, &entry.outcomes)
            .map_err(|e| serde_json::to_value(e).unwrap())?;
        let mut results = results_data(&entry.outcomes);
        if let Some(data) = entry.report["blockedOutcome"].get("data") {
            results.insert(step.id.clone(), data.clone());
        }
        (
            window.clone(),
            serde_json::to_value(condition).unwrap(),
            entry.contexts.get(&window).cloned().unwrap_or(Value::Null),
            results,
            AtomicBool::new(false),
            Notify::new(),
        )
    };
    let result = observe_condition(
        backend,
        &window,
        &condition,
        &context,
        &results,
        Instant::now() + Duration::from_millis(1000),
        &cancel,
        &notify,
    )
    .await;
    let mut entry = run.lock().await;
    entry.report["lateOutcome"] = json!({"verification":if result.is_ok(){"passed"}else{"inconclusive"},"correlation":"state_only","businessPersistence":"unobserved","originalActionReplayed":false,"quarantineReleased":false});
    manager
        .checkpoint(&mut entry, "reconciliation_result")
        .await?;
    Ok(())
}

// Successful durable workflow scenarios require the supported private-journal
// implementation. Non-Unix targets test the actual fail-closed contract below.
#[cfg(all(test, unix))]
#[path = "service_tests.rs"]
mod integration_tests;

#[cfg(all(test, not(unix)))]
#[path = "service_tests_no_private_journal.rs"]
mod unavailable_journal_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_authorization_generation_revokes_and_never_exposes_credentials() {
        const PRINCIPAL: &str = "fixture-service-token-32-bytes-long-12345";
        let service = WorkflowService::new(
            std::env::temp_dir().join(format!("inspection-auth-{}", uuid::Uuid::new_v4())),
        );
        service.set_token(Some(PRINCIPAL.into()));
        let generation = service
            .inspection_authorize(&json!({"authToken":PRINCIPAL}))
            .unwrap();
        assert!(service.inspection_authorized(generation));
        service.set_token(Some(PRINCIPAL.into()));
        assert!(service.inspection_authorized(generation));
        service.set_token(None);
        assert!(!service.inspection_authorized(generation));
        assert!(
            service
                .inspection_authorize(&json!({"authToken":PRINCIPAL}))
                .is_err()
        );
        service.set_token(Some(PRINCIPAL.into()));
        assert!(!service.inspection_authorized(generation));
    }

    #[test]
    fn cancelling_never_means_rollback() {
        assert_eq!(terminal_actions("cancelled", false), vec!["get"]);
        assert_eq!(
            terminal_actions("paused", true),
            vec!["get", "reconcile", "cancel"]
        );
    }

    #[test]
    fn redaction_removes_values_keys_and_error_echoes() {
        let input = serde_json::json!({"password":"hunter2", "note":"failed: hunter2", "data":{"fromInput":{"key":"not code"}}});
        let redacted = super::super::security::redact_payload(&input, &["hunter2".to_string()]);
        assert!(!redacted.to_string().contains("hunter2"));
        assert_eq!(redacted["data"]["fromInput"]["key"], "not code");
    }

    #[test]
    fn budget_trims_data_without_erasing_unknown_outcomes() {
        let report = serde_json::json!({"status":"paused","blockedOutcome":{"execution":"outcome_unknown","effect":"possible"},"steps":[{"data":"x".repeat(20000)}]});
        let small = super::super::output::project(report, 1024);
        assert_eq!(small["blockedOutcome"]["execution"], "outcome_unknown");
        assert_eq!(small["blockedOutcome"]["effect"], "possible");
        assert!(small.to_string().len() < 2048);
    }
}
