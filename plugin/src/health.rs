//! Bounded diagnostics deliberately bypass business dispatch and installation.
use crate::{bridge::Bridge, identity};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::Instant;

/// A freshly launched app is still loading its first page for this long.
const STARTUP_GRACE_MS: u64 = 15_000;
const STARTING_RETRY_MS: u64 = 250;

fn process_age_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| now.as_millis() as u64)
        .unwrap_or(0)
        .saturating_sub(identity::started_at())
}

/// Classify a completed runtime probe. The runtime is installed lazily, so an
/// absent runtime on a loaded page is the normal cold state, not a fault; a
/// page that is still loading is transient and reported as starting.
fn classify_probe(probe: &Value, starting: bool) -> (&'static str, &'static str) {
    if probe["runtime"]["ready"] == true {
        ("responsive", "runtime_responsive")
    } else if probe["runtime"].is_null() {
        let loaded = probe["readyState"]
            .as_str()
            .is_none_or(|state| state == "complete");
        if starting && !loaded
            || probe["readyState"]
                .as_str()
                .is_some_and(|state| state == "loading")
        {
            ("starting", "runtime_starting")
        } else {
            ("not_installed", "runtime_not_installed")
        }
    } else {
        ("unavailable", "runtime_stale")
    }
}

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
    let starting = identity::page_loading(window) || process_age_ms() < STARTUP_GRACE_MS;
    let (status, conclusion) = match probe {
        Some(Ok(probe)) => {
            active_observers = probe["runtime"]["activeObservers"].clone();
            input_guard = probe["runtime"]["inputGuard"].clone();
            if let Some(context) = probe["runtime"].get("context") {
                result["context"] = context.clone();
            }
            classify_probe(&probe, starting)
        }
        Some(Err(crate::bridge::BridgeError::NotDispatched { .. })) if starting => {
            ("starting", "runtime_starting")
        }
        Some(Err(crate::bridge::BridgeError::NotDispatched { .. })) => {
            ("unavailable", "runtime_target_unavailable")
        }
        Some(Err(_)) | None => ("unresponsive", "runtime_probe_not_completed"),
    };
    match conclusion {
        "runtime_starting" => {
            result["retryAfterMs"] = json!(STARTING_RETRY_MS);
            result["note"] = json!(
                "The page is still loading or its bridge has not connected yet; retry shortly"
            );
        }
        "runtime_not_installed" => {
            result["note"] = json!(
                "Normal cold state: the inspection runtime installs on the first inspection command (snapshot, find, locator, screenshot, wait)"
            );
        }
        _ => {}
    }
    result["checks"]["runtime"] =
        json!({"status":status,"durationMs":probe_start.elapsed().as_millis() as u64});
    result["conclusion"] = json!(conclusion);
    result["diagnostics"] = bridge.runtime_diagnostics();
    result["diagnostics"]["activeObservers"] = active_observers;
    result["diagnostics"]["inputGuard"] = input_guard;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::classify_probe;
    use serde_json::json;

    #[test]
    fn a_cold_loaded_page_is_not_installed_rather_than_missing() {
        let probe = json!({"available":false,"readyState":"complete","runtime":null});
        assert_eq!(classify_probe(&probe, false).1, "runtime_not_installed");
        assert_eq!(classify_probe(&probe, true).1, "runtime_not_installed");
    }

    #[test]
    fn a_loading_page_is_starting() {
        let probe = json!({"available":false,"readyState":"loading","runtime":null});
        assert_eq!(
            classify_probe(&probe, false),
            ("starting", "runtime_starting")
        );
        let interactive = json!({"available":false,"readyState":"interactive","runtime":null});
        assert_eq!(classify_probe(&interactive, true).1, "runtime_starting");
        assert_eq!(
            classify_probe(&interactive, false).1,
            "runtime_not_installed"
        );
    }

    #[test]
    fn ready_and_stale_runtimes_keep_their_conclusions() {
        assert_eq!(
            classify_probe(&json!({"runtime":{"ready":true}}), true).1,
            "runtime_responsive"
        );
        assert_eq!(
            classify_probe(&json!({"runtime":{"ready":false}}), false).1,
            "runtime_stale"
        );
    }
}
