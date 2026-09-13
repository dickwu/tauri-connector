//! Application-owned active selection. Transport requests are bounded waiters,
//! never owners of the page interaction, deadline, result or resource lease.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use connector_client::inspection::PickerRequest;
use serde_json::{Value, json};
use tauri::Manager;
use tokio::sync::Notify;

use crate::{
    bridge::Bridge,
    state::PluginState,
    workflow::resources::{Lease, ResourceArbiter},
};

pub(crate) const MAX_ACTIVE: usize = 4;
const MAX_RETAINED: usize = 128;
const RETENTION: Duration = Duration::from_secs(300);
const MAX_METADATA: usize = 16 * 1024;
const MAX_IMAGE_TOTAL: usize = 32 * 1024 * 1024;
const CLEANUP_MS: u64 = 2200;
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn error(code: &str) -> Value {
    json!({"code":code,"stage":"inspection","businessActionDispatched":false})
}

pub struct PickerService {
    registry: Arc<Mutex<Registry>>,
}
#[derive(Default)]
struct Registry {
    records: HashMap<String, Arc<Record>>,
    keys: HashMap<(u64, String), String>,
    gone: VecDeque<String>,
}
impl Registry {
    fn reap(&mut self) {
        let expired: Vec<_> = self
            .records
            .iter()
            .filter_map(|(id, record)| {
                let e = lock(&record.inner);
                (e.complete && e.lease.is_none() && Instant::now() >= e.retained_until)
                    .then(|| id.clone())
            })
            .collect();
        for id in expired {
            self.records.remove(&id);
            self.keys.retain(|_, v| v != &id);
            self.gone.push_back(id);
        }
        while self.gone.len() > 256 {
            self.gone.pop_front();
        }
    }
}
impl Default for PickerService {
    fn default() -> Self {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let weak = Arc::downgrade(&registry);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(registry) = weak.upgrade() else {
                    break;
                };
                lock(&registry).reap();
            }
        });
        Self { registry }
    }
}
struct Record {
    inner: Mutex<Entry>,
    notify: Notify,
}
struct Entry {
    id: String,
    nonce: String,
    generation: u64,
    request: PickerRequest,
    deadline: Instant,
    deadline_ms: u64,
    retained_until: Instant,
    retained_ms: u64,
    context: Value,
    status: String,
    revision: u64,
    sequence: u64,
    selection: Option<Value>,
    screenshot: Value,
    image: Option<Vec<u8>>,
    cleanup: String,
    cleanup_deadline: Option<Instant>,
    complete: bool,
    lease: Option<Lease>,
    warnings: Vec<String>,
    error: Option<Value>,
    cancel_capture: bool,
    metadata_reservation: Option<crate::workflow::budget::Reservation>,
    image_reservation: Option<crate::workflow::budget::Reservation>,
    connection_pin: Option<crate::bridge::BridgeConnectionPin>,
}
impl Entry {
    fn retain_after_slow_status(
        &self,
        command: &str,
        failure: &Value,
        connection_live: bool,
        probe: Result<&Value, &crate::bridge::BridgeError>,
    ) -> bool {
        self.active()
            && command == "status"
            && failure["transportFailure"] == true
            && connection_live
            && match probe {
                Ok(p) => {
                    p["pageEpoch"] == self.context["pageEpoch"]
                        && p["runtime"]["ready"] == true
                        && p["runtime"]["context"] == self.context
                }
                // Only an elapsed response wait is inconclusive. Host target,
                // authorization, script and channel failures remain terminal.
                Err(crate::bridge::BridgeError::DispatchedOutcomeUnknown { reason, .. }) => {
                    matches!(
                        reason.as_str(),
                        "WS bridge timeout" | "Script execution timeout (eval path)"
                    )
                }
                Err(_) => false,
            }
    }
    fn cleanup_budget(&self, maximum: u64) -> u64 {
        self.cleanup_deadline.map_or(maximum, |deadline| {
            (deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64)
                .min(maximum)
        })
    }
    fn begin_target_cleanup(&mut self) {
        self.terminal("target_changed");
        self.cancel_capture = true;
        if self.screenshot["status"] == "pending" {
            self.screenshot = json!({"status":"failed","error":{"code":"capture_context_changed"}});
            self.revision += 1;
        }
    }
    fn page_unavailable(&mut self, destroyed: bool) {
        if self.cleanup == "context_destroyed" {
            return;
        }
        self.begin_target_cleanup();
        if destroyed {
            self.cleanup = "context_destroyed".into();
            self.lease.take();
        } else {
            self.cleanup = "unconfirmed".into();
            self.warnings.push("cleanup_unconfirmed".into());
        }
        self.revision += 1;
    }
    fn active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "created" | "installing" | "awaiting_selection"
        )
    }
    fn terminal(&mut self, status: &str) -> bool {
        if !self.active() {
            return false;
        }
        self.status = status.into();
        self.cleanup_deadline = Some(Instant::now() + Duration::from_millis(CLEANUP_MS));
        self.revision += 1;
        self.retained_until = Instant::now() + RETENTION;
        self.retained_ms = now_ms() + RETENTION.as_millis() as u64;
        if status != "selected" && self.screenshot["status"] == "pending" {
            self.screenshot = json!({"status":"cancelled"});
        }
        true
    }
    fn revoke(&mut self) {
        self.terminal("cancelled");
        self.cancel_capture = true;
        self.selection = None;
        self.image = None;
        self.image_reservation.take();
        if self.screenshot["status"] == "pending" {
            self.screenshot = json!({"status":"cancelled"});
            self.revision += 1;
        }
    }

    fn accept(&mut self, report: &Value) -> bool {
        if self.cleanup == "context_destroyed"
            || report["pickerId"] != self.id
            || report["nonce"] != self.nonce
            || report["context"] != self.context
        {
            return false;
        }
        let Some(sequence) = report["sequence"].as_u64().filter(|n| *n > self.sequence) else {
            return false;
        };
        self.sequence = sequence;
        let status = report["status"].as_str().unwrap_or_default();
        if status == "selected" && self.active() && Instant::now() < self.deadline {
            let selection = &report["selection"];
            if !selection.is_object() || selection.to_string().len() > MAX_METADATA {
                self.terminal("failed");
                self.warnings.push("selection_metadata_rejected".into());
            } else if let Ok(selection) = sanitize_selection(selection) {
                self.selection = Some(selection);
                self.terminal("selected");
            } else {
                self.terminal("failed");
                self.warnings.push("selection_metadata_rejected".into());
            }
        } else if matches!(
            status,
            "cancelled" | "expired" | "target_changed" | "failed"
        ) {
            self.terminal(status);
        } else if status == "awaiting_selection"
            && self.active()
            && self.status != "awaiting_selection"
        {
            self.status = status.into();
            self.revision += 1;
        }
        if let Some(cleanup) = report["cleanup"]["status"]
            .as_str()
            .filter(|s| matches!(*s, "confirmed" | "unconfirmed" | "context_destroyed"))
        {
            if self.cleanup != cleanup {
                self.cleanup = cleanup.into();
                self.revision += 1;
            }
            if matches!(cleanup, "confirmed" | "context_destroyed") {
                self.lease.take();
            }
        }
        true
    }
    fn report(&self, include_image: bool) -> Value {
        let mut screenshot = self.screenshot.clone();
        if include_image && let Some(bytes) = &self.image {
            use base64::Engine;
            if bytes.len() <= 3 * 1024 * 1024 {
                screenshot["image"]["base64"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                screenshot["image"]["mimeType"] = json!("image/png");
            } else {
                screenshot["imageOmissionReason"] = json!("output_budget");
            }
        }
        json!({"pickerId":self.id,"requestKey":self.request.request_key,"status":self.status,"revision":self.revision,
            "resultComplete":self.complete,"context":self.context,"selection":self.selection,"screenshot":screenshot,
            "cleanup":{"status":self.cleanup,"leaseReleased":self.lease.is_none()},"deadlineAtMs":self.deadline_ms,
            "retainedUntil":self.retained_ms,"businessActionDispatched":false,"sideEffects":[],"error":self.error,"warnings":self.warnings,
            "limitations":["No isolation guarantee against earlier page-global listeners","Light DOM element boundaries only","Selection does not authorize workflow spec mutation","Screenshot and selection are separate observations"]})
    }
}

