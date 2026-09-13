use super::*;

#[test]
fn up_pk030_slow_read_on_same_connection_does_not_cancel_or_refresh_picker() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service
        .reserve(request("slow-status", "main"), 1, &arbiter)
        .unwrap();
    let mut e = lock(&record.inner);
    e.status = "awaiting_selection".into();
    e.context = json!({"pageEpoch":"page","runtimeId":"runtime"});
    let before = e.report(false);
    let deadline = e.deadline;
    let failure = json!({"transportFailure":true});
    let same = json!({"pageEpoch":"page","runtime":{"ready":true,"context":e.context}});
    assert!(e.retain_after_slow_status("status", &failure, true, Ok(&same)));
    for reason in ["WS bridge timeout", "Script execution timeout (eval path)"] {
        let timeout = crate::bridge::BridgeError::DispatchedOutcomeUnknown {
            request_id: "probe".into(),
            reason: reason.into(),
        };
        assert!(e.retain_after_slow_status("status", &failure, true, Err(&timeout)));
    }
    for reason in [
        "Target document is navigating",
        "Current origin is outside host configuration",
        "Target window is unavailable",
        "Host probe lookup deadline expired",
    ] {
        let concrete = crate::bridge::BridgeError::NotDispatched {
            request_id: "probe".into(),
            reason: reason.into(),
        };
        assert!(!e.retain_after_slow_status("status", &failure, true, Err(&concrete)));
    }
    let script_error = crate::bridge::BridgeError::ExecutionFailed {
        request_id: "probe".into(),
        error: "page error".into(),
    };
    assert!(!e.retain_after_slow_status("status", &failure, true, Err(&script_error)));
    let channel_error = crate::bridge::BridgeError::DispatchedOutcomeUnknown {
        request_id: "probe".into(),
        reason: "Bridge response channel closed".into(),
    };
    assert!(!e.retain_after_slow_status("status", &failure, true, Err(&channel_error)));
    let explicit_status_error = page_transport_failure("status", script_error);
    assert!(!e.retain_after_slow_status("status", &explicit_status_error, true, Ok(&same)));
    assert_eq!(e.report(false), before);
    assert_eq!(e.deadline, deadline);
    assert!(e.lease.is_some());
    assert!(!e.retain_after_slow_status("start", &failure, true, Ok(&same)));
    assert!(!e.retain_after_slow_status("status", &failure, false, Ok(&same)));
    assert!(!e.retain_after_slow_status(
        "status",
        &json!({"code":"picker_not_found"}),
        true,
        Ok(&same)
    ));
    assert!(!e.retain_after_slow_status("status", &failure, true, Ok(&json!({"pageEpoch":"new"}))));
    e.terminal("cancelled");
    assert!(!e.retain_after_slow_status("status", &failure, true, Ok(&same)));
}

#[test]
fn up_pk013_only_a_never_dispatched_start_proves_no_guard_needs_cleanup() {
    use crate::bridge::BridgeError;
    let not_dispatched = || BridgeError::NotDispatched {
        request_id: "test".into(),
        reason: "cancelled before queue".into(),
    };
    assert_eq!(
        page_transport_failure("start", not_dispatched())["cleanupConfirmed"],
        true
    );
    assert!(page_transport_failure("cancel", not_dispatched())["cleanupConfirmed"].is_null());
    for failure in [
        BridgeError::ExecutionFailed {
            request_id: "test".into(),
            error: "not_dispatched is just business text".into(),
        },
        BridgeError::DispatchedOutcomeUnknown {
            request_id: "test".into(),
            reason: "lost reply".into(),
        },
    ] {
        assert!(page_transport_failure("start", failure)["cleanupConfirmed"].is_null());
    }
}

