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
