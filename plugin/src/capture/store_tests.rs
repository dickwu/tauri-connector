use super::*;
use serde_json::json;
fn fixture() -> (Store, String, String) {
    let mut store = Store::default();
    let context =
        json!({"windowId":"main","pageEpoch":"p","windowInstanceId":"w","appInstanceId":"a"});
    let source = store.source("main", context);
    let id = store
        .start(1, "main", CaptureOptions::default(), &source)
        .unwrap();
    (store, id, source)
}
fn packet(source: &str, sequence: u64, id: &str, phase: &str) -> Value {
    json!({"sourceId":source,"events":[{"sourceId":source,"sourceSequence":sequence,"invocationId":id,"phase":phase,"command":"business","wallTimeMs":1,"resultPreview":{"token":"secret"}}]})
}
#[test]
fn capture_sessions_do_not_clear_each_other_and_require_domain() {
    let (mut s, id, source) = fixture();
    let other = s
        .start(2, "main", CaptureOptions::default(), &source)
        .unwrap();
    s.ingest("main", packet(&source, 1, "a", "started"));
    s.stop(&id, 1).unwrap();
    assert!(s.status(&other, 1).is_err());
    assert_eq!(s.status(&other, 2).unwrap()["desired"], true);
    assert_eq!(
        s.query(&id, 1, &json!({})).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn capture_out_of_order_duplicate_and_interrupted_pending() {
    let (mut s, id, source) = fixture();
    s.ingest("main", packet(&source, 2, "a", "succeeded"));
    s.ingest("main", packet(&source, 1, "a", "started"));
    s.ingest("main", packet(&source, 2, "a", "succeeded"));
    s.ingest("main", packet(&source, 3, "b", "started"));
    let r = s.query(&id, 1, &json!({})).unwrap();
    assert_eq!(r["events"].as_array().unwrap().len(), 3);
    assert_eq!(r["pending"].as_array().unwrap().len(), 1);
    s.stop(&id, 1).unwrap();
    let r = s.query(&id, 1, &json!({})).unwrap();
    assert_eq!(
        r["pending"][0]["observationStatus"],
        "observation_interrupted"
    );
}
#[test]
fn capture_cursor_expired_and_namespace_bound() {
    let (mut s, id, source) = fixture();
    s.max_events = 2;
    s.ingest("main", packet(&source, 1, "a", "started"));
    let cursor = s.query(&id, 1, &json!({})).unwrap()["nextCursor"].clone();
    for i in 2..6 {
        s.ingest("main", packet(&source, i, &format!("id{i}"), "started"));
    }
    let r = s.query(&id, 1, &json!({"cursor":cursor})).unwrap();
    assert_eq!(r["status"], "cursor_expired");
    assert!(r["droppedEvents"].as_u64().unwrap() > 0);
    assert!(s.query(&id, 1, &json!({"cursor":"foreign:1"})).is_err());
}
#[test]
fn capture_preview_is_metadata_default_and_host_redacted_before_storage() {
    let (mut s, id, source) = fixture();
    s.ingest("main", packet(&source, 1, "a", "succeeded"));
    let raw = serde_json::to_string(&s.query(&id, 1, &json!({})).unwrap()).unwrap();
    assert!(!raw.contains("secret"));
    assert!(!raw.contains("\"token\""));
}
#[test]
fn capture_source_window_validation_and_resource_caps() {
    let (mut s, id, source) = fixture();
    s.ingest("other", packet(&source, 1, "a", "started"));
    assert_eq!(
        s.query(&id, 1, &json!({})).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    for _ in 0..7 {
        s.start(1, "main", CaptureOptions::default(), &source)
            .unwrap();
    }
    assert!(
        s.start(1, "main", CaptureOptions::default(), &source)
            .is_err()
    );
}

#[test]
fn capture_query_strict_utf8_byte_budget_and_preview_authority() {
    let (mut s, id, source) = fixture();
    s.preview_commands = (0..50)
        .map(|i| format!("{i}-{}", "x".repeat(240)))
        .collect();
    s.preview_commands.push("business".into());
    s.preview_paths = vec!["id".into()];
    let options = CaptureOptions {
        result_policy: "preview".into(),
        ..CaptureOptions::default()
    };
    let preview_id = s.start(1, "main", options, &source).unwrap();
    let mut input = packet(&source, 1, "a", "succeeded");
    input["events"][0]["resultPreview"] =
        json!({"id":"🙂".repeat(2000),"password":"private-sentinel"});
    s.ingest("main", input);
    for max_bytes in [1024, 2048, 32768] {
        let result = s
            .query(&preview_id, 1, &json!({"maxBytes":max_bytes}))
            .unwrap();
        assert!(
            serde_json::to_vec(&result).unwrap().len() <= max_bytes as usize,
            "maxBytes={max_bytes}"
        );
        assert!(!result.to_string().contains("private-sentinel"));
    }
    let metadata = s.query(&id, 1, &json!({})).unwrap();
    assert!(metadata["events"][0].get("resultPreview").is_none());
}

#[test]
fn capture_duplicate_after_eviction_cannot_resurrect_completion() {
    let (mut s, id, source) = fixture();
    s.max_events = 1;
    s.ingest("main", packet(&source, 1, "a", "succeeded"));
    s.ingest("main", packet(&source, 2, "b", "started"));
    let before = s.sequence;
    s.ingest("main", packet(&source, 1, "a", "succeeded"));
    assert_eq!(s.sequence, before);
    assert_eq!(
        s.query(&id, 1, &json!({})).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn capture_reload_interrupts_old_source_and_follows_only_opt_in() {
    let (mut s, id, source) = fixture();
    let follow = s
        .start(
            1,
            "main",
            CaptureOptions {
                follow_pages: true,
                ..CaptureOptions::default()
            },
            &source,
        )
        .unwrap();
    s.ingest("main", packet(&source, 1, "pending", "started"));
    s.interrupt_window("main");
    assert_eq!(s.status(&id, 1).unwrap()["desired"], false);
    assert_eq!(s.status(&follow, 1).unwrap()["desired"], true);
    let fresh = s.source("main", json!({"windowId":"main","pageEpoch":"p2"}));
    s.follow_source("main", &fresh);
    s.ingest("main", packet(&source, 2, "pending", "succeeded"));
    s.ingest("main", packet(&fresh, 1, "new", "started"));
    let result = s.query(&follow, 1, &json!({})).unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
    assert!(
        result["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["invocationId"] == "pending"
                && p["observationStatus"] == "observation_interrupted")
    );
}

#[test]
fn capture_stopped_session_does_not_observe_other_sessions_late_completion() {
    let (mut s, id, source) = fixture();
    let other = s
        .start(1, "main", CaptureOptions::default(), &source)
        .unwrap();
    s.ingest("main", packet(&source, 1, "slow", "started"));
    s.stop(&id, 1).unwrap();
    s.ingest("main", packet(&source, 2, "slow", "succeeded"));
    assert_eq!(
        s.query(&id, 1, &json!({})).unwrap()["pending"][0]["observationStatus"],
        "observation_interrupted"
    );
    assert!(
        s.query(&other, 1, &json!({})).unwrap()["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn capture_monotonic_deadline_survives_follow_and_expires_pending() {
    use std::time::{Duration, Instant};
    let base = Instant::now();
    let mut s = Store {
        session_lifetime: Duration::from_millis(100),
        ..Store::default()
    };
    let source = s.source("main", json!({"windowId":"main","pageEpoch":"old"}));
    let id = s
        .start_at(
            1,
            "main",
            CaptureOptions {
                follow_pages: true,
                ..CaptureOptions::default()
            },
            &source,
            base,
        )
        .unwrap();
    s.ingest("main", packet(&source, 1, "slow", "started"));
    let deadline = s.sessions[&id].deadline;
    s.interrupt_window("main");
    let fresh = s.source("main", json!({"windowId":"main","pageEpoch":"new"}));
    s.follow_source("main", &fresh);
    assert_eq!(s.sessions[&id].deadline, deadline);
    s.expire_at(base + Duration::from_millis(101));
    assert_eq!(s.status(&id, 1).unwrap()["status"], "expired");
    assert!(!s.accepts_source("main", &fresh));
    assert_eq!(
        s.query(&id, 1, &json!({})).unwrap()["pending"][0]["observationStatus"],
        "observation_interrupted"
    );
}

#[test]
fn capture_preparation_cannot_restart_expired_lifetime_and_retention_releases_data() {
    use std::time::{Duration, Instant};
    let mut s = Store {
        session_lifetime: Duration::from_millis(5),
        retention: Duration::from_millis(10),
        ..Store::default()
    };
    let source = s.source("main", json!({"windowId":"main","pageEpoch":"p"}));
    assert_eq!(
        s.start_at(
            1,
            "main",
            CaptureOptions::default(),
            &source,
            Instant::now() - Duration::from_millis(10)
        )
        .unwrap_err()["code"],
        "capture_expired"
    );
    let id = s
        .start(1, "main", CaptureOptions::default(), &source)
        .unwrap();
    s.ingest("main", packet(&source, 1, "slow", "started"));
    s.stop(&id, 1).unwrap();
    let stopped = s.sessions[&id].stopped_at.unwrap();
    s.expire_at(stopped + Duration::from_millis(11));
    assert!(s.sessions.is_empty());
    assert!(s.events.is_empty());
    assert!(s.invocations.is_empty());
    assert_eq!(s.bytes, 0);
}
