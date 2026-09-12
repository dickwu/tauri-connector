//! Keep protocol identity separate from untrusted strings when projecting reports.
//!
//! A secret can be as short as "a" or equal "completed". Replacing it throughout
//! a report changes UUIDs and state-machine enums, breaking recovery and lookup.
//! Payloads may be redacted; trusted metadata must retain its original meaning.

use serde_json::{Map, Value, json};

const CONTEXT_METADATA: &[&str] = &[
    "appId",
    "appInstanceId",
    "windowId",
    "windowInstanceId",
    "pageEpoch",
    "runtimeId",
    "runtimeVersion",
    "semanticVersion",
    "bundleHash",
];

const REPORT_METADATA: &[&str] = &[
    "runId",
    "appInstanceId",
    "buildId",
    "revision",
    "cursor",
    "checkpointId",
    "status",
    "reason",
    "originalTestVerdict",
    "goalStatus",
    "recoveryOccurred",
    "mayHaveEffects",
    "summary",
    "blockedStep",
    "allowedNextActions",
    "evidenceRefs",
    "coverage",
    "dispatchContext",
    "resourceIsolation",
    "cancellation",
    "recoveredFromJournal",
    "lateOutcome",
];

const OUTCOME_METADATA: &[&str] = &[
    "execution",
    "verification",
    "effect",
    "effectScope",
    "timing",
    "dispatch",
    "evidenceRefs",
    "coverage",
];

/// Project an in-memory report. Result values stay useful to callers after
/// redaction, while metadata remains usable for polling, cancellation and CAS.
pub(super) fn public_report(report: &Value, secrets: &[String]) -> Value {
    let Some(object) = report.as_object() else {
        return Value::Null;
    };
    Value::Object(
        object
            .iter()
            .map(|(key, value)| {
                let value = match key.as_str() {
                    "steps" => Value::Array(
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|step| {
                                let mut projected = pick(step, &["id", "status"]);
                                if let Some(outcome) = step.get("outcome") {
                                    projected["outcome"] = public_outcome(outcome, secrets);
                                }
                                projected
                            })
                            .collect(),
                    ),
                    "blockedOutcome" => public_outcome(value, secrets),
                    "dispatchContext" => dispatch_metadata(value),
                    "events" => Value::Array(
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|event| pick(event, &["seq", "event", "status"]))
                            .collect(),
                    ),
                    "evidence" => Value::Object(
                        value
                            .as_object()
                            .into_iter()
                            .flatten()
                            .map(|(id, item)| {
                                let mut projected = evidence_metadata(item);
                                if let Some(details) = item.get("details") {
                                    projected["details"] = redact_payload(details, secrets);
                                }
                                (id.clone(), projected)
                            })
                            .collect(),
                    ),
                    key if REPORT_METADATA.contains(&key) => value.clone(),
                    _ => redact_payload(value, secrets),
                };
                (key.clone(), value)
            })
            .collect(),
    )
}

/// Only metadata needed to inspect/recover a logical run may reach disk.
/// Unknown fields, raw outcome values, errors, warnings and evidence bodies
/// are omitted even if their sensitive contents were never supplied as inputs.
pub(super) fn journal_report(report: &Value, _secrets: &[String]) -> Value {
    let mut projected = pick(report, REPORT_METADATA);
    projected["summary"] = pick(
        &report["summary"],
        &["completedSteps", "remainingSteps", "blockedStep"],
    );
    if let Some(context) = report.get("dispatchContext") {
        projected["dispatchContext"] = dispatch_metadata(context);
    }
    if let Some(coverage) = report.get("coverage") {
        projected["coverage"] = coverage_metadata(coverage);
    }
    if let Some(cancellation) = report.get("cancellation") {
        projected["cancellation"] = pick(
            cancellation,
            &["requested", "inFlight", "rollback", "acknowledged"],
        );
    }
    if let Some(late) = report.get("lateOutcome") {
        projected["lateOutcome"] = pick(
            late,
            &[
                "execution",
                "verification",
                "effect",
                "correlation",
                "businessPersistence",
                "originalActionReplayed",
                "quarantineReleased",
            ],
        );
    }
    if let Some(outcome) = report.get("blockedOutcome") {
        let mut filtered = pick(
            outcome,
            &[
                "execution",
                "verification",
                "effect",
                "effectScope",
                "evidenceRefs",
            ],
        );
        if let Some(dispatch) = outcome.get("dispatch") {
            filtered["dispatch"] = dispatch_metadata(dispatch);
        }
        if let Some(coverage) = outcome.get("coverage") {
            filtered["coverage"] = coverage_metadata(coverage);
        }
        if let Some(error) = outcome.get("error").filter(|error| error.is_object()) {
            filtered["error"] = pick(error, &["code", "stage", "retryableBeforeDispatch"]);
        }
        projected["blockedOutcome"] = filtered;
    }
    projected
}

