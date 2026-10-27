use connector_client::discovery::{
    discover_instances, select_unique, validate_identity, ConnectionSource, ConnectorInstance,
    ResolvedConnection,
};
use connector_client::identity::AppIdentity;
use std::path::PathBuf;
fn identity() -> AppIdentity {
    AppIdentity {
        app_instance_id: "one".into(),
        app_id: "fixture".into(),
        pid: 42,
        started_at: 123,
        workspace_id: None,
        workspace_path: None,
        inspection_protocol_version: 1,
        workflow_protocol_version: Some(1),
    }
}
fn hint() -> ConnectorInstance {
    ConnectorInstance {
        pid: 42,
        ws_port: 9555,
        mcp_port: None,
        bridge_port: None,
        app_name: None,
        app_id: Some("fixture".into()),
        app_instance_id: Some("one".into()),
        log_dir: None,
        exe: None,
        started_at: Some(123),
        pid_file: PathBuf::from("fixture.connector.json"),
    }
}
#[test]
fn explicit_identity_and_stale_pid_are_rejected() {
    assert!(validate_identity(&identity(), Some("fixture"), Some("one"), Some(&hint())).is_ok());
    assert!(validate_identity(&identity(), Some("other"), None, None).is_err());
    assert!(validate_identity(&identity(), None, Some("other"), None).is_err());
    let mut stale = hint();
    stale.started_at = Some(122);
    assert!(validate_identity(&identity(), None, None, Some(&stale)).is_err());
    stale = hint();
    stale.app_instance_id = Some("prior-process".into());
    assert!(validate_identity(&identity(), None, None, Some(&stale)).is_err());
    stale = hint();
    stale.pid = 43;
    assert!(validate_identity(&identity(), None, None, Some(&stale)).is_err());
}
#[test]
fn ambiguous_candidates_never_choose_recent_or_first() {
    let candidate = ResolvedConnection {
        host: "127.0.0.1".into(),
        port: 9555,
        source: ConnectionSource::PortScan,
        instance: None,
        identity: Some(identity()),
    };
    assert!(select_unique(vec![])
        .unwrap_err()
        .starts_with("app_not_found"));
    assert!(select_unique(vec![candidate.clone(), candidate.clone()])
        .unwrap_err()
        .starts_with("ambiguous_app"));
    assert_eq!(
        select_unique(vec![candidate])
            .unwrap()
            .identity
            .unwrap()
            .app_instance_id,
        "one"
    );
}
#[test]
fn explicit_missing_pid_file_cannot_search_nearby_files() {
    let root = std::env::temp_dir().join(format!(
        "connector-routing-{}",
        connector_client::inspection::new_request_key()
    ));
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(
        root.join("target/.connector.json"),
        serde_json::to_vec(&hint()).unwrap(),
    )
    .unwrap();
    assert!(discover_instances(&root, None, Some(&root.join("missing.json"))).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

/// Serve WebSocket connections the way tauri-plugin-connector 0.15 did: it
/// answers `bridge_status` without an inspection protocol and replies to any
/// unknown command with id `unknown`, so identity requests never settle;
/// `ping` still answers `pong`.
/// With `legacy == false` the server never answers anything.
async fn spawn_plugin_without_identity(legacy: bool) -> u16 {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(message)) = ws.next().await {
                    let Ok(text) = message.to_text() else {
                        continue;
                    };
                    let Ok(request) = serde_json::from_str::<serde_json::Value>(text) else {
                        continue;
                    };
                    if !legacy {
                        continue;
                    }
                    let reply = if request["type"] == "ping" {
                        serde_json::json!({"id":request["id"],"result":"pong"})
                    } else if request["type"] == "bridge_status" {
                        serde_json::json!({"id":request["id"],"result":{"running":true,"connected_windows":1}})
                    } else {
                        serde_json::json!({"id":"unknown","error":"Invalid request: unknown variant `inspection`"})
                    };
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            reply.to_string().into(),
                        ))
                        .await;
                }
            });
        }
    });
    port
}

fn legacy_pid_file(port: u16) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "connector-legacy-{}",
        connector_client::inspection::new_request_key()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(".connector.json");
    std::fs::write(
        &path,
        serde_json::json!({"pid":std::process::id(),"ws_port":port,"app_id":"com.example.hub","started_at":1})
            .to_string(),
    )
    .unwrap();
    (root, path)
}

fn options(root: &std::path::Path) -> connector_client::discovery::ConnectionOptions {
    connector_client::discovery::ConnectionOptions {
        cwd: root.to_path_buf(),
        host: None,
        port: None,
        app_id: None,
        app_instance_id: None,
        pid_file: None,
    }
}

#[tokio::test]
async fn pid_file_routing_to_a_pre_identity_plugin_says_upgrade_not_app_not_found() {
    use connector_client::discovery::resolve_connection;
    let port = spawn_plugin_without_identity(true).await;
    let (root, path) = legacy_pid_file(port);

    let mut by_pid_file = options(&root);
    by_pid_file.pid_file = Some(path.clone());
    let error = resolve_connection(by_pid_file).await.unwrap_err();
    assert!(error.starts_with("capability_unavailable"), "{error}");
    assert!(error.contains("older than 0.16.0"), "{error}");
    assert!(error.contains(&format!("--port {port}")), "{error}");

    let mut by_app_id = options(&root);
    by_app_id.app_id = Some("com.example.hub".into());
    by_app_id.pid_file = Some(path.clone());
    let error = resolve_connection(by_app_id).await.unwrap_err();
    assert!(error.contains("older than 0.16.0"), "{error}");

    // An explicit port plus an identity constraint still refuses to connect
    // unverified, but names the real cause.
    let mut explicit = options(&root);
    explicit.port = Some(port);
    explicit.app_id = Some("com.example.hub".into());
    let error = resolve_connection(explicit).await.unwrap_err();
    assert!(error.contains("older than 0.16.0"), "{error}");

    // `status` reports the instance as alive with an old plugin, not stale.
    let statuses =
        connector_client::discovery::instance_statuses(&root, None, Some(&path), None).await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state(), "legacy_plugin");
    assert!(statuses[0].pid_alive && statuses[0].ws_reachable);

    // A bare --port keeps connecting without identity verification.
    let mut bare = options(&root);
    bare.port = Some(port);
    let resolved = resolve_connection(bare).await.unwrap();
    assert_eq!(resolved.port, port);
    assert!(resolved.identity.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_silent_endpoint_is_not_mistaken_for_a_legacy_plugin() {
    let port = spawn_plugin_without_identity(false).await;
    let (root, path) = legacy_pid_file(port);
    let mut by_pid_file = options(&root);
    let path_for_status = path.clone();
    by_pid_file.pid_file = Some(path);
    let error = connector_client::discovery::resolve_connection(by_pid_file)
        .await
        .unwrap_err();
    assert!(error.starts_with("app_not_found"), "{error}");
    let statuses =
        connector_client::discovery::instance_statuses(&root, None, Some(&path_for_status), None)
            .await;
    assert_eq!(statuses[0].state(), "stale");
    std::fs::remove_dir_all(root).unwrap();
}