// Defense in depth: only known bounded metadata is accepted from a page result.
fn sanitize_selection(value: &Value) -> Result<Value, Value> {
    let mut out = serde_json::Map::new();
    for key in [
        "selectionId",
        "capturedAtMs",
        "context",
        "semanticVersion",
        "tag",
        "role",
        "name",
        "description",
        "states",
        "rect",
        "coordinateSpace",
        "visible",
        "actionable",
        "locatorCandidates",
        "redaction",
        "warnings",
    ] {
        if let Some(value) = value.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    for field in ["name", "description", "tag", "role"] {
        if out
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|text| text.len() > 256)
        {
            return Err(error("selection_metadata_rejected"));
        }
    }
    if out
        .get("locatorCandidates")
        .and_then(Value::as_array)
        .is_none_or(|c| c.len() > 5)
    {
        return Err(error("selection_metadata_rejected"));
    }
    if out
        .get("redaction")
        .is_none_or(|r| r["status"] != "applied")
    {
        return Err(error("selection_metadata_rejected"));
    }
    Ok(Value::Object(out))
}
impl PickerService {
    pub fn window_closed(&self, window: &str) {
        let registry = lock(&self.registry);
        for record in registry.records.values() {
            let mut e = lock(&record.inner);
            if e.request.window_id == window && (!e.complete || e.lease.is_some()) {
                e.terminal("target_changed");
                e.cancel_capture = true;
                e.cleanup = "context_destroyed".into();
                e.lease.take();
                if e.screenshot["status"] == "pending" {
                    e.screenshot =
                        json!({"status":"failed","error":{"code":"capture_context_changed"}});
                }
                e.revision += 1;
                record.notify.notify_waiters();
            }
        }
    }

