//! Protected screenshot pipeline. Raw frames never enter legacy artifacts.

mod geometry;
mod service;
pub use service::{ScreenshotService, artifact_handle, handle};
#[cfg(all(feature = "native-screenshot", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "native-screenshot", target_os = "macos"))]
mod macos;
#[cfg(feature = "capture-codec")]
mod pixels;
#[cfg(all(feature = "native-screenshot", target_os = "windows"))]
mod windows;

use crate::bridge::Bridge;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;
use tokio::sync::oneshot;

pub use geometry::{Geometry, RectCss, Viewport};

pub(super) const MAX_PIXELS: u64 = 16_000_000;
#[cfg(feature = "capture-codec")]
pub(super) const MAX_RAW_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_ENCODED_BYTES: usize = 16 * 1024 * 1024;
static CAPTURES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
static NATIVE_CALLBACKS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
#[cfg(feature = "capture-codec")]
pub(super) static CODECS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    #[default]
    Auto,
    WebviewNative,
    WindowNative,
    DomRendering,
}

pub struct CaptureRequest {
    pub window_id: String,
    pub context: Value,
    pub source: CaptureSource,
    pub target: Option<RectCss>,
    pub timeout_ms: u64,
    pub max_width: Option<u32>,
}

/// This type can only be constructed by the mask pipeline. The picker retains
/// it within its authorized entry; it is never inserted into legacy artifacts.
pub struct ProtectedCapture {
    pub metadata: Value,
    pub png: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Probe {
    viewport: Viewport,
    sensitive_rects: Vec<RectCss>,
    uncovered_reasons: Vec<String>,
    ready: bool,
    document_stamp: f64,
    scroll_x: f64,
    scroll_y: f64,
    document_width: f64,
    document_height: f64,
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Channel layouts are selected by platform feature gates.
pub(super) enum PixelOrder {
    Rgba,
    Bgra,
    Argb,
    Abgr,
}

#[allow(dead_code)] // A base plugin has no decoder or native bitmap producer.
pub(super) enum RawFrame {
    Encoded(Vec<u8>),
    Bitmap {
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        stride: usize,
        order: PixelOrder,
        premultiplied: bool,
        opaque: bool,
    },
}

struct CallbackSlot {
    sender: Option<oneshot::Sender<Result<RawFrame, String>>>,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
type Callback = Arc<Mutex<CallbackSlot>>;

#[cfg_attr(not(feature = "native-screenshot"), allow(dead_code))]
fn callback_pending(callback: &Callback) -> bool {
    callback
        .lock()
        .ok()
        .and_then(|guard| guard.sender.as_ref().map(|tx| !tx.is_closed()))
        .unwrap_or(false)
}

#[cfg_attr(not(feature = "native-screenshot"), allow(dead_code))]
fn finish_callback(callback: &Callback, value: Result<RawFrame, String>) {
    if let Some(sender) = callback.lock().ok().and_then(|mut slot| {
        slot._permit.take();
        slot.sender.take()
    }) {
        // A dropped receiver means timeout/cancel; sending simply frees pixels.
        let _ = sender.send(value);
    }
}

fn check_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        Err("capture_budget_exceeded: image dimensions".into())
    } else {
        Ok(())
    }
}

/// Monotonic measurements retain sub-millisecond work without rounding it to
/// a fabricated zero. Wall-clock capture timestamps remain separate metadata.
pub(crate) fn elapsed_stage(started: Instant) -> Value {
    let elapsed = started.elapsed();
    json!({"status":"measured","elapsedNs":elapsed.as_nanos(),"elapsedMs":elapsed.as_secs_f64()*1000.0})
}

fn not_applicable_stage(reason: &str) -> Value {
    json!({"status":"not_applicable","reason":reason})
}

/// Captures once per selected backend, never performs business input, and
/// validates identity/authorization around every asynchronous boundary.
pub async fn capture_protected<V, F>(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    request: CaptureRequest,
    validate: V,
) -> Result<ProtectedCapture, String>
where
    V: Fn() -> F,
    F: Future<Output = Result<(), String>> + Send,
{
    if request.timeout_ms == 0 || request.timeout_ms > 120_000 || request.max_width == Some(0) {
        return Err("invalid_arguments: capture timeout or maxWidth".into());
    }
    let instance = request
        .context
        .get("appInstanceId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("capture_context_invalid: missing app instance")?
        .to_owned();
    tokio::time::timeout(Duration::from_millis(request.timeout_ms), async {
        let _permit = CAPTURES.acquire().await.map_err(|_| "capture_service_closed")?;
        validate().await?;
        bridge.execute_runtime(&request.window_id,"geometry",
            json!({"cmd":"frame_ready","expectedContext":request.context}),
            request.timeout_ms.min(1500),&uuid::Uuid::new_v4().to_string()).await.map_err(|error|error.to_string())?;
        validate().await?;
        let before = probe(bridge, &request).await?;
        if !before.ready { return Err("capture_not_ready: document is loading".into()); }
        let native_width=(before.viewport.width_css*before.viewport.device_pixel_ratio).ceil();
        let native_height=(before.viewport.height_css*before.viewport.device_pixel_ratio).ceil();
        if !native_width.is_finite() || !native_height.is_finite() || native_width>f64::from(u32::MAX) || native_height>f64::from(u32::MAX) {
            return Err("capture_budget_exceeded: viewport dimensions".into());
        }
        check_dimensions(native_width as u32,native_height as u32)?;
        if !before.uncovered_reasons.is_empty() {
            return Err(format!("redaction_unavailable: {}", before.uncovered_reasons.join(",")));
        }
        let sources: &[CaptureSource] = match request.source {
            CaptureSource::Auto => &[CaptureSource::WebviewNative, CaptureSource::WindowNative, CaptureSource::DomRendering],
            CaptureSource::WebviewNative => &[CaptureSource::WebviewNative],
            CaptureSource::WindowNative => &[CaptureSource::WindowNative],
            CaptureSource::DomRendering => &[CaptureSource::DomRendering],
        };
        let mut fallbacks = Vec::new();
        for source in sources {
            validate().await?;
            let capture_started=Instant::now();
            let frame = capture_source(app, bridge, &request, *source, &before).await;
            let capture_timing=elapsed_stage(capture_started);
            validate().await?;
            let after = probe(bridge, &request).await?;
            if before != after { return Err("capture_context_changed: viewport, page or mask changed".into()); }
            let result = match frame {
                Ok(frame) => process_frame(frame, before.clone(), request.target, request.max_width).await,
                Err(error) => Err(error),
            };
            match result {
                Ok((png, geometry, width, height, mut timings)) => {
                    validate().await?;
                    timings["capture"]=capture_timing;
                    timings["protectedMemoryStore"]=not_applicable_stage("caller_owned_retention");
                    timings["diskSave"]=not_applicable_stage("protected_memory_only");
                    timings["formatConversion"]=not_applicable_stage("png_output");
                    let captured_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| "capture_clock_unavailable")?.as_millis();
                    return Ok(ProtectedCapture { png, metadata: json!({
                        "artifactId":format!("{instance}:artifact:{}", uuid::Uuid::new_v4()),
                        "captureSource":source, "capturedAt":captured_at, "context":request.context,
                        "image":{"widthPx":width,"heightPx":height,"mimeType":"image/png"},
                        "viewport":before.viewport, "geometry":geometry,
                        "redaction":{"status":"applied","policyVersion":"1","policy":"required",
                            "maskedRegions":before.sensitive_rects.len(),"coverage":"declared_regions_and_field_heuristics",
                            "maskFingerprint":crate::runtime::manifest::content_hash(serde_json::to_string(&before.sensitive_rects).map_err(|_|"mask_serialization_failed")?.as_bytes()),
                            "uncoveredReasons":[]},
                        "consistency":{"status":"stable_context","atomic":false},
                        "sideEffects":[],"fallbacks":fallbacks,"timings":timings
                    }) });
                }
                Err(error) => {
                    if request.source != CaptureSource::Auto { return Err(error); }
                    fallbacks.push(json!({"source":source,"reason":error,"captureTiming":capture_timing}));
                }
            }
        }
        Err(format!("capture_failed: {}", serde_json::to_string(&fallbacks).unwrap_or_default()))
    }).await.map_err(|_| "capture_timeout: late native results will be discarded".to_string())?
}

async fn probe(bridge: &Bridge, request: &CaptureRequest) -> Result<Probe, String> {
    let value = bridge
        .execute_runtime(
            &request.window_id,
            "geometry",
            json!({"cmd":"probe","expectedContext":request.context}),
            request.timeout_ms.min(5000),
            &uuid::Uuid::new_v4().to_string(),
        )
        .await
        .map_err(|error| error.to_string())?;
    let result: Probe =
        serde_json::from_value(value).map_err(|_| "geometry_unknown: malformed geometry probe")?;
    if result.sensitive_rects.len() > 2048 || result.uncovered_reasons.len() > 16 {
        return Err("capture_budget_exceeded: geometry probe".into());
    }
    Ok(result)
}

async fn capture_source(
    app: &tauri::AppHandle,
    bridge: &Bridge,
    request: &CaptureRequest,
    source: CaptureSource,
    _probe: &Probe,
) -> Result<RawFrame, String> {
    match source {
        CaptureSource::WebviewNative => native(app, &request.window_id).await,
        CaptureSource::WindowNative => window_native(app, &request.window_id).await,
        CaptureSource::DomRendering => {
            use base64::Engine;
            let result = bridge
                .execute_runtime(
                    &request.window_id,
                    "geometry",
                    json!({"cmd":"dom_capture","expectedContext":request.context}),
                    request.timeout_ms.min(10_000),
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await
                .map_err(|error| error.to_string())?;
            let encoded = result
                .get("base64")
                .and_then(Value::as_str)
                .ok_or("capture_failed: missing DOM image")?;
            if encoded.len() > MAX_ENCODED_BYTES * 4 / 3 + 4 {
                return Err("capture_budget_exceeded: DOM data".into());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "capture_failed: malformed DOM data")?;
            Ok(RawFrame::Encoded(bytes))
        }
        CaptureSource::Auto => Err("invalid_arguments: unexpanded auto source".into()),
    }
}

async fn native(app: &tauri::AppHandle, window_id: &str) -> Result<RawFrame, String> {
    let window = app
        .get_webview_window(window_id)
        .ok_or("window_not_found")?;
    let permit = NATIVE_CALLBACKS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "capture_service_closed")?;
    let (tx, _rx) = oneshot::channel();
    // The native callback owns this permit after the caller times out. An OS
    // API that never completes cannot accumulate an unbounded callback queue.
    let callback = Arc::new(Mutex::new(CallbackSlot {
        sender: Some(tx),
        _permit: Some(permit),
    }));
    #[cfg(all(feature = "native-screenshot", target_os = "macos"))]
    macos::schedule(&window, callback)?;
    #[cfg(all(feature = "native-screenshot", target_os = "windows"))]
    windows::schedule(&window, callback)?;
    #[cfg(all(feature = "native-screenshot", target_os = "linux"))]
    linux::schedule(&window, callback)?;
    #[cfg(not(all(
        feature = "native-screenshot",
        any(target_os = "macos", target_os = "windows", target_os = "linux")
    )))]
    {
        let _ = (window, callback);
        return Err("capture_backend_unavailable: native-screenshot feature or platform".into());
    }
    #[allow(unreachable_code)]
    tokio::time::timeout(Duration::from_secs(8), _rx)
        .await
        .map_err(|_| "capture_timeout: native callback discarded".to_string())?
        .map_err(|_| "capture_callback_closed".to_string())?
}

