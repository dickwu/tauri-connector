use super::{CaptureRequest, CaptureSource, ProtectedCapture, RectCss, capture_protected};
use crate::{bridge::Bridge, state::PluginState};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RETAINED_LIMIT: usize = 128;
const RETAINED_BYTES: usize = 32 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(300);

struct Artifact {
    generation: u64,
    until: Instant,
    metadata: Value,
    bytes: Arc<[u8]>,
    _reservation: crate::workflow::budget::Reservation,
}

pub struct ScreenshotService {
    artifacts: Arc<Mutex<HashMap<String, Artifact>>>,
}

impl Default for ScreenshotService {
    fn default() -> Self {
        Self::with_reap_interval(Duration::from_secs(30))
    }
}

impl ScreenshotService {
    fn with_reap_interval(interval: Duration) -> Self {
        let artifacts = Arc::new(Mutex::new(HashMap::<String, Artifact>::new()));
        let weak = Arc::downgrade(&artifacts);
        // Tauri constructs plugin state on its synchronous UI thread, where a
        // Tokio handle may not be entered. Use the application runtime directly.
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(store) = weak.upgrade() else {
                    break;
                };
                store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|_, entry| entry.until > Instant::now());
            }
        });
        Self { artifacts }
    }
}

fn error(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message,"stage":"screenshot"})
}

impl ScreenshotService {
    #[allow(dead_code)] // Host shutdown/revocation hook; generation checks also gate every output.
    pub fn revoke_all(&self) {
        self.artifacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    fn revoke_generation(&self, generation: u64) {
        self.artifacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, entry| entry.generation != generation);
    }

    fn insert(
        &self,
        generation: u64,
        capture: ProtectedCapture,
        reservation: crate::workflow::budget::Reservation,
    ) -> Result<Value, Value> {
        let started = Instant::now();
        let mut store = self.artifacts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        store.retain(|_, entry| entry.until > now);
        if store.len() >= RETAINED_LIMIT
            || store
                .values()
                .map(|e| e.bytes.len())
                .sum::<usize>()
                .saturating_add(capture.png.len())
                > RETAINED_BYTES
        {
            let mut failure = error(
                "artifact_capacity_exceeded",
                "Protected artifacts are retained until their advertised TTL; retry after expiry",
            );
            failure["timing"] = super::elapsed_stage(started);
            return Err(failure);
        }
        let id = capture
            .metadata
            .get("artifactId")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalid_artifact", "Missing artifact identity"))?
            .to_string();
        let mut metadata = capture.metadata;
        metadata["retainedUntil"] = json!(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                + TTL.as_millis()
        );
        metadata["storage"] = json!("protected_memory");
        store.insert(
            id.clone(),
            Artifact {
                generation,
                until: now + TTL,
                metadata: metadata.clone(),
                bytes: Arc::from(capture.png),
                _reservation: reservation,
            },
        );
        metadata["timings"]["protectedMemoryStore"] = super::elapsed_stage(started);
        if let Some(entry) = store.get_mut(&id) {
            entry.metadata = metadata.clone();
        }
        Ok(metadata)
    }

    fn read(&self, id: &str, generation: u64) -> Result<(Value, Arc<[u8]>), Value> {
        let started = Instant::now();
        let mut store = self.artifacts.lock().unwrap_or_else(|e| e.into_inner());
        store.retain(|_, entry| entry.until > Instant::now());
        let entry = store
            .get(id)
            .filter(|entry| entry.generation == generation)
            .ok_or_else(|| {
                error(
                    "artifact_gone",
                    "Protected artifact is unavailable in this authorization domain",
                )
            })?;
        let mut metadata = entry.metadata.clone();
        let bytes = entry.bytes.clone();
        metadata["timings"]["protectedMemoryRead"] = super::elapsed_stage(started);
        Ok((metadata, bytes))
    }
}

