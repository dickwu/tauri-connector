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

impl Server {
    pub fn new(
        bind_address: &str,
        port_range: (u16, u16),
        bridge: Bridge,
        state: PluginState,
    ) -> Result<Self, String> {
        let port = find_available_port(bind_address, port_range.0, port_range.1)
            .ok_or_else(|| {
                format!("No available port in range {}-{}", port_range.0, port_range.1)
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

                            let request: Request = match serde_json::from_str(&text) {
                                Ok(r) => r,
                                Err(e) => {
                                    let resp = Response::error("unknown".to_string(), format!("Invalid request: {e}"));
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
                                let app = app_handle.lock().await;
                                let response = handle_command(id, request.command, &bridge, app.as_ref(), &state).await;
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
    match command {
        Command::Ping => Response::success(id, serde_json::json!("pong")),

        // JS Execution
        Command::ExecuteJs { script, window_id } => {
            handlers::execute_js(&id, &script, &window_id, bridge).await
        }

        // Screenshot
        Command::Screenshot { format, quality, max_width, window_id } => {
            handlers::screenshot(&id, &format, quality, max_width, &window_id, bridge, app).await
        }

        // DOM
        Command::DomSnapshot { snapshot_type, selector, window_id } => {
            handlers::dom_snapshot(&id, &snapshot_type, selector.as_deref(), None, None, true, true, false, &window_id, bridge).await
        }
        Command::GetCachedDom { window_id } => {
            handlers::get_cached_dom(&id, &window_id, state).await
        }

        // Element Operations
        Command::FindElement { selector, strategy, window_id } => {
            handlers::find_element(&id, &selector, &strategy, &window_id, bridge).await
        }
        Command::GetStyles { selector, properties, window_id } => {
            handlers::get_styles(&id, &selector, properties.as_deref(), &window_id, bridge).await
        }
        Command::SelectElement { .. } => {
            Response::error(id, "Select element (visual picker) not yet implemented")
        }
        Command::GetPointedElement { .. } => {
            handlers::get_pointed_element(&id, state).await
        }

        // Interaction
        Command::Interact { action, selector, strategy, x, y, direction, distance, window_id } => {
            handlers::interact(&id, &action, selector.as_deref(), &strategy, x, y, direction.as_deref(), distance, &window_id, bridge).await
        }
        Command::Keyboard { action, text, key, modifiers, window_id } => {
            handlers::keyboard(&id, &action, text.as_deref(), key.as_deref(), modifiers.as_deref(), &window_id, bridge).await
        }
        Command::WaitFor { selector, strategy, text, timeout, window_id } => {
            handlers::wait_for(&id, selector.as_deref(), &strategy, text.as_deref(), timeout, &window_id, bridge).await
        }

        // Window Management
        Command::WindowList => handlers::window_list(&id, app).await,
        Command::WindowInfo { window_id } => handlers::window_info(&id, &window_id, app).await,
        Command::WindowResize { window_id, width, height } => {
            handlers::window_resize(&id, &window_id, width, height, app).await
        }

        // IPC
        Command::BackendState => handlers::backend_state(&id, app).await,
        Command::IpcExecuteCommand { command, args } => {
            handlers::ipc_execute_command(&id, &command, args.as_ref(), "main", bridge).await
        }
        Command::IpcMonitor { action } => handlers::ipc_monitor(&id, &action, state, bridge).await,
        Command::IpcGetCaptured { filter, pattern, limit, since } => {
            handlers::ipc_get_captured(&id, filter.as_deref(), pattern.as_deref(), limit, since, state).await
        }
        Command::IpcEmitEvent { event_name, payload } => {
            handlers::ipc_emit_event(&id, &event_name, payload.as_ref(), app).await
        }

        // Logs
        Command::ConsoleLogs { lines, filter, level, pattern, window_id } => {
            handlers::console_logs(&id, lines, filter.as_deref(), pattern.as_deref(), level.as_deref(), &window_id, state).await
        }
        Command::ClearLogs { source } => {
            handlers::clear_logs(&id, &source, state).await
        }
        Command::ReadLogFile { source, lines, level, pattern, since, window_id } => {
            handlers::read_log_file(&id, &source, lines, level.as_deref(), pattern.as_deref(), since, window_id.as_deref(), state).await
        }

        // Event Capture
        Command::IpcListen { action, events } => {
            handlers::ipc_listen(&id, &action, events.as_deref(), state, bridge).await
        }
        Command::EventGetCaptured { event, pattern, limit, since } => {
            handlers::event_get_captured(&id, event.as_deref(), pattern.as_deref(), limit, since, state).await
        }

        // Search
        Command::SearchSnapshot { pattern, context, mode, window_id } => {
            handlers::search_snapshot(&id, &pattern, context, &mode, &window_id, state, bridge).await
        }
    }
}

async fn send_response<S>(
    ws_tx: &Arc<Mutex<S>>,
    response: &Response,
) -> Result<(), String>
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