async fn window_native(app: &tauri::AppHandle, window_id: &str) -> Result<RawFrame, String> {
    #[cfg(feature = "xcap")]
    {
        let window = app
            .get_webview_window(window_id)
            .ok_or("window_not_found")?;
        let title = window.title().map_err(|_| "window_title_unavailable")?;
        let outer = window
            .outer_position()
            .map_err(|_| "window_geometry_unavailable")?;
        let outer_size = window
            .outer_size()
            .map_err(|_| "window_geometry_unavailable")?;
        let inner = window
            .inner_position()
            .map_err(|_| "window_geometry_unavailable")?;
        let inner_size = window
            .inner_size()
            .map_err(|_| "window_geometry_unavailable")?;
        let permit = CODECS
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "capture_service_closed")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let candidates = xcap::Window::all()
                .map_err(|_| "capture_failed: xcap enumeration")?
                .into_iter()
                .filter(|w| {
                    w.pid().ok() == Some(std::process::id())
                        && w.title().ok().as_deref() == Some(&title)
                })
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                return Err(
                    "capture_target_ambiguous: xcap requires unique process and exact title".into(),
                );
            }
            let target = &candidates[0];
            if target.is_minimized().unwrap_or(true) {
                return Err("capture_not_ready: window minimized".into());
            }
            check_dimensions(
                target.width().map_err(|_| "capture_extent_unavailable")?,
                target.height().map_err(|_| "capture_extent_unavailable")?,
            )?;
            let mut image = target
                .capture_image()
                .map_err(|_| "capture_failed: xcap window")?;
            check_dimensions(image.width(), image.height())?;
            // Prove either a client-only frame or an exact outer-window frame.
            // Unsupported xcap decoration/scale semantics are rejected.
            if image.dimensions() == (outer_size.width, outer_size.height) {
                let dx = inner
                    .x
                    .checked_sub(outer.x)
                    .ok_or("geometry_unknown: window offset")?;
                let dy = inner
                    .y
                    .checked_sub(outer.y)
                    .ok_or("geometry_unknown: window offset")?;
                if dx < 0
                    || dy < 0
                    || dx as u64 + u64::from(inner_size.width) > u64::from(image.width())
                    || dy as u64 + u64::from(inner_size.height) > u64::from(image.height())
                {
                    return Err("geometry_unknown: xcap client bounds".into());
                }
                image = image::imageops::crop_imm(
                    &image,
                    dx as u32,
                    dy as u32,
                    inner_size.width,
                    inner_size.height,
                )
                .to_image();
            } else if image.dimensions() != (inner_size.width, inner_size.height) {
                return Err("geometry_unknown: xcap extent differs from native bounds".into());
            }
            Ok(RawFrame::Bitmap {
                width: image.width(),
                height: image.height(),
                stride: image.width() as usize * 4,
                bytes: image.into_raw(),
                order: PixelOrder::Rgba,
                premultiplied: false,
                opaque: false,
            })
        })
        .await
        .map_err(|_| "capture_worker_failed".to_string())?
    }
    #[cfg(not(feature = "xcap"))]
    {
        let _ = (app, window_id);
        Err("capture_backend_unavailable: xcap feature".into())
    }
}

async fn process_frame(
    frame: RawFrame,
    probe: Probe,
    target: Option<RectCss>,
    max_width: Option<u32>,
) -> Result<(Vec<u8>, Geometry, u32, u32, Value), String> {
    #[cfg(feature = "capture-codec")]
    {
        let permit = CODECS
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "capture_service_closed")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            pixels::protect(frame, &probe, target, max_width)
        })
        .await
        .map_err(|_| "capture_worker_failed".to_string())?
    }
    #[cfg(not(feature = "capture-codec"))]
    {
        let _ = (frame, probe, target, max_width);
        Err("capture_backend_unavailable: capture-codec feature".into())
    }
}

#[cfg(test)]
mod tests;
