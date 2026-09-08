//! Service contract tests use isolated storage and a deterministic backend.
//! These validate orchestration, never native WebView behavior.
use super::*;
use std::sync::atomic::AtomicUsize;

const PRINCIPAL: &str = "fixture-service-token-32-bytes-long-12345";

#[derive(Default)]
struct MockBackend {
    dispatches: AtomicUsize,
    effects: AtomicUsize,
    conditions: AtomicUsize,
    cleanups: AtomicUsize,
    prepares: AtomicUsize,
    ready: AtomicBool,
    satisfied: AtomicBool,
    lose_response: AtomicBool,
    reject_prepare: AtomicBool,
    reject_prepare_number: AtomicUsize,
    stall_condition: AtomicBool,
    echo_value: AtomicBool,
    hold_execute: AtomicBool,
    execute_started: Notify,
    execute_release: Notify,
    omit_dispatch_metadata: AtomicBool,
    result_size: AtomicUsize,
    requests: Mutex<Vec<Value>>,
}
impl MockBackend {
    fn successful() -> Arc<Self> {
        Arc::new(Self {
            satisfied: AtomicBool::new(true),
            ..Self::default()
        })
    }
}
impl Backend for MockBackend {
    fn call<'a>(
        &'a self,
        _window: &'a str,
        request: Value,
        _timeout_ms: u64,
        _request_id: String,
    ) -> BackendFuture<'a> {
        Box::pin(async move {
            self.requests.lock().await.push(request.clone());
            match request["cmd"].as_str().unwrap() {
                "prepare" => {
                    let prepare_number = self.prepares.fetch_add(1, Ordering::SeqCst) + 1;
                    if self.reject_prepare.swap(false, Ordering::SeqCst)
                        || self.reject_prepare_number.load(Ordering::SeqCst) == prepare_number
                    {
                        return Ok(
                            json!({"ok":false,"error":{"code":"not_actionable","stage":"preparing","message":"Fixture obstruction"}}),
                        );
                    }
                    self.ready.store(true, Ordering::SeqCst);
                    let mut context = request["context"].clone();
                    context["pageEpoch"] = json!(1);
                    Ok(json!({"ok":true,"ready":true,"context":context}))
                }
                "execute" => {
                    assert!(
                        self.ready.load(Ordering::SeqCst),
                        "An action dispatched without observation ready"
                    );
                    self.dispatches.fetch_add(1, Ordering::SeqCst);
                    let writing = matches!(
                        request["step"]["op"].as_str(),
                        Some("click" | "fill" | "type" | "press")
                    );
                    if writing {
                        self.effects.fetch_add(1, Ordering::SeqCst);
                    }
                    self.execute_started.notify_one();
                    if self.hold_execute.load(Ordering::SeqCst) {
                        self.execute_release.notified().await;
                    }
                    if self.lose_response.swap(false, Ordering::SeqCst) {
                        return Err(WorkflowError::new(
                            "outcome_unknown",
                            "executing",
                            "Fixture dropped result after dispatch",
                        ));
                    }
                    let value = if self.result_size.load(Ordering::SeqCst) > 0 {
                        json!("x".repeat(self.result_size.load(Ordering::SeqCst)))
                    } else if self.echo_value.load(Ordering::SeqCst) {
                        request["step"]["value"]["literal"].clone()
                    } else {
                        json!("fixture-entity-001")
                    };
                    let mut result = json!({"ok":true,"dispatched":writing,"data":{"value":value},"effectScope":if writing{"ui_event_dispatch"}else{"none"}});
                    if self.omit_dispatch_metadata.load(Ordering::SeqCst) {
                        result.as_object_mut().unwrap().remove("dispatched");
                    }
                    Ok(result)
                }
                "condition" => {
                    self.conditions.fetch_add(1, Ordering::SeqCst);
                    if self.stall_condition.load(Ordering::SeqCst) {
                        std::future::pending::<()>().await;
                    }
                    Ok(
                        json!({"ok":true,"satisfied":self.satisfied.load(Ordering::SeqCst),"correlation":"state_only"}),
                    )
                }
                "cleanup" => {
                    self.cleanups.fetch_add(1, Ordering::SeqCst);
                    self.ready.store(false, Ordering::SeqCst);
                    Ok(json!({"ok":true}))
                }
                "evidence" => Ok(json!({"ok":true,"data":{"local":"fixture"}})),
                "tool" => Ok(json!({"ok":true,"dispatched":false,"data":{"status":"fixture"}})),
                other => panic!("Unexpected mock command {other}"),
            }
        })
    }
}

