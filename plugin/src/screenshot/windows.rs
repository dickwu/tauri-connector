//! WebView2 CapturePreview runs entirely in its owning COM apartment.
use super::{Callback, MAX_ENCODED_BYTES, RawFrame, callback_pending, finish_callback};
use webview2_com::{
    CapturePreviewCompletedHandler,
    Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
};
use windows::Win32::System::Com::{
    IStream, STREAM_SEEK_END, STREAM_SEEK_SET, StructuredStorage::CreateStreamOnHGlobal,
};

pub(super) fn schedule(window: &tauri::WebviewWindow, callback: Callback) -> Result<(), String> {
    window
        .with_webview(move |platform| {
            if !callback_pending(&callback) {
                return;
            }
            // SAFETY: Tauri dispatches with_webview to the controller's UI/COM
            // apartment. COM interfaces remain owned in this apartment and callback.
            let result = (|| unsafe {
                let view = platform
                    .controller()
                    .CoreWebView2()
                    .map_err(|_| "capture_failed: WebView2 controller")?;
                // delete-on-release gives the stream ownership of its HGLOBAL.
                let stream =
                    CreateStreamOnHGlobal(windows::Win32::Foundation::HGLOBAL::default(), true)
                        .map_err(|_| "capture_failed: native memory stream")?;
                let output = stream.clone();
                let completed = callback.clone();
                let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                    if callback_pending(&completed) {
                        let frame = result
                            .map_err(|_| "capture_failed: WebView2 CapturePreview".to_string())
                            .and_then(|()| read_stream(&output))
                            .map(RawFrame::Encoded);
                        finish_callback(&completed, frame);
                    }
                    Ok(())
                }));
                // The caller has already required a same-epoch geometry probe with
                // readyState interactive/complete (after ContentLoading), and will
                // recheck the page/window/runtime context after this callback.
                view.CapturePreview(
                    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                    &stream,
                    &handler,
                )
                .map_err(|_| "capture_failed: scheduling WebView2 CapturePreview")
            })();
            if let Err(error) = result {
                finish_callback(&callback, Err(error.into()));
            }
        })
        .map_err(|_| "capture_failed: scheduling WebView2 apartment".into())
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>, String> {
    // SAFETY: stream is owned and used only in its CapturePreview completion
    // apartment. Bound the allocation before reading into a live Vec buffer.
    unsafe {
        let mut size = 0u64;
        stream
            .Seek(0, STREAM_SEEK_END, Some(&mut size))
            .map_err(|_| "capture_failed: stream length")?;
        if size == 0 || size > MAX_ENCODED_BYTES as u64 {
            return Err("capture_budget_exceeded: WebView2 image".into());
        }
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(|_| "capture_failed: stream seek")?;
        let mut bytes = vec![0; size as usize];
        let mut read = 0u32;
        stream
            .Read(
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                Some(&mut read),
            )
            .ok()
            .map_err(|_| "capture_failed: stream read")?;
        if u64::from(read) != size {
            return Err("capture_failed: incomplete WebView2 image".into());
        }
        Ok(bytes)
    }
}
