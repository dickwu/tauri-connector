//! Internal WebSocket bridge for reliable JS execution in the webview.
//!
//! Instead of relying on `window.__TAURI__` (which may not be available in all
//! WebKit content worlds), this bridge injects a small JS client that connects
//! back to the plugin via a dedicated internal WebSocket. Results are delivered
//! through this channel, completely bypassing Tauri's IPC layer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{BridgeCommand, BridgeResult};

type PendingMap = Arc<StdMutex<HashMap<String, PendingRequest>>>;

/// Receives page results that arrive after their caller stopped waiting.
pub type LateSettlementHook =
    Arc<dyn Fn(&str, crate::workflow::resources::LateOutcome) + Send + Sync>;

/// How long a timed-out request keeps listening for its page result.
const LATE_SETTLEMENT_WINDOW: Duration = Duration::from_secs(600);

fn late_outcome(
    result: &Result<serde_json::Value, BridgeError>,
) -> crate::workflow::resources::LateOutcome {
    use crate::workflow::resources::LateOutcome;
    match result {
        Ok(_) => LateOutcome::Succeeded,
        Err(BridgeError::ExecutionFailed { effect, .. })
            if *effect == connector_client::outcome::EffectStatus::None =>
        {
            LateOutcome::FailedWithoutEffect
        }
        Err(BridgeError::ExecutionFailed { .. }) => LateOutcome::Failed,
        Err(_) => LateOutcome::Lost,
    }
}

struct PendingRequest {
    conn_id: String,
    registration_id: uuid::Uuid,
    tx: oneshot::Sender<Result<serde_json::Value, BridgeError>>,
    /// The caller already timed out; the entry only waits for a late result
    /// so it can settle quarantine. It is not an in-flight request.
    awaiting_late_result: bool,
}

/// Transport failure classification; only `NotDispatched` permits fallback.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    #[error("not_dispatched: {reason} (requestId: {request_id})")]
    NotDispatched { request_id: String, reason: String },
    #[error("execution_failed: {error} (requestId: {request_id})")]
    ExecutionFailed {
        request_id: String,
        error: String,
        /// `None` only when the page observed no backend-capable dispatch
        /// while the script ran; a write that may have landed stays `Possible`.
        effect: connector_client::outcome::EffectStatus,
    },
    #[error("outcome_unknown: {reason} (requestId: {request_id}); remote execution may continue")]
    DispatchedOutcomeUnknown { request_id: String, reason: String },
}

impl BridgeError {
    pub fn request_id(&self) -> &str {
        match self {
            Self::NotDispatched { request_id, .. }
            | Self::ExecutionFailed { request_id, .. }
            | Self::DispatchedOutcomeUnknown { request_id, .. } => request_id,
        }
    }

    pub fn is_dispatched(&self) -> bool {
        !matches!(self, Self::NotDispatched { .. })
    }
}

/// Synchronous cleanup also runs when a suspended future is dropped. The maps
/// protected here contain only small bookkeeping operations, never awaited I/O.
struct CleanupGuard<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for CleanupGuard<F> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

struct DispatchRequest<'a> {
    id: &'a str,
    deadline: Instant,
    authorize: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}

impl DispatchRequest<'_> {
    fn not_dispatched(&self, reason: impl Into<String>) -> BridgeError {
        BridgeError::NotDispatched {
            request_id: self.id.to_string(),
            reason: reason.into(),
        }
    }

    fn unknown(&self, reason: impl Into<String>) -> BridgeError {
        BridgeError::DispatchedOutcomeUnknown {
            request_id: self.id.to_string(),
            reason: reason.into(),
        }
    }

    fn ensure_budget(&self) -> Result<(), BridgeError> {
        if self.authorize.is_some_and(|check| !check()) {
            return Err(self.not_dispatched("Host authorization was revoked before dispatch"));
        }
        if Instant::now() >= self.deadline {
            Err(self.not_dispatched("Execution deadline expired before dispatch"))
        } else {
            Ok(())
        }
    }
}
type ClientMap = Arc<Mutex<HashMap<String, BridgeClient>>>;
type ConnectionRegistry = Arc<StdMutex<HashMap<String, ConnectionState>>>;

#[derive(Clone, Default, PartialEq, Eq)]
struct ConnectionState {
    bridge_key: String,
    conn_id: Option<String>,
    // Retain disconnect revisions so a no-WS pin cannot revive after a socket
    // appears and disappears again, even if nobody checked the pin in between.
    revision: u64,
}

impl ConnectionState {
    fn set_connection(&mut self, conn_id: Option<String>) {
        if self.conn_id != conn_id {
            self.conn_id = conn_id;
            self.revision += 1;
        }
    }
}

/// A transport identity captured once for a host-authorized operation/session.
/// It also represents the legitimate no-WS eval route. Authorization may check
/// this synchronously while dispatch holds the async clients lock.
#[derive(Clone)]
pub(crate) struct BridgeConnectionPin {
    window_id: String,
    bridge_key: String,
    connection: ConnectionState,
    tx: Option<mpsc::UnboundedSender<String>>,
    connections: ConnectionRegistry,
}

impl BridgeConnectionPin {
    pub(crate) fn is_current(&self) -> bool {
        if self.tx.as_ref().is_some_and(|tx| tx.is_closed())
            || !crate::identity::verify_bridge_key(&self.window_id, &self.bridge_key)
        {
            return false;
        }
        self.connections
            .lock()
            .unwrap()
            .get(&self.window_id)
            .cloned()
            .unwrap_or_default()
            == self.connection
    }
}

#[derive(Clone)]
pub struct BridgeClient {
    pub window_id: String,
    pub connected_at_ms: u64,
    pub bridge_key: String,
    /// Identity of the underlying WS connection. A late disconnect of a stale
    /// socket must not remove a newer client registered under the same label.
    pub conn_id: String,
    pub tx: mpsc::UnboundedSender<String>,
    /// The page reported that its CSP blocks string compilation (no
    /// 'unsafe-eval'), so commands are delivered as native eval code.
    pub eval_blocked: bool,
}

/// Manages the internal WebSocket bridge to the webview.
#[derive(Clone)]
pub struct Bridge {
    /// Port the internal WebSocket listens on
    port: u16,
    /// Connected webview bridge clients, keyed by Tauri window label.
    clients: ClientMap,
    /// Synchronous identity mirror for authorization closures. Every mutation
    /// holds clients first, then this registry; no guard crosses awaited I/O.
    connections: ConnectionRegistry,
    /// Pending JS evaluation results, keyed by request ID
    pending: PendingMap,
    /// App handle for eval-based fallback execution
    app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    runtime: Arc<crate::runtime::RuntimeManager>,
    /// Lifts resource quarantine when a timed-out request settles later.
    late_hook: Arc<StdMutex<Option<LateSettlementHook>>>,
    /// Test seam: when set, native deliveries go here instead of a webview.
    native_sink: Arc<StdMutex<Option<mpsc::UnboundedSender<String>>>>,
}

/// Inclusive port range for the internal bridge listener.
const BRIDGE_PORT_RANGE: (u16, u16) = (9300, 9400);

