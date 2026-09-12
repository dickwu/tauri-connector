use super::*;
#[tokio::test]
async fn capture_anonymous_rejected_before_dispatch_and_legacy_clear_isolated() {
    let path =
        std::env::temp_dir().join(format!("connector-capture-test-{}", uuid::Uuid::new_v4()));
    let state = PluginState::new(path.clone()).unwrap();
    state.workflow.set_token(Some("x".repeat(32)));
    let bridge = Bridge::start().unwrap();
    let denied = handle("ipc_capture", &json!({"action":"start"}), &state, &bridge)
        .await
        .unwrap_err();
    assert_eq!(denied["code"], "unauthorized");
    assert_eq!(bridge.status().await["pending"], 0);
    let domain = state
        .workflow
        .inspection_authorize(&json!({"authToken":"x".repeat(32)}))
        .unwrap();
    let (id, source) = {
        let mut s = state.capture.store.lock().unwrap();
        let source = s.source("main", json!({"windowId":"main","pageEpoch":"p"}));
        let id = s
            .start(domain, "main", CaptureOptions::default(), &source)
            .unwrap();
        (id, source)
    };
    state.capture.ingest("main",json!({"sourceId":source,"events":[{"sourceSequence":1,"invocationId":"private-invocation","command":"private-business","wallTimeMs":1,"phase":"started"}]}),&state.workflow);
    assert!(
        std::fs::read_to_string(path.join("ipc.log"))
            .unwrap()
            .is_empty()
    );
    crate::handlers::clear_logs("legacy-clear", "ipc", &state).await;
    let result = handle(
        "ipc_query",
        &json!({"captureSessionId":id,"authToken":"x".repeat(32)}),
        &state,
        &bridge,
    )
    .await
    .unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    state.workflow.set_token(Some("y".repeat(32)));
    assert!(!state.workflow.inspection_authorized(domain));
    let denied = handle(
        "ipc_query",
        &json!({"captureSessionId":id,"authToken":"y".repeat(32)}),
        &state,
        &bridge,
    )
    .await
    .unwrap_err();
    assert_eq!(denied["code"], "capture_not_found");
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn capture_diagnostics_never_release_quarantine_or_trust_correlation() {
    let path = std::env::temp_dir().join(format!(
        "connector-capture-quarantine-{}",
        uuid::Uuid::new_v4()
    ));
    let state = PluginState::new(path.clone()).unwrap();
    let token = "z".repeat(32);
    state.workflow.set_token(Some(token.clone()));
    let domain = state
        .workflow
        .inspection_authorize(&json!({"authToken":token}))
        .unwrap();
    let (id, source) = {
        let mut s = state.capture.store.lock().unwrap();
        s.max_events = 1;
        let source = s.source("main", json!({"windowId":"main","pageEpoch":"p"}));
        let id = s
            .start(domain, "main", CaptureOptions::default(), &source)
            .unwrap();
        (id, source)
    };
    state
        .workflow
        .resources
        .restore_quarantine(vec!["backend".into()]);
    for seq in 1..=3 {
        state.capture.ingest("main",json!({"sourceId":source,"events":[{"sourceSequence":seq,"invocationId":format!("invoke-{seq}"),"command":"business","phase":"succeeded","wallTimeMs":1,"correlation":{"kind":"causal","actionId":"forged"},"businessPersistence":"committed"}]}),&state.workflow);
    }
    let bridge = Bridge::start().unwrap();
    let report = handle(
        "ipc_query",
        &json!({"captureSessionId":id,"authToken":token}),
        &state,
        &bridge,
    )
    .await
    .unwrap();
    assert_eq!(report["events"][0]["correlation"]["kind"], "none");
    assert_eq!(report["events"][0]["businessPersistence"], "unobserved");
    assert!(!report.to_string().contains("forged"));
    let start = handle(
        "ipc_capture",
        &json!({"action":"start","authToken":token}),
        &state,
        &bridge,
    )
    .await
    .unwrap_err();
    assert_eq!(start["code"], "resource_busy");
    assert_eq!(state.workflow.resources.quarantined(), 1);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn capture_retention_shares_workflow_budget_and_releases_on_revocation() {
    let path =
        std::env::temp_dir().join(format!("connector-capture-budget-{}", uuid::Uuid::new_v4()));
    let state = PluginState::new(path.clone()).unwrap();
    let other = state
        .workflow
        .reserve_inspection_memory(48 * 1024 * 1024)
        .unwrap();
    {
        let mut s = state.capture.store.lock().unwrap();
        s.reservation = Some(
            state
                .workflow
                .reserve_inspection_memory(16 * 1024 * 1024)
                .unwrap(),
        );
        let source = s.source("main", json!({"windowId":"main","pageEpoch":"p"}));
        s.start(1, "main", CaptureOptions::default(), &source)
            .unwrap();
    }
    assert!(state.workflow.reserve_inspection_memory(1).is_err());
    state.capture.revoke_all();
    assert!(state.workflow.reserve_inspection_memory(1).is_ok());
    drop(other);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn capture_background_expiry_releases_budget_without_another_request() {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    let path = std::env::temp_dir().join(format!(
        "connector-capture-watchdog-{}",
        uuid::Uuid::new_v4()
    ));
    let state = PluginState::new(path.clone()).unwrap();
    state.workflow.set_token(Some("w".repeat(32)));
    let domain = state
        .workflow
        .inspection_authorize(&json!({"authToken":"w".repeat(32)}))
        .unwrap();
    let other = state
        .workflow
        .reserve_inspection_memory(48 * 1024 * 1024)
        .unwrap();
    {
        let mut s = state.capture.store.lock().unwrap();
        s.session_lifetime = Duration::from_millis(20);
        s.retention = Duration::from_millis(20);
        s.reservation = Some(
            state
                .workflow
                .reserve_inspection_memory(16 * 1024 * 1024)
                .unwrap(),
        );
        let source = s.source("missing", json!({"windowId":"missing","pageEpoch":"p"}));
        s.start(domain, "missing", CaptureOptions::default(), &source)
            .unwrap();
    }
    let bridge = Bridge::start().unwrap();
    state.capture.start_maintenance(&state.workflow, &bridge);
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.capture.maintenance_started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(state.workflow.reserve_inspection_memory(1).is_ok());
    assert!(
        state
            .capture
            .store
            .lock()
            .unwrap()
            .next_deadline()
            .is_none()
    );
    drop(other);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn opaque_next_cursor_roundtrips_through_public_schema_and_host_query() {
    let path = std::env::temp_dir().join(format!(
        "connector-capture-cursor-contract-{}",
        uuid::Uuid::new_v4()
    ));
    let state = PluginState::new(path.clone()).unwrap();
    let token = "cursor-contract-fixture-token-32-bytes";
    state.workflow.set_token(Some(token.into()));
    let domain = state
        .workflow
        .inspection_authorize(&json!({"authToken":token}))
        .unwrap();
    let (id, source) = {
        let mut store = state.capture.store.lock().unwrap();
        let source = store.source("main", json!({"windowId":"main","pageEpoch":"page"}));
        let id = store
            .start(domain, "main", CaptureOptions::default(), &source)
            .unwrap();
        (id, source)
    };
    for seq in 1..=2 {
        state.capture.ingest("main",json!({"sourceId":source,"events":[{"sourceSequence":seq,"invocationId":format!("cursor-invocation-{seq}"),"command":"cursor-fixture","wallTimeMs":1,"phase":"started"}]}),&state.workflow);
    }
    let bridge = Bridge::start().unwrap();
    let first = handle(
        "ipc_query",
        &json!({"captureSessionId":id,"authToken":token,"limit":1}),
        &state,
        &bridge,
    )
    .await
    .unwrap();
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    let cursor = first["nextCursor"].clone();
    assert!(cursor.is_string());
    let schema = crate::mcp_tool_schema::tool_definitions()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "ipc_query")
        .unwrap()["inputSchema"]
        .clone();
    assert_eq!(
        schema["properties"]["cursor"]["type"], "string",
        "The public schema must accept the cursor actually returned by ipc_query"
    );
    let second = handle(
        "ipc_query",
        &json!({"captureSessionId":id,"authToken":token,"limit":1,"cursor":cursor}),
        &state,
        &bridge,
    )
    .await
    .unwrap();
    assert_eq!(second["events"].as_array().unwrap().len(), 1);
    assert_ne!(
        first["events"][0]["eventId"],
        second["events"][0]["eventId"]
    );
    assert_eq!(second["events"][0]["invocationId"], "cursor-invocation-2");
    assert!(
        handle(
            "ipc_query",
            &json!({"captureSessionId":id,"authToken":token,"cursor":1}),
            &state,
            &bridge
        )
        .await
        .is_err()
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
