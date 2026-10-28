//! Read a WebView's current URL without tripping wry's macOS panic.
//!
//! wry 0.57's `url_from_webview` does `WKWebView::URL().unwrap()`. While a
//! first load is still provisional (for example a dev server compiling) the
//! URL is nil, and the panic fires on the AppKit main thread where
//! `catch_unwind` in plugin code cannot reach it. On macOS the plugin therefore
//! never calls `Webview::url()`; it reads the nil-able property itself on the
//! UI thread and reports "no URL yet" instead.
use tauri::Url;

pub const NO_URL: &str = "webview has no URL yet (page still loading)";

/// Turns a raw native URL string into a parsed URL; `None` means the WebView
/// has not committed a URL yet.
pub(crate) fn from_native(raw: Option<String>) -> Result<Url, String> {
    let raw = raw.filter(|raw| !raw.is_empty()).ok_or_else(|| NO_URL.to_owned())?;
    Url::parse(&raw).map_err(|error| format!("webview URL is not parseable: {error}"))
}

#[cfg(target_os = "macos")]
pub fn current<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> Result<Url, String> {
    use std::sync::mpsc;
    let (sender, receiver) = mpsc::channel();
    window
        .with_webview(move |platform| {
            let _ = sender.send(macos::read(platform.inner()));
        })
        .map_err(|error| error.to_string())?;
    // with_webview runs inline on the main thread and is queued elsewhere.
    let raw = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| NO_URL.to_owned())?;
    from_native(raw)
}

#[cfg(not(target_os = "macos"))]
pub fn current<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> Result<Url, String> {
    window.url().map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{msg_send, rc::Retained, runtime::AnyObject};
    use std::ffi::{CStr, c_char, c_void};

    pub(super) fn read(webview: *mut c_void) -> Option<String> {
        if webview.is_null() {
            return None;
        }
        // SAFETY: with_webview hands over the live WKWebView on its UI thread.
        // `URL` and `absoluteString` are nullable and checked before use; the
        // UTF-8 buffer is copied before the autoreleased string can go away.
        unsafe {
            let webview = &*(webview as *const AnyObject);
            let url: Option<Retained<AnyObject>> = msg_send![webview, URL];
            let url = url?;
            let text: Option<Retained<AnyObject>> = msg_send![&*url, absoluteString];
            let text = text?;
            let bytes: *const c_char = msg_send![&*text, UTF8String];
            (!bytes.is_null()).then(|| CStr::from_ptr(bytes).to_string_lossy().into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_webview_without_a_url_reports_loading_instead_of_panicking() {
        assert_eq!(from_native(None).unwrap_err(), NO_URL);
        assert_eq!(from_native(Some(String::new())).unwrap_err(), NO_URL);
        assert!(from_native(Some("not a url".into())).is_err());
        assert_eq!(
            from_native(Some("http://localhost:3000/a?b=1".into()))
                .unwrap()
                .as_str(),
            "http://localhost:3000/a?b=1"
        );
    }

    #[test]
    fn plugin_code_never_calls_the_panicking_url_getter() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, hits);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !path.ends_with("webview_url.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (line, code) in text.lines().enumerate() {
                        if (code.contains("window.url()") || code.contains("webview.url()"))
                            && !code.trim_start().starts_with("//")
                        {
                            hits.push(format!("{}:{}", path.display(), line + 1));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
        assert!(hits.is_empty(), "use webview_url::current instead: {hits:?}");
    }
}
