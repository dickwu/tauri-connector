use connector_client::inspection::{picker_exit_code, picker_mcp_content, PickerRequest};
use serde_json::json;

#[test]
fn picker_defaults_and_bounded_action_specific_fields() {
    let request = PickerRequest::parse(&json!({})).unwrap();
    assert_eq!(request.action, "start");
    assert!(request.convenience);
    assert_eq!(request.wait_ms, 10000);
    assert_eq!(request.timeout_ms, 60000);
    assert_eq!(request.window_id, "main");
    assert!(request.capture_screenshot);
    assert_eq!(
        PickerRequest::parse(&json!({"action":"start"}))
            .unwrap()
            .wait_ms,
        0
    );
    for bad in [
        json!({"action":"get","pickerId":"p","windowId":"other"}),
        json!({"action":"cancel","pickerId":"p","waitMs":0}),
        json!({"action":"start","pickerId":"p"}),
        json!({"action":"get"}),
        json!({"script":"alert(1)"}),
        json!({"timeoutMs":4999}),
        json!({"timeoutMs":120001}),
        json!({"waitMs":10001}),
        json!({"timeoutMs":60000,"timeout":60001}),
        json!({"captureScreenshot":false,"screenshotSource":"auto"}),
        json!({"captureScreenshot":false,"includeImage":true}),
        json!({"requestKey":""}),
        json!({"requestKey":"界".repeat(43)}),
        json!({"includeImage":"true"}),
        json!({"action":null}),
    ] {
        assert!(PickerRequest::parse(&bad).is_err(), "accepted {bad}");
    }
    assert!(PickerRequest::parse(&json!({"timeout":5000,"timeoutMs":5000})).is_ok());
    assert!(
        PickerRequest::parse(&json!({"action":"get","pickerId":"p","includeImage":true})).is_ok()
    );
}

#[test]
fn picker_fingerprint_excludes_wait_image_and_token() {
    let a = PickerRequest::parse(&json!({"requestKey":"k","authToken":"one","waitMs":0})).unwrap();
    let b = PickerRequest::parse(&json!({"action":"start","requestKey":"k","authToken":"two","waitMs":999,"includeImage":true})).unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert!(!a.fingerprint().to_string().contains("authToken"));
}

#[test]
fn picker_exit_status_does_not_claim_waiting_or_cancelled_selection() {
    for state in ["created", "installing", "awaiting_selection"] {
        assert_eq!(picker_exit_code(&json!({"status":state}), false), 2);
    }
    for state in ["cancelled", "expired", "target_changed", "failed"] {
        assert_eq!(picker_exit_code(&json!({"status":state}), false), 1);
    }
    assert_eq!(
        picker_exit_code(
            &json!({"status":"selected","screenshot":{"status":"failed"}}),
            false
        ),
        0
    );
    assert_eq!(picker_exit_code(&json!({"status":"cancelled"}), true), 0);
}

#[test]
fn picker_mcp_preserves_metadata_and_only_redacted_images() {
    let data = json!({"pickerId":"p","status":"selected","selection":{"tag":"button"},"screenshot":{"status":"captured","redaction":{"status":"applied"},"image":{"base64":"YWJj","mimeType":"image/png"}}});
    let content = picker_mcp_content(data.clone(), true);
    assert_eq!(content["structuredContent"]["selection"], data["selection"]);
    assert_eq!(content["content"][1]["type"], "image");
    assert!(content["structuredContent"]["screenshot"]["image"]
        .get("base64")
        .is_none());
    let mut raw = data;
    raw["screenshot"]["redaction"]["status"] = json!("unavailable");
    assert_eq!(
        picker_mcp_content(raw, true)["content"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_ne!(
        picker_mcp_content(json!({"status":"expired"}), false)["isError"],
        true
    );
    assert_eq!(
        picker_mcp_content(json!({"status":"failed"}), false)["isError"],
        true
    );
}

#[tokio::test]
async fn unsupported_plugin_never_receives_picker_or_execute_js() {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let request = ws.next().await.unwrap().unwrap();
        let request: serde_json::Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
        assert_eq!(request["type"], "bridge_status");
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"id":request["id"],"result":{"workflowProtocolVersion":1}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let next = tokio::time::timeout(std::time::Duration::from_millis(50), ws.next()).await;
        assert!(next.is_err(), "unsupported plugin received another command");
    });
    let mut client = connector_client::ConnectorClient::new();
    client.connect("127.0.0.1", port).await.unwrap();
    assert!(client
        .inspect("webview_select_element", &json!({"action":"start"}))
        .await
        .unwrap_err()
        .contains("capability_unavailable"));
    server.await.unwrap();
}