struct Fixture {
    directory: PathBuf,
    state: PluginState,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("connector-service-{}", uuid::Uuid::new_v4()));
        let state = PluginState::new(directory.join("logs")).unwrap();
        state.workflow.set_token(Some(PRINCIPAL.into()));
        Self { directory, state }
    }
    fn manager(&self) -> Arc<WorkflowService> {
        self.state.workflow.clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn click_spec(key: &str) -> WorkflowSpec {
    WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":key,"deadlineMs":5000,"steps":[{"id":"write","op":"click","target":{"by":"css","value":"button"}}]})).unwrap()
}
fn wait_spec(key: &str, deadline: u64) -> WorkflowSpec {
    WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":key,"deadlineMs":deadline,"steps":[{"id":"wait","op":"wait","condition":{"kind":"element","target":{"by":"css","value":".ready"},"state":"visible"}},{"id":"write","op":"click","target":{"by":"css","value":"button"}}]})).unwrap()
}
async fn settled(run: &Arc<Mutex<Run>>) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            {
                let entry = run.lock().await;
                if !entry.executing {
                    return entry.report.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Service failed to stop within the bounded test budget")
}
async fn saw_conditions(backend: &MockBackend) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while backend.conditions.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Condition was never observed");
}

#[tokio::test]
async fn concurrent_duplicate_submissions_dispatch_only_once() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        let (a, b) = tokio::join!(
            manager.create(click_spec("same"), PRINCIPAL, backend.clone()),
            manager.create(click_spec("same"), PRINCIPAL, backend.clone())
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let report = settled(&a).await;
        assert_eq!(report["status"], "completed");
        assert_eq!(report["goalStatus"], "not_requested");
        assert_eq!(backend.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn duplicate_key_with_changed_inputs_cannot_create_new_effects() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        let run = manager
            .create(click_spec("conflict"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        settled(&run).await;
        let mut changed = click_spec("conflict");
        changed.inputs.insert("new".into(), json!("value"));
        let error = match manager.create(changed, PRINCIPAL, backend.clone()).await {
            Err(error) => error,
            Ok(_) => panic!("Changed runKey reused"),
        };
        assert_eq!(error["code"], "run_key_conflict");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn deadline_failure_never_dispatches_later_write() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = Arc::new(MockBackend::default());
        let run = manager
            .create(wait_spec("deadline", 80), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["reason"], "condition_timeout");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
        assert!(backend.cleanups.load(Ordering::SeqCst) > 0);
        assert_eq!(report["blockedOutcome"]["effect"], "none");
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn stalled_condition_is_bounded_by_the_run_deadline() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = Arc::new(MockBackend {
            stall_condition: AtomicBool::new(true),
            ..MockBackend::default()
        });
        let run = manager
            .create(wait_spec("stalled", 80), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["reason"], "condition_timeout");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn cancelling_wait_cleans_observation_and_blocks_later_dispatch() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = Arc::new(MockBackend::default());
        let run = manager
            .create(wait_spec("cancel", 5000), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        saw_conditions(&backend).await;
        let run_id = run.lock().await.report["runId"].clone();
        let bridge = Bridge::start().unwrap();
        call(
            "workflow_cancel",
            &json!({"authToken":PRINCIPAL,"runId":run_id}),
            &bridge,
            None,
            &fixture.state,
        )
        .await
        .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["status"], "cancelled");
        assert_eq!(report["cancellation"]["rollback"], false);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
        assert!(backend.cleanups.load(Ordering::SeqCst) > 0);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn unknown_write_is_not_replayed_by_resubmission_or_reconciliation() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.lose_response.store(true, Ordering::SeqCst);
        let mut spec = click_spec("unknown");
        spec.steps[0].expect = Some(
            serde_json::from_value(
                json!({"kind":"element","target":{"by":"css","value":".saved"},"state":"visible"}),
            )
            .unwrap(),
        );
        let run = manager
            .create(spec.clone(), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let before = settled(&run).await;
        assert_eq!(before["blockedOutcome"]["execution"], "outcome_unknown");
        assert_eq!(before["blockedOutcome"]["effect"], "possible");
        assert!(manager.resources.quarantined() > 0);
        let duplicate = manager
            .create(spec, PRINCIPAL, backend.clone())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&run, &duplicate));
        reconcile(&manager, &run, &*backend).await.unwrap();
        let after = run.lock().await.report.clone();
        assert_eq!(after["lateOutcome"]["verification"], "passed");
        assert_eq!(after["lateOutcome"]["originalActionReplayed"], false);
        assert_eq!(after["originalTestVerdict"], before["originalTestVerdict"]);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
        assert!(manager.resources.quarantined() > 0);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn resume_rejects_stale_checkpoint_and_unknown_dispatch() {
    tokio::time::timeout(Duration::from_secs(5),async {

    let fixture = Fixture::new();
    let manager = fixture.manager();
    let backend = MockBackend::successful();
    backend.lose_response.store(true, Ordering::SeqCst);
    let run = manager
        .create(click_spec("cas"), PRINCIPAL, backend.clone())
        .await
        .unwrap();
    let report = settled(&run).await;
    let bridge = Bridge::start().unwrap();
    let mut args = json!({"authToken":PRINCIPAL,"runId":report["runId"],"expectedRevision":0,"checkpointId":report["checkpointId"],"intent":"continue"});
    let error = call("workflow_resume", &args, &bridge, None, &fixture.state)
        .await
        .unwrap_err();
    assert_eq!(error["code"], "stale_checkpoint");
    args["expectedRevision"] = report["revision"].clone();
    let error = call("workflow_resume", &args, &bridge, None, &fixture.state)
        .await
        .unwrap_err();
    assert_eq!(error["code"], "resume_not_safe");
    assert_eq!(backend.effects.load(Ordering::SeqCst), 1);

    }).await.expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn full_example_resolves_entity_bindings_and_goal_locally() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/workflow/create-task.json");
        if !path.exists() {
            return;
        } // Published crates omit repository examples.
        let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let run = manager
            .create(
                WorkflowSpec::parse(&raw).unwrap(),
                PRINCIPAL,
                backend.clone(),
            )
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["status"], "completed");
        assert_eq!(report["goalStatus"], "passed");
        assert_eq!(report["summary"]["completedSteps"], 4);
        assert_eq!(backend.dispatches.load(Ordering::SeqCst), 4);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 3);
        let requests = backend.requests.lock().await;
        let goal = requests
            .iter()
            .find(|r| {
                r["cmd"] == "condition"
                    && r["condition"]["target"]["entity"]["attribute"] == "data-task-id"
            })
            .unwrap();
        assert_eq!(
            goal["condition"]["target"]["entity"]["value"]["literal"],
            "fixture-entity-001"
        );
        let fill = requests
            .iter()
            .find(|r| r["cmd"] == "execute" && r["step"]["id"] == "fillName")
            .unwrap();
        assert_eq!(fill["step"]["value"]["literal"], raw["inputs"]["taskName"]);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn journal_restart_preserves_dedup_and_never_replays_unknown_writes() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let path = fixture.directory.join("restart-history");
        let manager = Arc::new(WorkflowService::new(path.clone()));
        let backend = MockBackend::successful();
        backend.lose_response.store(true, Ordering::SeqCst);
        let run = manager
            .create(click_spec("restart"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let old = settled(&run).await;
        let old_id = old["runId"].clone();
        drop(run);
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&manager) > 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(manager);
        let restored = Arc::new(WorkflowService::new(path));
        let duplicate = restored
            .create(click_spec("restart"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let entry = duplicate.lock().await;
        assert_eq!(entry.report["runId"], old_id);
        assert_eq!(entry.report["status"], "interrupted");
        assert_eq!(entry.report["reason"], "app_instance_changed");
        assert!(entry.spec.is_none());
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
        assert!(restored.resources.quarantined() > 0);
    })
    .await
    .expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn invalid_run_wait_budget_rejects_before_registering_a_run() {
    tokio::time::timeout(Duration::from_secs(5),async {

    let fixture=Fixture::new();let bridge=Bridge::start().unwrap();
    let args=json!({"authToken":PRINCIPAL,"spec":serde_json::to_value(click_spec("bad-wait-budget")).unwrap(),"waitMs":30001});
    let error=call("workflow_run",&args,&bridge,None,&fixture.state).await.unwrap_err();
    assert_eq!(error["code"],"invalid_spec");
    assert!(fixture.state.workflow.registry.lock().await.runs.is_empty(),"Malformed run arguments created execution state before validation");

    }).await.expect("Service contract test exceeded five seconds");
}

#[tokio::test]
async fn literal_input_secrets_are_redacted_without_corrupting_status_fields() {
    tokio::time::timeout(Duration::from_secs(5),async{
        let fixture=Fixture::new();let manager=fixture.manager();let backend=MockBackend::successful();backend.echo_value.store(true,Ordering::SeqCst);
        let secret="fixture-private-password-fb29a";
        let spec=WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":"redaction","inputs":{"statusCollision":"completed"},"steps":[{"id":"fill","op":"fill","target":{"by":"css","value":"input"},"value":secret}]})).unwrap();
        let run=manager.create(spec,PRINCIPAL,backend).await.unwrap();let report=settled(&run).await;
        let bridge=Bridge::start().unwrap();
        let public=call("workflow_get",&json!({"authToken":PRINCIPAL,"runId":report["runId"]}),&bridge,None,&fixture.state).await.unwrap();
        assert_eq!(public["status"],"completed");
        assert_eq!(public["steps"][0]["outcome"]["execution"],"completed");
        assert!(!public.to_string().contains(secret));
        let journal=std::fs::read_to_string(manager.directory.join("events.jsonl")).unwrap();
        assert!(!journal.contains(secret));assert!(!journal.contains(PRINCIPAL));
    }).await.expect("Redaction contract exceeded test timeout");
}

#[tokio::test]
async fn journal_intent_failure_stops_before_any_business_dispatch() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        let options = super::super::journal::JournalOptions {
            max_records: 2,
            ..Default::default()
        };
        let (journal, _) = Journal::open_with_options(&manager.directory, options).unwrap();
        assert!(manager.journal.set(Ok(journal)).is_ok());
        let run = manager
            .create(click_spec("journal-failure"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["reason"], "persistence_unavailable");
        assert_eq!(report["blockedOutcome"]["execution"], "not_dispatched");
        assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
        assert!(report.get("persistenceWarning").is_some());
    })
    .await
    .expect("Journal contract exceeded test timeout");
}

fn continue_args(report: &Value) -> Value {
    json!({"intent":"continue","expectedRevision":report["revision"],"checkpointId":report["checkpointId"]})
}

#[tokio::test]
async fn concurrent_continue_uses_one_checkpoint_and_dispatches_once() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.reject_prepare.store(true, Ordering::SeqCst);
        let run = manager
            .create(click_spec("continue-cas"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let paused = settled(&run).await;
        assert_eq!(paused["blockedOutcome"]["execution"], "not_dispatched");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
        let args = continue_args(&paused);
        let (a, b) = tokio::join!(
            resume_run(&manager, &run, &args, backend.clone()),
            resume_run(&manager, &run, &args, backend.clone())
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let loser = a.err().or_else(|| b.err()).unwrap();
        assert_eq!(loser["code"], "stale_checkpoint");
        let report = settled(&run).await;
        assert_eq!(report["status"], "completed");
        assert_eq!(report["recoveryOccurred"], true);
        assert_eq!(report["originalTestVerdict"], "failed");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Continue CAS exceeded test timeout");
}

fn two_step_spec(key: &str) -> WorkflowSpec {
    WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":key,"deadlineMs":800,"steps":[
        {"id":"first","op":"click","target":{"by":"css","value":".first"},"expect":{"kind":"element","target":{"by":"css","value":".first-result"},"state":"visible"}},
        {"id":"second","op":"click","target":{"by":"css","value":".second"}}
    ]})).unwrap()
}