impl Bridge {
    /// Start the internal bridge WebSocket server.
    /// Returns the Bridge handle and the port it's listening on.
    ///
    /// The listener is bound before returning, so a bind failure is reported
    /// to the caller instead of leaving a handle to a bridge nobody serves.
    /// Must be called from within a Tokio runtime.
    pub fn start() -> Result<Self, String> {
        let (std_listener, port) =
            crate::net::bind_in_range("127.0.0.1", BRIDGE_PORT_RANGE.0, BRIDGE_PORT_RANGE.1)?;
        let listener = TokioTcpListener::from_std(std_listener)
            .map_err(|e| format!("Failed to register bridge listener: {e}"))?;

        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let clients: ClientMap = Arc::new(Mutex::new(HashMap::new()));

        let bridge = Self {
            port,
            clients,
            connections: Arc::new(StdMutex::new(HashMap::new())),
            pending: pending.clone(),
            app_handle: Arc::new(Mutex::new(None)),
            runtime: Arc::new(crate::runtime::RuntimeManager::default()),
            late_hook: Arc::new(StdMutex::new(None)),
            native_sink: Arc::new(StdMutex::new(None)),
        };

        let bridge_clone = bridge.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge_clone.run_server(listener).await {
                eprintln!("[connector][bridge] Server error: {e}");
            }
        });

        println!("[connector][bridge] Internal bridge on port {port}");
        Ok(bridge)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Install the receiver for results that arrive after a caller timed out.
    pub fn set_late_settlement_hook(&self, hook: LateSettlementHook) {
        *self.late_hook.lock().unwrap() = Some(hook);
    }

    /// Keep waiting for a timed-out request's page result, then report it.
    fn watch_late_settlement(
        &self,
        request_id: &str,
        rx: oneshot::Receiver<Result<serde_json::Value, BridgeError>>,
        cleanup: impl FnOnce() + Send + 'static,
    ) {
        let hook = self.late_hook.lock().unwrap().clone();
        if let Some(entry) = self.pending.lock().unwrap().get_mut(request_id) {
            entry.awaiting_late_result = true;
        }
        let request_id = request_id.to_string();
        tokio::spawn(async move {
            let outcome = match tokio::time::timeout(LATE_SETTLEMENT_WINDOW, rx).await {
                Ok(Ok(result)) => late_outcome(&result),
                _ => crate::workflow::resources::LateOutcome::Lost,
            };
            cleanup();
            if let Some(hook) = hook {
                hook(&request_id, outcome);
            }
        });
    }

    /// Set the app handle for eval-based fallback JS execution.
    pub async fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    pub(crate) async fn pin_connection(&self, window_id: &str) -> BridgeConnectionPin {
        let clients = self.clients.lock().await;
        let client = clients.get(window_id);
        let bridge_key = client
            .map(|client| client.bridge_key.clone())
            .unwrap_or_else(|| crate::identity::bridge_key(window_id));
        let mut connections = self.connections.lock().unwrap();
        let connection = connections.entry(window_id.to_string()).or_default();
        if connection.bridge_key != bridge_key {
            // A recreated no-WS window starts a new transport lifetime even if
            // destruction cleanup for the old label has not acquired its lock.
            *connection = ConnectionState {
                bridge_key: bridge_key.clone(),
                conn_id: client.map(|client| client.conn_id.clone()),
                revision: 0,
            };
        }
        BridgeConnectionPin {
            window_id: window_id.to_string(),
            bridge_key,
            connection: connection.clone(),
            tx: client.map(|client| client.tx.clone()),
            connections: self.connections.clone(),
        }
    }

    /// Execute JavaScript; fallback is permitted only before dispatch.
    #[allow(dead_code)] // Retained as the legacy default-window convenience API.
    pub async fn execute_js(
        &self,
        script: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, String> {
        self.execute_js_for_window(script, timeout_ms, "main").await
    }

    /// Legacy adapter. New workflow callers retain the typed transport error.
    pub async fn execute_js_for_window(
        &self,
        script: &str,
        timeout_ms: u64,
        window_id: &str,
    ) -> Result<serde_json::Value, String> {
        // Preserve the historical single-window default only at this legacy
        // boundary. Typed workflow calls always require the requested label.
        let legacy_window = {
            let clients = self.clients.lock().await;
            if window_id == "main" && !clients.contains_key(window_id) && clients.len() == 1 {
                clients.keys().next().cloned()
            } else {
                None
            }
        };
        self.execute_js_for_window_typed(
            script,
            timeout_ms,
            legacy_window.as_deref().unwrap_or(window_id),
        )
        .await
        .map_err(|error| error.to_string())
    }

    pub async fn execute_js_for_window_typed(
        &self,
        script: &str,
        timeout_ms: u64,
        window_id: &str,
    ) -> Result<serde_json::Value, BridgeError> {
        let id = uuid::Uuid::new_v4().to_string();
        self.execute_js_for_window_with_request_id(script, timeout_ms, window_id, &id)
            .await
    }

    /// The workflow journal supplies the logical operation ID before dispatch.
    /// Reusing an ID is not an idempotency guarantee across page/app restarts.
    pub async fn execute_js_for_window_with_request_id(
        &self,
        script: &str,
        timeout_ms: u64,
        window_id: &str,
        request_id: &str,
    ) -> Result<serde_json::Value, BridgeError> {
        self.execute_js_authorized(script, timeout_ms, window_id, request_id, None)
            .await
    }

    pub async fn execute_js_authorized(
        &self,
        script: &str,
        timeout_ms: u64,
        window_id: &str,
        request_id: &str,
        authorize: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<serde_json::Value, BridgeError> {
        self.execute_page_script(
            &PageScript::trusted(script),
            timeout_ms,
            window_id,
            request_id,
            authorize,
        )
        .await
    }

    /// Whether the window's page reported a CSP that blocks string
    /// compilation. `None` when no bridge client is connected.
    pub async fn eval_blocked(&self, window_id: &str) -> Option<bool> {
        self.clients
            .lock()
            .await
            .get(window_id)
            .map(|client| client.eval_blocked)
    }

    /// Run a script that has several page forms (see [`PageScript`]).
    pub async fn execute_page_script(
        &self,
        script: &PageScript,
        timeout_ms: u64,
        window_id: &str,
        request_id: &str,
        authorize: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<serde_json::Value, BridgeError> {
        let request = DispatchRequest {
            id: request_id,
            deadline: Instant::now() + Duration::from_millis(timeout_ms),
            authorize,
        };
        match self.execute_js_ws(script, window_id, &request).await {
            Err(BridgeError::NotDispatched { .. }) => {
                // The same ID and absolute deadline cover both transports.
                request.ensure_budget()?;
                self.execute_js_via_eval(script, window_id, &request).await
            }
            result => result,
        }
    }

    async fn execute_js_ws(
        &self,
        script: &PageScript,
        window_id: &str,
        request: &DispatchRequest<'_>,
    ) -> Result<serde_json::Value, BridgeError> {
        request.ensure_budget()?;
        let clients = tokio::time::timeout_at(request.deadline, self.clients.lock())
            .await
            .map_err(|_| {
                request.not_dispatched("Deadline expired while selecting bridge client")
            })?;
        let client = clients.get(window_id).cloned().ok_or_else(|| {
            request.not_dispatched(format!(
                "Bridge client for window '{window_id}' is not connected"
            ))
        })?;
        if !crate::identity::verify_bridge_key(window_id, &client.bridge_key) {
            return Err(request.not_dispatched("Bridge belongs to a previous document or window"));
        }
        // Keep the registry lock through enqueue: disconnect cleanup cannot miss
        // a request inserted for a connection that it has already removed.
        let cmd = BridgeCommand {
            id: request.id.to_string(),
            script: script.ws.clone(),
        };
        let msg = serde_json::to_string(&cmd)
            .map_err(|error| request.not_dispatched(error.to_string()))?;
        // A strict CSP blocks the page from compiling the command string, so
        // it is delivered as native eval code; the reply still uses this socket.
        let native = client
            .eval_blocked
            .then(|| native_scripts(script, request.id, &NativeReply::Bridge));
        let sink = self.native_sink.lock().unwrap().clone();
        let app = if native.is_some() && sink.is_none() {
            tokio::time::timeout_at(request.deadline, self.app_handle.lock())
                .await
                .map_err(|_| request.not_dispatched("Deadline expired while selecting webview"))?
                .clone()
        } else {
            None
        };
        let window = match (&native, &sink, &app) {
            (Some(_), None, Some(app)) => {
                use tauri::Manager;
                Some(app.get_webview_window(window_id).ok_or_else(|| {
                    request.not_dispatched(format!("Window '{window_id}' not found"))
                })?)
            }
            (Some(_), None, None) => {
                return Err(request.not_dispatched("App handle not set for native delivery"));
            }
            _ => None,
        };
        request.ensure_budget()?;
        let (tx, mut rx) = oneshot::channel();
        let registration_id = uuid::Uuid::new_v4();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.contains_key(request.id) {
                return Err(request.unknown("A request with this ID is already pending"));
            }
            pending.insert(
                request.id.to_string(),
                PendingRequest {
                    conn_id: client.conn_id,
                    registration_id,
                    tx,
                    awaiting_late_result: false,
                },
            );
        }
        let mut waiter = CleanupGuard(Some(|| {
            let mut pending = self.pending.lock().unwrap();
            if pending
                .get(request.id)
                .is_some_and(|entry| entry.registration_id == registration_id)
            {
                pending.remove(request.id);
            }
        }));
        // A rejected enqueue proves this command never entered the transport.
        request.ensure_budget()?;
        if let Some(scripts) = native {
            let transmitted_bytes = scripts.iter().map(String::len).sum();
            for js in &scripts {
                let delivered = match (&sink, &window) {
                    (Some(sink), _) => sink.send(js.clone()).map_err(|e| e.to_string()),
                    (None, Some(window)) => window.eval(js).map_err(|e| e.to_string()),
                    (None, None) => Err("no native delivery target".into()),
                };
                // The webview may already hold part of the sequence; never replay.
                delivered.map_err(|e| request.unknown(format!("eval inject failed: {e}")))?;
            }
            self.runtime
                .record_transmission(&script.ws, transmitted_bytes);
        } else {
            let transmitted_bytes = msg.len();
            client
                .tx
                .send(msg)
                .map_err(|_| request.not_dispatched("Bridge client channel closed"))?;
            self.runtime
                .record_transmission(&script.ws, transmitted_bytes);
        }
        drop(clients);
        // Once queued, even a socket send failure cannot safely trigger replay.
        match tokio::time::timeout_at(request.deadline, &mut rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(request.unknown("Bridge response channel closed")),
            Err(_) => {
                // The page may still finish; keep the pending entry so its
                // result can settle the request (and lift quarantine) later.
                waiter.0.take();
                let pending = self.pending.clone();
                let id = request.id.to_string();
                self.watch_late_settlement(request.id, rx, move || {
                    let mut pending = pending.lock().unwrap();
                    if pending
                        .get(&id)
                        .is_some_and(|entry| entry.registration_id == registration_id)
                    {
                        pending.remove(&id);
                    }
                });
                Err(request.unknown("WS bridge timeout"))
            }
        }
    }

    async fn execute_js_via_eval(
        &self,
        script: &PageScript,
        window_id: &str,
        request: &DispatchRequest<'_>,
    ) -> Result<serde_json::Value, BridgeError> {
        use tauri::{Listener, Manager};

        request.ensure_budget()?;
        let app = tokio::time::timeout_at(request.deadline, self.app_handle.lock())
            .await
            .map_err(|_| request.not_dispatched("Deadline expired while preparing eval fallback"))?
            .clone()
            .ok_or_else(|| request.not_dispatched("App handle not set for eval fallback"))?;
        let window = app
            .get_webview_window(window_id)
            .ok_or_else(|| request.not_dispatched(format!("Window '{window_id}' not found")))?;
        let id = request.id.to_string();
        // Keep the event name valid even when a caller uses a non-UUID ID.
        let event_name = format!("connector-eval-{}", uuid::Uuid::new_v4());
        let (tx, mut rx) = oneshot::channel::<Result<serde_json::Value, BridgeError>>();
        let tx = StdMutex::new(Some(tx));
        let expected_id = id.clone();
        let listener_id = app.listen(&event_name, move |event| {
            let payload_str = event.payload();
            let inner = serde_json::from_str::<String>(payload_str)
                .unwrap_or_else(|_| payload_str.to_string());
            let result = match serde_json::from_str::<BridgeResult>(&inner) {
                Ok(result) if result.id == expected_id => {
                    let effect = result.reported_effect();
                    if let Some(error) = result.error {
                        Err(BridgeError::ExecutionFailed {
                            request_id: expected_id.clone(),
                            error,
                            effect,
                        })
                    } else {
                        Ok(result.result.unwrap_or(serde_json::Value::Null))
                    }
                }
                Ok(_) => return,
                Err(_) => Err(BridgeError::DispatchedOutcomeUnknown {
                    request_id: expected_id.clone(),
                    reason: "Invalid eval result payload".into(),
                }),
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });
        let mut listener = CleanupGuard(Some(|| app.unlisten(listener_id)));
        let scripts = native_scripts(script, &id, &NativeReply::Event(&event_name));
        let sink = self.native_sink.lock().unwrap().clone();
        self.enqueue_eval(request, || {
            self.runtime
                .record_transmission(&script.ws, scripts.iter().map(String::len).sum());
            for js in &scripts {
                match &sink {
                    Some(sink) => sink.send(js.clone()).map_err(|e| e.to_string())?,
                    None => window.eval(js).map_err(|error| error.to_string())?,
                }
            }
            Ok(())
        })
        .await?;
        match tokio::time::timeout_at(request.deadline, &mut rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(request.unknown("Eval result channel closed")),
            Err(_) => {
                listener.0.take();
                let app = app.clone();
                self.watch_late_settlement(request.id, rx, move || app.unlisten(listener_id));
                Err(request.unknown("Script execution timeout (eval path)"))
            }
        }
    }

    async fn enqueue_eval(
        &self,
        request: &DispatchRequest<'_>,
        inject: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), BridgeError> {
        let clients = tokio::time::timeout_at(request.deadline, self.clients.lock())
            .await
            .map_err(|_| request.not_dispatched("Deadline expired before eval dispatch"))?;
        // Serialize final authorization and synchronous injection with hello /
        // disconnect, just as WS enqueue does. Never hold this while waiting.
        request.ensure_budget()?;
        // The platform eval API may fail after handing work to the webview.
        // Conservatively retain uncertainty; never replay after this boundary.
        let result =
            inject().map_err(|error| request.unknown(format!("eval inject failed: {error}")));
        drop(clients);
        result
    }

    async fn runtime_target(
        &self,
        window_id: &str,
    ) -> Result<crate::runtime::RuntimeTarget, BridgeError> {
        use tauri::Manager;
        let failure = |reason: &str| BridgeError::NotDispatched {
            request_id: "runtime_target".into(),
            reason: reason.into(),
        };
        let app = self
            .app_handle
            .lock()
            .await
            .clone()
            .ok_or_else(|| failure("Host identity is unavailable"))?;
        let window = app
            .get_webview_window(window_id)
            .ok_or_else(|| failure("Target window is unavailable"))?;
        if crate::identity::page_loading(window_id) {
            return Err(failure("Target document is navigating"));
        }
        let url = window
            .url()
            .map_err(|_| failure("Cannot validate current origin"))?;
        if !crate::identity::origin_allowed(&app, &url) {
            return Err(failure("Current origin is outside host configuration"));
        }
        Ok(crate::runtime::RuntimeTarget {
            app_id: app.config().identifier.clone(),
            window_id: window_id.into(),
            window_instance_id: crate::identity::window_instance_id(window_id),
            origin: url.origin().ascii_serialization(),
            url: url.to_string(),
        })
    }

    pub async fn execute_runtime(
        &self,
        window_id: &str,
        module: &str,
        args: serde_json::Value,
        timeout_ms: u64,
        request_id: &str,
    ) -> Result<serde_json::Value, BridgeError> {
        self.execute_runtime_authorized(window_id, module, args, timeout_ms, request_id, &|| true)
            .await
    }

    pub async fn execute_runtime_authorized(
        &self,
        window_id: &str,
        module: &str,
        args: serde_json::Value,
        timeout_ms: u64,
        request_id: &str,
        authorize: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<serde_json::Value, BridgeError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let target = tokio::time::timeout_at(deadline, self.runtime_target(window_id))
            .await
            .map_err(|_| BridgeError::NotDispatched {
                request_id: request_id.into(),
                reason: "Host target lookup deadline expired".into(),
            })??;
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        self.runtime
            .execute_authorized(
                self,
                &target,
                module,
                args,
                remaining,
                request_id,
                Some(authorize),
            )
            .await
    }

    pub async fn runtime_context(
        &self,
        window_id: &str,
        timeout_ms: u64,
    ) -> Result<crate::identity::ExecutionContext, BridgeError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let target = tokio::time::timeout_at(deadline, self.runtime_target(window_id))
            .await
            .map_err(|_| BridgeError::NotDispatched {
                request_id: request_id.clone(),
                reason: "Host target lookup deadline expired".into(),
            })??;
        self.runtime
            .ensure(self, &target, deadline, &request_id)
            .await
    }

    pub async fn probe_bridge(
        &self,
        window_id: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, BridgeError> {
        self.probe_page(window_id,timeout_ms,"/*__CONNECTOR_RUNTIME_COMMAND__*/({responsive:window.top===window&&window.__CONNECTOR_BRIDGE__===true})").await
    }

    /// Does not install, repair, reload, or replay any business operation.
    pub async fn probe_runtime(
        &self,
        window_id: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, BridgeError> {
        self.probe_page(window_id, timeout_ms, crate::runtime::PROBE)
            .await
    }

    async fn probe_page(
        &self,
        window_id: &str,
        timeout_ms: u64,
        script: &str,
    ) -> Result<serde_json::Value, BridgeError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        tokio::time::timeout_at(deadline, self.runtime_target(window_id))
            .await
            .map_err(|_| BridgeError::NotDispatched {
                request_id: request_id.clone(),
                reason: "Host probe lookup deadline expired".into(),
            })??;
        self.execute_js_for_window_with_request_id(
            script,
            crate::runtime::remaining(deadline, &request_id)?,
            window_id,
            &request_id,
        )
        .await
    }

    pub async fn bridge_connected(&self, window_id: &str) -> bool {
        self.clients
            .lock()
            .await
            .get(window_id)
            .is_some_and(|client| {
                !client.tx.is_closed()
                    && crate::identity::verify_bridge_key(window_id, &client.bridge_key)
            })
    }

    pub fn window_destroyed(&self, window_id: &str) {
        self.runtime.window_destroyed(window_id);
        // lib.rs removes the host identity before this callback. Cleanup may
        // wait for dispatch, so check the stored key again before removing a
        // label that could already belong to a recreated window.
        let bridge = self.clone();
        let window_id = window_id.to_string();
        tauri::async_runtime::spawn(async move {
            bridge.remove_destroyed_window(&window_id).await;
        });
    }

    async fn remove_destroyed_window(&self, window_id: &str) {
        let mut clients = self.clients.lock().await;
        let mut connections = self.connections.lock().unwrap();
        if connections.get(window_id).is_some_and(|connection| {
            !crate::identity::verify_bridge_key(window_id, &connection.bridge_key)
        }) {
            clients.remove(window_id);
            connections.remove(window_id);
        }
    }

    pub fn runtime_diagnostics(&self) -> serde_json::Value {
        self.runtime.diagnostics()
    }

    pub async fn status(&self) -> serde_json::Value {
        let clients = self.clients.lock().await;
        let now = now_ms();
        let list: Vec<serde_json::Value> = clients
            .values()
            .filter(|client| {
                crate::identity::verify_bridge_key(&client.window_id, &client.bridge_key)
            })
            .map(|client| {
                serde_json::json!({
                    "windowId": client.window_id,
                    "connected": true,
                    "evalBlocked": client.eval_blocked,
                    "ageMs": now.saturating_sub(client.connected_at_ms),
                })
            })
            .collect();
        let (awaiting_late_result, pending) = {
            let entries = self.pending.lock().unwrap();
            let late = entries.values().filter(|e| e.awaiting_late_result).count();
            (late, entries.len() - late)
        };
        serde_json::json!({
            "bridge_port": self.port,
            "workflowProtocolVersion": 1,
            "inspectionProtocolVersion": 1,
            "inspection": crate::capabilities::inspection(),
            "clients": list,
            "pending": pending,
            "awaitingLateResult": awaiting_late_result,
            "fallbackAvailable": self.app_handle.lock().await.is_some(),
        })
    }

    async fn run_server(&self, listener: TokioTcpListener) -> Result<(), String> {
        loop {
            let (stream, addr) = listener.accept().await.map_err(|e| e.to_string())?;
            println!("[connector][bridge] Webview client connected from {addr}");
            let clients = self.clients.clone();
            let connections = self.connections.clone();
            let pending = self.pending.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_bridge_client(stream, clients, connections, pending).await {
                    eprintln!("[connector][bridge] Client error: {e}");
                }
            });
        }
    }
}

async fn handle_bridge_client(
    stream: tokio::net::TcpStream,
    clients: ClientMap,
    connections: ConnectionRegistry,
    pending: PendingMap,
) -> Result<(), String> {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| e.to_string())?;
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut window_id: Option<String> = None;
    let conn_id = uuid::Uuid::new_v4().to_string();
    let _connection = CleanupGuard(Some(|| {
        reject_connection_pending(&pending, &conn_id);
    }));

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                match outbound {
                    Some(msg) => {
                        if ws_write.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            inbound = ws_read.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(new_window_id) = handle_bridge_message(
                            &text,
                            &tx,
                            &clients,
                            &connections,
                            &pending,
                            &conn_id,
                        ).await {
                            window_id = Some(new_window_id);
                        }
                    }
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }
    }

    if let Some(window_id) = window_id {
        if disconnect_bridge_client(&clients, &connections, &window_id, &conn_id).await {
            println!("[connector][bridge] Webview client disconnected: {window_id}");
        } else {
            println!(
                "[connector][bridge] Stale connection closed for {window_id}; keeping newer client"
            );
        }
    } else {
        println!("[connector][bridge] Webview client disconnected before hello");
    }

    Ok(())
}

async fn disconnect_bridge_client(
    clients: &ClientMap,
    connections: &ConnectionRegistry,
    window_id: &str,
    conn_id: &str,
) -> bool {
    let mut clients = clients.lock().await;
    if !clients
        .get(window_id)
        .is_some_and(|client| client.conn_id == conn_id)
    {
        return false;
    }
    let mut connections = connections.lock().unwrap();
    clients.remove(window_id);
    connections
        .entry(window_id.to_string())
        .or_default()
        .set_connection(None);
    true
}

async fn handle_bridge_message(
    text: &str,
    tx: &mpsc::UnboundedSender<String>,
    clients: &ClientMap,
    connections: &ConnectionRegistry,
    pending: &PendingMap,
    conn_id: &str,
) -> Option<String> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[connector][bridge] Invalid JSON message: {e}");
            return None;
        }
    };

    if value.get("type").and_then(|v| v.as_str()) == Some("hello")
        || value.get("id").and_then(|v| v.as_str()) == Some("__bridge_hello__")
    {
        let window_id = value
            .get("windowId")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string();
        if !crate::identity::verify_bridge_key(
            &window_id,
            value
                .get("bridgeKey")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        ) {
            return None;
        }
        let client = BridgeClient {
            window_id: window_id.clone(),
            connected_at_ms: now_ms(),
            bridge_key: value["bridgeKey"].as_str().unwrap_or_default().into(),
            conn_id: conn_id.to_string(),
            tx: tx.clone(),
            eval_blocked: value
                .get("evalBlocked")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        };
        let mut clients = clients.lock().await;
        // Destruction/navigation may happen while hello waits for dispatch.
        if !crate::identity::verify_bridge_key(&window_id, &client.bridge_key) {
            return None;
        }
        let mut connections = connections.lock().unwrap();
        let connection = connections.entry(window_id.clone()).or_default();
        connection.bridge_key = client.bridge_key.clone();
        connection.set_connection(Some(conn_id.to_string()));
        let client_eval_blocked = client.eval_blocked;
        clients.insert(window_id.clone(), client);
        if client_eval_blocked {
            println!(
                "[connector][bridge] Registered window bridge: {window_id} (CSP blocks eval; using native delivery)"
            );
        } else {
            println!("[connector][bridge] Registered window bridge: {window_id}");
        }
        return Some(window_id);
    }

    match serde_json::from_value::<BridgeResult>(value) {
        Ok(result) => {
            let mut pending = pending.lock().unwrap();
            // A stale socket must never resolve a newer connection's request.
            if pending
                .get(&result.id)
                .is_some_and(|request| request.conn_id == conn_id)
            {
                let request = pending
                    .remove(&result.id)
                    .expect("request checked under same lock");
                let effect = result.reported_effect();
                let value = if let Some(error) = result.error {
                    Err(BridgeError::ExecutionFailed {
                        request_id: result.id,
                        error,
                        effect,
                    })
                } else {
                    Ok(result.result.unwrap_or(serde_json::Value::Null))
                };
                let _ = request.tx.send(value);
            }
        }
        Err(e) => {
            eprintln!("[connector][bridge] Invalid result message: {e}");
        }
    }

    None
}

fn reject_connection_pending(pending: &PendingMap, conn_id: &str) {
    let mut requests = pending.lock().unwrap();
    let ids: Vec<_> = requests
        .iter()
        .filter(|(_, request)| request.conn_id == conn_id)
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        if let Some(request) = requests.remove(&id) {
            let _ = request.tx.send(Err(BridgeError::DispatchedOutcomeUnknown {
                request_id: id,
                reason: "Bridge connection closed".into(),
            }));
        }
    }
}

/// One script in the forms the page can run it in.
///
/// Over the WebSocket the page compiles `ws` with `AsyncFunction`, which a
/// Content Security Policy without `'unsafe-eval'` blocks. Native delivery
/// (`webview.eval`, not subject to CSP) instead inlines each candidate as
/// code, one eval per candidate in order; the first that parses runs, and a
/// final step reports a syntax error with no effect when none parsed.
#[derive(Clone, Debug)]
pub struct PageScript {
    ws: String,
    candidates: Vec<InlineCandidate>,
}

#[derive(Clone, Debug)]
struct InlineCandidate {
    /// An expression; inlined as `async function(){ return (<expression>); }`.
    expression: String,
    /// Only usable when the page may compile strings (it calls eval itself).
    needs_eval: bool,
}

