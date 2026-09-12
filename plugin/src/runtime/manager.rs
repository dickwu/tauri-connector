use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::manifest;
use crate::bridge::{Bridge, BridgeError};
use crate::identity::{self, ExecutionContext};

#[derive(Default)]
struct InstallationFlight {
    failed_for: Option<String>,
}

#[derive(Default)]
pub struct RuntimeManager {
    flights: StdMutex<HashMap<String, Arc<Mutex<InstallationFlight>>>>,
    install_attempts: AtomicU64,
    install_successes: AtomicU64,
    bundle_bytes_sent: AtomicU64,
    command_bytes_sent: AtomicU64,
    runtime_reuses: AtomicU64,
    stale_context_rejects: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct RuntimeTarget {
    pub app_id: String,
    pub window_id: String,
    pub window_instance_id: String,
    pub origin: String,
    pub url: String,
}

fn not_dispatched(request_id: &str, reason: impl Into<String>) -> BridgeError {
    BridgeError::NotDispatched {
        request_id: request_id.into(),
        reason: reason.into(),
    }
}

pub fn remaining(deadline: Instant, request_id: &str) -> Result<u64, BridgeError> {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u64;
    if remaining == 0 {
        Err(not_dispatched(
            request_id,
            "Runtime preparation deadline expired",
        ))
    } else {
        Ok(remaining)
    }
}

impl RuntimeManager {
    pub fn window_destroyed(&self, window_id: &str) {
        self.flights
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(window_id);
    }

