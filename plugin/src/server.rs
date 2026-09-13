//! External WebSocket server that MCP servers connect to.

use std::net::TcpListener;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::bridge::Bridge;
use crate::handlers;
use crate::protocol::{Command, Request, Response};
use crate::state::PluginState;

pub struct Server {
    port: u16,
    bridge: Bridge,
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    state: PluginState,
}

fn parse_request(text: &str) -> Result<Request, Box<Response>> {
    let raw: serde_json::Value = serde_json::from_str(text).map_err(|error| {
        Box::new(Response::error(
            "unknown".into(),
            format!("Invalid request: {error}"),
        ))
    })?;
    let id = raw["id"].as_str().unwrap_or("unknown").to_owned();
    // The legacy enum must not silently discard an explicit source, mask or
    // authorization request and then capture through an anonymous old path.
    if raw["type"] == "screenshot" && connector_client::inspection::is_rich_screenshot(&raw) {
        return Err(Box::new(Response::error(
            id,
            "invalid_arguments: rich screenshot fields require type=inspection, operation=webview_screenshot and args",
        )));
    }
    serde_json::from_value(raw)
        .map_err(|error| Box::new(Response::error(id, format!("Invalid request: {error}"))))
}

impl Server {
    pub fn new(
        bind_address: &str,
        port_range: (u16, u16),
        bridge: Bridge,
        state: PluginState,
    ) -> Result<Self, String> {
        let port =
            find_available_port(bind_address, port_range.0, port_range.1).ok_or_else(|| {
                format!(
                    "No available port in range {}-{}",
                    port_range.0, port_range.1
                )
            })?;

        Ok(Self {
            port,
            bridge,
            app_handle: Arc::new(Mutex::new(None)),
            state,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn set_app_handle(&self, handle: tauri::AppHandle) {
        let app_handle = self.app_handle.clone();
        tokio::spawn(async move {
            *app_handle.lock().await = Some(handle);
        });
    }

    pub async fn run(&self, bind_address: String) -> Result<(), String> {
        let addr = format!("{bind_address}:{}", self.port);
        let listener = TokioTcpListener::bind(&addr)
            .await
            .map_err(|e| e.to_string())?;

        println!("[connector][server] Listening on {addr}");

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    println!("[connector][server] Client connected: {peer}");
                    let bridge = self.bridge.clone();
                    let app_handle = self.app_handle.clone();
                    let state = self.state.clone();

                    tokio::spawn(async move {
                        let ws = match tokio_tungstenite::accept_async(stream).await {
                            Ok(ws) => ws,
                            Err(e) => {
                                eprintln!("[connector][server] WebSocket handshake error: {e}");
                                return;
                            }
                        };

                        let (ws_tx, mut ws_rx) = ws.split();
                        let ws_tx = Arc::new(Mutex::new(ws_tx));

                        while let Some(Ok(msg)) = ws_rx.next().await {
                            let Message::Text(text) = msg else { continue };

                            let request = match parse_request(&text) {
                                Ok(r) => r,
                                Err(resp) => {
                                    let _ = send_response(&ws_tx, &resp).await;
                                    continue;
                                }
                            };

                            let id = request.id.clone();
                            let bridge = bridge.clone();
                            let app_handle = app_handle.clone();
                            let ws_tx = ws_tx.clone();
                            let state = state.clone();

                            tokio::spawn(async move {
                                // Clone the handle out so the lock is not held
                                // across command execution — concurrent
                                // commands (e.g. parallel batches) must not
                                // serialize on it.
                                let app = app_handle.lock().await.clone();
                                let response = handle_command(
                                    id,
                                    request.command,
                                    &bridge,
                                    app.as_ref(),
                                    &state,
                                )
                                .await;
                                let _ = send_response(&ws_tx, &response).await;
                            });
                        }

                        println!("[connector][server] Client disconnected");
                    });
                }
                Err(e) => {
                    eprintln!("[connector][server] Accept error: {e}");
                }
            }
        }
    }
}