#[test]
fn up_pk029_transient_cleanup_failure_retains_lease_until_original_page_confirms() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service
        .reserve(request("cleanup-retry", "main"), 1, &arbiter)
        .unwrap();
    let mut entry = lock(&record.inner);
    entry.terminal("cancelled");
    let deadline = entry.deadline;
    let cleanup_deadline = entry.cleanup_deadline;
    entry.begin_target_cleanup();
    assert_eq!(entry.status, "cancelled");
    assert_eq!(entry.cleanup, "pending");
    assert!(entry.lease.is_some());
    assert_eq!(entry.deadline, deadline);
    assert_eq!(entry.cleanup_deadline, cleanup_deadline);
    let report = json!({"pickerId":entry.id,"nonce":entry.nonce,"context":entry.context,"sequence":1,"status":"cancelled","cleanup":{"status":"confirmed"}});
    assert!(entry.accept(&report));
    assert_eq!(entry.status, "cancelled");
    assert_eq!(entry.cleanup, "confirmed");
    assert!(entry.lease.is_none());
}

#[test]
fn up_pk029_cleanup_budget_starts_at_first_terminal_and_never_refreshes() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service
        .reserve(request("cleanup-deadline", "main"), 1, &arbiter)
        .unwrap();
    let mut entry = lock(&record.inner);
    entry.terminal("cancelled");
    assert!(entry.cleanup_budget(CLEANUP_MS) > 500);
    entry.cleanup_deadline = Some(Instant::now() - Duration::from_millis(1));
    let deadline = entry.cleanup_deadline;
    entry.begin_target_cleanup();
    assert_eq!(entry.cleanup_deadline, deadline);
    assert_eq!(entry.cleanup_budget(500), 0);
    assert_eq!(entry.cleanup_budget(300), 0);
}

#[test]
fn up_pk031_late_bridge_failure_cannot_downgrade_confirmed_page_destruction() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service
        .reserve(request("navigation-race", "main"), 1, &arbiter)
        .unwrap();
    service.page_changed("main");
    let mut entry = lock(&record.inner);
    assert_eq!(entry.cleanup, "context_destroyed");
    let revision = entry.revision;
    let sequence = entry.sequence;
    entry.page_unavailable(false);
    assert_eq!(entry.cleanup, "context_destroyed");
    assert_eq!(entry.status, "target_changed");
    assert!(entry.lease.is_none());
    assert_eq!(entry.revision, revision);
    assert_eq!(entry.sequence, sequence);
    assert!(
        !entry
            .warnings
            .iter()
            .any(|warning| warning == "cleanup_unconfirmed")
    );
}

#[test]
fn up_pk031_late_page_report_cannot_downgrade_confirmed_page_destruction() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service
        .reserve(request("navigation-report-race", "main"), 1, &arbiter)
        .unwrap();
    service.page_changed("main");
    let mut entry = lock(&record.inner);
    let revision = entry.revision;
    let sequence = entry.sequence;
    let report = json!({"pickerId":entry.id,"nonce":entry.nonce,"context":entry.context,"sequence":1,"status":"cancelled","cleanup":{"status":"unconfirmed"}});
    assert!(!entry.accept(&report));
    assert_eq!(entry.cleanup, "context_destroyed");
    assert!(entry.lease.is_none());
    assert_eq!(entry.revision, revision);
    assert_eq!(entry.sequence, sequence);
}

fn request(key: &str, window: &str) -> PickerRequest {
    PickerRequest::parse(
        &json!({"action":"start","requestKey":key,"windowId":window,"captureScreenshot":false}),
    )
    .unwrap()
}

#[test]
fn up_pk007_dedup_keeps_handle_deadline_and_lease() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (first, new) = service
        .reserve(request("same", "main"), 1, &arbiter)
        .unwrap();
    assert!(new);
    let deadline = lock(&first.inner).deadline;
    let (second, new) = service
        .reserve(request("same", "main"), 1, &arbiter)
        .unwrap();
    assert!(!new);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(lock(&second.inner).deadline, deadline);
    assert!(
        service
            .reserve(request("same", "other"), 1, &arbiter)
            .is_err()
    );
}

#[test]
fn up_pk009_quota_and_quarantine_do_not_displace_active_pickers() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    for n in 0..4 {
        service
            .reserve(request(&n.to_string(), &n.to_string()), 1, &arbiter)
            .unwrap();
    }
    assert!(
        service
            .reserve(request("fifth", "fifth"), 1, &arbiter)
            .is_err()
    );
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    arbiter.restore_quarantine(vec!["ui".into()]);
    assert!(
        service
            .reserve(request("blocked", "main"), 1, &arbiter)
            .is_err()
    );
    assert_eq!(arbiter.quarantined(), 1);
}