pub async fn handle(
    args: &Value,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Result<Value, Value> {
    state.workflow.inspection_authorize(args)?;
    let timeout = args
        .get("timeoutMs")
        .map(|v| {
            v.as_u64()
                .filter(|n| *n > 0 && *n <= 30000)
                .ok_or_else(|| error("invalid_arguments", "Invalid timeoutMs"))
        })
        .transpose()?
        .unwrap_or(10000);
    tokio::time::timeout(
        Duration::from_millis(timeout),
        handle_inner(args, bridge, app, state),
    )
    .await
    .map_err(|_| {
        error(
            "capture_timeout",
            "Capture deadline expired; late native results are discarded",
        )
    })?
}

async fn handle_inner(
    args: &Value,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Result<Value, Value> {
    let generation = state.workflow.inspection_authorize(args)?;
    let object = args
        .as_object()
        .ok_or_else(|| error("invalid_arguments", "Expected screenshot object"))?;
    const FIELDS: &[&str] = &[
        "authToken",
        "windowId",
        "source",
        "target",
        "selector",
        "redaction",
        "allowWindowPreparation",
        "format",
        "quality",
        "maxWidth",
        "save",
        "outputDir",
        "nameHint",
        "overwrite",
        "annotate",
        "includeImage",
        "timeoutMs",
    ];
    if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(error("invalid_arguments", "Unknown screenshot field"));
    }
    if args.get("target").is_some() && args.get("selector").is_some() {
        return Err(error(
            "invalid_arguments",
            "target and selector are mutually exclusive",
        ));
    }
    if args
        .get("allowWindowPreparation")
        .is_some_and(|v| v != false)
    {
        return Err(error(
            "window_preparation_not_allowed",
            "Host passive capture policy does not allow window preparation",
        ));
    }
    if args.get("redaction").is_some_and(|v| v != "required") {
        return Err(error(
            "redaction_policy_not_allowed",
            "Host policy requires pixel redaction; no client downgrade is permitted",
        ));
    }
    for key in ["save", "overwrite", "annotate", "includeImage"] {
        if args.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(error(
                "invalid_arguments",
                "Expected boolean screenshot field",
            ));
        }
    }
    for key in ["format", "outputDir", "nameHint"] {
        if args.get(key).is_some_and(|value| !value.is_string()) {
            return Err(error(
                "invalid_arguments",
                "Expected string screenshot field",
            ));
        }
    }
    if args.get("annotate") == Some(&json!(true)) {
        return Err(error(
            "unsupported_feature",
            "Protected capture does not add legacy snapshot labels",
        ));
    }
    let format = args.get("format").and_then(Value::as_str).unwrap_or("png");
    if !["png", "jpeg", "jpg", "webp"].contains(&format) {
        return Err(error("invalid_arguments", "Invalid screenshot format"));
    }
    let source = serde_json::from_value::<CaptureSource>(
        args.get("source").cloned().unwrap_or(json!("auto")),
    )
    .map_err(|_| error("invalid_arguments", "Invalid screenshot source"))?;
    let window = args
        .get("windowId")
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| error("invalid_arguments", "Invalid windowId"))
        })
        .transpose()?
        .unwrap_or("main");
    let max_width = args
        .get("maxWidth")
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| error("invalid_arguments", "Invalid maxWidth"))
        })
        .transpose()?;
    let timeout = args
        .get("timeoutMs")
        .map(|v| {
            v.as_u64()
                .filter(|n| *n > 0 && *n <= 30000)
                .ok_or_else(|| error("invalid_arguments", "Invalid timeoutMs"))
        })
        .transpose()?
        .unwrap_or(10000);
    if args
        .get("quality")
        .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > 100))
    {
        return Err(error("invalid_arguments", "quality must be 1 through 100"));
    }
    let app = app.ok_or_else(|| {
        error(
            "app_unavailable",
            "Native application handle is unavailable",
        )
    })?;
    // Restore durable quarantines when present without requiring a new
    // Windows journal solely for protected in-memory evidence.
    state.workflow.recover_before_inspection().await?;
    let _lease =
        state
            .workflow
            .resources
            .try_acquire(crate::workflow::resources::tool_resources(
                "webview_screenshot",
                args,
            ))?;
    let context = bridge.runtime_context(window, timeout).await.map_err(|_| {
        error(
            "runtime_unavailable",
            "Cannot bind screenshot to a ready runtime",
        )
    })?;
    let context = serde_json::to_value(context)
        .map_err(|_| error("context_invalid", "Cannot encode screenshot context"))?;
    let locator = if let Some(target) = args.get("target") {
        Some(target.clone())
    } else if let Some(selector) = args.get("selector") {
        let selector = selector
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("invalid_arguments", "Invalid selector"))?;
        Some(json!({"by":"css","value":selector}))
    } else {
        None
    };
    let target_info = if let Some(locator) = locator {
        Some(
            bridge
                .execute_runtime(
                    window,
                    "geometry",
                    json!({"cmd":"target_begin","target":locator,"expectedContext":context}),
                    timeout,
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await
                .map_err(|e| error("target_resolution_failed", &e.to_string()))?,
        )
    } else {
        None
    };
    let target_id = target_info
        .as_ref()
        .and_then(|v| v.get("targetId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let target = target_info
        .as_ref()
        .map(|v| {
            serde_json::from_value::<RectCss>(v.get("rect").cloned().unwrap_or(Value::Null))
                .map_err(|_| error("geometry_unknown", "Invalid selected element geometry"))
        })
        .transpose()?;
    let validate = || async {
        if !state.workflow.inspection_authorized(generation) {
            return Err("unauthorized: screenshot authorization revoked".into());
        }
        let current = bridge
            .runtime_context(window, timeout)
            .await
            .map_err(|e| e.to_string())?;
        if serde_json::to_value(current).map_err(|e| e.to_string())? != context {
            return Err("capture_context_changed".into());
        }
        if let Some(target_id) = target_id.as_deref() {
            bridge
                .execute_runtime(
                    window,
                    "geometry",
                    json!({"cmd":"target_validate","targetId":target_id,"expectedContext":context}),
                    timeout,
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    let captured = capture_protected(
        app,
        bridge,
        CaptureRequest {
            window_id: window.into(),
            context: context.clone(),
            source,
            target,
            timeout_ms: timeout,
            max_width,
        },
        validate,
    )
    .await;
    if let Some(target_id) = target_id {
        let _ = bridge
            .execute_runtime(
                window,
                "geometry",
                json!({"cmd":"target_release","targetId":target_id,"expectedContext":context}),
                1000,
                &uuid::Uuid::new_v4().to_string(),
            )
            .await;
    }
    let mut capture =
        captured.map_err(|e| error(e.split(':').next().unwrap_or("capture_failed"), &e))?;
    if !state.workflow.inspection_authorized(generation) {
        return Err(error("unauthorized", "Screenshot authorization revoked"));
    }
    let mime = match format {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    if format != "png" {
        #[cfg(feature = "capture-codec")]
        {
            let format = format.to_string();
            let quality = args.get("quality").and_then(Value::as_u64).unwrap_or(80) as u8;
            let permit =
                super::CODECS.clone().acquire_owned().await.map_err(|_| {
                    error("capture_service_closed", "Capture codec queue unavailable")
                })?;
            let (bytes, timing) = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                super::pixels::transcode(capture.png, &format, quality)
            })
            .await
            .map_err(|_| error("capture_worker_failed", "Failed to encode protected image"))?
            .map_err(|e| error("capture_encode_failed", &e))?;
            capture.png = bytes;
            capture.metadata["timings"]["formatConversion"] = timing;
        }
        #[cfg(not(feature = "capture-codec"))]
        return Err(error(
            "capture_backend_unavailable",
            "capture-codec feature is required",
        ));
    }
    capture.metadata["image"]["mimeType"] = json!(mime);
    let bytes = capture.png.clone();
    let fallback_metadata = capture.metadata.clone();
    let metadata_bytes = serde_json::to_vec(&capture.metadata)
        .map_err(|_| error("artifact_invalid", "Cannot measure artifact metadata"))?
        .len();
    let charge = capture
        .png
        .len()
        .saturating_add(metadata_bytes.saturating_mul(3))
        .saturating_add(512);
    let saved = state
        .workflow
        .reserve_inspection_memory(charge)
        .and_then(|reservation| state.screenshot.insert(generation, capture, reservation));
    let mut result = match saved {
        Ok(metadata) => metadata,
        Err(warning) => {
            let mut result = fallback_metadata;
            if let Some(timing) = warning.get("timing") {
                result["timings"]["protectedMemoryStore"] = timing.clone();
                result["timings"]["protectedMemoryStore"]["outcome"] = json!("failed");
            }
            result["warning"] = warning;
            result["artifactId"] = Value::Null;
            result
        }
    };
    if args.get("save") == Some(&json!(true)) {
        result["saveWarning"] = json!({"code":"protected_disk_storage_unavailable","message":"Capture succeeded in protected memory; no private durable artifact storage is configured"});
    }
    if args.get("includeImage") != Some(&json!(false)) {
        result["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
        result["mimeType"] = json!(mime);
    }
    if !state.workflow.inspection_authorized(generation) {
        state.screenshot.revoke_generation(generation);
        return Err(error("unauthorized", "Screenshot authorization revoked"));
    }
    Ok(result)
}

pub fn artifact_handle(operation: &str, args: &Value, state: &PluginState) -> Result<Value, Value> {
    let generation = state.workflow.inspection_authorize(args)?;
    let result = match operation {
        "artifact_read" => {
            let id = args
                .get("artifactId")
                .and_then(Value::as_str)
                .ok_or_else(|| error("invalid_arguments", "artifactId is required"))?;
            let (mut metadata, bytes) = state.screenshot.read(id, generation)?;
            if args.get("includeImage") != Some(&json!(false)) {
                metadata["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
                metadata["mimeType"] = metadata
                    .pointer("/image/mimeType")
                    .cloned()
                    .unwrap_or(json!("image/png"));
            }
            metadata
        }
        "artifact_list" => {
            let mut store = state
                .screenshot
                .artifacts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            store.retain(|_, entry| entry.until > Instant::now());
            json!({"artifacts":store.values().filter(|entry|entry.generation==generation).map(|entry|entry.metadata.clone()).collect::<Vec<_>>()})
        }
        "artifact_compare" => {
            let baseline = args
                .get("baselineId")
                .and_then(Value::as_str)
                .ok_or_else(|| error("invalid_arguments", "baselineId is required"))?;
            let current = args
                .get("currentId")
                .and_then(Value::as_str)
                .ok_or_else(|| error("invalid_arguments", "currentId is required"))?;
            let (first, a) = state.screenshot.read(baseline, generation)?;
            let (second, b) = state.screenshot.read(current, generation)?;
            let compatible = [
                "/captureSource",
                "/context/appInstanceId",
                "/context/windowInstanceId",
                "/image",
                "/redaction/policyVersion",
                "/redaction/maskedRegions",
                "/redaction/maskFingerprint",
                "/geometry",
            ]
            .iter()
            .all(|path| first.pointer(path) == second.pointer(path));
            json!({"comparable":compatible,"method":"encoded_byte_equality","equal":compatible.then_some(a==b),
                "reason":if compatible {"same_capture_contract"}else{"capture_contract_changed"}})
        }
        "artifact_prune" => {
            let mut store = state
                .screenshot
                .artifacts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let pruned = store
                .values()
                .filter(|entry| entry.generation == generation && entry.until <= Instant::now())
                .count();
            store.retain(|_, entry| entry.until > Instant::now());
            json!({"pruned":pruned,"policy":"expired_only"})
        }
        _ => {
            return Err(error(
                "unknown_operation",
                "Unknown protected artifact operation",
            ));
        }
    };
    if !state.workflow.inspection_authorized(generation) {
        return Err(error("unauthorized", "Artifact authorization revoked"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(id: &str) -> ProtectedCapture {
        ProtectedCapture {
            metadata: json!({"artifactId":id}),
            png: vec![1, 2, 3],
        }
    }
    #[test]
    fn protected_store_rejects_other_generation_and_never_evicts_live_results() {
        let service = ScreenshotService::default();
        let budget = crate::workflow::budget::MemoryBudget::default();
        for n in 0..RETAINED_LIMIT {
            service
                .insert(
                    7,
                    capture(&format!("artifact-{n}")),
                    budget.try_reserve(3).unwrap(),
                )
                .unwrap();
        }
        assert!(
            service
                .insert(7, capture("extra"), budget.try_reserve(3).unwrap())
                .is_err()
        );
        assert!(service.read("artifact-0", 7).is_ok());
        assert!(service.read("artifact-0", 8).is_err());
        assert!(service.read("../../private-file", 7).is_err());
        service.revoke_all();
        assert!(service.read("artifact-0", 7).is_err());
        assert_eq!(budget.used_bytes(), 0);
    }
    #[test]
    fn expired_artifacts_are_gone_and_capacity_can_be_reused() {
        let service = ScreenshotService::default();
        let budget = crate::workflow::budget::MemoryBudget::default();
        service
            .insert(7, capture("old"), budget.try_reserve(3).unwrap())
            .unwrap();
        service
            .artifacts
            .lock()
            .unwrap()
            .get_mut("old")
            .unwrap()
            .until = Instant::now() - Duration::from_secs(1);
        assert!(service.read("old", 7).is_err());
        assert_eq!(budget.used_bytes(), 0);
        service
            .insert(7, capture("new"), budget.try_reserve(3).unwrap())
            .unwrap();
        assert!(service.read("new", 7).is_ok());
    }
    #[test]
    fn synchronous_host_setup_still_reaps_expired_images_without_api_reads() {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let service = ScreenshotService::with_reap_interval(Duration::from_millis(5));
        let budget = crate::workflow::budget::MemoryBudget::default();
        service
            .insert(1, capture("idle"), budget.try_reserve(3).unwrap())
            .unwrap();
        service
            .artifacts
            .lock()
            .unwrap()
            .get_mut("idle")
            .unwrap()
            .until = Instant::now() - Duration::from_secs(1);
        let until = Instant::now() + Duration::from_millis(200);
        while budget.used_bytes() != 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(budget.used_bytes(), 0);
    }

    #[test]
    fn cache_reports_measured_store_and_read_cost_without_changing_capture_timings() {
        let service = ScreenshotService::default();
        let budget = crate::workflow::budget::MemoryBudget::default();
        let mut capture = capture("timed");
        capture.metadata["capturedAt"] = json!(1234);
        capture.metadata["timings"] = json!({"clock":"monotonic","diskSave":super::super::not_applicable_stage("protected_memory_only")});
        let inserted = service
            .insert(1, capture, budget.try_reserve(3).unwrap())
            .unwrap();
        assert_eq!(
            inserted["timings"]["protectedMemoryStore"]["status"],
            "measured"
        );
        assert!(
            inserted["timings"]["protectedMemoryStore"]["elapsedNs"]
                .as_u64()
                .unwrap()
                > 0
        );
        let (read, _) = service.read("timed", 1).unwrap();
        let (again, _) = service.read("timed", 1).unwrap();
        assert_eq!(read["capturedAt"], 1234);
        assert_eq!(
            again["timings"]["protectedMemoryStore"],
            inserted["timings"]["protectedMemoryStore"]
        );
        assert_eq!(read["timings"]["protectedMemoryRead"]["status"], "measured");
        assert!(
            read["timings"]["protectedMemoryRead"]["elapsedNs"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert_eq!(read["timings"]["diskSave"]["status"], "not_applicable");
        assert!(read["timings"]["diskSave"].get("elapsedNs").is_none());
    }
}