async fn handle_command(
    id: String,
    command: Command,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Response {
    if let Command::SelectElement { args } = command {
        let mut args = args;
        if let Some(fields) = args.as_object_mut()
            && let Some(window) = fields.remove("window_id")
        {
            if fields
                .get("windowId")
                .is_some_and(|existing| existing != &window)
            {
                return Response::error(id,serde_json::json!({"code":"invalid_arguments","message":"window aliases conflict"}).to_string());
            }
            fields.insert("windowId".into(), window);
        }
        return crate::mcp_tools::inspection_response(
            &id,
            "webview_select_element",
            &args,
            bridge,
            app,
            state,
        )
        .await;
    }
    if let Command::Inspection { operation, args } = command {
        return crate::mcp_tools::inspection_response(&id, &operation, &args, bridge, app, state)
            .await;
    }
    if let Command::Workflow { operation, args } = command {
        return match crate::workflow::call(&operation, &args, bridge, app, state).await {
            Ok(report) => Response::success(id, report),
            Err(error) => Response::error(id, error.to_string()),
        };
    }
    let args = match serde_json::to_value(&command) {
        Ok(args) => args,
        Err(error) => return Response::not_dispatched(id, "invalid_spec", error.to_string()),
    };
    let wire_name = args
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let tool = match wire_name {
        "execute_js" => "webview_execute_js",
        "screenshot" => "webview_screenshot",
        "dom_snapshot" => "webview_dom_snapshot",
        "find_element" => "webview_find_element",
        "get_styles" => "webview_get_styles",
        "interact" => "webview_interact",
        "keyboard" => "webview_keyboard",
        "wait_for" => "webview_wait_for",
        "locator" => "webview_locator",
        "act_and_verify" => "webview_act_and_verify",
        "backend_state" => "ipc_get_backend_state",
        other => other,
    };
    let lease = match crate::workflow::resources::acquire(state, tool, &args).await {
        Ok(lease) => lease,
        Err(error) => return Response::rejected(id, &error),
    };
    let response = handle_command_unleased(id, command, bridge, app, state).await;
    let id = response.id.clone();
    let response = Response::with_outcome(id, response.into_outcome(tool));
    if response.requires_quarantine() {
        lease.quarantine("outcome_unknown");
    }
    response
}

async fn handle_command_unleased(
    id: String,
    command: Command,
    bridge: &Bridge,
    app: Option<&tauri::AppHandle>,
    state: &PluginState,
) -> Response {
    match command {
        Command::Workflow { .. } | Command::Inspection { .. } => {
            unreachable!("lifecycle commands are routed before legacy dispatch")
        }
        Command::Ping => Response::success(id, serde_json::json!("pong")),

        // JS Execution
        Command::ExecuteJs { script, window_id } => {
            handlers::execute_js(&id, &script, &window_id, bridge).await
        }
        Command::BridgeStatus => Response::success(id, bridge.status().await),

        // Screenshot
        Command::Screenshot {
            format,
            quality,
            max_width,
            window_id,
            save,
            output_dir,
            name_hint,
            overwrite,
            selector,
            annotate,
        } => {
            handlers::screenshot(
                &id,
                &format,
                quality,
                max_width,
                &window_id,
                bridge,
                app,
                state,
                save.unwrap_or(false),
                output_dir.as_deref(),
                name_hint.as_deref(),
                overwrite.unwrap_or(false),
                selector.as_deref(),
                annotate.unwrap_or(false),
            )
            .await
        }

        // DOM
        Command::DomSnapshot {
            mode,
            snapshot_type,
            selector,
            max_depth,
            max_elements,
            max_tokens,
            no_split,
            react_enrich,
            follow_portals,
            shadow_dom,
            window_id,
        } => {
            let mode = mode.or(snapshot_type).unwrap_or_else(|| "ai".to_string());
            let mt = if no_split.unwrap_or(false) {
                Some(0)
            } else {
                max_tokens.or(Some(4000))
            };
            handlers::dom_snapshot(
                &id,
                &mode,
                selector.as_deref(),
                max_depth,
                max_elements,
                mt,
                react_enrich.unwrap_or(true),
                follow_portals.unwrap_or(true),
                shadow_dom.unwrap_or(false),
                &window_id,
                bridge,
                state,
            )
            .await
        }
        Command::GetCachedDom { window_id } => {
            handlers::get_cached_dom(&id, &window_id, state).await
        }

        // Element Operations
        Command::FindElement {
            selector,
            strategy,
            target,
            window_id,
        } => {
            handlers::find_element(
                &id,
                &selector,
                &strategy,
                target.as_deref(),
                &window_id,
                bridge,
            )
            .await
        }
        Command::GetStyles {
            selector,
            properties,
            window_id,
        } => {
            handlers::get_styles(
                &id,
                &selector,
                properties.as_deref(),
                &window_id,
                bridge,
                state,
            )
            .await
        }
        Command::SelectElement { .. } => {
            unreachable!("picker alias is routed before legacy dispatch")
        }
        Command::GetPointedElement { .. } => handlers::get_pointed_element(&id, state).await,

        // Interaction
        Command::Interact {
            action,
            selector,
            strategy,
            x,
            y,
            direction,
            distance,
            target_selector,
            target_x,
            target_y,
            steps,
            duration_ms,
            drag_strategy,
            window_id,
        } => {
            if action == "drag" {
                handlers::drag(
                    &id,
                    selector.as_deref(),
                    &strategy,
                    x,
                    y,
                    target_selector.as_deref(),
                    target_x,
                    target_y,
                    steps.unwrap_or(10),
                    duration_ms.unwrap_or(300),
                    drag_strategy.as_deref().unwrap_or("auto"),
                    &window_id,
                    bridge,
                    state,
                )
                .await
            } else {
                handlers::interact(
                    &id,
                    &action,
                    selector.as_deref(),
                    &strategy,
                    x,
                    y,
                    direction.as_deref(),
                    distance,
                    &window_id,
                    bridge,
                    state,
                )
                .await
            }
        }
        Command::Keyboard {
            action,
            text,
            key,
            modifiers,
            window_id,
        } => {
            handlers::keyboard(
                &id,
                &action,
                text.as_deref(),
                key.as_deref(),
                modifiers.as_deref(),
                &window_id,
                bridge,
            )
            .await
        }
        Command::WaitFor {
            selector,
            strategy,
            text,
            url,
            load_state,
            function,
            state: selector_state,
            timeout,
            window_id,
        } => {
            handlers::wait_for(
                &id,
                selector.as_deref(),
                &strategy,
                text.as_deref(),
                url.as_deref(),
                load_state.as_deref(),
                function.as_deref(),
                selector_state.as_deref(),
                timeout,
                &window_id,
                bridge,
                state,
            )
            .await
        }
        Command::Locator {
            role,
            text,
            label,
            placeholder,
            alt,
            title,
            test_id,
            name,
            exact,
            first,
            last,
            nth,
            action,
            value,
            window_id,
        } => {
            handlers::locator(
                &id,
                role.as_deref(),
                text.as_deref(),
                label.as_deref(),
                placeholder.as_deref(),
                alt.as_deref(),
                title.as_deref(),
                test_id.as_deref(),
                name.as_deref(),
                exact.unwrap_or(false),
                first.unwrap_or(false),
                last.unwrap_or(false),
                nth,
                action.as_deref(),
                value.as_deref(),
                &window_id,
                bridge,
            )
            .await
        }

        // Window Management
        Command::WindowList => handlers::window_list(&id, app).await,
        Command::WindowInfo { window_id } => handlers::window_info(&id, &window_id, app).await,
        Command::WindowResize {
            window_id,
            width,
            height,
        } => handlers::window_resize(&id, &window_id, width, height, app).await,

        // IPC
        Command::BackendState => handlers::backend_state(&id, app).await,
        Command::IpcExecuteCommand { command, args } => {
            handlers::ipc_execute_command(&id, &command, args.as_ref(), "main", bridge).await
        }
        Command::IpcMonitor { action, window_id } => {
            handlers::ipc_monitor(&id, &action, &window_id, state, bridge).await
        }
        Command::IpcGetCaptured {
            filter,
            pattern,
            limit,
            since,
        } => {
            handlers::ipc_get_captured(
                &id,
                filter.as_deref(),
                pattern.as_deref(),
                limit,
                since,
                state,
            )
            .await
        }
        Command::IpcEmitEvent {
            event_name,
            payload,
        } => handlers::ipc_emit_event(&id, &event_name, payload.as_ref(), app).await,

        // Logs
        Command::ConsoleLogs {
            lines,
            filter,
            level,
            pattern,
            window_id,
        } => {
            handlers::console_logs(
                &id,
                lines,
                filter.as_deref(),
                pattern.as_deref(),
                level.as_deref(),
                &window_id,
                state,
            )
            .await
        }
        Command::ClearLogs { source } => handlers::clear_logs(&id, &source, state).await,
        Command::ReadLogFile {
            source,
            lines,
            level,
            pattern,
            since,
            window_id,
        } => {
            handlers::read_log_file(
                &id,
                &source,
                lines,
                level.as_deref(),
                pattern.as_deref(),
                since,
                window_id.as_deref(),
                state,
            )
            .await
        }

        // Event Capture
        Command::IpcListen { action, events } => {
            handlers::ipc_listen(&id, &action, events.as_deref(), state, bridge).await
        }
        Command::EventGetCaptured {
            event,
            pattern,
            limit,
            since,
        } => {
            handlers::event_get_captured(
                &id,
                event.as_deref(),
                pattern.as_deref(),
                limit,
                since,
                state,
            )
            .await
        }

        Command::RuntimeGetCaptured {
            kind,
            level,
            pattern,
            since,
            since_mark,
            limit,
            window_id,
        } => {
            handlers::runtime_get_captured(
                &id,
                kind.as_deref(),
                level.as_deref(),
                pattern.as_deref(),
                since,
                since_mark.as_deref(),
                limit,
                window_id.as_deref(),
                state,
            )
            .await
        }

        Command::ArtifactList { kind, limit } => {
            handlers::artifact_list(&id, kind.as_deref(), limit, state).await
        }
        Command::ArtifactRead { artifact } => handlers::artifact_read(&id, &artifact, state).await,
        Command::ArtifactCompare {
            before,
            after,
            threshold,
        } => handlers::artifact_compare(&id, &before, &after, threshold, state).await,
        Command::ArtifactPrune {
            keep,
            kind,
            delete_files,
        } => handlers::artifact_prune(&id, keep, kind.as_deref(), delete_files, state).await,

        Command::DebugMark { label } => handlers::debug_mark(&id, label.as_deref(), state).await,
        Command::DebugSnapshot {
            window_id,
            include_dom,
            include_screenshot,
            include_logs,
            include_ipc,
            include_events,
            include_runtime,
            since,
            since_mark,
            max_tokens,
            screenshot_name_hint,
        } => {
            handlers::debug_snapshot(
                &id,
                &window_id,
                include_dom,
                include_screenshot,
                include_logs,
                include_ipc,
                include_events,
                include_runtime,
                since,
                since_mark.as_deref(),
                max_tokens,
                screenshot_name_hint.as_deref(),
                bridge,
                app,
                state,
            )
            .await
        }
        Command::WebviewActAndVerify {
            action,
            selector,
            text,
            key,
            target_selector,
            wait_for_selector,
            wait_for_text,
            timeout,
            verify_dom,
            verify_screenshot,
            include_logs,
            include_ipc,
            include_runtime,
            window_id,
        } => {
            handlers::webview_act_and_verify(
                &id,
                &action,
                selector.as_deref(),
                text.as_deref(),
                key.as_deref(),
                target_selector.as_deref(),
                wait_for_selector.as_deref(),
                wait_for_text.as_deref(),
                timeout,
                verify_dom,
                verify_screenshot,
                include_logs,
                include_ipc,
                include_runtime,
                &window_id,
                bridge,
                app,
                state,
            )
            .await
        }

        // Search
        Command::SearchSnapshot {
            pattern,
            context,
            mode,
            window_id,
        } => {
            handlers::search_snapshot(&id, &pattern, context, &mode, &window_id, state, bridge)
                .await
        }
    }
}

async fn send_response<S>(ws_tx: &Arc<Mutex<S>>, response: &Response) -> Result<(), String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let json = serde_json::to_string(response).map_err(|e| e.to_string())?;
    let mut tx = ws_tx.lock().await;
    tx.send(Message::Text(json.into()))
        .await
        .map_err(|e| e.to_string())
}

