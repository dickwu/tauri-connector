//! WKWebView callbacks stay on the AppKit loop; only owned bytes cross threads.
use super::{
    Callback, MAX_RAW_BYTES, PixelOrder, RawFrame, callback_pending, check_dimensions,
    finish_callback,
};
use block2::RcBlock;
use objc2::{MainThreadMarker, runtime::NSObjectProtocol, sel};
use objc2_app_kit::NSImage;
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::NSError;
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

pub(super) fn schedule(window: &tauri::WebviewWindow, callback: Callback) -> Result<(), String> {
    window
        .with_webview(move |platform| {
            if !callback_pending(&callback) {
                return;
            }
            // SAFETY: with_webview invokes on the UI thread and supplies this
            // retained live WKWebView. No borrowed pointer escapes this closure.
            let webview = unsafe { &*(platform.inner() as *const WKWebView) };
            let Some(main_thread) = MainThreadMarker::new() else {
                finish_callback(
                    &callback,
                    Err("capture_failed: snapshot outside AppKit thread".into()),
                );
                return;
            };
            // Modern macOS can extend WKWebView bounds under the title bar while
            // WebKit's CSS viewport excludes its safe-area inset. Crop in native
            // view coordinates, never infer a hardcoded title-bar pixel offset.
            let bounds = webview.bounds();
            let mut region = bounds;
            if webview.respondsToSelector(sel!(safeAreaRect)) {
                // WK's snapshot raster starts at content origin, even when the
                // containing NSView extends under a title bar. Its default extent
                // still uses the larger view bounds. Preserve the content origin
                // and use the unobscured extent; applying safeAreaRect's origin
                // again would crop the first content rows (native marker-tested).
                region.size = webview.safeAreaRect().size;
            }
            // SAFETY: configuration and view are used on their owning AppKit
            // thread; safeAreaRect is a native region within these view bounds.
            let configuration = unsafe { WKSnapshotConfiguration::new(main_thread) };
            unsafe { configuration.setRect(region) };
            // Loading is checked in addition to document-epoch probes, never URL.
            if unsafe { webview.isLoading() } {
                finish_callback(
                    &callback,
                    Err("capture_not_ready: WKWebView loading".into()),
                );
                return;
            }
            let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                if !callback_pending(&callback) {
                    return;
                }
                let result = if !error.is_null() {
                    Err("capture_failed: WKWebView snapshot error".into())
                } else if image.is_null() {
                    Err("capture_failed: WKWebView returned no image".into())
                } else {
                    // SAFETY: WebKit retains its image for the callback. Pixel
                    // data is copied before returning; no NSImage leaves UI.
                    unsafe { copy_bitmap(&*image) }
                };
                finish_callback(&callback, result);
            });
            // SAFETY: WebKit copies the block until completion. Native region
            // selection does not focus, restore, resize or scroll the window.
            unsafe {
                webview
                    .takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &handler)
            };
        })
        .map_err(|_| "capture_failed: scheduling WKWebView snapshot".into())
}

unsafe fn copy_bitmap(image: &NSImage) -> Result<RawFrame, String> {
    // SAFETY: image is a live callback-owned NSImage on the AppKit thread;
    // null rect and absent context/hints request its intrinsic representation.
    let cg =
        unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
            .ok_or("capture_failed: snapshot has no CGImage")?;
    let width = u32::try_from(CGImage::width(Some(&cg))).map_err(|_| "capture_budget_exceeded")?;
    let height =
        u32::try_from(CGImage::height(Some(&cg))).map_err(|_| "capture_budget_exceeded")?;
    check_dimensions(width, height)?;
    let stride = CGImage::bytes_per_row(Some(&cg));
    if stride
        .checked_mul(height as usize)
        .is_none_or(|size| size > MAX_RAW_BYTES)
        || CGImage::bits_per_component(Some(&cg)) != 8
        || CGImage::bits_per_pixel(Some(&cg)) != 32
    {
        return Err("capture_format_unsupported: bounded 8-bit RGBA snapshot required".into());
    }
    let info = CGImage::bitmap_info(Some(&cg)).0;
    if info & 0x100 != 0 {
        return Err("capture_format_unsupported: floating components".into());
    }
    let alpha = CGImage::alpha_info(Some(&cg)).0;
    let order = match (info & 0x7000, alpha) {
        (0x2000, 2 | 4 | 6) => PixelOrder::Bgra,
        (0x2000, 1 | 3 | 5) => PixelOrder::Abgr,
        (0 | 0x4000, 2 | 4 | 6) => PixelOrder::Argb,
        (0 | 0x4000, 1 | 3 | 5) => PixelOrder::Rgba,
        _ => return Err("capture_format_unsupported: pixel channel order".into()),
    };
    let provider = CGImage::data_provider(Some(&cg)).ok_or("capture_failed: no image provider")?;
    let data = CGDataProvider::data(Some(&provider)).ok_or("capture_failed: no image data")?;
    if data.len() > MAX_RAW_BYTES {
        return Err("capture_budget_exceeded: native bytes".into());
    }
    Ok(RawFrame::Bitmap {
        bytes: data.to_vec(),
        width,
        height,
        stride,
        order,
        premultiplied: alpha == 1 || alpha == 2,
        opaque: alpha == 5 || alpha == 6,
    })
}