pub(super) fn dispatch_metadata(value: &Value) -> Value {
    pick(
        value,
        &[
            "requestId",
            "appId",
            "appInstanceId",
            "windowInstanceId",
            "runtimeId",
            "runtimeVersion",
            "semanticVersion",
            "bundleHash",
            "windowId",
            "pageEpoch",
            "attempt",
            "dispatched",
        ],
    )
}

fn coverage_metadata(value: &Value) -> Value {
    let mut projected = pick(
        value,
        &[
            "correlation",
            "businessPersistence",
            "truncated",
            "droppedEvents",
        ],
    );
    if let Some(sources) = value.get("sources") {
        projected["sources"] = pick(
            sources,
            &["dom", "frontendRuntime", "rustTrace", "businessState"],
        );
    }
    projected
}

fn evidence_metadata(value: &Value) -> Value {
    let mut projected = pick(value, &["captureKind", "available", "scope", "truncated"]);
    if let Some(context) = value.get("context") {
        projected["context"] = pick(context, CONTEXT_METADATA);
    }
    if let Some(elements) = value.get("elements").and_then(Value::as_array) {
        projected["elements"] = Value::Array(
            elements
                .iter()
                .map(|element| pick(element, &["tag", "role", "visible", "enabled", "editable"]))
                .collect(),
        );
    }
    if let Some(observation) = value.get("observation") {
        projected["observation"] = pick(observation, &["active", "mutationCount", "sampleCount"]);
    }
    if let Some(coverage) = value.get("coverage") {
        projected["coverage"] = coverage_metadata(coverage);
    }
    projected
}

fn public_outcome(outcome: &Value, secrets: &[String]) -> Value {
    let mut projected = pick(outcome, OUTCOME_METADATA);
    if let Some(data) = outcome.get("data") {
        projected["data"] = redact_payload(data, secrets);
    }
    if let Some(error) = outcome.get("error") {
        if error.is_object() {
            let mut filtered = pick(error, &["code", "stage", "retryableBeforeDispatch"]);
            if let Some(message) = error.get("message") {
                filtered["message"] = redact_payload(message, secrets);
            }
            projected["error"] = filtered;
        } else {
            projected["error"] = Value::Null;
        }
    }
    if let Some(warnings) = outcome.get("warnings") {
        projected["warnings"] = redact_payload(warnings, secrets);
    }
    projected
}

fn pick(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(*key).map(|value| ((*key).into(), value.clone())))
            .collect(),
    )
}