#[tokio::test]
async fn continuing_partial_run_revalidates_without_replaying_completed_step() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.reject_prepare_number.store(2, Ordering::SeqCst);
        let run = manager
            .create(
                two_step_spec("partial-continue"),
                PRINCIPAL,
                backend.clone(),
            )
            .await
            .unwrap();
        let paused = settled(&run).await;
        assert_eq!(paused["summary"]["completedSteps"], 1);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
        let conditions_before = backend.conditions.load(Ordering::SeqCst);
        resume_run(&manager, &run, &continue_args(&paused), backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["status"], "completed");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 2);
        assert!(
            backend.conditions.load(Ordering::SeqCst) > conditions_before,
            "Prior required state was not revalidated before continuation"
        );
        let requests = backend.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["cmd"] == "execute" && r["step"]["id"] == "first")
                .count(),
            1
        );
    })
    .await
    .expect("Partial continue exceeded test timeout");
}

#[tokio::test]
async fn changed_prior_expectation_blocks_resumed_write() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.reject_prepare_number.store(2, Ordering::SeqCst);
        let run = manager
            .create(two_step_spec("changed-prior"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let paused = settled(&run).await;
        assert_eq!(paused["summary"]["completedSteps"], 1);
        backend.satisfied.store(false, Ordering::SeqCst);
        resume_run(&manager, &run, &continue_args(&paused), backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_ne!(report["status"], "completed");
        assert_eq!(report["reason"], "precondition_failed");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Changed precondition exceeded test timeout");
}

#[tokio::test]
async fn verification_failure_keeps_effect_and_blocks_successor() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = Arc::new(MockBackend::default());
        let mut spec = two_step_spec("postcondition");
        spec.deadline_ms = 100;
        let run = manager
            .create(spec, PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["blockedOutcome"]["execution"], "completed");
        assert_eq!(report["blockedOutcome"]["verification"], "failed");
        assert_eq!(report["blockedOutcome"]["effect"], "confirmed");
        assert_eq!(report["reason"], "postcondition_failed");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
        assert_eq!(backend.dispatches.load(Ordering::SeqCst), 1);
        assert!(manager.resources.quarantined() > 0);
    })
    .await
    .expect("Verification failure exceeded test timeout");
}

