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
        plugin_version: None,
        app_name: None,
        app_version: None,
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
        allow_unrelated_app: false,
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

/// Answer `app_identity` as a 0.16+ plugin whose process runs in `workspace`.
async fn spawn_plugin_with_identity(workspace: PathBuf) -> u16 {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let workspace = workspace.clone();
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
                    let reply = serde_json::json!({"id":request["id"],"result":{
                        "appInstanceId":"nearby-instance","appId":"com.example.nearby",
                        "pid":std::process::id(),"startedAt":1,
                        "appName":"Nearby","pluginVersion":"0.17.5-test",
                        "workspacePath":workspace,"inspectionProtocolVersion":1}});
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

#[tokio::test]
async fn a_live_pid_file_below_the_working_directory_routes_every_command() {
    // R2 regression: the app runs with cwd `src-tauri`, so its workspace does
    // not contain the repo root. `status` listed it as ready while `windows`
    // and `runtime-health` failed with app_not_found from the repo root.
    let root = std::env::temp_dir().join(format!(
        "connector-nearby-{}",
        connector_client::inspection::new_request_key()
    ));
    let app_dir = root.join("src-tauri");
    std::fs::create_dir_all(app_dir.join("target")).unwrap();
    let port = spawn_plugin_with_identity(app_dir.canonicalize().unwrap()).await;
    std::fs::write(
        app_dir.join("target/.connector.json"),
        serde_json::json!({"pid":std::process::id(),"ws_port":port,
            "app_id":"com.example.nearby","app_instance_id":"nearby-instance","started_at":1})
        .to_string(),
    )
    .unwrap();
    let resolved = connector_client::discovery::resolve_connection(options(&root))
        .await
        .unwrap();
    assert_eq!(resolved.port, port);
    assert!(matches!(resolved.source, ConnectionSource::PidFile));

    // A stale identity behind the same file is still refused.
    std::fs::write(
        app_dir.join("target/.connector.json"),
        serde_json::json!({"pid":std::process::id(),"ws_port":port,
            "app_id":"com.example.nearby","app_instance_id":"previous-run","started_at":1})
        .to_string(),
    )
    .unwrap();
    let stale = connector_client::discovery::resolve_connection(options(&root)).await;
    assert!(stale.map(|r| r.port != port).unwrap_or(true));
    std::fs::remove_dir_all(root).unwrap();
}

fn exited_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn scanned(app_id: &str, port: u16) -> ResolvedConnection {
    let mut identity = identity();
    identity.app_id = app_id.into();
    identity.app_name = Some("GFIT Workout".into());
    ResolvedConnection {
        host: "127.0.0.1".into(),
        port,
        source: ConnectionSource::PortScan,
        instance: None,
        identity: Some(identity),
    }
}

fn stale_record() -> connector_client::discovery::StaleRecord {
    let mut instance = hint();
    instance.app_name = Some("R2".into());
    instance.app_id = Some("com.example.r2".into());
    instance.ws_port = 9560;
    connector_client::discovery::StaleRecord {
        instance,
        reason: "process 42 is not running".into(),
    }
}

#[test]
fn a_stale_pid_file_refuses_an_unrelated_port_scanned_app() {
    use connector_client::discovery::stale_record_gate;
    // R2 regression: a dead R2 record must not hand the session to GFIT
    // Workout listening on 9555.
    let error = stale_record_gate(
        &[stale_record()],
        &[scanned("com.gfit.workout", 9555)],
        false,
    )
    .unwrap_err();
    assert!(error.starts_with("stale_pid_file"), "{error}");
    assert!(error.contains("fixture.connector.json"), "{error}");
    assert!(error.contains("R2 (com.example.r2)"), "{error}");
    assert!(
        error.contains("GFIT Workout (com.gfit.workout) pid 42 on port 9555"),
        "{error}"
    );
    assert!(error.contains("--app-id com.gfit.workout"), "{error}");
    assert!(error.contains("TAURI_CONNECTOR_ANY_APP=1"), "{error}");

    // Nothing found at all still names the stale record.
    let error = stale_record_gate(&[stale_record()], &[], false).unwrap_err();
    assert!(
        error.contains("no running app with that identity"),
        "{error}"
    );
}

#[test]
fn a_stale_pid_file_allows_its_own_app_or_an_explicit_opt_in() {
    use connector_client::discovery::stale_record_gate;
    let others = [
        scanned("com.gfit.workout", 9555),
        scanned("com.example.r2", 9561),
    ];
    let same = stale_record_gate(&[stale_record()], &others, false)
        .unwrap()
        .unwrap();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].port, 9561);
    // Opt-in and "no stale record" leave the existing selection untouched.
    assert!(stale_record_gate(&[stale_record()], &others[..1], true)
        .unwrap()
        .is_none());
    assert!(stale_record_gate(&[], &others[..1], false)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn resolve_names_a_dead_pid_file_instead_of_attaching_elsewhere() {
    let root = std::env::temp_dir().join(format!(
        "connector-dead-{}",
        connector_client::inspection::new_request_key()
    ));
    std::fs::create_dir_all(root.join("target")).unwrap();
    let pid = exited_pid();
    std::fs::write(
        root.join("target/.connector.json"),
        serde_json::json!({"pid":pid,"ws_port":9599,"app_name":"Gone App",
            "app_id":"com.example.gone-for-test","started_at":1})
        .to_string(),
    )
    .unwrap();
    // Whatever else runs on this machine's scan range, it is not this app.
    let error = connector_client::discovery::resolve_connection(options(&root))
        .await
        .unwrap_err();
    assert!(error.starts_with("stale_pid_file"), "{error}");
    assert!(
        error.contains("Gone App (com.example.gone-for-test)"),
        "{error}"
    );
    assert!(
        error.contains(&format!("process {pid} is not running")),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn status_rows_carry_the_reported_plugin_version() {
    let root = std::env::temp_dir().join(format!(
        "connector-version-{}",
        connector_client::inspection::new_request_key()
    ));
    std::fs::create_dir_all(root.join("target")).unwrap();
    let port = spawn_plugin_with_identity(root.canonicalize().unwrap()).await;
    let path = root.join("target/.connector.json");
    std::fs::write(
        &path,
        serde_json::json!({"pid":std::process::id(),"ws_port":port,
            "app_id":"com.example.nearby","app_instance_id":"nearby-instance","started_at":1})
        .to_string(),
    )
    .unwrap();
    let statuses =
        connector_client::discovery::instance_statuses(&root, None, Some(&path), None).await;
    let identity = statuses[0].identity.as_ref().unwrap();
    assert_eq!(identity.plugin_version.as_deref(), Some("0.17.5-test"));
    assert_eq!(identity.app_name.as_deref(), Some("Nearby"));
    assert_eq!(
        connector_client::discovery::plugin_version_label(Some(identity)),
        "0.17.5-test"
    );
    // An older plugin answers without the field.
    let old = AppIdentity::parse(serde_json::json!({"appInstanceId":"a","appId":"b",
        "pid":1,"startedAt":1,"inspectionProtocolVersion":1}))
    .unwrap();
    assert!(old.plugin_version.is_none());
    assert_eq!(
        connector_client::discovery::plugin_version_label(Some(&old)),
        "<0.17.5 (unreported)"
    );
    std::fs::remove_dir_all(root).unwrap();
}