#[test]
fn up_pk032_first_terminal_wins_revision_monotonic() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service.reserve(request("r", "main"), 1, &arbiter).unwrap();
    let mut entry = lock(&record.inner);
    assert!(entry.terminal("selected"));
    let revision = entry.revision;
    assert!(!entry.terminal("cancelled"));
    assert_eq!(entry.status, "selected");
    assert_eq!(entry.revision, revision);
}

#[test]
fn up_pk011_stale_and_duplicate_page_reports_are_rejected() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let (record, _) = service.reserve(request("r", "main"), 1, &arbiter).unwrap();
    let mut entry = lock(&record.inner);
    entry.context = json!({"pageEpoch":"one"});
    let report = json!({"pickerId":entry.id,"nonce":entry.nonce,"sequence":1,"context":{"pageEpoch":"old"},"status":"selected","selection":{}});
    assert!(!entry.accept(&report));
    assert!(entry.selection.is_none());
}

#[test]
fn up_pk034_retained_results_not_evicted_to_make_room() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    for n in 0..MAX_RETAINED {
        let (record, _) = service
            .reserve(request(&n.to_string(), "main"), 1, &arbiter)
            .unwrap();
        let mut entry = lock(&record.inner);
        entry.terminal("cancelled");
        entry.cleanup = "confirmed".into();
        entry.lease.take();
        entry.complete = true;
    }
    assert_eq!(lock(&service.registry).records.len(), MAX_RETAINED);
    assert!(
        service
            .reserve(request("overflow", "main"), 1, &arbiter)
            .is_err()
    );
}

