//! WebKitGTK visible snapshots; Cairo memory is copied on its owning loop.
use super::{
    Callback, MAX_RAW_BYTES, PixelOrder, RawFrame, callback_pending, check_dimensions,
    finish_callback,
};
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

pub(super) fn schedule(window: &tauri::WebviewWindow, callback: Callback) -> Result<(), String> {
    window
        .with_webview(move |platform| {
            if !callback_pending(&callback) {
                return;
            }
            let view = platform.inner();
            if view.is_loading() {
                finish_callback(
                    &callback,
                    Err("capture_not_ready: WebKitGTK loading".into()),
                );
                return;
            }
            view.snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&gio::Cancellable>,
                move |result| {
                    if !callback_pending(&callback) {
                        return;
                    }
                    let frame = result
                        .map_err(|_| "capture_failed: WebKitGTK snapshot".to_string())
                        .and_then(|surface| {
                            let mut surface = cairo::ImageSurface::try_from(surface).map_err(
                                |_| "capture_format_unsupported: Cairo non-image surface",
                            )?;
                            let width = u32::try_from(surface.width())
                                .map_err(|_| "capture_budget_exceeded")?;
                            let height = u32::try_from(surface.height())
                                .map_err(|_| "capture_budget_exceeded")?;
                            check_dimensions(width, height)?;
                            let stride = usize::try_from(surface.stride())
                                .map_err(|_| "capture_budget_exceeded")?;
                            if stride
                                .checked_mul(height as usize)
                                .is_none_or(|size| size > MAX_RAW_BYTES)
                            {
                                return Err("capture_budget_exceeded: Cairo bytes".into());
                            }
                            let format = surface.format();
                            if !matches!(format, cairo::Format::ARgb32 | cairo::Format::Rgb24) {
                                return Err("capture_format_unsupported: Cairo format".into());
                            }
                            surface.flush();
                            let bytes = surface
                                .data()
                                .map_err(|_| "capture_failed: Cairo buffer borrowed")?
                                .to_vec();
                            Ok(RawFrame::Bitmap {
                                bytes,
                                width,
                                height,
                                stride,
                                order: if cfg!(target_endian = "little") {
                                    PixelOrder::Bgra
                                } else {
                                    PixelOrder::Argb
                                },
                                premultiplied: format == cairo::Format::ARgb32,
                                opaque: format == cairo::Format::Rgb24,
                            })
                        });
                    finish_callback(&callback, frame);
                },
            );
        })
        .map_err(|_| "capture_failed: scheduling WebKitGTK snapshot".into())
}