    pub fn page_changed(&self, window: &str) {
        self.window_closed(window);
    }

    fn reserve(
        &self,
        request: PickerRequest,
        generation: u64,
        arbiter: &ResourceArbiter,
    ) -> Result<(Arc<Record>, bool), Value> {
        let mut registry = lock(&self.registry);
        registry.reap();
        if let Some(key) = &request.request_key
            && let Some(id) = registry.keys.get(&(generation, key.clone()))
        {
            let record = registry.records[id].clone();
            if lock(&record.inner).request.fingerprint() != request.fingerprint() {
                return Err(error("request_key_conflict"));
            }
            return Ok((record, false));
        }
        let active = registry
            .records
            .values()
            .filter(|r| {
                let e = lock(&r.inner);
                e.active() || e.cleanup == "pending"
            })
            .count();
        if active >= MAX_ACTIVE || registry.records.len() >= MAX_RETAINED {
            return Err(error("resource_busy"));
        }
        // One UI-window lease permits independent windows but intersects the
        // existing workflow and legacy parent/UI-window leases and quarantine.
        let lease = arbiter.try_acquire(vec![format!("ui/window/{}", request.window_id)])?;
        let id = format!(
            "{}:picker:{}",
            crate::identity::app_instance_id(),
            uuid::Uuid::new_v4()
        );
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
        let deadline_ms = now_ms() + request.timeout_ms;
        let entry = Entry {
            id: id.clone(),
            nonce: uuid::Uuid::new_v4().to_string(),
            generation,
            deadline,
            deadline_ms,
            retained_until: deadline + RETENTION,
            retained_ms: deadline_ms + RETENTION.as_millis() as u64,
            context: Value::Null,
            status: "created".into(),
            revision: 1,
            sequence: 0,
            selection: None,
            screenshot: json!({"status":if request.capture_screenshot {"pending"} else {"not_requested"}}),
            image: None,
            cleanup: "pending".into(),
            cleanup_deadline: None,
            complete: false,
            lease: Some(lease),
            warnings: vec![],
            error: None,
            cancel_capture: false,
            metadata_reservation: None,
            image_reservation: None,
            connection_pin: None,
            request,
        };
        if let Some(key) = &entry.request.request_key {
            registry.keys.insert((generation, key.clone()), id.clone());
        }
        let record = Arc::new(Record {
            inner: Mutex::new(entry),
            notify: Notify::new(),
        });
        registry.records.insert(id, record.clone());
        Ok((record, true))
    }
    fn get(&self, id: &str, generation: u64) -> Result<Arc<Record>, Value> {
        let registry = lock(&self.registry);
        let record = registry.records.get(id).ok_or_else(|| {
            error(if registry.gone.iter().any(|gone| gone == id) {
                "picker_gone"
            } else {
                "picker_not_found"
            })
        })?;
        let e = lock(&record.inner);
        if e.generation != generation {
            return Err(error("unauthorized"));
        }
        if e.complete && Instant::now() >= e.retained_until {
            return Err(error("picker_gone"));
        }
        Ok(record.clone())
    }
    fn save_image(&self, record: &Record, image: Vec<u8>, metadata: Value) -> bool {
        let store_started = Instant::now();
        let registry = lock(&self.registry);
        let used: usize = registry
            .records
            .values()
            .map(|r| lock(&r.inner).image.as_ref().map_or(0, Vec::len))
            .sum();
        let mut e = lock(&record.inner);
        if e.connection_pin
            .as_ref()
            .is_some_and(|pin| !pin.is_current())
        {
            e.begin_target_cleanup();
            return false;
        }
        if Instant::now() >= e.deadline
            || e.cancel_capture
            || e.screenshot["status"] != "pending"
            || used.saturating_add(image.len()) > MAX_IMAGE_TOTAL
        {
            return false;
        }
        e.image = Some(image);
        e.screenshot = metadata;
        e.screenshot["status"] = json!("captured");
        e.screenshot["widthPx"] = e.screenshot["image"]["widthPx"].clone();
        e.screenshot["heightPx"] = e.screenshot["image"]["heightPx"].clone();
        e.screenshot["mimeType"] = json!("image/png");
        e.screenshot["artifactId"] = json!(format!("{}:image", e.id));
        e.screenshot["retainedUntil"] = json!(e.retained_ms);
        if !e.screenshot["timings"].is_object() {
            e.screenshot["timings"] = json!({"clock":"monotonic"});
        }
        e.screenshot["timings"]["protectedMemoryStore"] =
            crate::screenshot::elapsed_stage(store_started);
        e.revision += 1;
        true
    }
}