fn find_available_port(addr: &str, start: u16, end: u16) -> Option<u16> {
    (start..end).find(|&port| TcpListener::bind((addr, port)).is_ok())
}

#[cfg(test)]
mod rich_screenshot_envelope_tests {
    use super::*;

    #[test]
    fn rich_privacy_or_source_fields_cannot_be_discarded_by_legacy_ws_parsing() {
        for field in [
            "source",
            "target",
            "redaction",
            "allowWindowPreparation",
            "includeImage",
            "authToken",
            "timeoutMs",
        ] {
            for value in [serde_json::json!("webview_native"), serde_json::Value::Null] {
                let mut raw = serde_json::json!({"id":"requested-image","type":"screenshot"});
                raw[field] = value;
                let failure = parse_request(&raw.to_string())
                    .expect_err("rich request must not reach legacy screenshot dispatch");
                assert_eq!(failure.id, "requested-image");
                let crate::protocol::ResponsePayload::Error { error } = failure.payload else {
                    panic!("expected rejected response")
                };
                assert!(error.contains("inspection"));
            }
        }
        assert!(matches!(
            parse_request(r##"{"id":"legacy","type":"screenshot","selector":"#button"}"##)
                .unwrap()
                .command,
            Command::Screenshot { .. }
        ));
        assert!(matches!(parse_request(r#"{"id":"rich","type":"inspection","operation":"webview_screenshot","args":{"source":"webview_native"}}"#).unwrap().command, Command::Inspection { .. }));
    }
}

#[cfg(test)]
mod workflow_adapter_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn embedded_and_websocket_share_capabilities_auth_and_resource_boundary() {
        let directory =
            std::env::temp_dir().join(format!("connector-adapters-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let state = PluginState::new(directory.clone()).unwrap();
        state
            .workflow
            .set_token(Some("isolated-test-workflow-token-32-bytes".into()));
        let bridge = Bridge::start().unwrap();

        let response = handle_command(
            "ws".into(),
            Command::Workflow {
                operation: "workflow_capabilities".into(),
                args: json!({}),
            },
            &bridge,
            None,
            &state,
        )
        .await;
        let crate::protocol::ResponsePayload::Success { result } = response.payload else {
            panic!("capabilities failed");
        };
        let embedded =
            crate::mcp_tools::call_tool("workflow_capabilities", &json!({}), &bridge, None, &state)
                .await;
        assert_eq!(embedded["structuredContent"], result);
        assert_eq!(result["journal"]["privateStorage"], cfg!(unix));

        let denied = handle_command(
            "ws".into(),
            Command::Workflow {
                operation: "workflow_run".into(),
                args: json!({"spec":{}}),
            },
            &bridge,
            None,
            &state,
        )
        .await;
        let crate::protocol::ResponsePayload::Error { error } = denied.payload else {
            panic!("unauthorized workflow accepted");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&error).unwrap()["code"],
            "unauthorized"
        );
        let embedded =
            crate::mcp_tools::call_tool("workflow_run", &json!({"spec":{}}), &bridge, None, &state)
                .await;
        assert_eq!(embedded["isError"], true);
        assert_eq!(embedded["structuredContent"]["code"], "unauthorized");

        let lease = state
            .workflow
            .resources
            .try_acquire(vec!["ui".into(), "backend".into()])
            .unwrap();
        lease.quarantine("outcome_unknown");
        // Durable-history recovery precedes resource acquisition. Non-Unix
        // hosts must retain the earlier fail-closed storage rejection instead
        // of bypassing it just to reach the in-memory lease check.
        let rejection_code = if cfg!(unix) {
            "resource_busy"
        } else {
            "persistence_unavailable"
        };
        let command: Command = serde_json::from_value(
            json!({"type":"interact","action":"click","selector":"#isolated"}),
        )
        .unwrap();
        let blocked = handle_command("ws".into(), command, &bridge, None, &state).await;
        let blocked = serde_json::to_value(blocked.outcome.unwrap()).unwrap();
        assert_eq!(blocked["error"]["code"], rejection_code);
        assert_eq!(blocked["execution"], "not_dispatched");
        assert_eq!(blocked["effect"], "none");
        let embedded = crate::mcp_tools::call_tool(
            "webview_interact",
            &json!({"action":"click","selector":"#isolated"}),
            &bridge,
            None,
            &state,
        )
        .await;
        assert_eq!(
            embedded["structuredContent"]["outcome"]["execution"],
            "not_dispatched"
        );
        assert_eq!(
            embedded["structuredContent"]["outcome"]["error"]["code"],
            rejection_code
        );
        assert_eq!(embedded["structuredContent"]["outcome"]["effect"], "none");
        assert_eq!(bridge.runtime_diagnostics()["installAttempts"], 0);
        assert_eq!(bridge.runtime_diagnostics()["commandBytesSent"], 0);
        drop(lease);
        // Both entry points must leave the original quarantine intact on every
        // platform, even when storage refuses the operation before arbitration.
        let Err(conflict) = state
            .workflow
            .resources
            .try_acquire(vec!["ui/window/main".into()])
        else {
            panic!("A rejected adapter call released unknown-write quarantine");
        };
        assert_eq!(conflict["code"], "resource_busy");
        assert_eq!(conflict["quarantined"], true);
        assert_eq!(state.workflow.resources.quarantined(), 1);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn picker_inspection_alias_and_embedded_mcp_share_validation_and_authorization() {
        let directory =
            std::env::temp_dir().join(format!("inspection-adapters-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let state = PluginState::new(directory.clone()).unwrap();
        let token = "isolated-picker-adapter-token-32-bytes";
        state.workflow.set_token(Some(token.into()));
        let bridge = Bridge::start().unwrap();
        for (args, expected) in [
            (json!({"action":"get","pickerId":"unknown"}), "unauthorized"),
            (
                json!({"action":"get","pickerId":"unknown","authToken":token,"windowId":"other"}),
                "invalid_arguments",
            ),
            (
                json!({"action":"get","pickerId":"unknown","authToken":token}),
                "picker_not_found",
            ),
        ] {
            let direct = handle_command(
                "ws".into(),
                Command::Inspection {
                    operation: "webview_select_element".into(),
                    args: args.clone(),
                },
                &bridge,
                None,
                &state,
            )
            .await;
            let crate::protocol::ResponsePayload::Error { error } = direct.payload else {
                panic!("invalid picker request succeeded")
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&error).unwrap()["code"],
                expected
            );
            let embedded =
                crate::mcp_tools::call_tool("webview_select_element", &args, &bridge, None, &state)
                    .await;
            assert_eq!(embedded["isError"], true);
            assert_eq!(embedded["structuredContent"]["code"], expected);
            let mut alias = args;
            alias["type"] = json!("select_element");
            let alias = serde_json::from_value::<Command>(alias).unwrap();
            let alias = handle_command("alias".into(), alias, &bridge, None, &state).await;
            let crate::protocol::ResponsePayload::Error { error } = alias.payload else {
                panic!("picker alias accepted invalid request")
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&error).unwrap()["code"],
                expected
            );
        }
        for operation in [
            "app_identity",
            "runtime_health",
            "ipc_capture",
            "ipc_query",
            "artifact_read",
            "webview_screenshot",
        ] {
            let denied = handle_command(
                "auth".into(),
                Command::Inspection {
                    operation: operation.into(),
                    args: json!({}),
                },
                &bridge,
                None,
                &state,
            )
            .await;
            let crate::protocol::ResponsePayload::Error { error } = denied.payload else {
                panic!("inspection auth bypass")
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&error).unwrap()["code"],
                "unauthorized"
            );
        }
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