#[tokio::test]
async fn cancel_during_dispatched_write_waits_for_outcome_without_replaying() {
    tokio::time::timeout(Duration::from_secs(5),async{
        let fixture=Fixture::new();let manager=fixture.manager();let backend=MockBackend::successful();backend.hold_execute.store(true,Ordering::SeqCst);
        let spec=WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":"cancel-dispatched","deadlineMs":2000,"steps":[{"id":"write","op":"click","target":{"by":"css","value":"button"}},{"id":"later","op":"press","key":"Enter"}]})).unwrap();
        let run=manager.create(spec,PRINCIPAL,backend.clone()).await.unwrap();backend.execute_started.notified().await;
        let id=run.lock().await.report["runId"].clone();let bridge=Bridge::start().unwrap();
        let cancelling=call("workflow_cancel",&json!({"authToken":PRINCIPAL,"runId":id}),&bridge,None,&fixture.state).await.unwrap();
        assert_eq!(cancelling["status"],"cancelling");assert_eq!(cancelling["cancellation"]["inFlight"],true);
        backend.execute_release.notify_one();
        let report=settled(&run).await;assert_eq!(report["status"],"cancelled");assert_eq!(report["cancellation"]["rollback"],false);
        assert_eq!(backend.effects.load(Ordering::SeqCst),1);assert_eq!(backend.dispatches.load(Ordering::SeqCst),1);
    }).await.expect("Cancel dispatched write exceeded test timeout");
}