/// Only call on free-form payloads, never on a complete protocol report.
pub(super) fn redact_payload(value: &Value, secrets: &[String]) -> Value {
    match value {
        Value::String(string) => Value::String(redact_string(string, secrets)),
        Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|value| redact_payload(value, secrets))
                .collect(),
        ),
        Value::Object(object) if object.get("sensitive").and_then(Value::as_bool) == Some(true) => {
            json!({"redacted":true,"reason":"sensitive_source"})
        }
        Value::Object(object) => {
            let mut projected = Map::new();
            for (key, value) in object {
                let lower = key.to_ascii_lowercase();
                let value = if [
                    "password",
                    "token",
                    "secret",
                    "authorization",
                    "cookie",
                    "credential",
                ]
                .iter()
                .any(|sensitive| lower.contains(sensitive))
                {
                    json!("[redacted]")
                } else {
                    redact_payload(value, secrets)
                };
                // Payload keys can themselves contain input data. This never
                // applies to protocol field names or run/step identifiers.
                projected.insert(redact_string(key, secrets), value);
            }
            Value::Object(projected)
        }
        value => {
            let encoded = value.to_string();
            if secrets
                .iter()
                .any(|secret| !secret.is_empty() && encoded == *secret)
            {
                json!("[redacted]")
            } else {
                value.clone()
            }
        }
    }
}

