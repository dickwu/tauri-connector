//! A packaged app launched with `open` runs from `/` with its executable
//! outside any `target/` directory, so it writes its PID file to its app data
//! dir. Discovery must find it there from any working directory, and an app
//! that refuses identity must not be reported as "nothing running".
//!
//! Own test binary: it sets process-wide environment variables.
use connector_client::discovery::{
    app_data_pid_files, instance_statuses, resolve_connection, ConnectionOptions, ConnectionSource,
    APP_DATA_ROOT_ENV,
};
use futures_util::{SinkExt, StreamExt};
use std::path::{Path, PathBuf};

async fn spawn_app(
    reply: impl Fn(&serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
) -> u16 {
    let reply = std::sync::Arc::new(reply);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let reply = reply.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(message)) = ws.next().await {
                    let Ok(request) = serde_json::from_str::<serde_json::Value>(
                        message.to_text().unwrap_or_default(),
                    ) else {
                        continue;
                    };
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            reply(&request).to_string().into(),
                        ))
                        .await;
                }
            });
        }
    });
    port
}

fn options(cwd: &Path) -> ConnectionOptions {
    ConnectionOptions {
        cwd: cwd.to_path_buf(),
        host: None,
        port: None,
        app_id: None,
        app_instance_id: None,
        pid_file: None,
        allow_unrelated_app: false,
        project: None,
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "connector-{name}-{}",
        connector_client::inspection::new_request_key()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[tokio::test]
async fn a_bundle_outside_target_is_found_through_its_app_data_pid_file() {
    let data_root = temp("app-data");
    let cwd = temp("elsewhere");
    // Keep the real port range out of the picture: nothing listens here.
    std::env::set_var("TAURI_CONNECTOR_PORT_RANGE", "1-1");
    std::env::set_var(APP_DATA_ROOT_ENV, &data_root);

    let port = spawn_app(|request| {
        serde_json::json!({"id":request["id"],"result":{
            "appInstanceId":"bundle-instance","appId":"com.example.bundle",
            "pid":std::process::id(),"startedAt":7,"appName":"Bundle",
            "pluginVersion":"0.17.8","workspacePath":"/","inspectionProtocolVersion":1}})
    })
    .await;
    let record_dir = data_root.join("com.example.bundle/.tauri-connector");
    std::fs::create_dir_all(&record_dir).unwrap();
    let record = record_dir.join(".connector.json");
    std::fs::write(
        &record,
        serde_json::json!({"pid":std::process::id(),"ws_port":port,"app_name":"Bundle",
            "app_id":"com.example.bundle","app_instance_id":"bundle-instance","started_at":7})
        .to_string(),
    )
    .unwrap();
    // An exited app's record is ignored, not reported as stale.
    let dead_dir = data_root.join("com.example.closed/.tauri-connector");
    std::fs::create_dir_all(&dead_dir).unwrap();
    std::fs::write(
        dead_dir.join(".connector.json"),
        serde_json::json!({"pid":u32::MAX - 1,"ws_port":1,"app_id":"com.example.closed"})
            .to_string(),
    )
    .unwrap();
    assert_eq!(app_data_pid_files(&data_root).len(), 2);

    let resolved = resolve_connection(options(&cwd)).await.unwrap();
    assert_eq!(resolved.port, port);
    assert!(matches!(resolved.source, ConnectionSource::PidFile));
    assert_eq!(resolved.instance.unwrap().pid_file, record);

    let statuses = instance_statuses(&cwd, None, None, None).await;
    assert_eq!(statuses.len(), 1, "{statuses:?}");
    assert_eq!(statuses[0].state(), "ready");

    // Before 0.17.8 an app that needs a workflow token was skipped silently
    // and the CLI said app_not_found; now it names the port and the token.
    std::fs::remove_file(&record).unwrap();
    let locked = spawn_app(|request| {
        serde_json::json!({"id":request["id"],"error":
            r#"{"code":"unauthorized","message":"Workflow credential is missing or invalid"}"#})
    })
    .await;
    std::env::set_var("TAURI_CONNECTOR_PORT_RANGE", format!("{locked}-{locked}"));
    let error = resolve_connection(options(&cwd)).await.unwrap_err();
    assert!(error.starts_with("unauthorized"), "{error}");
    assert!(error.contains(&locked.to_string()), "{error}");
    assert!(error.contains("TAURI_CONNECTOR_WORKFLOW_TOKEN"), "{error}");
    let rows = connector_client::discovery::scan_statuses("127.0.0.1", &[]).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state(), "unauthorized");

    // Nothing at all: the error lists where discovery looked.
    std::env::set_var("TAURI_CONNECTOR_PORT_RANGE", "1-1");
    let error = resolve_connection(options(&cwd)).await.unwrap_err();
    assert!(error.starts_with("app_not_found"), "{error}");
    assert!(error.contains(&data_root.display().to_string()), "{error}");
    assert!(error.contains("ports 1-1"), "{error}");

    std::fs::remove_dir_all(data_root).unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}
