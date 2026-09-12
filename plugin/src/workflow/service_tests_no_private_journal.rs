//! Non-Unix contract: durable workflow execution is unavailable until private
//! journal ACL enforcement exists. These tests must never enable unsafe storage.
use super::*;
use std::sync::atomic::AtomicUsize;

const PRINCIPAL: &str = "fixture-service-token-32-bytes-long-12345";

struct Fixture {
    directory: PathBuf,
    state: PluginState,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "connector-unavailable-journal-{}",
            uuid::Uuid::new_v4()
        ));
        let state = PluginState::new(directory.join("logs")).unwrap();
        state.workflow.set_token(Some(PRINCIPAL.into()));
        Self { directory, state }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[derive(Default)]
struct MustNotDispatch(AtomicUsize);

impl Backend for MustNotDispatch {
    fn call<'a>(
        &'a self,
        _window: &'a str,
        _request: Value,
        _timeout_ms: u64,
        _request_id: String,
    ) -> BackendFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("Unavailable private journal must prevent every backend call") })
    }
}

fn spec() -> WorkflowSpec {
    WorkflowSpec::parse(&json!({
        "schemaVersion": 1,
        "runKey": "unavailable-journal-retry",
        "deadlineMs": 5000,
        "steps": [{"id":"write","op":"click","target":{"by":"css","value":"button"}}]
    }))
    .unwrap()
}

#[tokio::test]
async fn authenticated_duplicate_submissions_fail_before_registration_or_backend_dispatch() {
    let fixture = Fixture::new();
    let manager = &fixture.state.workflow;
    let principal = manager.authorize(&json!({"authToken": PRINCIPAL})).unwrap();
    let backend = Arc::new(MustNotDispatch::default());
    let (first, duplicate) = tokio::join!(
        manager.create(spec(), &principal, backend.clone()),
        manager.create(spec(), &principal, backend.clone())
    );
    for attempt in [first, duplicate] {
        let Err(failure) = attempt else {
            panic!("An unavailable private journal must not create a workflow run");
        };
        assert_eq!(failure["code"], "persistence_unavailable");
    }
    let registry = manager.registry.lock().await;
    assert!(registry.runs.is_empty());
    assert!(registry.by_key.is_empty());
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    assert_eq!(manager.memory.used_bytes(), 0);
}

#[tokio::test]
async fn public_workflow_endpoints_cannot_resume_replay_or_cancel_through_missing_storage() {
    let fixture = Fixture::new();
    let bridge = Bridge::start().unwrap();
    let cases = [
        (
            "workflow_run",
            json!({"authToken":PRINCIPAL,"spec":spec(),"waitMs":0}),
        ),
        (
            "workflow_get",
            json!({"authToken":PRINCIPAL,"runId":"unavailable-run"}),
        ),
        (
            "workflow_cancel",
            json!({"authToken":PRINCIPAL,"runId":"unavailable-run"}),
        ),
        (
            "workflow_resume",
            json!({"authToken":PRINCIPAL,"runId":"unavailable-run","expectedRevision":1,"checkpointId":"checkpoint","intent":"continue"}),
        ),
        (
            "workflow_resume",
            json!({"authToken":PRINCIPAL,"runId":"unavailable-run","expectedRevision":1,"checkpointId":"checkpoint","intent":"reconcile"}),
        ),
    ];
    for (operation, args) in cases {
        let failure = call(operation, &args, &bridge, None, &fixture.state)
            .await
            .unwrap_err();
        assert_eq!(failure["code"], "persistence_unavailable", "{operation}");
        assert!(!failure.to_string().contains(PRINCIPAL));
    }
    assert!(fixture.state.workflow.registry.lock().await.runs.is_empty());
    assert_eq!(bridge.runtime_diagnostics()["installAttempts"], 0);
    assert_eq!(bridge.runtime_diagnostics()["commandBytesSent"], 0);
}

#[tokio::test]
async fn unsupported_private_journal_capability_is_explicit_and_authentication_remains_required() {
    let fixture = Fixture::new();
    let bridge = Bridge::start().unwrap();
    let capabilities = call(
        "workflow_capabilities",
        &json!({}),
        &bridge,
        None,
        &fixture.state,
    )
    .await
    .unwrap();
    assert_eq!(capabilities["journal"]["privateStorage"], false);
    assert_eq!(capabilities["recovery"]["restartHistory"], false);
    assert_eq!(capabilities["recovery"]["automaticRestartReplay"], false);
    assert_eq!(capabilities["recovery"]["unknownWriteReplay"], false);
    let failure = call(
        "workflow_run",
        &json!({"spec":spec()}),
        &bridge,
        None,
        &fixture.state,
    )
    .await
    .unwrap_err();
    assert_eq!(failure["code"], "unauthorized");
}

#[tokio::test]
async fn inspection_authority_and_memory_leases_remain_independent_without_releasing_quarantine() {
    let fixture = Fixture::new();
    let manager = &fixture.state.workflow;
    let failure = manager.recover_before_write().await.unwrap_err();
    assert_eq!(failure["code"], "persistence_unavailable");
    let generation = manager
        .inspection_authorize(&json!({"authToken":PRINCIPAL}))
        .unwrap();
    assert!(manager.inspection_authorized(generation));

    // This is the application's same in-memory arbiter used by picker and rich
    // screenshot services. It is not a mock native picker/screenshot claim.
    let picker_scope = vec!["ui/window/main".to_owned()];
    let screenshot_scope =
        super::super::resources::tool_resources("webview_screenshot", &json!({"windowId":"main"}));
    let picker_lease = manager.resources.try_acquire(picker_scope.clone()).unwrap();
    assert!(
        manager
            .resources
            .try_acquire(screenshot_scope.clone())
            .is_err()
    );
    drop(picker_lease);
    drop(
        manager
            .resources
            .try_acquire(screenshot_scope.clone())
            .unwrap(),
    );
    manager
        .resources
        .restore_quarantine(vec!["ui/window/main".into()]);
    for scope in [picker_scope, screenshot_scope] {
        let Err(failure) = manager.resources.try_acquire(scope) else {
            panic!("Inspection must not cross retained workflow quarantine");
        };
        assert_eq!(failure["code"], "resource_busy");
        assert_eq!(failure["quarantined"], true);
    }
    assert!(manager.resources.try_acquire(Vec::new()).is_ok());
    assert_eq!(manager.resources.quarantined(), 1);
    assert!(manager.inspection_authorized(generation));
}