#[test]
fn up_pk033_late_capture_never_registers_after_monotonic_deadline() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let mut req = request("late", "main");
    req.capture_screenshot = true;
    let (record, _) = service.reserve(req, 1, &arbiter).unwrap();
    {
        let mut e = lock(&record.inner);
        e.terminal("selected");
        e.deadline = Instant::now() - Duration::from_millis(1);
    }
    assert!(!service.save_image(
        &record,
        vec![1, 2, 3],
        json!({"redaction":{"status":"applied"}})
    ));
    assert!(lock(&record.inner).image.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn up_pk035_persisted_unknown_write_is_recovered_before_picker_reservation() {
    use std::os::unix::fs::PermissionsExt;

    let directory = std::env::temp_dir().join(format!("picker-history-{}", uuid::Uuid::new_v4()));
    let workflow_directory = directory.join("workflow");
    std::fs::create_dir_all(&workflow_directory).unwrap();
    std::fs::set_permissions(&workflow_directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    // A genuine on-disk journal record consumed by the production parser. The
    // prior application stopped after dispatch intent, so its effect is unknown.
    let history = json!({
        "runId":"previous-app-run", "revision":1, "eventSeq":1,
        "event":"step_dispatch_intent", "appInstanceId":"previous-app-instance",
        "specHash":"a".repeat(64), "runKeyHash":"b".repeat(64), "principalHash":"c".repeat(64),
        "report":{"status":"running","mayHaveEffects":true,"completedSteps":0,
            "blockedOutcome":{"execution":"outcome_unknown","effect":"possible"},
            "resourceIsolation":"quarantined","originalTestVerdict":"failed"}
    });
    let history_bytes = format!("{history}\n").into_bytes();
    let events = workflow_directory.join("events.jsonl");
    let key = workflow_directory.join("fingerprint.key");
    std::fs::write(&events, &history_bytes).unwrap();
    std::fs::write(&key, [42u8; 32]).unwrap();
    for path in [&events, &key] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let state = PluginState::new(directory.clone()).unwrap();
    state
        .workflow
        .set_token(Some("fixture-picker-recovery-token-with-32-bytes".into()));
    assert_eq!(
        state.workflow.resources.quarantined(),
        0,
        "history is initially lazy"
    );
    state.workflow.recover_before_inspection().await.unwrap();
    assert_eq!(state.workflow.resources.quarantined(), 1);
    let result = state.picker.reserve(
        request("after-restart", "main"),
        1,
        &state.workflow.resources,
    );
    assert!(
        matches!(result, Err(ref error) if error["code"] == "resource_busy" && error["quarantined"] == true)
    );
    assert!(lock(&state.picker.registry).records.is_empty());
    assert!(lock(&state.picker.registry).keys.is_empty());
    assert_eq!(state.workflow.resources.quarantined(), 1);
    // Repeated preparation must not duplicate quarantine or rewrite prior facts.
    state.workflow.recover_before_inspection().await.unwrap();
    assert_eq!(state.workflow.resources.quarantined(), 1);
    assert_eq!(std::fs::read(&events).unwrap(), history_bytes);
    assert_eq!(std::fs::read(&key).unwrap(), vec![42u8; 32]);
    drop(state);
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn up_pk035_unreadable_prior_history_fails_before_picker_admission() {
    let directory =
        std::env::temp_dir().join(format!("picker-history-invalid-{}", uuid::Uuid::new_v4()));
    let state = PluginState::new(directory.clone()).unwrap();
    state
        .workflow
        .set_token(Some("fixture-picker-recovery-token-with-32-bytes".into()));
    state.workflow.disable_storage();
    let failure = state
        .workflow
        .recover_before_inspection()
        .await
        .unwrap_err();
    assert_eq!(failure["code"], "persistence_unavailable");
    assert!(lock(&state.picker.registry).records.is_empty());
    assert!(lock(&state.picker.registry).keys.is_empty());
    drop(state);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn up_pk013_revocation_discards_capture_and_cannot_accept_late_selection() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let mut req = request("revoke", "main");
    req.capture_screenshot = true;
    let (record, _) = service.reserve(req, 1, &arbiter).unwrap();
    let mut e = lock(&record.inner);
    e.revoke();
    assert_eq!(e.status, "cancelled");
    assert_eq!(e.screenshot["status"], "cancelled");
    let report = json!({"pickerId":e.id,"nonce":e.nonce,"sequence":1,"context":e.context,"status":"selected","selection":{"locatorCandidates":[],"redaction":{"status":"applied"}},"cleanup":{"status":"confirmed"}});
    e.accept(&report);
    assert_eq!(e.status, "cancelled");
    assert!(e.selection.is_none());
    assert!(e.lease.is_none());
}

#[test]
fn up_pk031_navigation_cancels_image_after_input_lease_released() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let mut req = request("nav-after-cleanup", "main");
    req.capture_screenshot = true;
    let (record, _) = service.reserve(req, 1, &arbiter).unwrap();
    {
        let mut e = lock(&record.inner);
        e.terminal("selected");
        e.selection = Some(json!({"selectionId":"historic"}));
        e.cleanup = "confirmed".into();
        e.lease.take();
    }
    service.page_changed("main");
    let e = lock(&record.inner);
    assert_eq!(e.status, "selected");
    assert_eq!(e.selection.as_ref().unwrap()["selectionId"], "historic");
    assert!(e.cancel_capture);
    assert_eq!(e.screenshot["status"], "failed");
    assert_eq!(e.cleanup, "context_destroyed");
}

#[tokio::test]
async fn up_pk033_replaced_connection_rejects_late_image_as_context_change() {
    let service = PickerService::default();
    let arbiter = crate::workflow::resources::ResourceArbiter::default();
    let window = format!("late-picker-{}", uuid::Uuid::new_v4());
    let mut req = request("late-pin", &window);
    req.capture_screenshot = true;
    let (record, _) = service.reserve(req, 1, &arbiter).unwrap();
    let bridge = crate::bridge::Bridge::start().unwrap();
    let pin = bridge.pin_connection(&window).await;
    {
        let mut e = lock(&record.inner);
        e.terminal("selected");
        e.connection_pin = Some(pin);
    }
    crate::identity::page_navigation(&window);
    assert!(!service.save_image(&record, vec![1, 2, 3], json!({})));
    let e = lock(&record.inner);
    assert_eq!(e.status, "selected");
    assert_eq!(e.screenshot["error"]["code"], "capture_context_changed");
    assert!(e.image.is_none());
    crate::identity::window_destroyed(&window);
}
