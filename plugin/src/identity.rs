//! Host-owned process and window identities. Labels and ports are not identities.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

static APP_INSTANCE: OnceLock<String> = OnceLock::new();
static STARTED_AT: OnceLock<u64> = OnceLock::new();
struct WindowIdentity {
    instance_id: String,
    bridge_key: String,
    loading: bool,
}
static WINDOWS: OnceLock<Mutex<HashMap<String, WindowIdentity>>> = OnceLock::new();

pub fn app_instance_id() -> &'static str {
    APP_INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

pub fn started_at() -> u64 {
    *STARTED_AT.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    })
}

pub fn window_instance_id(label: &str) -> String {
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(label.to_owned())
        .or_insert_with(|| WindowIdentity {
            instance_id: uuid::Uuid::new_v4().to_string(),
            bridge_key: uuid::Uuid::new_v4().to_string(),
            loading: false,
        })
        .instance_id
        .clone()
}

pub fn bridge_key(label: &str) -> String {
    window_instance_id(label);
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())[label]
        .bridge_key
        .clone()
}

pub fn page_navigation(label: &str) {
    if let Some(window) = WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_mut(label)
    {
        window.bridge_key = uuid::Uuid::new_v4().to_string();
        window.loading = true;
    }
}

pub fn page_loaded(label: &str) {
    if let Some(window) = WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_mut(label)
    {
        window.loading = false;
    }
}

pub fn page_loading(label: &str) -> bool {
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(label)
        .is_some_and(|window| window.loading)
}

pub fn verify_bridge_key(label: &str, supplied: &str) -> bool {
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(label)
        .is_some_and(|window| {
            window.bridge_key.len() == supplied.len()
                && window
                    .bridge_key
                    .bytes()
                    .zip(supplied.bytes())
                    .fold(0u8, |difference, (left, right)| difference | (left ^ right))
                    == 0
        })
}

pub fn window_destroyed(label: &str) {
    WINDOWS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(label);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContext {
    pub app_id: String,
    pub app_instance_id: String,
    pub window_id: String,
    pub window_instance_id: String,
    pub page_epoch: String,
    pub runtime_id: String,
    pub runtime_version: String,
    pub semantic_version: String,
    pub bundle_hash: String,
    pub origin: String,
}

pub fn app_identity(app: &tauri::AppHandle) -> Value {
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    json!({
        "appId": app.config().identifier, "appInstanceId": app_instance_id(),
        "pid": std::process::id(), "startedAt": started_at(),
        "workspacePath": workspace.as_ref().map(|path|path.to_string_lossy().into_owned()),
        "workspaceId": workspace.map(|path| super::runtime::manifest::content_hash(path.to_string_lossy().as_bytes())),
        "inspectionProtocolVersion": 1, "workflowProtocolVersion": 1
    })
}

pub fn origin_allowed(app: &tauri::AppHandle, current: &tauri::Url) -> bool {
    let config = app.config();
    (current.scheme() == "tauri" && current.host_str() == Some("localhost"))
        || (matches!(current.scheme(), "http" | "https")
            && current.host_str() == Some("tauri.localhost"))
        || config
            .build
            .dev_url
            .as_ref()
            .is_some_and(|url| url.origin() == current.origin())
        || config.app.windows.iter().any(|window| match &window.url {
            tauri::WebviewUrl::External(url) => url.origin() == current.origin(),
            tauri::WebviewUrl::CustomProtocol(url) => {
                url.scheme() == current.scheme() && url.host_str() == current.host_str()
            }
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn label_reuse_changes_host_identity() {
        let label = uuid::Uuid::new_v4().to_string();
        let first = window_instance_id(&label);
        assert_eq!(first, window_instance_id(&label));
        window_destroyed(&label);
        assert_ne!(first, window_instance_id(&label));
        window_destroyed(&label);
    }
    #[test]
    fn navigation_rotates_transport_binding_without_reusing_window_lifetime() {
        let label = uuid::Uuid::new_v4().to_string();
        let instance = window_instance_id(&label);
        let first = bridge_key(&label);
        assert!(!page_loading(&label));
        page_navigation(&label);
        assert!(page_loading(&label));
        assert_eq!(window_instance_id(&label), instance);
        assert!(!verify_bridge_key(&label, &first));
        assert!(verify_bridge_key(&label, &bridge_key(&label)));
        page_loaded(&label);
        assert!(!page_loading(&label));
        assert_eq!(window_instance_id(&label), instance);
        window_destroyed(&label);
    }

    #[test]
    fn process_identity_is_shared_and_stable() {
        assert_eq!(app_instance_id(), app_instance_id());
        assert_eq!(started_at(), started_at());
    }
}