    /// Accounting occurs at bridge transport enqueue/eval, not source creation.
    pub fn record_transmission(&self, script: &str, bytes: usize) {
        if script.starts_with("/*__CONNECTOR_INSTALL_BUNDLE__*/") {
            self.bundle_bytes_sent
                .fetch_add(bytes as u64, Ordering::Relaxed);
        } else if script.starts_with("/*__CONNECTOR_RUNTIME_COMMAND__*/") {
            self.command_bytes_sent
                .fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }
    pub fn diagnostics(&self) -> Value {
        json!({"installAttempts":self.install_attempts.load(Ordering::Relaxed),"installSuccesses":self.install_successes.load(Ordering::Relaxed),"bundleBytesSent":self.bundle_bytes_sent.load(Ordering::Relaxed),"commandBytesSent":self.command_bytes_sent.load(Ordering::Relaxed),"runtimeReuses":self.runtime_reuses.load(Ordering::Relaxed),"staleContextRejects":self.stale_context_rejects.load(Ordering::Relaxed)})
    }

    async fn observe(
        &self,
        bridge: &Bridge,
        target: &RuntimeTarget,
        deadline: Instant,
        request_id: &str,
    ) -> Result<Value, BridgeError> {
        let probe = bridge
            .execute_js_for_window_with_request_id(
                super::PROBE,
                remaining(deadline, request_id)?,
                &target.window_id,
                &format!("{request_id}:probe"),
            )
            .await
            .map_err(|error| {
                not_dispatched(request_id, format!("Runtime context probe failed: {error}"))
            })?;
        if probe["available"] != true
            || probe["suspended"] == true
            || probe["url"].as_str() != Some(target.url.as_str())
            || (target.origin != "null" && probe["origin"].as_str() != Some(target.origin.as_str()))
        {
            return Err(not_dispatched(
                request_id,
                "Trusted bootstrap or allowed origin unavailable",
            ));
        }
        Ok(probe)
    }

    fn expected_context(
        &self,
        target: &RuntimeTarget,
        probe: &Value,
        request_id: &str,
    ) -> Result<ExecutionContext, BridgeError> {
        let page_epoch = probe["pageEpoch"]
            .as_str()
            .filter(|epoch| !epoch.is_empty() && epoch.len() <= 256)
            .ok_or_else(|| not_dispatched(request_id, "Missing page lifecycle identity"))?;
        Ok(ExecutionContext {
            app_id: target.app_id.clone(),
            app_instance_id: identity::app_instance_id().into(),
            window_id: target.window_id.clone(),
            window_instance_id: target.window_instance_id.clone(),
            page_epoch: page_epoch.into(),
            runtime_id: String::new(),
            runtime_version: manifest::RUNTIME_VERSION.into(),
            semantic_version: manifest::SEMANTIC_VERSION.into(),
            bundle_hash: manifest::bundle_hash(),
            origin: probe["origin"].as_str().unwrap_or_default().into(),
        })
    }

    fn confirm_existing(
        &self,
        probe: &Value,
        expected: &ExecutionContext,
        request_id: &str,
    ) -> Result<ExecutionContext, BridgeError> {
        if probe["runtime"]["ready"] != true {
            self.stale_context_rejects.fetch_add(1, Ordering::Relaxed);
            return Err(not_dispatched(
                request_id,
                "Bound runtime is missing or stale; no installation permitted",
            ));
        }
        let existing: ExecutionContext =
            serde_json::from_value(probe["runtime"]["context"].clone())
                .map_err(|_| not_dispatched(request_id, "Invalid runtime identity handshake"))?;
        let mut bound = expected.clone();
        bound.runtime_id = existing.runtime_id.clone();
        if bound != existing
            || existing.runtime_id.is_empty()
            || existing.runtime_id.len() > 256
            || probe["runtime"]["runtimeProtocolVersion"] != manifest::PROTOCOL_VERSION
        {
            self.stale_context_rejects.fetch_add(1, Ordering::Relaxed);
            return Err(not_dispatched(
                request_id,
                "Runtime identity/version mismatch; reload required",
            ));
        }
        self.runtime_reuses.fetch_add(1, Ordering::Relaxed);
        Ok(existing)
    }

    pub async fn ensure(
        &self,
        bridge: &Bridge,
        target: &RuntimeTarget,
        deadline: Instant,
        request_id: &str,
    ) -> Result<ExecutionContext, BridgeError> {
        manifest::validate(&manifest::modules())
            .map_err(|reason| not_dispatched(request_id, reason))?;
        let flight = {
            let mut flights = self.flights.lock().unwrap_or_else(|p| p.into_inner());
            // A reused label replaces the retired window's key. This is bounded
            // by host windows, with a hard ceiling for abandoned labels.
            if flights.len() >= 128 && !flights.contains_key(&target.window_id) {
                return Err(not_dispatched(
                    request_id,
                    "Runtime window capacity exceeded",
                ));
            }
            flights.entry(target.window_id.clone()).or_default().clone()
        };
        let mut flight = tokio::time::timeout_at(deadline, flight.lock())
            .await
            .map_err(|_| not_dispatched(request_id, "Runtime installation wait expired"))?;
        let probe = self.observe(bridge, target, deadline, request_id).await?;
        let mut expected = self.expected_context(target, &probe, request_id)?;
        if probe["runtime"]["ready"] == true {
            let existing = self.confirm_existing(&probe, &expected, request_id)?;
            flight.failed_for = None;
            return Ok(existing);
        }
        expected.runtime_id = uuid::Uuid::new_v4().to_string();
        let page_epoch = &expected.page_epoch;
        let installation_key = format!(
            "{}:{}:{}",
            target.window_instance_id, page_epoch, expected.bundle_hash
        );
        if flight.failed_for.as_ref() == Some(&installation_key) {
            return Err(not_dispatched(
                request_id,
                "Runtime installation already failed in this document; reload required",
            ));
        }
        // Waiting callers observe the same failed attempt until the document
        // changes or a successful ready probe proves the installation completed.
        flight.failed_for = Some(installation_key);
        let script = super::install_script(
            &json!({"context":expected,"origin":expected.origin,"runtimeVersion":manifest::RUNTIME_VERSION,"semanticVersion":manifest::SEMANTIC_VERSION,"bundleHash":expected.bundle_hash}),
        );
        self.install_attempts.fetch_add(1, Ordering::Relaxed);
        let response = bridge
            .execute_js_for_window_with_request_id(
                &script,
                remaining(deadline, request_id)?,
                &target.window_id,
                &format!("{request_id}:install"),
            )
            .await
            .map_err(|error| {
                not_dispatched(
                    request_id,
                    format!("Runtime installation failed before business dispatch: {error}"),
                )
            })?;
        let actual = serde_json::from_value::<ExecutionContext>(response["context"].clone()).ok();
        if response["ready"] != true
            || actual.as_ref() != Some(&expected)
            || response["runtimeProtocolVersion"] != manifest::PROTOCOL_VERSION
        {
            return Err(not_dispatched(
                request_id,
                "Runtime installation handshake rejected",
            ));
        }
        // Host lifecycle may change while the page answers an old connection.
        if identity::window_instance_id(&target.window_id) != target.window_instance_id {
            return Err(not_dispatched(
                request_id,
                "Window lifetime changed during runtime installation",
            ));
        }
        flight.failed_for = None;
        self.install_successes.fetch_add(1, Ordering::Relaxed);
        Ok(expected)
    }

    #[cfg(test)]
    pub async fn execute(
        &self,
        bridge: &Bridge,
        target: &RuntimeTarget,
        module: &str,
        args: Value,
        timeout_ms: u64,
        request_id: &str,
    ) -> Result<Value, BridgeError> {
        self.execute_authorized(bridge, target, module, args, timeout_ms, request_id, None)
            .await
    }

    // The additional argument is the host revocation check used at the exact
    // transport enqueue boundary; it cannot be part of client-controlled JSON.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_authorized(
        &self,
        bridge: &Bridge,
        target: &RuntimeTarget,
        module: &str,
        args: Value,
        timeout_ms: u64,
        request_id: &str,
        authorize: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<Value, BridgeError> {
        if authorize.is_some_and(|check| !check()) {
            return Err(not_dispatched(request_id, "Host authorization revoked"));
        }
        if !manifest::modules()
            .iter()
            .any(|entry| entry.module_id == module)
            || module == "semantic"
        {
            return Err(not_dispatched(
                request_id,
                "Unsupported trusted runtime module",
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let fully_bound = ["context", "expectedContext"].iter().any(|field| {
            args.get(*field).is_some_and(|context| {
                context.get("runtimeId").is_some() && context.get("pageEpoch").is_some()
            })
        });
        let context = if fully_bound {
            // A retained operation/cleanup identity cannot authorize installing
            // a new page runtime, even when business dispatch will be rejected.
            let probe = self.observe(bridge, target, deadline, request_id).await?;
            let expected = self.expected_context(target, &probe, request_id)?;
            self.confirm_existing(&probe, &expected, request_id)?
        } else {
            self.ensure(bridge, target, deadline, request_id).await?
        };
        let actual = serde_json::to_value(&context).expect("context serializes");
        for field in ["expectedContext", "context"] {
            if let Some(expected) = args.get(field).filter(|value| !value.is_null())
                && expected.as_object().is_none_or(|fields| {
                    fields
                        .iter()
                        .any(|(key, value)| actual.get(key) != Some(value))
                })
            {
                self.stale_context_rejects.fetch_add(1, Ordering::Relaxed);
                return Err(not_dispatched(
                    request_id,
                    "Requested runtime context is stale",
                ));
            }
        }
        let packet = json!({"runtimeProtocolVersion":manifest::PROTOCOL_VERSION,"requestId":request_id,"expectedContext":context,"module":module,"operation":"dispatch","remainingMs":remaining(deadline,request_id)?,"args":args});
        let script = super::dispatch_script(&packet);
        // Never retry this call, including runtime_missing and helper-text errors.
        // Runtime repair belongs to preparation of the next authorized operation.
        bridge
            .execute_js_authorized(
                &script,
                remaining(deadline, request_id)?,
                &target.window_id,
                request_id,
                authorize,
            )
            .await
    }
}
