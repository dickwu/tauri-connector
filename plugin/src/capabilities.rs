//! Anonymous inspection metadata derived only from the compiled implementation.
use serde_json::{Value, json};

#[derive(Clone, Copy)]
struct BuildFeatures {
    platform: &'static str,
    native: bool,
    xcap: bool,
    codec: bool,
}
impl BuildFeatures {
    fn current() -> Self {
        Self {
            platform: std::env::consts::OS,
            native: cfg!(feature = "native-screenshot"),
            xcap: cfg!(feature = "xcap"),
            codec: cfg!(feature = "capture-codec"),
        }
    }
}

/// Pure build metadata: deliberately has no Bridge, AppHandle, state, or token.
/// It does not establish that a particular page/OS capture backend is ready.
pub fn inspection() -> Value {
    for_build(BuildFeatures::current())
}

fn for_build(build: BuildFeatures) -> Value {
    let desktop = matches!(build.platform, "macos" | "windows" | "linux");
    let native = desktop && build.native && build.codec;
    let window = desktop && build.xcap && build.codec;
    let dom = desktop && build.codec;
    let source = |compiled| json!({"compiled":compiled,"runtimeAvailability":if compiled {"unprobed"} else {"unavailable"}});
    let sources = json!({"auto":source(native||window||dom),"webview_native":source(native),"window_native":source(window),"dom_rendering":source(dom)});
    let isolation = json!({
        "mode":"document_start_capture_guard",
        "requiresEarlyGuard":true,
        "readiness":"checked_on_start",
        "idleByDefault":true,
        "tailDrainMaxMs":2000,
        "limitations":[
            "Earlier application capture listeners cannot be undone",
            "CSS hover and background application activity may still occur",
            "Held input or pointer capture prevents activation",
            "Cleanup may be unconfirmed after the bounded drain deadline"
        ]
    });
    json!({
        "inspectionProtocolVersion":connector_client::inspection::INSPECTION_PROTOCOL_VERSION,
        "authentication":{"required":true,"credential":"host_token","sharedTokenMeansSharedAuthorizationDomain":true},
        "identity":{"application":"process_lifetime","window":"window_lifetime","page":"document_lifetime","checkedBeforeDispatch":true},
        "runtime":{
            "runtimeProtocolVersion":crate::runtime::manifest::PROTOCOL_VERSION,
            "runtimeVersion":crate::runtime::manifest::RUNTIME_VERSION,
            "semanticVersion":crate::runtime::manifest::SEMANTIC_VERSION,
            "bundleHash":crate::runtime::manifest::bundle_hash(),
            "installSource":"build_embedded",
            "installScope":"document",
            "modules":crate::runtime::manifest::modules().iter().map(|module|module.module_id).collect::<Vec<_>>(),
            "automaticBusinessReplay":false
        },
        "health":{"depths":["transport","bridge","runtime"],"installsRuntime":false,"automaticRecovery":false},
        "picker":{
            "tool":"webview_select_element",
            "actions":["start","get","cancel"],
            "convenienceCall":true,
            "authenticationRequired":true,
            "supportedPlatforms":["macos","windows","linux"],
            "availableOnThisBuild":desktop,
            "supportedScopes":["top_level_light_dom"],
            "boundaryOnly":["iframe","shadow_host","canvas"],
            "entryPoints":["cli","websocket","embedded_mcp","stdio_mcp"],
            "maxActive":crate::picker::MAX_ACTIVE,
            "timeoutMs":{"minimum":5000,"maximum":120000,"default":60000},
            "inputIsolation":isolation,
            "captureScreenshotCanBeDisabled":true,
            "mutatesWorkflowSpec":false,
            "releasesUnknownWriteQuarantine":false
        },
        "screenshots":{"sources":sources,"autoOrder":["webview_native","window_native","dom_rendering"],"runtimeReadinessRequired":true,"explicitSourceFallback":false},
        "ipcCapture":{"actions":["start","status","stop"],"query":true,"resultPolicies":["metadata","preview"],"previewRequiresHostAllowlist":true,"coverage":{"frontendInvoke":"runtime_selected_hook","rustTrace":"unobserved","businessPersistence":"unobserved"}},
        "availabilityScope":"compiled_support_only_runtime_and_native_validation_are_separate"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn screenshot_availability_tracks_backend_codec_and_platform_flags() {
        for platform in ["macos", "windows", "linux", "unsupported"] {
            for native in [false, true] {
                for xcap in [false, true] {
                    for codec in [false, true] {
                        let capabilities = for_build(BuildFeatures {
                            platform,
                            native,
                            xcap,
                            codec,
                        });
                        let desktop = platform != "unsupported";
                        let sources = &capabilities["screenshots"]["sources"];
                        assert_eq!(
                            sources["webview_native"]["compiled"],
                            desktop && native && codec
                        );
                        assert_eq!(
                            sources["window_native"]["compiled"],
                            desktop && xcap && codec
                        );
                        assert_eq!(sources["dom_rendering"]["compiled"], desktop && codec);
                        assert_eq!(sources["auto"]["compiled"], desktop && codec);
                        assert_eq!(capabilities["picker"]["availableOnThisBuild"], desktop);
                    }
                }
            }
        }
    }

    #[test]
    fn exported_capability_uses_actual_compilation_and_contract_limits() {
        assert_eq!(inspection(), for_build(BuildFeatures::current()));
        let capability = inspection();
        assert_eq!(
            capability["inspectionProtocolVersion"],
            connector_client::inspection::INSPECTION_PROTOCOL_VERSION
        );
        assert_eq!(
            capability["runtime"]["runtimeVersion"],
            crate::runtime::manifest::RUNTIME_VERSION
        );
        assert_eq!(
            capability["runtime"]["semanticVersion"],
            crate::runtime::manifest::SEMANTIC_VERSION
        );
        assert_eq!(
            capability["picker"]["actions"],
            json!(["start", "get", "cancel"])
        );
        assert_eq!(capability["picker"]["maxActive"], 4);
        assert_eq!(
            capability["picker"]["timeoutMs"],
            json!({"minimum":5000,"maximum":120000,"default":60000})
        );
        for timeout in [5000, 120000] {
            assert!(
                connector_client::inspection::PickerRequest::parse(
                    &json!({"action":"start","timeoutMs":timeout})
                )
                .is_ok()
            );
        }
        for timeout in [4999, 120001] {
            assert!(
                connector_client::inspection::PickerRequest::parse(
                    &json!({"action":"start","timeoutMs":timeout})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn anonymous_capability_never_contains_handles_paths_or_observations() {
        fn check(value: &Value) {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        assert!(
                            ![
                                "appInstanceId",
                                "windowId",
                                "windowInstanceId",
                                "pageEpoch",
                                "runtimeId",
                                "pickerId",
                                "selectionId",
                                "captureSessionId",
                                "artifactId",
                                "workspacePath",
                                "cwd",
                                "url",
                                "title",
                                "text",
                                "image",
                                "authToken"
                            ]
                            .contains(&key.as_str()),
                            "private field {key}"
                        );
                        check(value);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        check(value);
                    }
                }
                Value::String(text) => assert!(
                    !text.contains("/Users/")
                        && !text.contains("/Volumes/")
                        && !text.contains("file://")
                ),
                _ => {}
            }
        }
        let capabilities = inspection();
        check(&capabilities);
        assert_eq!(capabilities["authentication"]["required"], true);
        assert_eq!(
            capabilities["picker"]["inputIsolation"]["readiness"],
            "checked_on_start"
        );
        assert!(
            capabilities.to_string().len() < 4096,
            "anonymous metadata stays small"
        );
    }
}
