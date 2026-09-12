//! Bounded diagnostics deliberately bypass business dispatch and installation.
use crate::{bridge::Bridge, identity};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::Instant;

pub async fn query(bridge: &Bridge, args: &Value) -> Result<Value, String> {
    let fields = args
        .as_object()
        .ok_or("Health arguments must be an object")?;
    if fields
        .keys()
        .any(|key| !["authToken", "windowId", "depth", "timeoutMs"].contains(&key.as_str()))
    {
        return Err("Unknown health argument".into());
    }
    for field in ["windowId", "depth"] {
        if args.get(field).is_some_and(|value| {
            value
                .as_str()
                .is_none_or(|text| text.is_empty() || text.len() > 128)
        }) {
            return Err(format!("{field} must be a bounded nonempty string"));
        }
    }
    let window = args
        .get("windowId")
        .and_then(Value::as_str)
        .unwrap_or("main");
    let depth = args
        .get("depth")
        .and_then(Value::as_str)
        .unwrap_or("runtime");
    if !["transport", "bridge", "runtime"].contains(&depth) {
        return Err("depth must be transport, bridge, or runtime".into());
    }
    let timeout = args
        .get("timeoutMs")
        .map(|value| value.as_u64().ok_or("timeoutMs must be an integer"))
        .transpose()?
        .unwrap_or(2000);
    if !(100..=10000).contains(&timeout) {
        return Err("timeoutMs must be between 100 and 10000".into());
    }
    let start = Instant::now();
    let deadline = start + Duration::from_millis(timeout);
    let mut result = json!({"context":{"appInstanceId":identity::app_instance_id(),"windowId":window},"checks":{"transport":{"status":"responsive","durationMs":0},"bridge":{"status":"not_checked","durationMs":0},"runtime":{"status":"not_checked","durationMs":0}},"conclusion":"transport_responsive","executionReplayed":false,"limitations":["Probe success or timeout does not establish the original operation outcome or a deadlock"]});
    if depth == "transport" {
        return Ok(result);
    }
    let connected = tokio::time::timeout_at(deadline, bridge.bridge_connected(window))
        .await
        .unwrap_or(false);
    let bridge_budget = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u64;
    let bridge_probe = if bridge_budget > 0 {
        tokio::time::timeout_at(deadline, bridge.probe_bridge(window, bridge_budget))
            .await
            .ok()
    } else {
        None
    };
    let bridge_status = match bridge_probe {
        Some(Ok(value)) if value["responsive"] == true => "responsive",
        Some(Ok(_)) | Some(Err(crate::bridge::BridgeError::NotDispatched { .. })) => "unavailable",
        _ => "unresponsive",
    };
    result["checks"]["bridge"] = json!({"status":bridge_status,"connected":connected,"durationMs":start.elapsed().as_millis() as u64});
    result["conclusion"] = json!(format!("bridge_{bridge_status}"));
    if depth == "bridge" {
        return Ok(result);
    }
    let probe_start = Instant::now();
    let budget = deadline.saturating_duration_since(probe_start).as_millis() as u64;
    let probe = if budget > 0 {
        tokio::time::timeout_at(deadline, bridge.probe_runtime(window, budget))
            .await
            .ok()
    } else {
        None
    };
    let mut active_observers = Value::Null;
    let mut input_guard = Value::Null;
    let (status, conclusion) = match probe {
        Some(Ok(probe)) => {
            active_observers = probe["runtime"]["activeObservers"].clone();
            input_guard = probe["runtime"]["inputGuard"].clone();
            if let Some(context) = probe["runtime"].get("context") {
                result["context"] = context.clone();
            }
            if probe["runtime"]["ready"] == true {
                ("responsive", "runtime_responsive")
            } else if probe["runtime"].is_null() {
                ("unavailable", "runtime_missing")
            } else {
                ("unavailable", "runtime_stale")
            }
        }
        Some(Err(crate::bridge::BridgeError::NotDispatched { .. })) => {
            ("unavailable", "runtime_target_unavailable")
        }
        Some(Err(_)) | None => ("unresponsive", "runtime_probe_not_completed"),
    };
    result["checks"]["runtime"] =
        json!({"status":status,"durationMs":probe_start.elapsed().as_millis() as u64});
    result["conclusion"] = json!(conclusion);
    result["diagnostics"] = bridge.runtime_diagnostics();
    result["diagnostics"]["activeObservers"] = active_observers;
    result["diagnostics"]["inputGuard"] = input_guard;
    Ok(result)
}