#[tokio::test]
async fn unknown_write_quarantines_conflicting_new_run() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.lose_response.store(true, Ordering::SeqCst);
        let unknown = manager
            .create(click_spec("unknown-lock"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        settled(&unknown).await;
        let conflict = manager
            .create(click_spec("conflicting-run"), PRINCIPAL, backend.clone())
            .await
            .unwrap();
        let report = settled(&conflict).await;
        assert_eq!(report["reason"], "resource_busy");
        assert_eq!(report["blockedOutcome"]["execution"], "not_dispatched");
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Unknown quarantine exceeded test timeout");
}

#[tokio::test]
async fn success_without_dispatch_metadata_is_unknown_and_never_replayed() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let backend = MockBackend::successful();
        backend.omit_dispatch_metadata.store(true, Ordering::SeqCst);
        let run = manager
            .create(
                click_spec("missing-dispatch-meta"),
                PRINCIPAL,
                backend.clone(),
            )
            .await
            .unwrap();
        let report = settled(&run).await;
        assert_eq!(report["blockedOutcome"]["execution"], "outcome_unknown");
        assert_eq!(report["blockedOutcome"]["effect"], "possible");
        assert!(manager.resources.quarantined() > 0);
        let error = resume_run(&manager, &run, &continue_args(&report), backend.clone())
            .await
            .unwrap_err();
        assert_eq!(error["code"], "resume_not_safe");
        assert_eq!(backend.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("Missing dispatch metadata exceeded test timeout");
}

#[tokio::test]
async fn exhausted_host_memory_rejects_run_before_registration_or_dispatch() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let fixture = Fixture::new();
        let mut service = WorkflowService::new(fixture.directory.join("tiny-budget"));
        service.memory = super::super::budget::MemoryBudget::new(1);
        let manager = Arc::new(service);
        let backend = MockBackend::successful();
        let error = match manager
            .create(click_spec("memory-full"), PRINCIPAL, backend.clone())
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("Run exceeded host retention memory budget"),
        };
        assert_eq!(error["code"], "resource_busy");
        assert!(manager.registry.lock().await.runs.is_empty());
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
        assert_eq!(manager.memory.used_bytes(), 0);
    })
    .await
    .expect("Memory admission exceeded test timeout");
}