impl PageScript {
    /// A connector-built expression; the same source works in every form.
    pub fn trusted(script: &str) -> Self {
        Self {
            ws: script.to_string(),
            candidates: vec![InlineCandidate {
                expression: script.to_string(),
                needs_eval: false,
            }],
        }
    }

    /// `ws` compiles strings in the page; it is also the first native
    /// candidate when eval is allowed. `inline` are CSP-safe fallbacks.
    pub fn with_inline(ws: String, inline: Vec<String>) -> Self {
        let mut candidates = vec![InlineCandidate {
            expression: ws.clone(),
            needs_eval: true,
        }];
        candidates.extend(inline.into_iter().map(|expression| InlineCandidate {
            expression,
            needs_eval: false,
        }));
        Self { ws, candidates }
    }
}

/// Where a natively delivered script sends its result.
enum NativeReply<'a> {
    /// The page bridge's `__CONNECTOR_RUN__`, replying on its WebSocket.
    Bridge,
    /// A self-contained runner replying through a Tauri event.
    Event(&'a str),
}

/// Native delivery scripts for tests; `event` selects the event runner.
#[cfg(test)]
pub(crate) fn test_native_scripts(
    script: &PageScript,
    id: &str,
    event: Option<&str>,
) -> Vec<String> {
    match event {
        Some(event) => native_scripts(script, id, &NativeReply::Event(event)),
        None => native_scripts(script, id, &NativeReply::Bridge),
    }
}

/// Node harness simulating a page whose CSP blocks string compilation when
/// `strings` is false. Input: JSON `[{id, strings, scripts}]` on stdin.
/// Each script is evaluated separately, like `webview.eval`; parse errors
/// are skipped. Output: JSON map id -> reply payload.
#[cfg(test)]
pub(crate) const NATIVE_HARNESS: &str = r#"
const vm = require('vm');
const cases = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const out = {};
const waits = [];
for (const c of cases) {
  const sandbox = { setTimeout, clearTimeout, Promise, console };
  sandbox.window = sandbox;
  sandbox.__CONNECTOR_EFFECTS__ = { mark() { return 0; }, unchangedSince() { return true; } };
  waits.push(new Promise((resolve) => {
    sandbox.__CONNECTOR_RUN__ = function (id, fn, o) {
      Promise.resolve().then(fn).then(
        (r) => resolve(out[c.id] = { id, result: r === undefined ? null : r }),
        (e) => resolve(out[c.id] = { id, error: String(e && e.message || e), effect: o || 'none' }));
    };
    sandbox.__TAURI_INTERNALS__ = { invoke(cmd, args) {
      const p = JSON.parse(args.payload);
      resolve(out[c.id] = Object.assign({ event: args.event, result: null }, p));
    } };
    setTimeout(() => resolve(out[c.id] = out[c.id] || { timeout: true }), 2000).unref();
  }));
  const ctx = vm.createContext(sandbox, { codeGeneration: { strings: c.strings, wasm: false } });
  for (const script of c.scripts) {
    try { vm.runInContext(script, ctx); } catch (e) { if (!e || e.name !== 'SyntaxError') throw e; }
  }
}
Promise.all(waits).then(() => { process.stdout.write(JSON.stringify(out)); process.exit(0); });
"#;