#[tokio::test]
async fn matching_handshake_forwards_exact_inspection_and_rejects_instance_switch() {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        for expected in [
            "bridge_status",
            "app_identity",
            "webview_select_element",
            "bridge_status",
            "app_identity",
        ] {
            let request = ws.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(request.to_text().unwrap()).unwrap();
            let operation = request
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| request["type"].as_str().unwrap());
            assert_eq!(operation, expected);
            let result = match operation {
                "bridge_status" => json!({"inspectionProtocolVersion":1}),
                "app_identity" => {
                    json!({"appInstanceId":"a","appId":"fixture","pid":1,"startedAt":1,"inspectionProtocolVersion":1})
                }
                _ => {
                    assert_eq!(request["args"]["requestKey"], "key");
                    json!({"pickerId":"a:picker:p","status":"awaiting_selection"})
                }
            };
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        }
    });
    let mut client = connector_client::ConnectorClient::new();
    client.connect("127.0.0.1", port).await.unwrap();
    let result = client
        .inspect(
            "webview_select_element",
            &json!({"action":"start","requestKey":"key","authToken":"fixture"}),
        )
        .await
        .unwrap();
    assert_eq!(result["status"], "awaiting_selection");
    assert!(client
        .inspect(
            "webview_select_element",
            &json!({"action":"get","pickerId":"b:picker:p"})
        )
        .await
        .unwrap_err()
        .contains("app_identity_mismatch"));
    assert!(client
        .inspect(
            "webview_select_element",
            &json!({"action":"get","pickerId":"a:picker:p","authToken":"fixture"})
        )
        .await
        .is_err());
    server.await.unwrap();
}

#[test]
fn shipped_picker_examples_parse_as_the_actual_contract() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if workspace.join("plugin/Cargo.toml").exists() {
        for name in [
            "picker-start.json",
            "picker-get.json",
            "picker-cancel.json",
            "picker-convenience.json",
        ] {
            let source = std::fs::read(workspace.join("examples/inspection").join(name)).unwrap();
            let vendored = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/inspection")
                    .join(name),
            )
            .unwrap();
            assert_eq!(
                source, vendored,
                "published client fixture differs from workspace example {name}"
            );
        }
    }
    for text in [
        include_str!("fixtures/inspection/picker-start.json"),
        include_str!("fixtures/inspection/picker-get.json"),
        include_str!("fixtures/inspection/picker-cancel.json"),
        include_str!("fixtures/inspection/picker-convenience.json"),
    ] {
        PickerRequest::parse(&serde_json::from_str(text).unwrap()).unwrap();
    }
}

#[test]
fn native_dimension_metadata_survives_mcp_image_extraction() {
    let metadata = json!({"widthPx":192,"heightPx":64,"mimeType":"image/png"});
    for include_image in [false, true] {
        let report = json!({"status":"selected","screenshot":{"status":"captured","image":metadata,"redaction":{"status":"applied"}},"image":{"base64":"YWJj","mimeType":"image/png"}});
        let result = picker_mcp_content(report, include_image);
        assert_eq!(result["structuredContent"]["screenshot"]["image"], metadata);
        assert_eq!(
            result["content"].as_array().unwrap().len(),
            if include_image { 2 } else { 1 }
        );
    }
}

