use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
#[cfg(feature = "dev-connector")]
use tauri::Manager;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Task {
    id: String,
    name: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct FixtureData {
    save_calls: usize,
    pending_invocations: usize,
    tasks: Vec<Task>,
    delay_ms: u64,
    input_effects: std::collections::BTreeMap<String, usize>,
}

struct FixtureStore {
    data: Mutex<FixtureData>,
    evidence: Option<PathBuf>,
}

// Boot diagnostics are opt-in and separate from the business evidence store.
struct FixtureBootEvidence(Option<PathBuf>);
struct FixtureStallEvidence(Option<PathBuf>);

#[tauri::command]
async fn fixture_wait_for_stall(
    evidence: tauri::State<'_, FixtureStallEvidence>,
) -> Result<(), String> {
    let path = evidence
        .0
        .as_ref()
        .ok_or("fixture stall evidence not configured")?;
    let staging = path.with_extension("tmp");
    std::fs::write(&staging, br#"{"phase":"armed"}"#).map_err(|e| e.to_string())?;
    std::fs::rename(staging, path).map_err(|e| e.to_string())?;
    let signal = path.with_extension("signal");
    for _ in 0..600 {
        if signal.exists() {
            std::fs::remove_file(signal).map_err(|e| e.to_string())?;
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    Err("fixture stall signal timed out".into())
}

#[tauri::command]
fn fixture_stall_finished(
    elapsed_ms: f64,
    evidence: tauri::State<'_, FixtureStallEvidence>,
) -> Result<(), String> {
    let path = evidence
        .0
        .as_ref()
        .ok_or("fixture stall evidence not configured")?;
    if !elapsed_ms.is_finite() || !(1000.0..=5000.0).contains(&elapsed_ms) {
        return Err("fixture stall duration outside bounds".into());
    }
    let staging = path.with_extension("tmp");
    std::fs::write(
        &staging,
        serde_json::to_vec(&serde_json::json!({"phase":"finished","elapsedMs":elapsed_ms}))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(staging, path).map_err(|e| e.to_string())
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrontendDiagnostic {
    document_ready_state: String,
    fixture_ready: bool,
    connector_globals: Vec<String>,
    hook_markers: Vec<String>,
}

#[tauri::command]
fn fixture_frontend_ready(
    window: tauri::WebviewWindow,
    diagnostic: FrontendDiagnostic,
    evidence: tauri::State<'_, FixtureBootEvidence>,
) -> Result<(), String> {
    let Some(path) = &evidence.0 else {
        return Ok(());
    };
    if window.label() != "main" {
        return Ok(());
    }
    if diagnostic.document_ready_state.len() > 16
        || diagnostic.connector_globals.len() > 128
        || diagnostic.hook_markers.len() > 128
        || diagnostic
            .connector_globals
            .iter()
            .chain(&diagnostic.hook_markers)
            .any(|name| name.len() > 128)
    {
        return Err("fixture boot diagnostic exceeds bounds".into());
    }
    let mut value = serde_json::to_value(diagnostic).map_err(|e| e.to_string())?;
    value["processId"] = serde_json::json!(std::process::id());
    value["windowId"] = serde_json::json!(window.label());
    value["connectorFeature"] = serde_json::json!(cfg!(feature = "dev-connector"));
    let staging = path.with_extension("tmp");
    std::fs::write(
        &staging,
        serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(staging, path).map_err(|e| e.to_string())
}

impl FixtureStore {
    fn publish(&self, data: &FixtureData) -> Result<(), String> {
        if let Some(path) = &self.evidence {
            let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
            let staging = path.with_extension("tmp");
            std::fs::write(&staging, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(staging, path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Isolated in-memory business fixture. The event handler passes React state;
/// the independent evidence file records what the Rust handler actually saw.
#[tauri::command]
async fn fixture_create_task(
    name: String,
    store: tauri::State<'_, FixtureStore>,
) -> Result<Task, String> {
    let (task, delay_ms) = {
        let mut data = store.data.lock().map_err(|e| e.to_string())?;
        data.save_calls += 1;
        let task = Task {
            id: format!("fixture-task-{}", data.save_calls),
            name,
        };
        data.tasks.push(task.clone());
        store.publish(&data)?;
        (task, data.delay_ms)
    };
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    Ok(task)
}

#[tauri::command]
fn fixture_state(store: tauri::State<'_, FixtureStore>) -> Result<FixtureData, String> {
    Ok(store.data.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn fixture_reset(store: tauri::State<'_, FixtureStore>) -> Result<(), String> {
    let mut data = store.data.lock().map_err(|e| e.to_string())?;
    *data = FixtureData::default();
    store.publish(&data)
}

#[tauri::command]
fn fixture_set_delay(delay_ms: u64, store: tauri::State<'_, FixtureStore>) -> Result<(), String> {
    let mut data = store.data.lock().map_err(|e| e.to_string())?;
    data.delay_ms = delay_ms.min(60_000);
    store.publish(&data)
}

#[tauri::command]
fn fixture_record_input(kind: String, store: tauri::State<'_, FixtureStore>) -> Result<(), String> {
    if ![
        "click",
        "submit",
        "change",
        "navigation",
        "earlyCapture",
        "escapeKeydown",
        "escapeKeyup",
    ]
    .contains(&kind.as_str())
    {
        return Err("invalid fixture counter".into());
    }
    let mut data = store.data.lock().map_err(|e| e.to_string())?;
    *data.input_effects.entry(kind).or_default() += 1;
    store.publish(&data)
}

#[tauri::command]
async fn fixture_slow_write(store: tauri::State<'_, FixtureStore>) -> Result<Task, String> {
    fixture_create_task("isolated slow write".into(), store).await
}

#[tauri::command]
async fn fixture_fail_after_write(store: tauri::State<'_, FixtureStore>) -> Result<(), String> {
    fixture_create_task("isolated failing write".into(), store).await?;
    Err("fixture rejection password=must-not-appear".into())
}

#[tauri::command]
fn fixture_binary_result() -> tauri::ipc::Response {
    tauri::ipc::Response::new(vec![1, 2, 3, 4])
}

#[tauri::command]
async fn fixture_pending(store: tauri::State<'_, FixtureStore>) -> Result<(), String> {
    {
        let mut data = store.data.lock().map_err(|e| e.to_string())?;
        data.pending_invocations += 1;
        store.publish(&data)?;
    }
    std::future::pending().await
}

#[tauri::command]
fn fixture_window(window: tauri::WebviewWindow) -> Result<serde_json::Value, String> {
    window.set_focus().map_err(|e| e.to_string())?;
    let position = window.inner_position().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"x":position.x,"y":position.y,"scale":scale}))
}

fn main() {
    let store = FixtureStore {
        data: Mutex::new(FixtureData::default()),
        evidence: std::env::var_os("CONNECTOR_FIXTURE_EVIDENCE").map(PathBuf::from),
    };
    store
        .publish(&FixtureData::default())
        .expect("write isolated fixture evidence");
    let builder = tauri::Builder::default()
        .manage(store)
        .manage(FixtureStallEvidence(
            std::env::var_os("CONNECTOR_FIXTURE_STALL_EVIDENCE").map(PathBuf::from),
        ))
        .manage(FixtureBootEvidence(
            std::env::var_os("CONNECTOR_FIXTURE_BOOT_EVIDENCE").map(PathBuf::from),
        ))
        .invoke_handler(tauri::generate_handler![
            fixture_create_task,
            fixture_state,
            fixture_reset,
            fixture_set_delay,
            fixture_record_input,
            fixture_slow_write,
            fixture_fail_after_write,
            fixture_binary_result,
            fixture_pending,
            fixture_window,
            fixture_frontend_ready,
            fixture_wait_for_stall,
            fixture_stall_finished
        ]);
    #[cfg(feature = "dev-connector")]
    let builder = {
        let token = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN")
            .expect("set an isolated workflow token (at least 32 bytes)");
        assert!(
            token.len() >= 32,
            "fixture workflow token must be at least 32 bytes"
        );
        builder
            .plugin(
                tauri_plugin_connector::ConnectorBuilder::new()
                    .port_range(19555, 19556)
                    .mcp_port_range(19556, 19557)
                    .workflow_token(token)
                    .build(),
            )
            .setup(|app| {
                #[cfg(target_os = "macos")]
                if std::env::var("CONNECTOR_FIXTURE_BACKGROUND").as_deref() == Ok("1") {
                    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
                app.add_capability(include_str!("../capabilities-dev/connector.json"))?;
                Ok(())
            })
    };
    let mut context = tauri::generate_context!();
    if std::env::var("CONNECTOR_FIXTURE_BACKGROUND").as_deref() == Ok("1") {
        for window in &mut context.config_mut().app.windows {
            window.focus = false;
        }
    }
    if let Ok(identifier) = std::env::var("CONNECTOR_FIXTURE_ID") {
        assert!(identifier.starts_with("dev.connector.workflow-fixture."));
        context.config_mut().identifier = identifier;
    }
    builder.run(context).expect("run isolated workflow fixture");
}
