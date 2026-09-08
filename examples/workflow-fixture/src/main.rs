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
    tasks: Vec<Task>,
    delay_ms: u64,
}

struct FixtureStore {
    data: Mutex<FixtureData>,
    evidence: Option<PathBuf>,
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
        .invoke_handler(tauri::generate_handler![
            fixture_create_task,
            fixture_state,
            fixture_reset,
            fixture_set_delay
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
                app.add_capability(include_str!("../capabilities-dev/connector.json"))?;
                Ok(())
            })
    };
    let mut context = tauri::generate_context!();
    if let Ok(identifier) = std::env::var("CONNECTOR_FIXTURE_ID") {
        assert!(identifier.starts_with("dev.connector.workflow-fixture."));
        context.config_mut().identifier = identifier;
    }
    builder.run(context).expect("run isolated workflow fixture");
}