/// Run [`NATIVE_HARNESS`]; `None` when node is unavailable.
#[cfg(test)]
pub(crate) fn run_native_harness(cases: &serde_json::Value) -> Option<serde_json::Value> {
    use std::io::Write;
    let mut child = std::process::Command::new("node")
        .arg("-e")
        .arg(NATIVE_HARNESS)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(cases.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(serde_json::from_slice(&output.stdout).unwrap())
}

/// Error reported, with no effect, when no candidate form parsed.
pub const PAGE_SCRIPT_SYNTAX: &str = "page_script_syntax";

/// Native eval scripts for one request: one per candidate, then a final step.
/// Pass code as code and identity as JSON data; template literals are never
/// used, so `${...}` in a script is not interpolated.
fn native_scripts(script: &PageScript, id: &str, reply: &NativeReply<'_>) -> Vec<String> {
    let id = serde_json::to_string(id).expect("strings serialize");
    let (runner, ready) = match reply {
        NativeReply::Bridge => (
            "window.__CONNECTOR_RUN__".to_string(),
            "if(typeof window.__CONNECTOR_RUN__!=='function')return;",
        ),
        NativeReply::Event(event) => {
            let event = serde_json::to_string(event).expect("strings serialize");
            (
                format!(
                    r#"(function(id,fn,o){{var ev={event};function send(p){{var t=window.__TAURI_INTERNALS__;if(t)t.invoke('plugin:event|emit',{{event:ev,payload:JSON.stringify(p)}});}}var fx=window.__CONNECTOR_EFFECTS__,mark=fx?fx.mark():0;Promise.resolve().then(fn).then(function(r){{var v;try{{JSON.stringify(r);v=r}}catch(_){{v=String(r)}}send({{id:id,result:v}});}},function(e){{send({{id:id,error:(e&&e.message)||String(e),effect:o||(fx&&fx.unchangedSince(mark)?'none':'possible')}});}});}})"#
                ),
                "",
            )
        }
    };
    let registry = "var g=window.__CONNECTOR_PARSED__;if(!g){g=new Set();try{Object.defineProperty(window,'__CONNECTOR_PARSED__',{value:g,configurable:true})}catch(_){window.__CONNECTOR_PARSED__=g}}";
    let blocked = "var b=window.__CONNECTOR_EVAL_BLOCKED__;if(b===undefined){try{Function('');b=false}catch(_){b=true}}if(b)return;";
    let skip_eval = matches!(reply, NativeReply::Bridge);
    let mut scripts: Vec<String> = script
        .candidates
        .iter()
        // A bridge that reported a blocking CSP cannot use compiling forms.
        .filter(|candidate| !(skip_eval && candidate.needs_eval))
        .map(|candidate| {
            let guard = if candidate.needs_eval { blocked } else { "" };
            format!(
                "(function(){{var id={id};{registry}if(g.has(id))return;{guard}{ready}g.add(id);{runner}(id,async function(){{return (\n{}\n);}});}})()",
                candidate.expression
            )
        })
        .collect();
    let message = serde_json::to_string(&format!(
        "{PAGE_SCRIPT_SYNTAX}: the script could not be parsed as an expression or an async function body (native delivery compiles scripts as code; under a Content Security Policy without 'unsafe-eval', multi-statement scripts need an explicit `return`)"
    ))
    .expect("strings serialize");
    scripts.push(format!(
        "(function(){{var id={id};var g=window.__CONNECTOR_PARSED__;if(g&&g.has(id)){{g.delete(id);return;}}{ready}{runner}(id,function(){{throw new SyntaxError({message});}},'none');}})()"
    ));
    scripts
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Generate the JavaScript bridge client code that gets injected into the webview.
pub fn bridge_init_script(port: u16, window_id: &str) -> String {
    let capture_source = include_str!("capture/page.js");
    let bridge_key = serde_json::to_string(&crate::identity::bridge_key(window_id))
        .expect("bridge key serializes");
    let window_id = serde_json::to_string(window_id).expect("window label serializes");
    let semantic_source = include_str!("semantic/core.js");
    let effects_source = include_str!("effects.js");
    let console_format_source = include_str!("../js/console-format.js");
    format!(
        r#"(function() {{
  if (window.top !== window) return;
  if (window.__CONNECTOR_BRIDGE__) {{
    window.__CONNECTOR_BRIDGE_REBIND__?.({bridge_key});
    return;
  }}
  window.__CONNECTOR_BRIDGE__ = true;

  // Capture native WebSocket before frameworks (Next.js/Turbopack HMR) can patch it
  const NativeWebSocket = window.WebSocket;

  const BRIDGE_PORT = {port};
  const WINDOW_ID = {window_id};
  // A CSP without 'unsafe-eval' blocks string compilation; the plugin then
  // delivers commands as native eval code that calls __CONNECTOR_RUN__.
  const EVAL_BLOCKED = (function() {{
    try {{ return Function('return 1')() !== 1; }} catch (_) {{ return true; }}
  }})();
  try {{
    Object.defineProperty(window, '__CONNECTOR_EVAL_BLOCKED__', {{ value: EVAL_BLOCKED, configurable: true }});
  }} catch (_) {{}}
  let BRIDGE_KEY = {bridge_key};
  let ws = null;
  let reconnectTimer = null;
  const consoleLogs = [];
  const MAX_LOGS = 500;
  let consoleSeq = 0;

  // Rejected scripts release their lease only with this page evidence.
  const effectMonitor = ({effects_source})(window, NativeWebSocket, function(socket) {{ return socket === ws; }});
  try {{
    Object.defineProperty(window, '__CONNECTOR_EFFECTS__', {{ value: effectMonitor }});
  }} catch (_) {{}}

  // Intercept console methods for log capture
  const formatConsole = {console_format_source};
  const origConsole = {{}};
  ['log', 'warn', 'error', 'info', 'debug'].forEach(function(level) {{
    origConsole[level] = console[level];
    console[level] = function() {{
      let message;
      try {{ message = formatConsole(arguments); }}
      catch (_) {{ message = Array.from(arguments).map(String).join(' '); }}
      consoleLogs.push({{
        level: level,
        message: message,
        timestamp: Date.now(),
        seq: ++consoleSeq
      }});
      if (consoleLogs.length > MAX_LOGS) consoleLogs.shift();
      origConsole[level].apply(console, arguments);
    }};
  }});

  // Expose logs for retrieval
  window.__CONNECTOR_LOGS__ = consoleLogs;

  // === Runtime Capture ===
  window.__CONNECTOR_RUNTIME__ = window.__CONNECTOR_RUNTIME__ || [];
  const runtimeEntries = window.__CONNECTOR_RUNTIME__;
  function runtimeIpc() {{
    return window.__CONNECTOR_ORIG_INVOKE__ ||
      (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) ||
      (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
  }}
  function captureRuntime(kind, level, message, data) {{
    const entry = {{
      kind: kind,
      level: level || 'info',
      message: String(message || ''),
      timestamp: Date.now(),
      windowId: WINDOW_ID,
      data: data || {{}}
    }};
    runtimeEntries.push(entry);
    if (runtimeEntries.length > 1000) runtimeEntries.shift();
    const ipc = runtimeIpc();
    if (ipc) {{
      ipc('plugin:connector|push_runtime', {{ payload: entry }}).catch(function(){{}});
    }}
  }}
  if (!window.__CONNECTOR_RUNTIME_LISTENERS__) {{
    window.__CONNECTOR_RUNTIME_LISTENERS__ = true;
    window.addEventListener('error', function(e) {{
      const target = e.target;
      if (target && target !== window && (target.tagName || target.src || target.href)) {{
        captureRuntime('resource_error', 'error', 'Resource failed to load', {{
          tag: target.tagName || '',
          src: target.src || target.href || '',
          outerHTML: target.outerHTML ? target.outerHTML.substring(0, 500) : ''
        }});
        return;
      }}
      captureRuntime('window_error', 'error', e.message || 'window error', {{
        filename: e.filename || '',
        lineno: e.lineno || 0,
        colno: e.colno || 0,
        stack: e.error && e.error.stack ? String(e.error.stack) : ''
      }});
    }}, true);
    window.addEventListener('unhandledrejection', function(e) {{
      const reason = e.reason || {{}};
      captureRuntime('unhandledrejection', 'error', reason.message || String(reason), {{
        stack: reason.stack ? String(reason.stack) : '',
        reason: typeof reason === 'object' ? String(reason) : reason
      }});
    }});
    window.addEventListener('popstate', function() {{
      captureRuntime('navigation', 'info', 'popstate', {{ url: String(location.href || '') }});
    }});
    window.addEventListener('hashchange', function() {{
      captureRuntime('navigation', 'info', 'hashchange', {{ url: String(location.href || '') }});
    }});
  }}
  if (typeof window.fetch === 'function' && !window.__CONNECTOR_FETCH_WRAPPED__) {{
    window.__CONNECTOR_FETCH_WRAPPED__ = true;
    const origFetch = window.fetch;
    window.fetch = async function(input, init) {{
      const t0 = Date.now();
      const url = typeof input === 'string' ? input : (input && input.url) || '';
      try {{
        const res = await origFetch.apply(this, arguments);
        if (!res.ok) {{
          captureRuntime('network', 'warn', 'fetch returned HTTP ' + res.status, {{
            url: String(url),
            status: res.status,
            ok: res.ok,
            durationMs: Date.now() - t0
          }});
        }}
        return res;
      }} catch (err) {{
        captureRuntime('network', 'error', err && err.message ? err.message : String(err), {{
          url: String(url),
          durationMs: Date.now() - t0
        }});
        throw err;
      }}
    }};
  }}
  if (window.XMLHttpRequest && !window.__CONNECTOR_XHR_WRAPPED__) {{
    window.__CONNECTOR_XHR_WRAPPED__ = true;
    const OrigXHR = window.XMLHttpRequest;
    window.XMLHttpRequest = function() {{
      const xhr = new OrigXHR();
      let method = '';
      let url = '';
      let t0 = 0;
      const origOpen = xhr.open;
      xhr.open = function(m, u) {{
        method = m || '';
        url = u || '';
        return origOpen.apply(xhr, arguments);
      }};
      const origSend = xhr.send;
      xhr.send = function() {{
        t0 = Date.now();
        xhr.addEventListener('loadend', function() {{
          if (xhr.status >= 400 || xhr.status === 0) {{
            captureRuntime('network', xhr.status === 0 ? 'error' : 'warn', 'xhr completed with status ' + xhr.status, {{
              method: method,
              url: String(url),
              status: xhr.status,
              durationMs: Date.now() - t0
            }});
          }}
        }});
        xhr.addEventListener('error', function() {{
          captureRuntime('network', 'error', 'xhr network error', {{
            method: method,
            url: String(url),
            durationMs: Date.now() - t0
          }});
        }});
        return origSend.apply(xhr, arguments);
      }};
      return xhr;
    }};
  }}
  ['pushState', 'replaceState'].forEach(function(name) {{
    if (!history[name].__connectorWrapped) {{
      const orig = history[name];
      const wrapped = function() {{
        const result = orig.apply(this, arguments);
        captureRuntime('navigation', 'info', name, {{ url: String(location.href || '') }});
        return result;
      }};
      wrapped.__connectorWrapped = true;
      history[name] = wrapped;
    }}
  }});
  // One shared transparent hook feeds protected v2 sessions and the legacy projection.
  ({capture_source})(window);

  function connect() {{
    try {{
      ws = new NativeWebSocket('ws://127.0.0.1:' + BRIDGE_PORT);
    }} catch (e) {{
      scheduleReconnect();
      return;
    }}

    ws.onopen = function() {{
      origConsole.log('[connector] Bridge connected on port ' + BRIDGE_PORT);
      sendHello();
    }};

    ws.onmessage = function(event) {{
      let cmd;
      try {{
        cmd = JSON.parse(typeof event.data === 'string' ? event.data : '');
      }} catch (e) {{
        return;
      }}

      executeCommand(cmd);
    }};

    ws.onclose = function() {{
      origConsole.log('[connector] Bridge disconnected, reconnecting...');
      scheduleReconnect();
    }};

    ws.onerror = function() {{
      // onclose will fire after this
    }};
  }}

  function sendHello() {{
    try {{
      ws.send(JSON.stringify({{type:'hello',bridgeKey:BRIDGE_KEY,id:'__bridge_hello__',windowId:WINDOW_ID,evalBlocked:EVAL_BLOCKED}}));
    }} catch (_) {{}}
  }}
  window.__CONNECTOR_BRIDGE_REBIND__ = function(key) {{
    BRIDGE_KEY = key;
    if (ws && ws.readyState === 1) sendHello();
  }};

  function scheduleReconnect() {{
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(function() {{
      reconnectTimer = null;
      connect();
    }}, 1000);
  }}

  async function executeCommand(cmd) {{
    const id = cmd.id;
    const script = cmd.script;

    const effectMark = effectMonitor.mark();
    try {{
      const AsyncFunction = Object.getPrototypeOf(async function(){{}}).constructor;
      const fn = new AsyncFunction('return (' + script + ')');
      const result = await fn();
      sendResult(id, result, null);
    }} catch (e) {{
      const effect = effectMonitor.unchangedSince(effectMark) ? 'none' : 'possible';
      sendResult(id, null, e.message || String(e), effect);
    }}
  }}

  // Runner for natively delivered commands; `fn` is already compiled code.
  function runNative(id, fn, effectOverride) {{
    const effectMark = effectMonitor.mark();
    Promise.resolve().then(fn).then(function(result) {{
      sendResult(id, result, null);
    }}, function(e) {{
      const effect = effectOverride || (effectMonitor.unchangedSince(effectMark) ? 'none' : 'possible');
      sendResult(id, null, (e && e.message) || String(e), effect);
    }});
  }}
  try {{
    Object.defineProperty(window, '__CONNECTOR_RUN__', {{ value: runNative, configurable: true }});
  }} catch (_) {{}}

  function sendResult(id, result, error, effect) {{
    if (!ws || ws.readyState !== WebSocket.OPEN) {{
      origConsole.error('[connector] Cannot send result: bridge not connected');
      return;
    }}

    const payload = {{ id: id }};
    if (error !== null && error !== undefined) {{
      payload.error = error;
      payload.effect = effect || 'possible';
    }} else {{
      try {{
        // Ensure result is JSON-serializable
        JSON.stringify(result);
        payload.result = result;
      }} catch (_) {{
        payload.result = String(result);
      }}
    }}

    ws.send(JSON.stringify(payload));
  }}

  // === Unified Snapshot Engine ===
  const semantic = {semantic_source};

  // Outer-scope: cached React fiber key (undefined=not looked up, null=not React)
  let fiberKey;

  const GENERIC_WRAPPERS = new Set([
    'App', 'Layout', 'ConfigProvider', 'ThemeProvider',
    'Fragment', 'Suspense', 'ErrorBoundary', 'StrictMode'
  ]);

  function findFiberKey(el) {{
    if (fiberKey && el[fiberKey]) return fiberKey;
    const keys = Object.keys(el);
    for (let i = 0; i < keys.length; i++) {{
      if (keys[i].startsWith('__reactFiber$')) {{
        fiberKey = keys[i];
        return fiberKey;
      }}
    }}
    fiberKey = null;
    return fiberKey;
  }}

  function getComponentName(el) {{
    const key = findFiberKey(el);
    if (!key) return null;
    let fiber = el[key];
    while (fiber) {{
      const t = fiber.type;
      if (t && typeof t === 'function') {{
        const name = t.displayName || t.name || null;
        if (name && !GENERIC_WRAPPERS.has(name)) return name;
      }}
      fiber = fiber.return;
    }}
    return null;
  }}

  const getRole = element => window.__CONNECTOR_SEMANTIC__.getRole(element);
  const getName = element => window.__CONNECTOR_SEMANTIC__.getAccessibleName(element);

  // Interactive roles that get ref= attributes in ai mode
  const INTERACTIVE_ROLES = new Set([
    'button', 'link', 'textbox', 'checkbox', 'radio', 'combobox',
    'listbox', 'option', 'menuitem', 'tab', 'switch', 'slider',
    'spinbutton', 'searchbox', 'menuitemcheckbox', 'menuitemradio'
  ]);

  window.__CONNECTOR_SNAPSHOT__ = function(options) {{
    const semantic = window.__CONNECTOR_SEMANTIC__;
    if (!semantic) throw new Error('Trusted semantic runtime is missing');
    const opts = options || {{}};
    const mode = opts.mode || 'ai';
    const maxDepth = Math.min(64, opts.maxDepth || 64);
    const maxElements = Math.min(20000, opts.maxElements || 20000);
    const semanticDeadline = performance.now() + 250;
    const reactEnrich = opts.reactEnrich !== false;
    const followPortals = opts.followPortals !== false;
    const shadowDom = opts.shadowDom === true;
    const maxTokens = opts.maxTokens || 0;

    const rootEl = opts.selector
      ? document.querySelector(opts.selector)
      : document.body;
    if (!rootEl) return {{ snapshot: '', refs: {{}}, allRefs: null, subtrees: [], meta: {{ semanticVersion: semantic.version, semanticCoverage: semantic.coverage, elementCount: 0, truncated: false, split: false, inlineComplete: true, portalCount: 0, virtualScrollContainers: 0, inlineTokens: 0, overlays: [] }} }};

    // State
    let elementCount = 0;
    let truncated = false;
    let refCounter = 0;
    let portalCount = 0;
    let virtualScrollCount = 0;
    const refs = {{}};
    const portalLinks = [];
    const claimedPortalIds = new Set();
    const depthMap = new WeakMap();
    const treeNodeMap = new WeakMap();
    const treeParentMap = new WeakMap();
    const stitchedPortals = new WeakSet();
    // Overlay detection runs only on full-document ai/accessibility snapshots;
    // a scoped snapshot means the caller already chose its focus.
    const overlayCandidates = [];
    const detectOverlays = mode !== 'structure' && !opts.selector;

    // TreeWalker filter
    function nodeFilter(node) {{
      const tag = node.tagName;
      if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE')
        return NodeFilter.FILTER_REJECT;
      if (semantic.isConnectorOwned(node)) return NodeFilter.FILTER_REJECT;
      if (mode !== 'structure' && !semantic.isAccessibilityExposed(node))
        return NodeFilter.FILTER_REJECT;
      try {{
        const cs = getComputedStyle(node);
        if (cs.display === 'none') return NodeFilter.FILTER_REJECT;
        if (cs.visibility === 'hidden') return NodeFilter.FILTER_REJECT;
      }} catch (_) {{}}
      const role = semantic.getRole(node);
      if (role === 'presentation' || role === 'none')
        return NodeFilter.FILTER_SKIP;
      return NodeFilter.FILTER_ACCEPT;
    }}

    function directTextOf(el) {{
      var tagName = el.tagName;
      if (tagName === 'TEXTAREA' || tagName === 'SELECT' || tagName === 'OPTION') return '';
      var parts = [];
      var nodes = el.childNodes || [];
      for (var ti = 0; ti < nodes.length; ti++) {{
        if (nodes[ti].nodeType === 3) parts.push(nodes[ti].textContent || '');
      }}
      return parts.join(' ').replace(/[\s\u00a0\u2000-\u200b\u202f\u205f\u3000]+/g, ' ').trim();
    }}

    // Build a tree node for one element
    function buildNode(el, depth) {{
      const role = getRole(el);
      const sensitive = semantic.isSensitive(el);
      const name = sensitive ? '[redacted]' : getName(el);
      const description = sensitive ? '[redacted]' : semantic.getAccessibleDescription(el);
      const tag = el.tagName.toLowerCase();
      const attrs = [];
      let refId = null;

      if (mode === 'ai') {{
        // Assign ref to interactive elements
        const isInteractive = (role && INTERACTIVE_ROLES.has(role)) ||
          el.hasAttribute('onclick') ||
          el.hasAttribute('tabindex') ||
          (function() {{ try {{ return getComputedStyle(el).cursor === 'pointer'; }} catch(_) {{ return false; }} }})();
        if (isInteractive) {{
          refId = 'e' + (refCounter++);
          attrs.push('ref=' + refId);
          if (opts.stampRefs !== false) {{ try {{ el.setAttribute('data-connector-ref', refId); }} catch (_) {{}} }}
          refs[refId] = {{
            tag: tag,
            role: role || null,
            name: (name || '').substring(0, 100),
            selector: buildSelector(el),
            nth: null,
            ...semantic.rememberRef(el),
          }};
        }}
        // React component enrichment
        if (reactEnrich) {{
          const comp = getComponentName(el);
          if (comp) attrs.push('component=' + comp);
        }}
      }}

      // Shared states preserve false and mixed; description never becomes name.
      for (const [key, value] of Object.entries(semantic.getAriaStates(el))) {{
        attrs.push(value === true ? key : key + '=' + String(value));
      }}
      const level = el.getAttribute('aria-level') || (/^H([1-6])$/.test(el.tagName) ? el.tagName.charAt(1) : null);
      if (level) attrs.push('level=' + level);
      if (description) attrs.push('description=' + JSON.stringify(description.slice(0, 512)));

      // Virtual scroll detection
      if (el.classList && el.classList.contains('rc-virtual-list-holder')) {{
        virtualScrollCount++;
        const inner = el.querySelector('.rc-virtual-list-holder-inner');
        const visibleCount = inner ? inner.children.length : 0;
        attrs.push('virtual-scroll');
        attrs.push('visible=' + visibleCount);
      }}

      // Portal links (aria-controls / aria-owns)
      if (followPortals) {{
        const controls = el.getAttribute('aria-controls');
        const owns = el.getAttribute('aria-owns');
        var linkedIds = [];
        if (controls) linkedIds = linkedIds.concat(controls.split(/\s+/));
        if (owns) linkedIds = linkedIds.concat(owns.split(/\s+/));
        // store for pass 2; treeNode will be attached after creation
        if (linkedIds.length > 0) {{
          for (var li = 0; li < linkedIds.length; li++) {{
            if (linkedIds[li]) {{
              portalLinks.push({{ targetId: linkedIds[li], depth: depth, treeNode: null }});
              claimedPortalIds.add(linkedIds[li]);
            }}
          }}
        }}
      }}

      // Structure mode: tag, id, classes, data-testid
      if (mode === 'structure') {{
        var structAttrs = [];
        if (el.id) structAttrs.push('id=' + el.id);
        if (el.className && typeof el.className === 'string') {{
          var cls = el.className.trim().split(/\s+/).slice(0, 5).filter(Boolean);
          if (cls.length > 0) structAttrs.push('class=' + cls.join('.'));
        }}
        var testId = el.getAttribute('data-testid');
        if (testId) structAttrs.push('data-testid=' + testId);
        return {{
          label: tag,
          name: '',
          attrs: structAttrs,
          children: [],
          depth: depth,
          el: el
        }};
      }}

      var nodeObj = {{
        label: role || tag,
        name: name || '',
        attrs: attrs,
        children: [],
        depth: depth,
        el: el
      }};

      // Accessibility mode shows text that sits directly in an element whose
      // accessible name does not already carry it (a generic div holding a
      // total, a paragraph between links). AI mode (-i) stays ref-focused.
      if (mode === 'accessibility' && !sensitive) {{
        var directText = directTextOf(el);
        if (directText && (name || '').indexOf(directText) === -1) {{
          nodeObj.children.push({{ label: 'text', name: directText.substring(0, 200), attrs: [], children: [], depth: depth + 1, el: null }});
        }}
      }}

      if (detectOverlays) {{
        try {{
          var isRoleOverlay = role === 'dialog' || role === 'alertdialog' ||
            (tag === 'dialog' && el.hasAttribute('open'));
          var overlayOptIn = el.hasAttribute('data-connector-overlay');
          var popoverOpen = false;
          try {{ popoverOpen = el.hasAttribute('popover') && el.matches(':popover-open'); }} catch (_) {{}}
          var fixedOverlay = false;
          if (!isRoleOverlay && !overlayOptIn && !popoverOpen) {{
            var ocs = getComputedStyle(el);
            if (ocs.position === 'fixed' && ocs.zIndex !== 'auto') {{
              var obr = el.getBoundingClientRect();
              // Size gate skips tooltips and small floating buttons but keeps
              // dock bars and every real window/panel.
              if (obr.width >= 96 && obr.height >= 40) fixedOverlay = true;
            }}
          }}
          if (isRoleOverlay || overlayOptIn || popoverOpen || fixedOverlay) {{
            overlayCandidates.push({{ el: el, node: nodeObj, viaRole: isRoleOverlay }});
          }}
        }} catch (_) {{}}
      }}

      return nodeObj;
    }}

    // Build a minimal CSS selector for ref lookup
    function buildSelector(el) {{
      if (el.id) return '#' + CSS.escape(el.id);
      var tag = el.tagName.toLowerCase();
      var sel = tag;
      var testId = el.getAttribute('data-testid');
      if (testId) return tag + '[data-testid="' + CSS.escape(testId) + '"]';
      if (el.className && typeof el.className === 'string') {{
        var cls = el.className.trim().split(/\s+/).slice(0, 2).filter(Boolean);
        if (cls.length > 0) sel += '.' + cls.map(CSS.escape).join('.');
      }}
      // nth-child disambiguation
      if (el.parentElement) {{
        var siblings = el.parentElement.children;
        var idx = 0;
        for (var s = 0; s < siblings.length; s++) {{
          if (siblings[s] === el) {{ idx = s + 1; break; }}
        }}
        if (siblings.length > 1) sel += ':nth-child(' + idx + ')';
      }}
      return sel;
    }}

    // ~3.5 chars per token for indented tree format (conservative estimate)
    function estimateTokens(text) {{
      return Math.ceil(text.length / 3.5);
    }}

    // === Sibling run detection ===
    function structHash(node) {{
      var h = node.label;
      // Include class fingerprint (not in ai-mode attrs but needed for differentiation)
      if (node.el && node.el.className && typeof node.el.className === 'string') {{
        var cls = node.el.className.trim().split(/\s+/).slice(0, 5).filter(Boolean);
        if (cls.length > 0) h += '|cls=' + cls.join('.');
      }}
      for (var ai = 0; ai < node.attrs.length; ai++) {{
        var a = node.attrs[ai];
        if (a.indexOf('ref=') === 0 || a.indexOf('component=') === 0) continue;
        h += '|' + a;
      }}
      return h;
    }}

    function collapseRepeats(children) {{
      if (children.length < 5) return children;
      var result = [];
      var i = 0;
      while (i < children.length) {{
        var hash = structHash(children[i]);
        var runEnd = i + 1;
        while (runEnd < children.length && structHash(children[runEnd]) === hash) {{
          runEnd++;
        }}
        var runLen = runEnd - i;
        if (runLen >= 5) {{
          result.push(children[i]);
          result.push(children[i + 1]);
          var collapsed = [];
          for (var ci = i + 2; ci < runEnd; ci++) {{
            collapsed.push(children[ci]);
          }}
          result.push({{
            label: '... ' + (runLen - 2) + ' more ' + children[i].label + ' items',
            name: '',
            attrs: ['collapsed-run'],
            children: [],
            _collapsedNodes: collapsed,
            depth: children[i].depth,
            el: children[i].el
          }});
          i = runEnd;
        }} else {{
          for (var ri = i; ri < runEnd; ri++) {{
            result.push(children[ri]);
          }}
          i = runEnd;
        }}
      }}
      return result;
    }}

    function collapseTree(node) {{
      for (var ci = 0; ci < node.children.length; ci++) {{
        collapseTree(node.children[ci]);
      }}
      node.children = collapseRepeats(node.children);
    }}

    function collectCollapsed(node) {{
      var parts = [];
      for (var ci = 0; ci < node.children.length; ci++) {{
        var child = node.children[ci];
        if (child._collapsedNodes) {{
          var lines = [];
          for (var ni = 0; ni < child._collapsedNodes.length; ni++) {{
            lines.push(renderNode(child._collapsedNodes[ni], 0));
          }}
          parts.push({{ label: child.label, content: lines.join('\n') }});
        }}
        var nested = collectCollapsed(child);
        for (var pi = 0; pi < nested.length; pi++) {{
          parts.push(nested[pi]);
        }}
      }}
      return parts;
    }}

    // === Pass 1: Main DOM walk ===
    var rootNode = {{ label: 'root', name: '', attrs: [], children: [], depth: -1, el: rootEl }};
    var walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_ELEMENT, {{
      acceptNode: nodeFilter
    }});

    depthMap.set(rootEl, 0);
    var currentEl = walker.currentNode;
    if (currentEl === rootEl && currentEl.nodeType === 1) {{
      // Process root element itself if it's an element
      var rn = buildNode(currentEl, 0);
      elementCount++;
      treeNodeMap.set(currentEl, rn);
      rootNode.children.push(rn);
      treeParentMap.set(rn, rootNode);
      // Back-link portal links
      for (var pi = portalLinks.length - 1; pi >= 0; pi--) {{
        if (portalLinks[pi].treeNode === null && portalLinks[pi].depth === 0) {{
          portalLinks[pi].treeNode = rn;
        }}
      }}
    }}

    while (true) {{
      currentEl = walker.nextNode();
      if (!currentEl) break;

      if (elementCount >= maxElements || performance.now() > semanticDeadline) {{
        truncated = true;
        break;
      }}

      // Compute depth from parent
      var parentEl = currentEl.parentElement;
      var parentDepth = depthMap.has(parentEl) ? depthMap.get(parentEl) : 0;
      var myDepth = parentDepth + 1;
      depthMap.set(currentEl, myDepth);

      if (maxDepth > 0 && myDepth > maxDepth) continue;

      var node = buildNode(currentEl, myDepth);
      elementCount++;
      treeNodeMap.set(currentEl, node);

      // Attach to parent tree node
      var parentNode = treeNodeMap.has(parentEl) ? treeNodeMap.get(parentEl) : rootNode;
      parentNode.children.push(node);
      treeParentMap.set(node, parentNode);

      // Back-link latest portal links to their treeNode
      for (var pj = portalLinks.length - 1; pj >= 0; pj--) {{
        if (portalLinks[pj].treeNode === null) {{
          portalLinks[pj].treeNode = node;
        }} else {{
          break;
        }}
      }}

      // Shadow DOM opt-in
      if (shadowDom && currentEl.shadowRoot) {{
        var shadowWalker = document.createTreeWalker(currentEl.shadowRoot, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var sEl = shadowWalker.nextNode();
        while (sEl) {{
          if (elementCount >= maxElements || performance.now() > semanticDeadline) {{ truncated = true; break; }}
          var sDepth = myDepth + 1;
          depthMap.set(sEl, sDepth);
          if (maxDepth === 0 || sDepth <= maxDepth) {{
            var sNode = buildNode(sEl, sDepth);
            elementCount++;
            treeNodeMap.set(sEl, sNode);
            var sParent = treeNodeMap.has(sEl.parentElement) ? treeNodeMap.get(sEl.parentElement) : node;
            sParent.children.push(sNode);
            treeParentMap.set(sNode, sParent);
          }}
          sEl = shadowWalker.nextNode();
        }}
      }}
    }}

    // === Pass 2: Portal stitching ===
    if (followPortals) {{
      for (var pk = 0; pk < portalLinks.length; pk++) {{
        var link = portalLinks[pk];
        var targetEl = document.getElementById(link.targetId);
        if (!targetEl || !link.treeNode || stitchedPortals.has(targetEl) || nodeFilter(targetEl) !== NodeFilter.FILTER_ACCEPT) continue;
        if (elementCount >= maxElements || performance.now() > semanticDeadline) {{ truncated = true; break; }}
        // An owned ancestor would form an accessibility cycle. Existing nodes
        // move to their semantic owner rather than being counted a second time.
        var existingTarget = treeNodeMap.get(targetEl);
        var ownerAncestor = link.treeNode;
        var cyclicPortal = false;
        while (ownerAncestor) {{
          if (ownerAncestor === existingTarget || ownerAncestor.el === targetEl) {{ cyclicPortal = true; break; }}
          ownerAncestor = treeParentMap.get(ownerAncestor);
        }}
        if (cyclicPortal || targetEl.contains(link.treeNode.el)) continue;
        stitchedPortals.add(targetEl);
        if (existingTarget) {{
          var oldParent = treeParentMap.get(existingTarget);
          if (oldParent) oldParent.children = oldParent.children.filter(function(child) {{ return child !== existingTarget; }});
          link.treeNode.children.push(existingTarget);
          treeParentMap.set(existingTarget, link.treeNode);
          existingTarget.attrs.push('portal');
          portalCount++;
          continue;
        }}

        portalCount++;
        var baseDepth = link.depth + 1;
        var portalWalker = document.createTreeWalker(targetEl, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var portalDepthMap = new WeakMap();
        portalDepthMap.set(targetEl, baseDepth);

        // Process target root
        var targetNode = buildNode(targetEl, baseDepth);
        targetNode.attrs.push('portal');
        elementCount++;
        link.treeNode.children.push(targetNode);
        treeNodeMap.set(targetEl, targetNode);
        treeParentMap.set(targetNode, link.treeNode);

        var pEl = portalWalker.nextNode();
        while (pEl) {{
          if (elementCount >= maxElements || performance.now() > semanticDeadline) {{ truncated = true; break; }}
          var pParent = pEl.parentElement;
          var pParentDepth = portalDepthMap.has(pParent) ? portalDepthMap.get(pParent) : baseDepth;
          var pDepth = pParentDepth + 1;
          portalDepthMap.set(pEl, pDepth);
          if (maxDepth === 0 || pDepth <= maxDepth) {{
            var pNode = buildNode(pEl, pDepth);
            elementCount++;
            // Attach to portal parent
            var pParentNode = treeNodeMap.has(pParent) ? treeNodeMap.get(pParent) : targetNode;
            pParentNode.children.push(pNode);
            treeParentMap.set(pNode, pParentNode);
            treeNodeMap.set(pEl, pNode);
          }}
          pEl = portalWalker.nextNode();
        }}
      }}

      // Orphan portals: body direct children with ant-/rc- class not claimed.
      // Full-document snapshots only: a scoped snapshot means the caller chose
      // its focus, and unrelated open modals must not leak into it (aria-linked
      // portals still stitch via pass 2 from any scope).
      var bodyChildren = opts.selector ? [] : document.body.children;
      for (var oi = 0; oi < bodyChildren.length; oi++) {{
        var orphan = bodyChildren[oi];
        if (orphan.id && claimedPortalIds.has(orphan.id)) continue;
        if (treeNodeMap.has(orphan) || nodeFilter(orphan) !== NodeFilter.FILTER_ACCEPT) continue;
        if (elementCount >= maxElements || performance.now() > semanticDeadline) {{ truncated = true; break; }}
        var oClass = typeof orphan.className === 'string' ? orphan.className : '';
        if (!/\b(ant-|rc-)/.test(oClass)) continue;
        // Treat as orphan portal
        portalCount++;
        var oNode = buildNode(orphan, 1);
        oNode.attrs.push('orphan-portal');
        elementCount++;
        rootNode.children.push(oNode);
        treeNodeMap.set(orphan, oNode);
        treeParentMap.set(oNode, rootNode);
        // Walk children of orphan portal
        var orphanWalker = document.createTreeWalker(orphan, NodeFilter.SHOW_ELEMENT, {{
          acceptNode: nodeFilter
        }});
        var oDepthMap = new WeakMap();
        oDepthMap.set(orphan, 1);
        var oEl = orphanWalker.nextNode();
        while (oEl) {{
          if (elementCount >= maxElements || performance.now() > semanticDeadline) {{ truncated = true; break; }}
          var oParent = oEl.parentElement;
          var oParentD = oDepthMap.has(oParent) ? oDepthMap.get(oParent) : 1;
          var oDep = oParentD + 1;
          oDepthMap.set(oEl, oDep);
          if (maxDepth === 0 || oDep <= maxDepth) {{
            var oChild = buildNode(oEl, oDep);
            elementCount++;
            var oParNode = treeNodeMap.has(oParent) ? treeNodeMap.get(oParent) : oNode;
            oParNode.children.push(oChild);
            treeParentMap.set(oChild, oParNode);
            treeNodeMap.set(oEl, oChild);
          }}
          oEl = orphanWalker.nextNode();
        }}
      }}
    }}

    // === Overlay post-pass: pick roots, rank, annotate, build meta ===
    var overlayEntries = [];
    var overlaysMeta = [];
    if (detectOverlays && overlayCandidates.length > 0) {{
      try {{
        var coversViewport = function(e) {{
          try {{
            var vr = e.getBoundingClientRect();
            return vr.width >= window.innerWidth * 0.9 && vr.height >= window.innerHeight * 0.9;
          }} catch (_) {{ return false; }}
        }};
        // Dedupe by element (portal stitching can build a second node for the same el).
        var seenOverlayEls = new Set();
        var cands = [];
        for (var ui = 0; ui < overlayCandidates.length; ui++) {{
          if (seenOverlayEls.has(overlayCandidates[ui].el)) continue;
          seenOverlayEls.add(overlayCandidates[ui].el);
          cands.push(overlayCandidates[ui]);
        }}
        for (var ci2 = 0; ci2 < cands.length; ci2++) {{
          var top = cands[ci2];
          var contained = false;
          for (var cj = 0; cj < cands.length; cj++) {{
            if (cj !== ci2 && cands[cj].el.contains(top.el)) {{ contained = true; break; }}
          }}
          if (contained) continue;
          // Root = shallowest role-bearing candidate in this chain (antd: the
          // .ant-modal panel, not the full-viewport wrap), else the top itself.
          var root = null;
          for (var ck = 0; ck < cands.length; ck++) {{
            var cc = cands[ck];
            if (cc.viaRole && (cc.el === top.el || top.el.contains(cc.el))) {{ root = cc; break; }}
          }}
          if (!root) root = top;
          // Skip bare backdrops: a role-less fixed layer with no children and no
          // text (e.g. .ant-modal-mask) is a mask, not an overlay of its own.
          if (root.el === top.el && !root.viaRole && !root.el.firstElementChild && !(root.el.textContent || '').trim()) continue;
          var zEff = 0;
          try {{
            var zcs = getComputedStyle(top.el);
            zEff = zcs.zIndex === 'auto' ? 0 : (parseInt(zcs.zIndex, 10) || 0);
            if (zEff === 0 && root.el !== top.el) {{
              var rzcs = getComputedStyle(root.el);
              if (rzcs.zIndex !== 'auto') zEff = parseInt(rzcs.zIndex, 10) || 0;
            }}
          }} catch (_) {{}}
          var orect = {{ x: 0, y: 0, width: 0, height: 0 }};
          try {{
            var obr2 = root.el.getBoundingClientRect();
            orect = {{ x: Math.round(obr2.x), y: Math.round(obr2.y), width: Math.round(obr2.width), height: Math.round(obr2.height) }};
          }} catch (_) {{}}
          var ofocused = false;
          try {{ ofocused = !!(document.activeElement && root.el.contains(document.activeElement)); }} catch (_) {{}}
          var oModal = false;
          try {{
            if (root.el.getAttribute('aria-modal') === 'true' || top.el.getAttribute('aria-modal') === 'true') {{
              oModal = true;
            }} else if (coversViewport(top.el)) {{
              oModal = true;
            }} else {{
              // A backdrop renders immediately BEFORE the layer it masks (antd:
              // .ant-modal-mask then .ant-modal-wrap) and is element-childless.
              var pbs = top.el.previousElementSibling;
              if (pbs && !pbs.firstElementChild && getComputedStyle(pbs).position === 'fixed' && coversViewport(pbs)) {{
                oModal = true;
              }}
            }}
          }} catch (_) {{}}
          var otitle = getName(root.el) || '';
          if (!otitle && root.el !== top.el) otitle = getName(top.el) || '';
          if (!otitle) {{
            var rdesc = root.el.querySelector('[role="dialog"], [role="alertdialog"]');
            if (rdesc) otitle = getName(rdesc) || '';
          }}
          if (!otitle) {{
            var ohd = root.el.querySelector('h1, h2, h3, h4, h5, h6, [role="heading"]');
            if (ohd) otitle = (ohd.textContent || '').trim().replace(/\s+/g, ' ');
          }}
          if (!otitle) otitle = getComponentName(root.el) || '';
          if (!otitle) otitle = (root.el.textContent || '').trim().replace(/\s+/g, ' ').substring(0, 40);
          otitle = otitle.substring(0, 60);
          overlayEntries.push({{ el: root.el, node: root.node, topEl: top.el, title: otitle, zIndex: zEff, rect: orect, focused: ofocused, modal: oModal }});
        }}
        overlayEntries.sort(function(a, b) {{
          if (a.focused !== b.focused) return a.focused ? -1 : 1;
          return b.zIndex - a.zIndex;
        }});
        overlayEntries = overlayEntries.slice(0, 8);
        for (var oe2 = 0; oe2 < overlayEntries.length; oe2++) {{
          var oent = overlayEntries[oe2];
          var oid = 'o' + (oe2 + 1);
          var osel;
          if (oent.el.id) {{
            osel = '#' + oent.el.id;
          }} else {{
            var omk = oent.el.getAttribute('data-modal-key');
            var otid = oent.el.getAttribute('data-testid');
            if (omk) {{
              osel = '[data-modal-key="' + omk.replace(/"/g, '\\"') + '"]';
            }} else if (otid) {{
              osel = '[data-testid="' + otid.replace(/"/g, '\\"') + '"]';
            }} else {{
              var stamped = oent.el.getAttribute('data-connector-overlay');
              if (!stamped) {{
                try {{ oent.el.setAttribute('data-connector-overlay', oid); stamped = oid; }} catch (_) {{}}
              }}
              osel = stamped ? '[data-connector-overlay="' + stamped + '"]' : buildSelector(oent.el);
            }}
          }}
          oent.id = oid;
          oent.selector = osel;
          oent.node.attrs.push('overlay=' + oid);
          if (oent.zIndex) oent.node.attrs.push('z=' + oent.zIndex);
          if (oent.focused) oent.node.attrs.push('focused');
          if (oent.modal) oent.node.attrs.push('modal');
          overlaysMeta.push({{
            id: oid,
            title: oent.title,
            selector: osel,
            zIndex: oent.zIndex,
            rect: oent.rect,
            focused: oent.focused,
            modal: oent.modal
          }});
        }}
      }} catch (_) {{
        overlayEntries = [];
        overlaysMeta = [];
      }}
    }}

    // === Render: recursive stringify ===
    function renderNode(node, depth, skipOverlays) {{
      if (skipOverlays && skipOverlays.has(node)) {{
        return '  '.repeat(depth) + '- [overlay ' + skipOverlays.get(node) + ': shown above]';
      }}
      var line = '  '.repeat(depth) + '- ' + node.label;
      if (node.name) line += ' "' + node.name.replace(/"/g, '\\"') + '"';
      if (node.attrs.length > 0) line += ' [' + node.attrs.join(', ') + ']';
      var lines = [line];
      for (var ci = 0; ci < node.children.length; ci++) {{
        lines.push(renderNode(node.children[ci], depth + 1, skipOverlays));
      }}
      return lines.join('\n');
    }}

    // === Collapse repeating siblings before rendering ===
    collapseTree(rootNode);

    // === Budget-aware section rendering ===
    var budgetActive = maxTokens > 0;
    var usedTokens = 0;
    var split = false;
    var subtrees = [];
    var snapshotLines = [];

    // Overlay header: one line of situational awareness before the tree.
    if (overlayEntries.length > 0) {{
      var hparts = [];
      for (var hi = 0; hi < overlayEntries.length; hi++) {{
        var he = overlayEntries[hi];
        var hflags = [];
        if (he.focused) hflags.push('focused');
        if (he.modal) hflags.push('modal');
        if (he.zIndex) hflags.push('z=' + he.zIndex);
        hflags.push(he.rect.width + 'x' + he.rect.height);
        hparts.push(he.id + ' "' + he.title.replace(/"/g, '\\"') + '" [' + hflags.join(', ') + ']');
      }}
      var overlayHeader = '# overlays: ' + hparts.join(' | ') + ' -- rescope via meta.overlays[].selector';
      snapshotLines.push(overlayHeader);
      if (budgetActive) usedTokens += estimateTokens(overlayHeader);
    }}

    // With a budget, overlays render first (focused, then z-desc) so an open
    // modal stays inline and the background page spills instead; the overlay's
    // place in the page tree keeps a one-line pointer. Unlimited output keeps
    // pure DOM order.
    var inlineFirst = new Map();
    if (budgetActive) {{
      for (var ofi = 0; ofi < overlayEntries.length; ofi++) {{
        if (overlayEntries[ofi].node && overlayEntries[ofi].id) inlineFirst.set(overlayEntries[ofi].node, overlayEntries[ofi].id);
      }}
    }}

    function ownLine(node, depth) {{
      var line = '  '.repeat(depth) + '- ' + node.label;
      if (node.name) line += ' "' + node.name.replace(/"/g, '\\"') + '"';
      if (node.attrs.length > 0) line += ' [' + node.attrs.join(', ') + ']';
      return line;
    }}

    function sectionLabelOf(node) {{
      var label = node.label || 'section';
      try {{
        if (node.el) {{
          if (node.el.id) {{
            label += '#' + node.el.id;
          }} else if (node.el.className && typeof node.el.className === 'string') {{
            var scls = node.el.className.trim().split(/\s+/).slice(0, 2).filter(Boolean);
            if (scls.length > 0) label += '.' + scls.join('.');
          }}
        }}
        if (node.name) label += ' "' + node.name.substring(0, 30) + '"';
      }} catch (_) {{}}
      return label;
    }}

    function spill(label, content, depth) {{
      split = true;
      subtrees.push({{ label: label, content: content }});
      var placeholder = '  '.repeat(depth) + '- [subtree: ' + label + '] (' + estimateTokens(content) + ' tokens, see subtrees[' + (subtrees.length - 1) + '])';
      snapshotLines.push(placeholder);
      // Charge placeholder lines too: the budget tracks the whole payload.
      usedTokens += estimateTokens(placeholder);
    }}

    // A node that does not fit keeps its own line inline and recurses into its
    // children, so one oversized wrapper (an app root, a layout shell) no longer
    // spills the entire page into a single subtree.
    var MIN_RECURSE_TOKENS = 48;
    function emitNode(node, depth, level, skipOverlays) {{
      var text = renderNode(node, depth, skipOverlays);
      if (!budgetActive) {{ snapshotLines.push(text); return; }}
      var tokens = estimateTokens(text);
      var overlayPointer = skipOverlays && skipOverlays.has(node);
      if (overlayPointer || usedTokens + tokens <= maxTokens) {{
        snapshotLines.push(text);
        usedTokens += tokens;
        return;
      }}
      var line = ownLine(node, depth);
      var lineTokens = estimateTokens(line);
      var canRecurse = node.children.length > 0 && level < 24 &&
        maxTokens - usedTokens >= lineTokens + MIN_RECURSE_TOKENS;
      if (!canRecurse) {{
        spill(sectionLabelOf(node), text, depth);
        return;
      }}
      split = true;
      snapshotLines.push(line);
      usedTokens += lineTokens;
      for (var ci = 0; ci < node.children.length; ci++) {{
        if (maxTokens - usedTokens < MIN_RECURSE_TOKENS / 2 && ci < node.children.length - 1) {{
          var rest = [];
          for (var rj = ci; rj < node.children.length; rj++) rest.push(renderNode(node.children[rj], depth + 1, skipOverlays));
          spill('rest of ' + sectionLabelOf(node) + ' (' + (node.children.length - ci) + ' nodes)', rest.join('\n'), depth + 1);
          return;
        }}
        emitNode(node.children[ci], depth + 1, level + 1, skipOverlays);
      }}
    }}

    var fullParts = [];
    for (var fpi = 0; fpi < rootNode.children.length; fpi++) fullParts.push(renderNode(rootNode.children[fpi], 0));
    var fullText = fullParts.join('\n');
    if (!budgetActive || usedTokens + estimateTokens(fullText) <= maxTokens) {{
      if (fullText) snapshotLines.push(fullText);
      if (budgetActive) usedTokens += estimateTokens(fullText);
    }} else {{
      inlineFirst.forEach(function(oid, overlayNode) {{
        emitNode(overlayNode, 0, 0, null);
      }});
      for (var rii = 0; rii < rootNode.children.length; rii++) {{
        emitNode(rootNode.children[rii], 0, 0, inlineFirst);
      }}
    }}

    // === Collect collapsed sibling content for subtree files ===
    if (budgetActive) {{
      var collapsedParts = collectCollapsed(rootNode);
      for (var cpi = 0; cpi < collapsedParts.length; cpi++) {{
        subtrees.push(collapsedParts[cpi]);
      }}
    }}

    var snapshot = snapshotLines.join('\n');
    if (truncated) {{
      snapshot += '\n# ... truncated (' + maxElements + ' of ' + elementCount + '+ elements shown)';
    }}

    // Extract only refs visible in the inline snapshot
    var inlineRefs = {{}};
    var refMatches = snapshot.match(/ref=(e\d+)/g);
    if (refMatches) {{
      for (var mi = 0; mi < refMatches.length; mi++) {{
        var refId = refMatches[mi].replace('ref=', '');
        if (refs[refId]) {{
          inlineRefs[refId] = refs[refId];
        }}
      }}
    }}

    return {{
      snapshot: snapshot,
      refs: inlineRefs,
      allRefs: split ? refs : null,
      subtrees: subtrees,
      meta: {{
        semanticVersion: semantic.version,
        semanticCoverage: semantic.coverage,
        elementCount: elementCount,
        truncated: truncated,
        split: split,
        inlineComplete: !split,
        portalCount: portalCount,
        virtualScrollContainers: virtualScrollCount,
        inlineTokens: usedTokens,
        overlays: overlaysMeta
      }}
    }};
  }};

  // === Auto-push DOM via Tauri IPC (when available) ===
  function autoPushDom() {{
    // Active selection owns the interaction environment. Do not stamp refs or
    // expose connector UI through the legacy raw-DOM cache during its lease.
    if (window.__CONNECTOR_INPUT_GUARD__ && window.__CONNECTOR_INPUT_GUARD__.active) return;
    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (!ipc || !ipc.invoke) return;

    try {{
      const result = window.__CONNECTOR_SNAPSHOT__({{
        mode: 'ai',
        maxDepth: 0,
        maxElements: 5000,
        reactEnrich: true,
        followPortals: true,
        shadowDom: false,
        // Background pushes must not renumber the refs a caller is using.
        stampRefs: false
      }});
      ipc.invoke('plugin:connector|push_dom', {{
        payload: {{
          windowId: WINDOW_ID,
          html: document.body.innerHTML.substring(0, 500000),
          textContent: document.body.innerText.substring(0, 200000),
          snapshot: result.snapshot || '',
          snapshotMode: 'ai',
          refs: JSON.stringify(result.refs || {{}}),
          meta: JSON.stringify(result.meta || {{}})
        }}
      }}).catch(function() {{}});
    }} catch (_) {{}}
  }}

  // Push DOM on load and after navigation/mutations
  if (document.readyState === 'complete') {{
    setTimeout(autoPushDom, 2000);
  }} else {{
    window.addEventListener('load', function() {{ setTimeout(autoPushDom, 2000); }});
  }}

  // Re-push on significant DOM changes (debounced)
  let pushTimer = null;
  const observer = new MutationObserver(function() {{
    if (pushTimer) clearTimeout(pushTimer);
    pushTimer = setTimeout(autoPushDom, 5000);
  }});
  observer.observe(document.body, {{ childList: true, subtree: true }});

  // === Auto-push console logs via Tauri IPC ===
  // Entries are tracked by sequence number, not array index: the buffer is
  // capped at MAX_LOGS and shifts, so an index would stop advancing once full.
  let lastLogPushSeq = 0;

  function takePendingLogs() {{
    const pending = [];
    for (let i = 0; i < consoleLogs.length; i++) {{
      const l = consoleLogs[i];
      if (l.seq > lastLogPushSeq) pending.push({{ level: l.level, message: l.message, timestamp: l.timestamp, windowId: WINDOW_ID }});
    }}
    if (consoleLogs.length) lastLogPushSeq = Math.max(lastLogPushSeq, consoleLogs[consoleLogs.length - 1].seq);
    return pending;
  }}

  // The host calls this before reading console.log so a read right after
  // logging sees the entries without waiting for the next periodic push.
  try {{
    Object.defineProperty(window, '__CONNECTOR_TAKE_LOGS__', {{ value: takePendingLogs, configurable: true }});
  }} catch (_) {{}}

  function autoPushLogs() {{
    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (!ipc || !ipc.invoke) return;
    const newEntries = takePendingLogs();
    if (!newEntries.length) return;

    ipc.invoke('plugin:connector|push_logs', {{
      payload: {{ entries: newEntries }}
    }}).catch(function() {{}});
  }}

  setInterval(autoPushLogs, 3000);

  // === Alt+Shift+Click element picker ===
  document.addEventListener('click', function(e) {{
    if (!e.altKey || !e.shiftKey) return;
    e.preventDefault();
    e.stopPropagation();

    const el = e.target;
    const rect = el.getBoundingClientRect();
    const info = {{
      tag: el.tagName.toLowerCase(),
      id: el.id || null,
      className: el.className || null,
      text: el.textContent ? el.textContent.trim().substring(0, 200) : null,
      rect: {{ x: rect.x, y: rect.y, width: rect.width, height: rect.height }},
      attributes: {{}},
    }};

    Array.from(el.attributes).forEach(function(attr) {{
      info.attributes[attr.name] = attr.value;
    }});

    const ipc = window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core);
    if (ipc && ipc.invoke) {{
      ipc.invoke('plugin:connector|set_pointed_element', {{
        payload: {{ element: info }}
      }}).catch(function() {{}});
    }}

    origConsole.log('[connector] Element picked:', info.tag, info.id || '', info.className || '');
  }}, true);

  // Start connection
  connect();
}})();
"#
    )
}

/// A bridge with one registered `main` client whose outgoing commands are
/// readable from the returned receiver.
#[cfg(test)]
pub(crate) async fn test_queued_bridge() -> (Bridge, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let bridge = Bridge {
        port: 0,
        clients: Arc::new(Mutex::new(HashMap::new())),
        connections: Arc::new(StdMutex::new(HashMap::from([(
            "main".into(),
            ConnectionState {
                bridge_key: crate::identity::bridge_key("main"),
                conn_id: Some("connection-a".into()),
                revision: 1,
            },
        )]))),
        pending: Arc::new(StdMutex::new(HashMap::new())),
        app_handle: Arc::new(Mutex::new(None)),
        runtime: Arc::new(crate::runtime::RuntimeManager::default()),
        late_hook: Arc::new(StdMutex::new(None)),
        native_sink: Arc::new(StdMutex::new(None)),
    };
    bridge.clients.lock().await.insert(
        "main".into(),
        BridgeClient {
            window_id: "main".into(),
            connected_at_ms: 0,
            bridge_key: crate::identity::bridge_key("main"),
            conn_id: "connection-a".into(),
            tx,
            eval_blocked: false,
        },
    );
    (bridge, rx)
}

/// Deliver a page message on the registered `main` connection.
#[cfg(test)]
pub(crate) async fn test_page_message(bridge: &Bridge, message: serde_json::Value) {
    let client = bridge.clients.lock().await["main"].clone();
    handle_bridge_message(
        &message.to_string(),
        &client.tx,
        &bridge.clients,
        &bridge.connections,
        &bridge.pending,
        &client.conn_id,
    )
    .await;
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use serde_json::json;

    async fn queued_bridge() -> (Bridge, mpsc::UnboundedReceiver<String>) {
        super::test_queued_bridge().await
    }

    #[tokio::test]
    async fn connection_pin_stays_current_on_the_same_socket() {
        let (bridge, _rx) = queued_bridge().await;
        let pin = bridge.pin_connection("main").await;
        let clone = pin.clone();
        let client = bridge.clients.lock().await["main"].clone();
        assert!(pin.is_current());
        handle_bridge_message(
            &json!({"type":"hello","windowId":"main","bridgeKey":client.bridge_key}).to_string(),
            &client.tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            &client.conn_id,
        )
        .await;
        assert!(pin.is_current());
        assert!(clone.is_current());
    }

    #[tokio::test]
    async fn connection_pin_rejects_a_replacement_before_enqueue_or_fallback() {
        let (bridge, mut old_rx) = queued_bridge().await;
        let pin = bridge.pin_connection("main").await;
        let (tx, mut new_rx) = mpsc::unbounded_channel();
        handle_bridge_message(
            &json!({"type":"hello","windowId":"main","bridgeKey":crate::identity::bridge_key("main")})
                .to_string(),
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "replacement",
        )
        .await;
        assert!(!pin.is_current());
        assert!(bridge.pin_connection("main").await.is_current());
        let authorize = || pin.is_current();
        // If fallback preparation runs, this lock consumes the deadline and
        // yields a different failure. An invalid pin must reject it first.
        let _app_guard = bridge.app_handle.lock().await;
        let error = bridge
            .execute_js_authorized("write()", 100, "main", "pinned", Some(&authorize))
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. }
            if reason.contains("authorization was revoked"))
        );
        assert!(old_rx.try_recv().is_err());
        assert!(new_rx.try_recv().is_err());
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connection_pin_disconnect_only_invalidates_its_own_window() {
        let (bridge, _rx) = queued_bridge().await;
        let label = uuid::Uuid::new_v4().to_string();
        let (tx, _other_rx) = mpsc::unbounded_channel();
        handle_bridge_message(
            &json!({"type":"hello","windowId":label,"bridgeKey":crate::identity::bridge_key(&label)})
                .to_string(),
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "other-connection",
        )
        .await;
        let main_pin = bridge.pin_connection("main").await;
        let other_pin = bridge.pin_connection(&label).await;
        assert!(
            disconnect_bridge_client(&bridge.clients, &bridge.connections, "main", "connection-a",)
                .await
        );
        assert!(!main_pin.is_current());
        assert!(other_pin.is_current());
        assert!(bridge.pin_connection("main").await.is_current());
        crate::identity::window_destroyed(&label);
    }

    #[tokio::test]
    async fn connection_pin_ignores_a_stale_disconnect_after_replacement() {
        let (bridge, _rx) = queued_bridge().await;
        let original = bridge.pin_connection("main").await;
        let (tx, _new_rx) = mpsc::unbounded_channel();
        handle_bridge_message(
            &json!({"type":"hello","windowId":"main","bridgeKey":crate::identity::bridge_key("main")})
                .to_string(),
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "replacement",
        )
        .await;
        let replacement = bridge.pin_connection("main").await;
        assert!(
            !disconnect_bridge_client(
                &bridge.clients,
                &bridge.connections,
                "main",
                "connection-a",
            )
            .await
        );
        assert!(!original.is_current());
        assert!(replacement.is_current());
        assert_eq!(bridge.clients.lock().await["main"].conn_id, "replacement");
    }

    #[tokio::test]
    async fn connection_pin_for_eval_does_not_revive_after_a_socket_comes_and_goes() {
        let (bridge, _rx) = queued_bridge().await;
        let label = uuid::Uuid::new_v4().to_string();
        let eval_pin = bridge.pin_connection(&label).await;
        assert!(eval_pin.is_current());
        assert!(eval_pin.tx.is_none());
        let (tx, _other_rx) = mpsc::unbounded_channel();
        handle_bridge_message(
            &json!({"type":"hello","windowId":label,"bridgeKey":crate::identity::bridge_key(&label)})
                .to_string(),
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "temporary-socket",
        )
        .await;
        disconnect_bridge_client(
            &bridge.clients,
            &bridge.connections,
            &label,
            "temporary-socket",
        )
        .await;
        // Check only after both transitions: observing an invalid intermediate
        // state must not be required for permanent invalidation of the old pin.
        assert!(!eval_pin.is_current());
        let current_eval_pin = bridge.pin_connection(&label).await;
        assert!(current_eval_pin.is_current());
        assert!(current_eval_pin.tx.is_none());
        let authorize = || current_eval_pin.is_current();
        let error = bridge
            .execute_js_authorized("read()", 100, &label, "eval-route", Some(&authorize))
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. }
            if reason.contains("App handle not set for eval fallback")),
            "a current no-WS pin must still permit eval preparation: {error}"
        );
        crate::identity::window_destroyed(&label);
        assert!(!current_eval_pin.is_current());
    }

    #[tokio::test]
    async fn connection_pin_rejects_a_closed_sender_before_disconnect_cleanup() {
        let (bridge, rx) = queued_bridge().await;
        let pin = bridge.pin_connection("main").await;
        assert!(pin.is_current());
        drop(rx);
        assert!(!pin.is_current());
        let authorize = || pin.is_current();
        let error = bridge
            .execute_js_authorized("write()", 100, "main", "closed", Some(&authorize))
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. }
            if reason.contains("authorization was revoked"))
        );
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connection_pin_is_rechecked_after_waiting_to_enqueue() {
        let (bridge, mut old_rx) = queued_bridge().await;
        let pin = bridge.pin_connection("main").await;
        let (tx, mut new_rx) = mpsc::unbounded_channel();
        let hello = json!({"type":"hello","windowId":"main","bridgeKey":crate::identity::bridge_key("main")})
            .to_string();
        let clients = bridge.clients.lock().await;
        let replacement = handle_bridge_message(
            &hello,
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "replacement",
        );
        tokio::pin!(replacement);
        // Queue replacement first on the fair clients mutex, then dispatch.
        tokio::select! {
            biased;
            _ = &mut replacement => panic!("replacement passed the held clients lock"),
            _ = tokio::task::yield_now() => {}
        }
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let authorize = || {
            checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            pin.is_current()
        };
        let dispatch =
            bridge.execute_js_authorized("write()", 1000, "main", "enqueue-race", Some(&authorize));
        tokio::pin!(dispatch);
        tokio::select! {
            biased;
            _ = &mut dispatch => panic!("dispatch passed the held clients lock"),
            _ = tokio::task::yield_now() => {}
        }
        assert_eq!(checks.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(clients);
        replacement.await;
        let error = dispatch.await.unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. }
            if reason.contains("authorization was revoked"))
        );
        assert!(checks.load(std::sync::atomic::Ordering::SeqCst) > 1);
        assert!(old_rx.try_recv().is_err());
        assert!(new_rx.try_recv().is_err());
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connection_pin_serializes_eval_authorization_and_injection() {
        let (bridge, _rx) = queued_bridge().await;
        let label = uuid::Uuid::new_v4().to_string();
        let pin = bridge.pin_connection(&label).await;
        let authorize = || pin.is_current();
        let request = DispatchRequest {
            id: "eval-boundary",
            deadline: Instant::now() + Duration::from_secs(1),
            authorize: Some(&authorize),
        };
        let mut injected = false;
        bridge
            .enqueue_eval(&request, || {
                assert!(bridge.clients.try_lock().is_err());
                injected = true;
                Ok(())
            })
            .await
            .unwrap();
        assert!(injected);
        assert!(bridge.clients.try_lock().is_ok());
        let (tx, _new_rx) = mpsc::unbounded_channel();
        handle_bridge_message(
            &json!({"type":"hello","windowId":label,"bridgeKey":crate::identity::bridge_key(&label)})
                .to_string(),
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "new-socket",
        )
        .await;
        injected = false;
        let error = bridge
            .enqueue_eval(&request, || {
                injected = true;
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(!injected);
        assert!(matches!(error, BridgeError::NotDispatched { .. }));
        assert!(bridge.clients.try_lock().is_ok());
        crate::identity::window_destroyed(&label);
    }

    #[tokio::test]
    async fn connection_pin_destruction_reclaims_unique_window_registrations() {
        let (bridge, _rx) = queued_bridge().await;
        let mut old_pins = Vec::new();
        for _ in 0..16 {
            let label = uuid::Uuid::new_v4().to_string();
            let (tx, _rx) = mpsc::unbounded_channel();
            handle_bridge_message(
                &json!({"type":"hello","windowId":label,"bridgeKey":crate::identity::bridge_key(&label)})
                    .to_string(),
                &tx,
                &bridge.clients,
                &bridge.connections,
                &bridge.pending,
                "temporary-socket",
            )
            .await;
            old_pins.push(bridge.pin_connection(&label).await);
            disconnect_bridge_client(
                &bridge.clients,
                &bridge.connections,
                &label,
                "temporary-socket",
            )
            .await;
            // Match the real lib.rs lifecycle order, including the synchronous
            // callback that schedules cleanup on Tauri's async runtime.
            crate::identity::window_destroyed(&label);
            bridge.window_destroyed(&label);
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while bridge.connections.lock().unwrap().len() != 1 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(bridge.clients.lock().await.len(), 1);
        assert!(old_pins.iter().all(|pin| !pin.is_current()));
        assert!(bridge.pin_connection("main").await.is_current());
    }

    #[tokio::test]
    async fn connection_pin_delayed_destruction_preserves_recreated_eval_window() {
        let (bridge, _rx) = queued_bridge().await;
        let label = uuid::Uuid::new_v4().to_string();
        let old_pin = bridge.pin_connection(&label).await;
        crate::identity::window_destroyed(&label);
        let fresh_pin = bridge.pin_connection(&label).await;
        assert!(fresh_pin.is_current());
        assert!(!old_pin.is_current());
        bridge.remove_destroyed_window(&label).await;
        assert!(fresh_pin.is_current());
        assert!(!old_pin.is_current());
        assert!(bridge.connections.lock().unwrap().contains_key(&label));
        crate::identity::window_destroyed(&label);
        bridge.remove_destroyed_window(&label).await;
        assert!(!fresh_pin.is_current());
        assert!(!bridge.connections.lock().unwrap().contains_key(&label));
    }

    #[tokio::test]
    async fn connection_pin_destroyed_window_rejects_hello_waiting_for_dispatch() {
        let (bridge, _rx) = queued_bridge().await;
        let label = uuid::Uuid::new_v4().to_string();
        let pin = bridge.pin_connection(&label).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let hello = json!({"type":"hello","windowId":label,"bridgeKey":crate::identity::bridge_key(&label)})
            .to_string();
        let clients = bridge.clients.lock().await;
        let registration = handle_bridge_message(
            &hello,
            &tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "late-socket",
        );
        tokio::pin!(registration);
        tokio::select! {
            biased;
            _ = &mut registration => panic!("hello passed the held clients lock"),
            _ = tokio::task::yield_now() => {}
        }
        crate::identity::window_destroyed(&label);
        drop(clients);
        assert!(registration.await.is_none());
        bridge.remove_destroyed_window(&label).await;
        assert!(!pin.is_current());
        assert!(!bridge.clients.lock().await.contains_key(&label));
        assert!(!bridge.connections.lock().unwrap().contains_key(&label));
    }

    async fn simulated_runtime_bridge(
        mode: &'static str,
    ) -> (
        Bridge,
        Arc<StdMutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
        crate::runtime::RuntimeTarget,
    ) {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let scripts = Arc::new(StdMutex::new(Vec::new()));
        let recorded = scripts.clone();
        let target = crate::runtime::RuntimeTarget {
            app_id: "fixture".into(),
            window_id: "main".into(),
            window_instance_id: crate::identity::window_instance_id("main"),
            origin: "https://fixture.invalid".into(),
            url: "https://fixture.invalid/".into(),
        };
        let task = tokio::spawn(async move {
            let mut installed = serde_json::Value::Null;
            while let Some(message) = rx.recv().await {
                let command: serde_json::Value = serde_json::from_str(&message).unwrap();
                let script = command["script"].as_str().unwrap();
                recorded.lock().unwrap().push(script.into());
                let result = if script == crate::runtime::PROBE {
                    json!({"available":true,"suspended":false,"origin":"https://fixture.invalid","url":"https://fixture.invalid/","pageEpoch":"page-a","runtime":installed})
                } else if script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/") {
                    let input = script
                        .split("const c=")
                        .nth(1)
                        .unwrap()
                        .split("; const b=")
                        .next()
                        .unwrap();
                    let encoded = input
                        .strip_prefix("JSON.parse(")
                        .unwrap()
                        .strip_suffix(')')
                        .unwrap();
                    let decoded: String = serde_json::from_str(encoded).unwrap();
                    let configuration: serde_json::Value = serde_json::from_str(&decoded).unwrap();
                    if mode == "install_failure" {
                        json!({"ready":false,"error":"fixture install fault"})
                    } else {
                        installed = json!({"ready":true,"context":configuration["context"],"runtimeProtocolVersion":1});
                        installed.clone()
                    }
                } else {
                    json!({"ok":true,"dispatched":true})
                };
                let client = worker.clients.lock().await["main"].clone();
                let response = if mode == "business_error"
                    && script != crate::runtime::PROBE
                    && !script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/")
                {
                    json!({"id":command["id"],"error":"runtime_missing helper exception after side effect"})
                } else {
                    json!({"id":command["id"],"result":result})
                };
                handle_bridge_message(
                    &response.to_string(),
                    &client.tx,
                    &worker.clients,
                    &worker.connections,
                    &worker.pending,
                    &client.conn_id,
                )
                .await;
            }
        });
        (bridge, scripts, task, target)
    }

    #[tokio::test]
    async fn warm_runtime_transports_one_bundle_for_twenty_operations() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        for index in 0..20 {
            bridge
                .runtime
                .execute(
                    &bridge,
                    &target,
                    "workflow",
                    json!({"cmd":"execute"}),
                    1000,
                    &format!("operation-{index}"),
                )
                .await
                .unwrap();
        }
        let scripts = scripts.lock().unwrap();
        assert_eq!(
            scripts
                .iter()
                .filter(|script| script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/"))
                .count(),
            1
        );
        assert_eq!(
            scripts.len(),
            41,
            "twenty probes, twenty short commands and one bundle"
        );
        assert!(
            scripts
                .iter()
                .filter(|script| !script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/"))
                .all(|script| script.len() < 4096)
        );
        let metrics = bridge.runtime.diagnostics();
        assert_eq!(metrics["installSuccesses"], 1);
        assert_eq!(metrics["runtimeReuses"], 19);
        assert!(metrics["bundleBytesSent"].as_u64().unwrap() > 10000);
        assert!(metrics["commandBytesSent"].as_u64().unwrap() > 0);
        assert!(bridge.pending.lock().unwrap().is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn concurrent_runtime_preparation_is_single_flight() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        let mut joins = Vec::new();
        for index in 0..12 {
            let bridge = bridge.clone();
            let target = target.clone();
            joins.push(tokio::spawn(async move {
                bridge
                    .runtime
                    .ensure(
                        &bridge,
                        &target,
                        Instant::now() + Duration::from_secs(2),
                        &format!("prepare-{index}"),
                    )
                    .await
                    .unwrap()
            }));
        }
        let mut contexts = Vec::new();
        for join in joins {
            contexts.push(join.await.unwrap());
        }
        assert!(contexts.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(
            scripts
                .lock()
                .unwrap()
                .iter()
                .filter(|script| script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/"))
                .count(),
            1
        );
        task.abort();
    }

    #[tokio::test]
    async fn stale_operation_context_never_enters_business_transport() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        let error = bridge
            .runtime
            .execute(
                &bridge,
                &target,
                "picker",
                json!({"cmd":"status","context":{"pageEpoch":"obsolete"}}),
                1000,
                "old-picker",
            )
            .await
            .unwrap_err();
        assert!(!error.is_dispatched());
        assert_eq!(
            scripts.lock().unwrap().len(),
            2,
            "only probe and installer were sent"
        );
        task.abort();
    }

    #[tokio::test]
    async fn stale_fully_bound_cleanup_never_installs_a_new_runtime() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        for (module, field) in [
            ("picker", "context"),
            ("ipcCapture", "context"),
            ("geometry", "expectedContext"),
            ("workflow", "context"),
        ] {
            let mut args = json!({"cmd":"cleanup"});
            args[field] = json!({"pageEpoch":"old-page","runtimeId":"old-runtime"});
            let error = bridge
                .runtime
                .execute(
                    &bridge,
                    &target,
                    module,
                    args,
                    1000,
                    &format!("cleanup-{module}"),
                )
                .await
                .unwrap_err();
            assert!(!error.is_dispatched());
        }
        assert_eq!(bridge.runtime.diagnostics()["installAttempts"], 0);
        assert_eq!(bridge.runtime.diagnostics()["bundleBytesSent"], 0);
        assert_eq!(
            scripts.lock().unwrap().len(),
            4,
            "one readonly probe per bound followup"
        );
        assert!(
            scripts
                .lock()
                .unwrap()
                .iter()
                .all(|script| script == crate::runtime::PROBE)
        );
        task.abort();
    }

    #[tokio::test]
    async fn matching_fully_bound_operation_reuses_existing_runtime_without_install() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        let context = bridge
            .runtime
            .ensure(
                &bridge,
                &target,
                Instant::now() + Duration::from_secs(1),
                "initial",
            )
            .await
            .unwrap();
        let result = bridge
            .runtime
            .execute(
                &bridge,
                &target,
                "workflow",
                json!({"cmd":"execute","context":context}),
                1000,
                "bound-operation",
            )
            .await
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(bridge.runtime.diagnostics()["installAttempts"], 1);
        assert_eq!(
            scripts.lock().unwrap().len(),
            4,
            "cold probe+install then readonly probe+bound dispatch"
        );
        task.abort();
    }

    #[tokio::test]
    async fn anonymous_capability_views_match_without_installation_or_private_state() {
        let (bridge, mut rx) = queued_bridge().await;
        let directory = std::env::temp_dir().join(format!(
            "connector-capability-test-{}",
            uuid::Uuid::new_v4()
        ));
        let state = crate::state::PluginState::new(directory.clone()).unwrap();
        state
            .set_pointed_element(
                json!({"text":"private-capability-test-marker","pickerId":"private-handle"}),
            )
            .await;
        let bridge_status = bridge.status().await;
        let workflow =
            crate::workflow::call("workflow_capabilities", &json!({}), &bridge, None, &state)
                .await
                .unwrap();
        assert_eq!(bridge_status["inspection"], workflow["inspection"]);
        assert_eq!(
            bridge_status["inspection"],
            crate::capabilities::inspection()
        );
        assert!(
            !workflow["ops"]
                .as_array()
                .unwrap()
                .contains(&json!("webview_select_element"))
        );
        assert!(
            !workflow
                .to_string()
                .contains("private-capability-test-marker")
        );
        assert!(!workflow.to_string().contains("private-handle"));
        assert!(rx.try_recv().is_err());
        assert_eq!(bridge.runtime.diagnostics()["installAttempts"], 0);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn failed_installation_is_shared_and_not_repeated_in_same_document() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("install_failure").await;
        for index in 0..8 {
            let error = bridge
                .runtime
                .ensure(
                    &bridge,
                    &target,
                    Instant::now() + Duration::from_secs(1),
                    &format!("failed-{index}"),
                )
                .await
                .unwrap_err();
            assert!(!error.is_dispatched());
        }
        assert_eq!(
            scripts
                .lock()
                .unwrap()
                .iter()
                .filter(|script| script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/"))
                .count(),
            1
        );
        assert_eq!(bridge.runtime.diagnostics()["installAttempts"], 1);
        task.abort();
    }

    #[tokio::test]
    async fn runtime_business_error_is_never_reprepared_or_replayed() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("business_error").await;
        let error = bridge
            .runtime
            .execute(
                &bridge,
                &target,
                "workflow",
                json!({"cmd":"execute"}),
                1000,
                "write",
            )
            .await
            .unwrap_err();
        assert!(error.is_dispatched());
        assert!(error.to_string().contains("runtime_missing helper"));
        assert_eq!(
            scripts.lock().unwrap().len(),
            3,
            "probe, bundle, exactly one business dispatch"
        );
        task.abort();
    }

    #[tokio::test]
    async fn revocation_during_install_prevents_final_business_dispatch() {
        let (bridge, scripts, task, target) = simulated_runtime_bridge("ready").await;
        let authorized = || bridge.runtime.diagnostics()["installSuccesses"] == 0;
        let error = bridge
            .runtime
            .execute_authorized(
                &bridge,
                &target,
                "workflow",
                json!({"cmd":"execute"}),
                1000,
                "revoked-write",
                Some(&authorized),
            )
            .await
            .unwrap_err();
        assert!(!error.is_dispatched());
        assert!(error.to_string().contains("authorization"));
        assert_eq!(
            scripts.lock().unwrap().len(),
            2,
            "probe and preparation only"
        );
        assert!(bridge.pending.lock().unwrap().is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn health_separates_transport_from_unavailable_runtime_without_repair() {
        let (bridge, mut rx) = queued_bridge().await;
        let health = crate::health::query(&bridge, &json!({"depth":"runtime","timeoutMs":100}))
            .await
            .unwrap();
        assert_eq!(health["checks"]["transport"]["status"], "responsive");
        // No page bridge: unavailable, or starting during the startup grace.
        let runtime = health["checks"]["runtime"]["status"].as_str().unwrap();
        assert!(["unavailable", "starting"].contains(&runtime), "{health}");
        if runtime == "starting" {
            assert_eq!(health["conclusion"], "runtime_starting");
            assert!(health["retryAfterMs"].as_u64().is_some());
        }
        assert_eq!(health["executionReplayed"], false);
        assert!(rx.try_recv().is_err());
        assert_eq!(bridge.runtime.diagnostics()["installAttempts"], 0);
        for invalid in [
            json!({"depth":"reload"}),
            json!({"depth":42}),
            json!({"timeoutMs":99}),
            json!({"timeoutMs":10001}),
            json!({"windowId":""}),
            json!({"script":"business()"}),
        ] {
            assert!(crate::health::query(&bridge, &invalid).await.is_err());
        }
        let anonymous = bridge.status().await.to_string();
        assert!(!anonymous.contains("title"));
        assert!(!anonymous.contains("url"));
        assert!(!anonymous.contains("workspace"));
    }

    #[tokio::test]
    async fn bridge_hello_requires_host_window_binding() {
        let clients = Arc::new(Mutex::new(HashMap::new()));
        let connections = Arc::new(StdMutex::new(HashMap::new()));
        let pending = Arc::new(StdMutex::new(HashMap::new()));
        let (tx, _rx) = mpsc::unbounded_channel();
        let label = uuid::Uuid::new_v4().to_string();
        let key = crate::identity::bridge_key(&label);
        let invalid = json!({"type":"hello","windowId":label,"bridgeKey":"forged"});
        assert!(
            handle_bridge_message(
                &invalid.to_string(),
                &tx,
                &clients,
                &connections,
                &pending,
                "untrusted"
            )
            .await
            .is_none()
        );
        let valid = json!({"type":"hello","windowId":label,"bridgeKey":key});
        assert!(
            handle_bridge_message(
                &valid.to_string(),
                &tx,
                &clients,
                &connections,
                &pending,
                "bound"
            )
            .await
            .is_some()
        );
        crate::identity::page_navigation(&label);
        assert!(
            handle_bridge_message(
                &valid.to_string(),
                &tx,
                &clients,
                &connections,
                &pending,
                "stale-document"
            )
            .await
            .is_none()
        );
        crate::identity::window_destroyed(&label);
    }

    #[tokio::test]
    async fn ws_exception_does_not_attempt_eval_replay() {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window("writeThenThrow()", 1000, "main")
                .await
        });
        let command: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        let client = bridge.clients.lock().await["main"].clone();
        handle_bridge_message(
            &json!({"id": command["id"], "error": "fixture exception"}).to_string(),
            &client.tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            &client.conn_id,
        )
        .await;
        let error = task.await.unwrap().unwrap_err();
        assert!(error.contains("fixture exception"), "{error}");
        assert!(rx.try_recv().is_err());
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn ws_timeout_does_not_attempt_eval_replay() {
        let (bridge, mut rx) = queued_bridge().await;
        let error = bridge
            .execute_js_for_window("slowWrite()", 100, "main")
            .await
            .unwrap_err();
        let command: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert!(error.contains("outcome_unknown"), "{error}");
        assert!(error.contains(command["id"].as_str().unwrap()), "{error}");
        assert!(rx.try_recv().is_err());
        // The timed-out request stays registered so a late result can settle it,
        // but bridge_status no longer counts it as an in-flight request.
        assert!(
            bridge
                .pending
                .lock()
                .unwrap()
                .contains_key(command["id"].as_str().unwrap())
        );
        let status = bridge.status().await;
        assert_eq!(status["pending"], 0, "{status}");
        assert_eq!(status["awaitingLateResult"], 1, "{status}");
    }

    async fn late_settlement_after_timeout(
        page_result: serde_json::Value,
    ) -> (String, crate::workflow::resources::LateOutcome) {
        let (bridge, mut rx) = queued_bridge().await;
        let (seen_tx, seen_rx) = oneshot::channel();
        let seen_tx = StdMutex::new(Some(seen_tx));
        bridge.set_late_settlement_hook(Arc::new(move |id, outcome| {
            if let Some(tx) = seen_tx.lock().unwrap().take() {
                let _ = tx.send((id.to_string(), outcome));
            }
        }));
        let error = bridge
            .execute_js_for_window_typed("slowWrite()", 100, "main")
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::DispatchedOutcomeUnknown { .. }),
            "{error:?}"
        );
        let command: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        let id = command["id"].as_str().unwrap().to_string();
        let mut message = page_result;
        message["id"] = json!(id);
        super::test_page_message(&bridge, message).await;
        let (seen_id, outcome) = tokio::time::timeout(Duration::from_secs(5), seen_rx)
            .await
            .expect("late settlement reported")
            .unwrap();
        assert_eq!(seen_id, id);
        assert!(bridge.pending.lock().unwrap().is_empty());
        (seen_id, outcome)
    }

    #[tokio::test]
    async fn a_late_page_result_reports_success_after_timeout() {
        use crate::workflow::resources::LateOutcome;
        let (_, outcome) = late_settlement_after_timeout(json!({"result": 1})).await;
        assert_eq!(outcome, LateOutcome::Succeeded);
    }

    #[tokio::test]
    async fn a_late_effect_free_failure_is_reported_as_releasable() {
        use crate::workflow::resources::LateOutcome;
        let (_, outcome) =
            late_settlement_after_timeout(json!({"error": "boom", "effect": "none"})).await;
        assert_eq!(outcome, LateOutcome::FailedWithoutEffect);
        assert!(outcome.releases_quarantine());
        let (_, outcome) =
            late_settlement_after_timeout(json!({"error": "boom", "effect": "possible"})).await;
        assert_eq!(outcome, LateOutcome::Failed);
        assert!(!outcome.releases_quarantine());
    }

    #[tokio::test]
    async fn typed_calls_require_the_exact_window_label() {
        let (bridge, mut rx) = queued_bridge().await;
        {
            let mut clients = bridge.clients.lock().await;
            let mut client = clients.remove("main").unwrap();
            client.window_id = "secondary".into();
            clients.insert("secondary".into(), client);
        }
        let error = bridge
            .execute_js_for_window_typed("write()", 100, "main")
            .await
            .unwrap_err();
        assert!(matches!(error, BridgeError::NotDispatched { .. }));
        assert!(
            rx.try_recv().is_err(),
            "strict targeting must not dispatch into the sole other window"
        );
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn ws_slow_completion_waits_beyond_the_old_two_second_limit() {
        let (bridge, mut rx) = queued_bridge().await;
        let remote = bridge.clone();
        let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = effects.clone();
        let responder = tokio::spawn(async move {
            let command: serde_json::Value =
                serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(2100)).await;
            let client = remote.clients.lock().await["main"].clone();
            handle_bridge_message(
                &json!({"id": command["id"], "result": {"saved": true}}).to_string(),
                &client.tx,
                &remote.clients,
                &remote.connections,
                &remote.pending,
                &client.conn_id,
            )
            .await;
            assert!(rx.try_recv().is_err());
        });
        let result = bridge
            .execute_js_for_window_typed("slowWrite()", 5000, "main")
            .await
            .unwrap();
        assert_eq!(result, json!({"saved": true}));
        responder.await.unwrap();
        assert_eq!(effects.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejected_ws_enqueue_cleans_up_and_allows_eval_with_same_id() {
        let (bridge, rx) = queued_bridge().await;
        drop(rx);
        let request = DispatchRequest {
            id: "logical-id",
            deadline: Instant::now() + Duration::from_secs(1),
            authorize: None,
        };
        let error = bridge
            .execute_js_ws(&PageScript::trusted("write()"), "main", &request)
            .await
            .unwrap_err();
        assert!(matches!(error, BridgeError::NotDispatched { .. }));
        assert_eq!(error.request_id(), "logical-id");
        assert!(!error.is_dispatched());
        assert!(bridge.pending.lock().unwrap().is_empty());
        let error = bridge
            .execute_js_for_window_with_request_id("write()", 1000, "main", "logical-id")
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. } if reason.contains("App handle not set"))
        );
        assert_eq!(error.request_id(), "logical-id");
    }

    #[tokio::test]
    async fn fallback_does_not_reset_a_spent_deadline() {
        let (bridge, rx) = queued_bridge().await;
        drop(rx);
        // Hold the fallback prerequisite so it must consume the remaining budget.
        let _app_guard = bridge.app_handle.lock().await;
        let error = bridge
            .execute_js_for_window_with_request_id("write()", 2, "main", "same-id")
            .await
            .unwrap_err();
        assert!(
            matches!(error, BridgeError::NotDispatched { ref reason, .. } if reason.to_ascii_lowercase().contains("deadline expired"))
        );
        assert_eq!(error.request_id(), "same-id");
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropping_bridge_waiter_cleans_up_without_replaying_or_cancelling() {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window_typed("write()", 5000, "main")
                .await
        });
        rx.recv().await.unwrap();
        assert_eq!(bridge.pending.lock().unwrap().len(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(bridge.pending.lock().unwrap().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn disconnect_rejects_only_its_own_pending_requests_as_unknown() {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window_with_request_id("write()", 5000, "main", "write-id")
                .await
        });
        rx.recv().await.unwrap();
        let (other_tx, _other_rx) = oneshot::channel();
        bridge.pending.lock().unwrap().insert(
            "other-id".into(),
            PendingRequest {
                conn_id: "connection-b".into(),
                registration_id: uuid::Uuid::new_v4(),
                tx: other_tx,
                awaiting_late_result: false,
            },
        );
        reject_connection_pending(&bridge.pending, "connection-a");
        let error = task.await.unwrap().unwrap_err();
        assert!(matches!(
            error,
            BridgeError::DispatchedOutcomeUnknown { .. }
        ));
        assert_eq!(error.request_id(), "write-id");
        assert!(error.is_dispatched());
        assert!(bridge.pending.lock().unwrap().contains_key("other-id"));
    }

    #[tokio::test]
    async fn stale_connection_cannot_resolve_another_connections_operation() {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window_with_request_id("write()", 1000, "main", "write-id")
                .await
        });
        rx.recv().await.unwrap();
        let client = bridge.clients.lock().await["main"].clone();
        let result = json!({"id":"write-id", "result":"saved"}).to_string();
        handle_bridge_message(
            &result,
            &client.tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            "stale-connection",
        )
        .await;
        assert_eq!(bridge.pending.lock().unwrap().len(), 1);
        handle_bridge_message(
            &result,
            &client.tx,
            &bridge.clients,
            &bridge.connections,
            &bridge.pending,
            &client.conn_id,
        )
        .await;
        assert_eq!(task.await.unwrap().unwrap(), json!("saved"));
    }

    #[tokio::test]
    async fn duplicate_inflight_id_does_not_replace_waiter_or_dispatch_twice() {
        let (bridge, mut rx) = queued_bridge().await;
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window_with_request_id("write()", 1000, "main", "write-id")
                .await
        });
        rx.recv().await.unwrap();
        let error = bridge
            .execute_js_for_window_with_request_id("write()", 1000, "main", "write-id")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            BridgeError::DispatchedOutcomeUnknown { .. }
        ));
        assert_eq!(bridge.pending.lock().unwrap().len(), 1);
        assert!(rx.try_recv().is_err());
        task.abort();
        let _ = task.await;
        assert!(bridge.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_guard_runs_on_error_and_future_drop() {
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let counter = active.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = CleanupGuard(Some(|| {
                counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }));
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
        let mut cleaned = false;
        let mut fail = || -> Result<(), ()> {
            let _guard = CleanupGuard(Some(|| {
                cleaned = true;
            }));
            Err(())?;
            Ok(())
        };
        assert!(fail().is_err());
        assert!(cleaned);
    }

    #[test]
    fn generated_page_scripts_are_valid_javascript() {
        let Ok(probe) = std::process::Command::new("node").arg("--version").output() else {
            eprintln!("node unavailable; skipping generated script syntax check");
            return;
        };
        assert!(probe.status.success());
        let directory = std::env::temp_dir().join(format!("bridge-js-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        for (name, source) in [("init.js", bridge_init_script(9555, "main"))]
            .into_iter()
            .chain(
                test_native_scripts(
                    &PageScript::trusted("(async () => 1)()"),
                    "id",
                    Some("event"),
                )
                .into_iter()
                .chain(test_native_scripts(
                    &PageScript::with_inline("1".into(), vec!["2".into()]),
                    "id",
                    None,
                ))
                .enumerate()
                .map(|(index, source)| {
                    (
                        Box::leak(format!("native-{index}.js").into_boxed_str()) as &str,
                        source,
                    )
                }),
            )
        {
            let path = directory.join(name);
            std::fs::write(&path, source).unwrap();
            let check = std::process::Command::new("node")
                .arg("--check")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                check.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&check.stderr)
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// R2-4: the console buffer is capped at 500 entries and shifts; pending
    /// logs must still be handed over (on demand and by the periodic push)
    /// after it is full, and each entry exactly once.
    #[test]
    fn console_logs_keep_flushing_after_the_buffer_is_full() {
        let Ok(probe) = std::process::Command::new("node").arg("--version").output() else {
            eprintln!("node unavailable; skipping console flush check");
            return;
        };
        assert!(probe.status.success());
        let directory = std::env::temp_dir().join(format!("bridge-logs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let init = directory.join("init.js");
        let runner = directory.join("probe.cjs");
        std::fs::write(&init, bridge_init_script(9555, "main")).unwrap();
        std::fs::write(&runner, PROBE).unwrap();
        let output = std::process::Command::new("node")
            .arg(&runner)
            .arg(&init)
            .output()
            .unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result,
            json!({"first": 500, "firstHead": "line 20", "second": ["after cap"], "third": 0})
        );
        const PROBE: &str = r#"const fs = require('fs');
const src = fs.readFileSync(process.argv[2], 'utf8');
const any = () => new Proxy(function () {}, { get: (t, k) => k === Symbol.toPrimitive ? () => '' : k === 'then' ? undefined : any(), apply: () => any(), construct: () => any() });
const ctx = { console: { log() {}, warn() {}, error() {}, info() {}, debug() {} }, setTimeout() { return 0; }, clearTimeout() {}, setInterval() { return 0; }, clearInterval() {},
  MutationObserver: class { observe() {} }, addEventListener() {}, removeEventListener() {},
  __TAURI_INTERNALS__: { invoke() { return Promise.resolve(); } } };
ctx.window = ctx; ctx.top = ctx; ctx.self = ctx;
const scope = new Proxy(ctx, {
  has: () => true,
  get: (t, k) => k === Symbol.unscopables ? undefined : k in t ? t[k] : k in globalThis ? globalThis[k] : any(),
});
new Function('scope', 'with (scope) {' + src + '}')(scope);
for (let i = 0; i < 520; i++) ctx.console.log('line ' + i);
const first = ctx.__CONNECTOR_TAKE_LOGS__();
ctx.console.log('after cap');
const second = ctx.__CONNECTOR_TAKE_LOGS__();
process.stdout.write(JSON.stringify({ first: first.length, firstHead: first[0].message, second: second.map(e => e.message), third: ctx.__CONNECTOR_TAKE_LOGS__().length }));
"#;
    }

    #[tokio::test]
    async fn page_effect_report_reaches_execution_failure() {
        use connector_client::outcome::EffectStatus;
        for (report, expected) in [
            (Some("none"), EffectStatus::None),
            (Some("possible"), EffectStatus::Possible),
            (Some("forged"), EffectStatus::Possible),
            (None, EffectStatus::Possible),
        ] {
            let (bridge, mut rx) = queued_bridge().await;
            let worker = bridge.clone();
            let task = tokio::spawn(async move {
                worker
                    .execute_js_for_window_typed("readThenThrow()", 1000, "main")
                    .await
            });
            let command: serde_json::Value =
                serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            let mut message = json!({"id": command["id"], "error": "page threw"});
            if let Some(report) = report {
                message["effect"] = json!(report);
            }
            super::test_page_message(&bridge, message).await;
            let Err(BridgeError::ExecutionFailed { effect, .. }) = task.await.unwrap() else {
                panic!("expected an execution failure");
            };
            assert_eq!(effect, expected, "{report:?}");
        }
    }

    #[test]
    fn native_scripts_embed_identity_as_json_data() {
        let script = "({value: '\\u4f60', text: \"${globalThis.unintended++} ` \\\\ \\\"\"})";
        let id = "id'`\\\"\\n${unsafe}";
        let generated =
            test_native_scripts(&PageScript::trusted(script), id, Some("event")).join("\n");
        assert!(
            generated.contains(script),
            "trusted code is inlined as code"
        );
        assert!(generated.contains(&format!("var id={}", serde_json::to_string(id).unwrap())));
        assert!(
            !generated.contains("+`")
                && !generated.contains("AsyncFunction")
                && !generated.contains("new Function"),
            "native scripts must not compile strings or use template literals"
        );
        let cases = json!([{"id": "t", "strings": false, "scripts": test_native_scripts(&PageScript::trusted(script), id, Some("event"))}]);
        let Some(out) = run_native_harness(&cases) else {
            return;
        };
        assert_eq!(out["t"]["id"], json!(id));
        assert_eq!(out["t"]["result"]["value"], json!("\u{4f60}"));
    }

    #[test]
    fn trusted_scripts_run_under_a_csp_that_blocks_eval() {
        let ok = PageScript::trusted("(async () => ({ n: 1 + 1 }))()");
        let bad = PageScript::trusted(")(");
        let throws = PageScript::trusted("(() => { throw new Error('boom') })()");
        let cases = json!([
            {"id": "ws-ok", "strings": false, "scripts": test_native_scripts(&ok, "a", None)},
            {"id": "ev-ok", "strings": false, "scripts": test_native_scripts(&ok, "b", Some("evt"))},
            {"id": "ws-bad", "strings": false, "scripts": test_native_scripts(&bad, "c", None)},
            {"id": "ev-bad", "strings": false, "scripts": test_native_scripts(&bad, "d", Some("evt"))},
            {"id": "throws", "strings": false, "scripts": test_native_scripts(&throws, "e", None)},
        ]);
        let Some(out) = run_native_harness(&cases) else {
            eprintln!("node unavailable; skipping");
            return;
        };
        assert_eq!(out["ws-ok"]["result"], json!({"n": 2}));
        assert_eq!(out["ev-ok"]["result"], json!({"n": 2}));
        assert_eq!(out["ev-ok"]["event"], json!("evt"));
        for case in ["ws-bad", "ev-bad"] {
            assert!(
                out[case]["error"]
                    .as_str()
                    .unwrap()
                    .contains(PAGE_SCRIPT_SYNTAX),
                "{case}: {}",
                out[case]
            );
            assert_eq!(out[case]["effect"], json!("none"));
        }
        assert_eq!(out["throws"]["error"], json!("boom"));
    }

    #[tokio::test]
    async fn eval_blocked_clients_receive_native_code_and_reply_over_the_socket() {
        let (bridge, mut rx) = queued_bridge().await;
        bridge
            .clients
            .lock()
            .await
            .get_mut("main")
            .unwrap()
            .eval_blocked = true;
        let (sink_tx, mut sink_rx) = mpsc::unbounded_channel();
        *bridge.native_sink.lock().unwrap() = Some(sink_tx);
        assert_eq!(bridge.eval_blocked("main").await, Some(true));
        let worker = bridge.clone();
        let task = tokio::spawn(async move {
            worker
                .execute_js_for_window_with_request_id("1 + 1", 1000, "main", "native-id")
                .await
        });
        let first = sink_rx.recv().await.unwrap();
        assert!(first.contains("__CONNECTOR_RUN__") && first.contains("1 + 1"));
        let done = sink_rx.recv().await.unwrap();
        assert!(done.contains(PAGE_SCRIPT_SYNTAX));
        assert!(rx.try_recv().is_err(), "no string command over the socket");
        super::test_page_message(&bridge, json!({"id": "native-id", "result": 2})).await;
        assert_eq!(task.await.unwrap().unwrap(), json!(2));
    }

    #[tokio::test]
    async fn hello_records_eval_blocked_and_status_reports_it() {
        let (bridge, _rx) = queued_bridge().await;
        let key = crate::identity::bridge_key("main");
        super::test_page_message(
            &bridge,
            json!({"type": "hello", "id": "__bridge_hello__", "bridgeKey": key, "windowId": "main", "evalBlocked": true}),
        )
        .await;
        assert_eq!(bridge.eval_blocked("main").await, Some(true));
        let status = bridge.status().await;
        assert!(
            status.to_string().contains("\"evalBlocked\":true"),
            "{status}"
        );
    }
}
