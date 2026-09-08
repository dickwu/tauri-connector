use connector_client::batch::{run_batch, run_from_value, BatchSpec};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn failed_verification_blocks_the_dependent_dispatch() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let spec=BatchSpec::parse(&json!({"actions":[{"id":"write","tool":"webview_act_and_verify"},{"tool":"write_again","dependsOn":["write"]}]})).unwrap();
    let report = run_batch(&spec, move |_, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(json!({"verdict":"failed","observed":{"value":"old"}})) }
    })
    .await;
    assert!(!report.ok);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.logs[0].outcome.as_ref().unwrap().data["observed"]["value"],
        "old"
    );
}
#[tokio::test]
async fn ordinary_query_error_text_is_business_data() {
    let spec = BatchSpec::parse(&json!({"actions":[{"tool":"webview_execute_js"}]})).unwrap();
    let report = run_batch(&spec, |_, _| async {
        Ok(json!({"error":"user text","found":false,"ok":false}))
    })
    .await;
    assert!(report.ok);
}
#[tokio::test]
async fn report_persistence_failure_preserves_execution() {
    let root = std::env::temp_dir().join(format!("connector-report-{}", uuid::Uuid::new_v4()));
    std::fs::write(&root, "occupied").unwrap();
    let path = root.join("report.json");
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let report = run_from_value(
        &json!({"save":path,"actions":[{"tool":"write"}]}),
        move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(json!({"value":1})) }
        },
    )
    .await
    .unwrap();
    assert!(report.ok);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(report.saved_to.is_none());
    assert!(report.persistence_warning.is_some());
    std::fs::remove_file(root).unwrap();
}
#[tokio::test]
async fn saved_report_matches_the_returned_report() {
    let path = std::env::temp_dir().join(format!("connector-report-{}.json", uuid::Uuid::new_v4()));
    let report = run_from_value(
        &json!({"save":path,"actions":[{"tool":"query"}]}),
        |_, _| async { Ok(Value::Null) },
    )
    .await
    .unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved, serde_json::to_value(report).unwrap());
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn batch_screenshots_default_to_saved_artifact_and_compact_inline_bytes() {
    let path = std::env::temp_dir().join(format!("connector-shot-{}.png", uuid::Uuid::new_v4()));
    let output_path = path.clone();
    let spec = BatchSpec::parse(&json!({"actions":[{"tool":"webview_screenshot"}]})).unwrap();
    let report=run_batch(&spec,move|_,args|{
        let path=output_path.clone();
        async move {
            assert_eq!(args["save"],true);
            std::fs::write(&path,b"isolated artifact fixture").unwrap();
            Ok(json!({"base64":"a".repeat(100000),"width":100,"height":100,"artifact":{"id":"shot-test","path":path,"bytes":25}}))
        }
    }).await;
    assert!(report.ok);
    assert!(serde_json::to_vec(&report).unwrap().len() < 2048);
    assert_eq!(
        report.logs[0].outcome.as_ref().unwrap().evidence_refs,
        vec!["shot-test"]
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"isolated artifact fixture");
    assert_eq!(
        report.logs[0].result.as_ref().unwrap()["artifact"]["id"],
        "shot-test"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn explicit_inline_screenshot_and_missing_artifact_keep_original_data() {
    use connector_client::batch::{compact_tool_outcome, prepare_tool_args};
    use connector_client::outcome::{EffectStatus, ExecutionOutcome};
    assert_eq!(
        prepare_tool_args("webview_screenshot", json!({"save":false}))["save"],
        false
    );
    let mut outcome =
        ExecutionOutcome::completed(json!({"base64":"essential-evidence"}), EffectStatus::None);
    compact_tool_outcome("webview_screenshot", &mut outcome);
    assert_eq!(outcome.data["base64"], "essential-evidence");
    let data = json!({"base64":"ordinary-business-value","artifact":{"id":"x","path":"y"}});
    let mut ordinary = ExecutionOutcome::completed(data.clone(), EffectStatus::None);
    compact_tool_outcome("webview_execute_js", &mut ordinary);
    assert_eq!(ordinary.data, data);
}