/// Read already masked bytes through the same authority as picker get.
/// The opaque artifact handle is not a filesystem path or a bearer credential.
pub fn artifact_read(id: &str, args: &Value, state: &PluginState) -> Result<Value, Value> {
    let generation = state.workflow.inspection_authorize(args)?;
    let picker_id = id
        .strip_suffix(":image")
        .ok_or_else(|| error("artifact_not_found"))?;
    let record = state.picker.get(picker_id, generation)?;
    let e = lock(&record.inner);
    let bytes = e
        .image
        .as_ref()
        .ok_or_else(|| error("artifact_not_found"))?;
    let mut report = json!({"artifact":e.screenshot,"context":e.context});
    if args
        .get("includeImage")
        .or_else(|| args.get("includeBase64"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        use base64::Engine;
        if bytes.len() <= 3 * 1024 * 1024 {
            report["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
            report["mimeType"] = json!("image/png");
        } else {
            report["imageOmissionReason"] = json!("output_budget");
        }
    }
    if !state.workflow.inspection_authorized(generation) {
        return Err(error("unauthorized"));
    }
    Ok(report)
}

pub async fn handle(
    args: &Value,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Result<Value, Value> {
    let generation = state.workflow.inspection_authorize(args)?;
    let request = PickerRequest::parse(args)?;
    let cancel_request = request.action == "cancel";
    let convenience = request.convenience;
    let wait_ms = request.wait_ms;
    let include_image = request.include_image;
    let record = if request.action == "start" {
        let app = app.ok_or_else(|| error("target_changed"))?;
        validate_window(app, &request.window_id)?;
        state.workflow.recover_before_inspection().await?;
        let (record, new) = state
            .picker
            .reserve(request, generation, &state.workflow.resources)?;
        if new {
            let reservation = state.workflow.reserve_inspection_memory(64 * 1024);
            match reservation {
                Ok(reservation) => lock(&record.inner).metadata_reservation = Some(reservation),
                Err(failure) => {
                    let mut e = lock(&record.inner);
                    e.terminal("failed");
                    e.cleanup = "confirmed".into();
                    e.lease.take();
                    e.complete = true;
                    return Err(failure);
                }
            }
            let (record, bridge, state, app) =
                (record.clone(), bridge.clone(), state.clone(), app.clone());
            tauri::async_runtime::spawn(async move {
                run(record, bridge, app, state).await;
            });
        }
        record
    } else {
        let record = state
            .picker
            .get(request.picker_id.as_deref().unwrap_or_default(), generation)?;
        if request.action == "cancel" {
            let retry_cleanup = {
                let mut e = lock(&record.inner);
                e.terminal("cancelled");
                if e.screenshot["status"] == "pending" {
                    e.cancel_capture = true;
                    e.screenshot = json!({"status":"cancelled"});
                    e.revision += 1;
                }
                e.complete && e.cleanup == "unconfirmed"
            };
            record.notify.notify_waiters();
            if retry_cleanup {
                if let Ok(report) = page_call(bridge, &record, "status", 500).await {
                    lock(&record.inner).accept(&report);
                }
                record.notify.notify_waiters();
            }
        }
        record
    };
    let until = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    let revision = lock(&record.inner).revision;
    loop {
        let changed = record.notify.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let done = {
            let e = lock(&record.inner);
            e.complete
                || if convenience {
                    !e.active()
                } else {
                    e.revision != revision
                }
        };
        if done || wait_ms == 0 || tokio::time::timeout_at(until, &mut changed).await.is_err() {
            break;
        }
    }
    if !state.workflow.inspection_authorized(generation) {
        return Err(error("unauthorized"));
    }
    let mut report = lock(&record.inner).report(include_image);
    if cancel_request {
        report["cancellationAccepted"] = json!(true);
    }
    if !state.workflow.inspection_authorized(generation) {
        return Err(error("unauthorized"));
    }
    Ok(report)
}
fn validate_window(app: &tauri::AppHandle, window: &str) -> Result<(), Value> {
    let window = app
        .get_webview_window(window)
        .ok_or_else(|| error("target_changed"))?;
    let url = window.url().map_err(|_| error("unauthorized"))?;
    if !crate::identity::origin_allowed(app, &url) {
        return Err(error("unauthorized"));
    }
    Ok(())
}
async fn page_call(
    bridge: &Bridge,
    record: &Record,
    command: &str,
    budget: u64,
) -> Result<Value, Value> {
    page_call_authorized(bridge, record, command, budget, &|| true).await
}
fn page_transport_failure(command: &str, failure: crate::bridge::BridgeError) -> Value {
    let mut report = error("target_changed");
    report["transportFailure"] = json!(!matches!(
        failure,
        crate::bridge::BridgeError::ExecutionFailed { .. }
    ));
    // The typed transport boundary proves the UI start never reached the page.
    // A dispatched failure/unknown outcome does not permit this shortcut.
    if command == "start" && !failure.is_dispatched() {
        report["cleanupConfirmed"] = json!(true);
    }
    report
}
async fn page_call_authorized(
    bridge: &Bridge,
    record: &Record,
    command: &str,
    budget: u64,
    authorize: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Value, Value> {
    let (window, args, pin, require_pin) = {
        let e = lock(&record.inner);
        (
            e.request.window_id.clone(),
            json!({"cmd":command,"pickerId":e.id,"nonce":e.nonce,"context":e.context,"timeoutMs":e.deadline.saturating_duration_since(Instant::now()).as_millis() as u64}),
            e.connection_pin.clone(),
            command == "start" || command == "geometry" || (command == "status" && e.active()),
        )
    };
    let permitted =
        || authorize() && (!require_pin || pin.as_ref().is_some_and(|pin| pin.is_current()));
    let result = bridge
        .execute_runtime_authorized(
            &window,
            "picker",
            args.clone(),
            budget,
            &uuid::Uuid::new_v4().to_string(),
            &permitted,
        )
        .await
        .map_err(|failure| page_transport_failure(command, failure))?;
    if require_pin && !pin.as_ref().is_some_and(|pin| pin.is_current()) {
        return Err(error("target_changed"));
    }
    if result.get("error").is_some_and(|e| !e.is_null()) {
        let mut failure = result["error"].clone();
        if result["cleanup"]["status"] == "confirmed"
            && result["pickerId"] == args["pickerId"]
            && result["nonce"] == args["nonce"]
        {
            failure["cleanupConfirmed"] = json!(true);
        }
        return Err(failure);
    }
    Ok(result)
}
async fn run(record: Arc<Record>, bridge: Bridge, app: tauri::AppHandle, state: PluginState) {
    let (window, generation, budget) = {
        let mut e = lock(&record.inner);
        if !e.active() || Instant::now() >= e.deadline {
            e.terminal("expired");
            e.cleanup = "confirmed".into();
            e.lease.take();
            e.complete = true;
            e.revision += 1;
            record.notify.notify_waiters();
            return;
        }
        e.status = "installing".into();
        e.revision += 1;
        (
            e.request.window_id.clone(),
            e.generation,
            e.deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64,
        )
    };
    record.notify.notify_waiters();
    let connection_pin = bridge.pin_connection(&window).await;
    lock(&record.inner).connection_pin = Some(connection_pin);
    let prepared = bridge.runtime_context(&window, budget.min(5000)).await;
    match prepared {
        Ok(context) => {
            lock(&record.inner).context = serde_json::to_value(context).unwrap_or(Value::Null);
        }
        Err(_) => {
            let mut e = lock(&record.inner);
            e.terminal("failed");
            e.cleanup = "confirmed".into();
            e.lease.take();
            e.warnings.push("picker_install_failed".into());
            e.error = Some(error("picker_install_failed"));
            e.complete = true;
            record.notify.notify_waiters();
            return;
        }
    }
    if state.workflow.inspection_authorized(generation) && lock(&record.inner).active() {
        // The record and its receive path exist before the page exposes UI.
        let remaining = lock(&record.inner)
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        match page_call_authorized(&bridge, &record, "start", remaining.min(5000), &|| {
            state.workflow.inspection_authorized(generation) && {
                let e = lock(&record.inner);
                e.active() && Instant::now() < e.deadline
            }
        })
        .await
        {
            Ok(report) => {
                let mut e = lock(&record.inner);
                if !state.workflow.inspection_authorized(generation) {
                    e.revoke();
                }
                if e.active()
                    && e.connection_pin
                        .as_ref()
                        .is_some_and(|pin| !pin.is_current())
                {
                    e.begin_target_cleanup();
                } else {
                    e.accept(&report);
                }
            }
            Err(failure) => {
                let mut e = lock(&record.inner);
                e.terminal("failed");
                e.warnings.push("picker_install_failed".into());
                let code = failure["code"]
                    .as_str()
                    .filter(|code| {
                        matches!(
                            *code,
                            "input_isolation_unavailable"
                                | "unauthorized"
                                | "target_changed"
                                | "runtime_version_mismatch"
                                | "runtime_missing"
                        )
                    })
                    .unwrap_or("picker_install_failed");
                e.error = Some(error(code));
                if failure["cleanupConfirmed"] == true {
                    e.cleanup = "confirmed".into();
                    e.lease.take();
                }
            }
        }
        record.notify.notify_waiters();
    } else {
        let mut e = lock(&record.inner);
        e.terminal("cancelled");
        // Cancellation during runtime preparation did not dispatch a UI start.
        if e.cleanup != "context_destroyed" {
            e.cleanup = "confirmed".into();
            e.lease.take();
            e.revision += 1;
        }
    }
    let mut capture_started = false;
    loop {
        let authorized = state.workflow.inspection_authorized(generation);
        {
            let mut e = lock(&record.inner);
            if !authorized {
                e.revoke();
            }
            if e.connection_pin
                .as_ref()
                .is_some_and(|pin| !pin.is_current())
            {
                e.begin_target_cleanup();
            }
            if Instant::now() >= e.deadline {
                e.terminal("expired");
                if e.screenshot["status"] == "pending" {
                    e.cancel_capture = true;
                    e.screenshot = json!({"status":"failed","error":{"code":"expired"}});
                    e.revision += 1;
                }
            }
        }
        let (active, selected, cleanup_done, needs_capture, cancel_capture) = {
            let e = lock(&record.inner);
            (
                e.active(),
                e.status == "selected",
                matches!(e.cleanup.as_str(), "confirmed" | "context_destroyed"),
                e.screenshot["status"] == "pending",
                e.cancel_capture,
            )
        };
        if selected && needs_capture && !capture_started && authorized {
            capture_started = true;
            let (rec, b, a, s) = (record.clone(), bridge.clone(), app.clone(), state.clone());
            tauri::async_runtime::spawn(async move {
                capture(rec, b, a, s).await;
            });
        }
        if !active && cleanup_done && !needs_capture {
            break;
        }
        if !active && lock(&record.inner).cleanup_budget(CLEANUP_MS) == 0 && !cleanup_done {
            {
                let mut e = lock(&record.inner);
                let warned = e
                    .warnings
                    .iter()
                    .any(|warning| warning == "cleanup_unconfirmed");
                if e.cleanup != "unconfirmed" || !warned {
                    e.cleanup = "unconfirmed".into();
                    if !warned {
                        e.warnings.push("cleanup_unconfirmed".into());
                    }
                    e.revision += 1;
                }
            }
            // Retain the UI lease; it is distinct from workflow write quarantine.
            if !needs_capture {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        let cmd = if !active && (!selected || cancel_capture) {
            "cancel"
        } else {
            "status"
        };
        let call_budget = lock(&record.inner).cleanup_budget(if active { 500 } else { CLEANUP_MS });
        match page_call(&bridge, &record, cmd, call_budget).await {
            Ok(report) => {
                let mut e = lock(&record.inner);
                if !state.workflow.inspection_authorized(generation) {
                    e.revoke();
                }
                if e.active()
                    && e.connection_pin
                        .as_ref()
                        .is_some_and(|pin| !pin.is_current())
                {
                    e.begin_target_cleanup();
                    record.notify.notify_waiters();
                } else if e.accept(&report) {
                    record.notify.notify_waiters();
                }
            }
            Err(failure) => {
                let destroyed = app.get_webview_window(&window).is_none()
                    || crate::identity::window_instance_id(&window)
                        != lock(&record.inner).context["windowInstanceId"]
                            .as_str()
                            .unwrap_or_default();
                {
                    let mut e = lock(&record.inner);
                    if destroyed || e.cleanup == "context_destroyed" {
                        e.page_unavailable(true);
                        break;
                    }
                }
                let probe_budget = {
                    let e = lock(&record.inner);
                    if e.active() {
                        (e.deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis() as u64)
                            .min(1500)
                    } else {
                        e.cleanup_budget(300)
                    }
                };
                let probe = bridge.probe_runtime(&window, probe_budget).await;
                let context_changed = probe.as_ref().ok().is_some_and(|p| {
                    (p["pageEpoch"].is_string()
                        && p["pageEpoch"] != lock(&record.inner).context["pageEpoch"])
                        || (p["runtime"]["ready"] == true
                            && p["runtime"]["context"] != lock(&record.inner).context)
                });
                let mut e = lock(&record.inner);
                if destroyed || context_changed || e.cleanup == "context_destroyed" {
                    e.page_unavailable(true);
                    break;
                }
                let connection_live = e
                    .connection_pin
                    .as_ref()
                    .is_some_and(|pin| pin.is_current());
                if e.retain_after_slow_status(cmd, &failure, connection_live, probe.as_ref()) {
                    // A slow read on the same connection/document is not a
                    // target change. Keep its original state and deadline;
                    // only status is polled again, never start or business work.
                    continue;
                }
                // Retry only this picker's idempotent cancel/status within the
                // existing terminal budget, retaining its original context.
                e.begin_target_cleanup();
                record.notify.notify_waiters();
            }
        }
        let pause = lock(&record.inner).cleanup_budget(50);
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
    // The selected-node reference belongs to the image phase. Release it before
    // resultComplete, without probing a context already known to be destroyed.
    let release_budget = {
        let e = lock(&record.inner);
        if e.cleanup == "context_destroyed" {
            0
        } else if e.cleanup == "confirmed" {
            (e.deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64)
                .min(300)
        } else {
            e.cleanup_budget(300)
        }
    };
    if release_budget > 0 {
        let _ = page_call(&bridge, &record, "release", release_budget).await;
    }
    {
        let mut e = lock(&record.inner);
        e.complete = true;
        e.revision += 1;
        if !state.workflow.inspection_authorized(generation) {
            e.revoke();
        }
    }
    record.notify.notify_waiters();
}

async fn capture(record: Arc<Record>, bridge: Bridge, app: tauri::AppHandle, state: PluginState) {
    let (window, context, target, source, budget, generation) = {
        let e = lock(&record.inner);
        (
            e.request.window_id.clone(),
            e.context.clone(),
            e.selection
                .as_ref()
                .map(|s| s["rect"].clone())
                .unwrap_or(Value::Null),
            e.request.screenshot_source.clone(),
            e.deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64,
            e.generation,
        )
    };
    let result: Result<crate::screenshot::ProtectedCapture, String> = {
        let capture_work = async {
            let target = serde_json::from_value::<crate::screenshot::RectCss>(target)
                .map_err(|_| "capture_context_changed".to_string())?;
            let request = crate::screenshot::CaptureRequest {
                window_id: window,
                context,
                source: serde_json::from_value(json!(source))
                    .map_err(|_| "invalid_source".to_string())?,
                target: Some(target),
                timeout_ms: budget,
                max_width: None,
            };
            crate::screenshot::capture_protected(&app, &bridge, request, || async {
                if !state.workflow.inspection_authorized(generation) {
                    return Err("unauthorized".into());
                }
                if lock(&record.inner).cancel_capture {
                    return Err("cancelled".into());
                }
                let geometry = page_call(&bridge, &record, "geometry", 500)
                    .await
                    .map_err(|_| "capture_context_changed".to_string())?;
                if geometry["ready"] != true {
                    return Err("capture_context_changed".into());
                }
                Ok(())
            })
            .await
        };
        tokio::pin!(capture_work);
        loop {
            let changed = record.notify.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if lock(&record.inner).cancel_capture {
                break Err("cancelled".into());
            }
            if !state.workflow.inspection_authorized(generation) {
                break Err("unauthorized".into());
            }
            tokio::select! {
                result=&mut capture_work=>break result,
                _=&mut changed=>{}
            }
        }
    }; // Dropping the wait drops receivers; native work keeps its bounded permit until its callback.
    match result {
        Ok(captured) if state.workflow.inspection_authorized(generation) => {
            let charge = state.workflow.reserve_inspection_memory(captured.png.len());
            if let Ok(charge) = charge {
                lock(&record.inner).image_reservation = Some(charge);
            } else {
                let mut e = lock(&record.inner);
                if e.screenshot["status"] == "pending" {
                    e.screenshot =
                        json!({"status":"failed","error":{"code":"capture_budget_exceeded"}});
                    e.revision += 1;
                }
                record.notify.notify_waiters();
                return;
            }
            if !state
                .picker
                .save_image(&record, captured.png, captured.metadata)
            {
                let mut e = lock(&record.inner);
                e.image_reservation.take();
                if e.screenshot["status"] == "pending" {
                    e.screenshot =
                        json!({"status":"failed","error":{"code":"capture_budget_exceeded"}});
                    e.revision += 1;
                }
            }
        }
        result => {
            let mut e = lock(&record.inner);
            if e.screenshot["status"] == "pending" {
                // Avoid reflecting backend exception strings into a rich observation.
                let code = match result.as_ref().err().and_then(|s| s.split(':').next()) {
                    Some(
                        code @ ("redaction_unavailable"
                        | "capture_context_changed"
                        | "capture_timeout"
                        | "capture_budget_exceeded"
                        | "geometry_unknown"
                        | "unauthorized"
                        | "cancelled"),
                    ) => code,
                    _ => {
                        if result.is_err() {
                            "capture_failed"
                        } else {
                            "unauthorized"
                        }
                    }
                };
                e.screenshot = json!({"status":"failed","error":{"code":code}});
                e.warnings.push(code.into());
                e.revision += 1;
            }
        }
    }
    record.notify.notify_waiters();
}

#[cfg(test)]
mod tests;