fn redact_string(string: &str, secrets: &[String]) -> String {
    if secrets.iter().all(String::is_empty) {
        return string.into();
    }
    // Match against the original text, never against an inserted marker (the
    // secret "a" also occurs in "[redacted]"). Memory stays bounded by output
    // size, rather than allocating one range for every matching secret.
    let mut output = String::new();
    let mut cursor = 0;
    let mut redacting = false;
    while cursor < string.len() {
        if let Some(secret) = secrets
            .iter()
            .filter(|secret| !secret.is_empty() && string[cursor..].starts_with(secret.as_str()))
            .max_by_key(|secret| secret.len())
        {
            if !redacting {
                output.push_str("[redacted]");
            }
            cursor += secret.len();
            redacting = true;
        } else {
            let character = string[cursor..]
                .chars()
                .next()
                .expect("cursor is on a character boundary");
            output.push(character);
            cursor += character.len_utf8();
            redacting = false;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_status_matching_secrets_cannot_corrupt_machine_metadata() {
        let report = json!({
            "runId":"a-run-id", "checkpointId":"a-checkpoint", "status":"completed",
            "originalTestVerdict":"passed", "allowedNextActions":["get"],
            "steps":[{"id":"save","status":"succeeded","outcome":{"execution":"completed","verification":"passed","effect":"confirmed","data":{"note":"completed a"}}}],
            "blockedOutcome":{"execution":"outcome_unknown","effect":"possible","data":"a"}
        });
        let public = public_report(&report, &["a".into(), "completed".into()]);
        assert_eq!(public["runId"], "a-run-id");
        assert_eq!(public["checkpointId"], "a-checkpoint");
        assert_eq!(public["status"], "completed");
        assert_eq!(public["steps"][0]["id"], "save");
        assert_eq!(public["steps"][0]["outcome"]["execution"], "completed");
        assert_eq!(public["blockedOutcome"]["effect"], "possible");
        assert_eq!(
            public["steps"][0]["outcome"]["data"]["note"],
            "[redacted] [redacted]"
        );
    }

    #[test]
    fn sensitive_payloads_error_echoes_and_numeric_inputs_are_redacted() {
        let report = json!({"blockedOutcome":{
            "execution":"failed", "effect":"possible",
            "data":{"password":"unknown password", "pin":1234,"user-secret":"value"},
            "error":{"code":"execution_failed","stage":"executing","message":"failed: user-secret"}
        }});
        let public = public_report(&report, &["user-secret".into(), "1234".into()]);
        let encoded = public.to_string();
        for secret in ["unknown password", "user-secret", "1234"] {
            assert!(!encoded.contains(secret));
        }
        assert_eq!(
            public["blockedOutcome"]["error"]["code"],
            "execution_failed"
        );
        assert_eq!(
            redact_payload(&json!({"value":"private", "sensitive":true}), &[]),
            json!({"redacted":true,"reason":"sensitive_source"})
        );
    }

    #[test]
    fn useful_query_entity_ids_remain_available() {
        let report = json!({"steps":[{"id":"read","status":"succeeded","outcome":{
            "execution":"completed","verification":"not_requested","effect":"none", "data":{"value":"entity-42","entityId":42}
        }}]});
        assert_eq!(
            public_report(&report, &[])["steps"][0]["outcome"]["data"]["entityId"],
            42
        );
    }

    #[test]
    fn structural_evidence_remains_useful_without_raw_dom_content() {
        let report = json!({"evidence":{"evidence-a":{
            "captureKind":"scoped_snapshot","scope":"target","available":true,
            "context":{"windowId":"main","pageEpoch":"epoch-a","url":"private"},
            "elements":[{"tag":"button","role":"button","enabled":false,"text":"private","id":"private"}],
            "observation":{"active":false,"mutationCount":3,"sampleCount":2,"raw":"private"}
        }}});
        let public = public_report(&report, &["button".into(), "a".into()]);
        assert!(!public.to_string().contains("private"));
        assert_eq!(
            public["evidence"]["evidence-a"]["elements"][0]["tag"],
            "button"
        );
        assert_eq!(
            public["evidence"]["evidence-a"]["context"]["windowId"],
            "main"
        );
        assert_eq!(
            public["evidence"]["evidence-a"]["observation"]["mutationCount"],
            3
        );
    }

    #[test]
    fn journal_never_persists_unknown_output_payloads_or_error_text() {
        let report = json!({
            "runId":"run-a", "revision":3,"cursor":3,"status":"paused","resourceIsolation":"quarantined", "summary":{"completedSteps":1,"remainingSteps":1,"raw":"private"},
            "blockedOutcome":{"execution":"failed","effect":"possible","data":{"anything":"private"},"error":{"code":"execution_failed","stage":"executing","message":"private"},"dispatch":{"requestId":"request-a","args":"private"},"warnings":["private"]},
            "coverage":{"sources":{"dom":"available","raw":"private"},"raw":"private"},
            "steps":[{"outcome":{"data":"private"}}],"evidence":{"body":"private"},"events":[{"data":"private"}],"extra":"private","persistenceWarning":"private"
        });
        let persisted = journal_report(&report, &[]);
        assert!(!persisted.to_string().contains("private"));
        assert_eq!(persisted["runId"], "run-a");
        assert_eq!(persisted["resourceIsolation"], "quarantined");
        assert_eq!(persisted["blockedOutcome"]["effect"], "possible");
        assert_eq!(
            persisted["blockedOutcome"]["dispatch"]["requestId"],
            "request-a"
        );
        assert!(persisted["blockedOutcome"].get("data").is_none());
        assert!(persisted.get("steps").is_none());
    }

    #[test]
    fn overlapping_unicode_secrets_are_masked_without_rescanning_markers() {
        assert_eq!(
            redact_string("aaaa", &["a".into(), "aa".into()]),
            "[redacted]"
        );
        assert_eq!(
            redact_string("🔑password🔑", &["password".into(), "🔑".into()]),
            "[redacted]"
        );
        assert_eq!(redact_string("safe", &[]), "safe");
    }
}

#[cfg(test)]
mod inspection_context_tests {
    use super::*;
    #[test]
    fn up_t012_context_keeps_lifecycle_identity_without_private_page_fields() {
        let context = json!({"appId":"fixture","appInstanceId":"app","windowId":"main","windowInstanceId":"window","pageEpoch":"page","runtimeId":"runtime","runtimeVersion":"1","semanticVersion":"1","bundleHash":"hash","url":"private-query","authToken":"private"});
        let report = json!({"dispatchContext":context,"evidence":{"e":{"context":context}}});
        let public = public_report(&report, &[]);
        assert_eq!(public["evidence"]["e"]["context"]["runtimeId"], "runtime");
        assert_eq!(
            public["evidence"]["e"]["context"]["windowInstanceId"],
            "window"
        );
        assert!(public["evidence"]["e"]["context"].get("url").is_none());
        let journal = journal_report(&report, &[]);
        assert_eq!(journal["dispatchContext"]["runtimeId"], "runtime");
        assert_eq!(journal["dispatchContext"]["windowInstanceId"], "window");
        assert!(journal["dispatchContext"].get("url").is_none());
        assert!(journal.get("evidence").is_none());
    }
}