#[tokio::test]
async fn retention_failure_preserves_completed_effect_and_blocks_later_dispatch() {
    tokio::time::timeout(Duration::from_secs(5),async{
        let fixture=Fixture::new();let mut service=WorkflowService::new(fixture.directory.join("result-budget"));
        service.memory=super::super::budget::MemoryBudget::new(256*1024);
        let manager=Arc::new(service);let backend=MockBackend::successful();backend.result_size.store(128*1024,Ordering::SeqCst);
        let spec=WorkflowSpec::parse(&json!({"schemaVersion":1,"runKey":"large-result","deadlineMs":2000,"steps":[{"id":"write","op":"click","target":{"by":"css","value":"button"}},{"id":"later","op":"press","key":"Enter"}]})).unwrap();
        let run=manager.create(spec,PRINCIPAL,backend.clone()).await.unwrap();let report=settled(&run).await;
        assert_eq!(report["blockedOutcome"]["execution"],"completed");assert_eq!(report["blockedOutcome"]["effect"],"confirmed");
        assert_eq!(report["blockedOutcome"]["error"]["code"],"capture_incomplete");assert_eq!(report["blockedOutcome"]["data"],Value::Null);
        assert_eq!(report["blockedOutcome"]["coverage"]["truncated"],true);
        assert_eq!(backend.effects.load(Ordering::SeqCst),1);assert_eq!(backend.dispatches.load(Ordering::SeqCst),1);
        assert!(manager.memory.used_bytes()<=manager.memory.limit_bytes());
    }).await.expect("Result retention exceeded test timeout");
}

#[tokio::test]
async fn evidence_paging_roundtrips_utf8_with_bounded_responses() {
    tokio::time::timeout(Duration::from_secs(5),async{
        let fixture=Fixture::new();let manager=fixture.manager();let backend=MockBackend::successful();
        let mut spec=click_spec("evidence-pages");spec.evidence.max_inline_bytes=1024;
        let run=manager.create(spec,PRINCIPAL,backend).await.unwrap();let report=settled(&run).await;
        let details="α中🦀".repeat(120);let evidence_id="unicode-evidence";
        {let mut entry=run.lock().await;entry.evidence.insert(evidence_id.into(),json!({"captureKind":"scoped_snapshot","details":details}));entry.report["evidenceRefs"].as_array_mut().unwrap().push(json!(evidence_id));}
        let bridge=Bridge::start().unwrap();let mut offset=0;let mut collected=String::new();let mut pages=0;
        loop {
            let page=call("workflow_get",&json!({"authToken":PRINCIPAL,"runId":report["runId"],"evidenceId":evidence_id,"offset":offset}),&bridge,None,&fixture.state).await.unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len()<=1024);assert_eq!(page["evidencePage"]["offset"],offset);
            let content=page["evidencePage"]["content"].as_str().unwrap();assert!(!content.is_empty());collected.push_str(content);pages+=1;
            match page["evidencePage"]["nextOffset"].as_u64(){Some(next)=>{assert!(next>offset);offset=next;},None=>{assert_eq!(page["evidencePage"]["totalBytes"],collected.len());break;}}
            assert!(pages<100,"Paging did not make bounded progress");
        }
        assert!(pages>1);
        let decoded:Value=serde_json::from_str(&collected).unwrap();assert_eq!(decoded["details"],details);
        let invalid_boundary=collected.find('🦀').unwrap()+1;
        for invalid in [invalid_boundary,collected.len()+1] {
            let error=call("workflow_get",&json!({"authToken":PRINCIPAL,"runId":report["runId"],"evidenceId":evidence_id,"offset":invalid}),&bridge,None,&fixture.state).await.unwrap_err();assert_eq!(error["code"],"invalid_spec");
        }
        let error=call("workflow_get",&json!({"authToken":PRINCIPAL,"runId":report["runId"],"offset":0}),&bridge,None,&fixture.state).await.unwrap_err();assert_eq!(error["code"],"invalid_spec");
    }).await.expect("Evidence paging exceeded test timeout");
}