#[test]
fn screenshot_targets_are_static_strict_and_bounded() {
    use connector_client::inspection::validate_static_locator;
    assert!(validate_static_locator(&json!({"by":"role","value":"button","name":"Save","scope":{"by":"css","value":"#dialog"},"entity":{"attribute":"data-id","value":"record-1"}})).is_ok());
    for target in [
        json!({"by":"css","value":"#x","force":true}),
        json!({"by":"ref","value":"@e1"}),
        json!({"by":"css","value":{"fromInput":{"key":"x"}}}),
        json!({"by":"css","value":""}),
    ] {
        assert!(validate_static_locator(&target).is_err());
    }
    let mut nested = json!({"by":"css","value":"div"});
    for _ in 0..10 {
        nested = json!({"by":"css","value":"div","scope":nested});
    }
    assert!(validate_static_locator(&nested).is_err());
}

#[test]
fn mcp_only_emits_negotiated_structured_content_and_keeps_text_images() {
    let payload = json!({"structuredContent":{"status":"selected"},"isError":false,"content":[{"type":"text","text":"metadata"},{"type":"image","data":"YWJj","mimeType":"image/png"}]});
    for version in [None, Some("2024-11-05"), Some("2025-03-26")] {
        let result = connector_client::inspection::shape_mcp_result(payload.clone(), version);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result["content"], payload["content"]);
        assert_eq!(result["isError"], false);
    }
    for version in ["2025-06-18", "2025-11-25"] {
        assert_eq!(
            connector_client::inspection::shape_mcp_result(payload.clone(), Some(version)),
            payload
        );
    }
}

#[test]
fn protected_artifact_aliases_cannot_silently_change_the_requested_identity() {
    use connector_client::inspection::protected_artifact_args;
    assert!(protected_artifact_args(&json!({"artifact":"one","artifactId":"two"})).is_err());
    assert_eq!(
        protected_artifact_args(&json!({"before":"a","baselineId":"a","after":"b"})).unwrap(),
        json!({"baselineId":"a","currentId":"b"})
    );
}

#[test]
fn accepted_cancel_of_prior_terminal_picker_is_success_without_rewriting_its_state() {
    for status in [
        "selected",
        "cancelled",
        "expired",
        "target_changed",
        "failed",
    ] {
        let report = json!({"status":status,"cancellationAccepted":true});
        assert_eq!(picker_exit_code(&report, true), 0);
        let mcp = picker_mcp_content(report, false);
        assert_eq!(mcp["structuredContent"]["status"], status);
        assert_eq!(mcp["isError"], false);
    }
    assert_eq!(
        picker_exit_code(&json!({"status":"failed","code":"unauthorized"}), true),
        1
    );
}

#[tokio::test]
async fn sdk_generates_one_request_key_and_retains_it_after_a_lost_response() {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        for stage in 0..3 {
            let message = ws.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(message.to_text().unwrap()).unwrap();
            let result = match stage {
                0 => {
                    assert_eq!(request["type"], "bridge_status");
                    json!({"inspectionProtocolVersion":1})
                }
                1 => {
                    assert_eq!(request["operation"], "app_identity");
                    json!({"appInstanceId":"fixture","appId":"fixture","pid":1,"startedAt":1,"inspectionProtocolVersion":1})
                }
                _ => {
                    assert_eq!(request["operation"], "webview_select_element");
                    let key = request["args"]["requestKey"].as_str().unwrap().to_owned();
                    assert!(!key.is_empty());
                    ws.close(None).await.unwrap();
                    return key;
                }
            };
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        }
        unreachable!()
    });
    let mut client = connector_client::ConnectorClient::new();
    client.connect("127.0.0.1", port).await.unwrap();
    let error = client
        .inspect(
            "webview_select_element",
            &json!({"action":"start","authToken":"fixture-token"}),
        )
        .await
        .unwrap_err();
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["requestKey"], server.await.unwrap());
    assert!(error["error"].as_str().unwrap().contains("outcome_unknown"));
}
