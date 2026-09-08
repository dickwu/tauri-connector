//! Bounded presentation only. Internal outcomes and bindings remain untouched.

use serde_json::{Value, json};
use std::io::Write;

const MAX_INLINE_EVIDENCE_REFS: usize = 64;

/// The workflow validator admits budgets of 1024..=65536 bytes. If diagnostic
/// bodies do not fit, retain facts that prevent replay plus retrievable IDs.
pub(super) fn project(mut report: Value, budget: usize) -> Value {
    if fits(&report, budget) {
        return report;
    }
    if let Some(object) = report.as_object_mut() {
        object.remove("evidence");
        object.remove("events");
        object.remove("warnings");
        if let Some(outcome) = object.get_mut("blockedOutcome") {
            strip_outcome(outcome);
        }
        if let Some(steps) = object.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                if let Some(step) = step.as_object_mut() {
                    step.remove("data");
                    if let Some(outcome) = step.get_mut("outcome") {
                        strip_outcome(outcome);
                    }
                }
            }
        }
        if !object.get("coverage").is_some_and(Value::is_object) {
            object.insert("coverage".into(), json!({}));
        }
        object["coverage"]["truncated"] = json!(true);
        object["coverage"]["omittedFields"] = json!([
            "/evidence",
            "/events",
            "/steps/*/outcome/data",
            "/blockedOutcome/data",
            "/blockedOutcome/error/message"
        ]);
    }
    if fits(&report, budget) {
        return report;
    }

    let refs = report
        .get("evidenceRefs")
        .or_else(|| report.get("blockedOutcome")?.get("evidenceRefs"))
        .and_then(Value::as_array);
    let mut compact = json!({"coverage":{"truncated":true},"evidenceRefs":[]});
    if let Some(refs) = refs {
        // Reserve this accounting field before filling the rest of the budget.
        compact["coverage"]["omittedEvidenceRefs"] = json!(refs.len());
    }
    // Outcome facts precede optional metadata: a long diagnostic must never
    // turn a possible side effect into an apparent safe-to-retry operation.
    if let Some(outcome) = report.get("blockedOutcome") {
        let mut facts = pick_bounded(outcome, &["execution", "verification", "effect"], budget);
        if let Some(request_id) = outcome
            .get("dispatch")
            .and_then(|dispatch| dispatch.get("requestId"))
        {
            let mut dispatch = json!({});
            insert_if_fits(&mut dispatch, "requestId", request_id, budget);
            facts["dispatch"] = dispatch;
        }
        insert_if_fits(&mut compact, "blockedOutcome", &facts, budget);
    }
    for key in [
        "runId",
        "revision",
        "status",
        "checkpointId",
        "cursor",
        "reason",
        "originalTestVerdict",
        "goalStatus",
        "recoveryOccurred",
        "mayHaveEffects",
        "resourceIsolation",
        "allowedNextActions",
    ] {
        if let Some(value) = report.get(key) {
            insert_if_fits(&mut compact, key, value, budget);
        }
    }
    if let Some(refs) = refs {
        for (index, reference) in refs.iter().take(MAX_INLINE_EVIDENCE_REFS).enumerate() {
            if !reference.is_string() || !fits(reference, budget) {
                break;
            }
            compact["evidenceRefs"]
                .as_array_mut()
                .unwrap()
                .push(reference.clone());
            compact["coverage"]["omittedEvidenceRefs"] = json!(refs.len() - index - 1);
            if !fits(&compact, budget) {
                compact["evidenceRefs"].as_array_mut().unwrap().pop();
                compact["coverage"]["omittedEvidenceRefs"] = json!(refs.len() - index);
                break;
            }
        }
    }
    for key in [
        "summary",
        "blockedStep",
        "dispatchContext",
        "cancellation",
        "lateOutcome",
        "appInstanceId",
        "buildId",
    ] {
        if let Some(value) = report.get(key) {
            insert_if_fits(&mut compact, key, value, budget);
        }
    }
    if let Some(coverage) = report.get("coverage") {
        for key in ["correlation", "businessPersistence", "sources"] {
            if let Some(value) = coverage.get(key)
                && fits(value, budget)
            {
                compact["coverage"][key] = value.clone();
                if !fits(&compact, budget) {
                    compact["coverage"].as_object_mut().unwrap().remove(key);
                }
            }
        }
    }
    // The caller already validates a minimum budget. This fallback also keeps
    // malformed metadata from defeating the bound for direct internal callers.
    if fits(&compact, budget) {
        compact
    } else {
        let fallback = json!({"coverage":{"truncated":true},"reason":"output_budget_exceeded"});
        if fits(&fallback, budget) {
            fallback
        } else {
            Value::Null
        }
    }
}

fn strip_outcome(outcome: &mut Value) {
    if let Some(object) = outcome.as_object_mut() {
        object.remove("data");
        object.remove("warnings");
        if let Some(error) = object.get_mut("error").and_then(Value::as_object_mut) {
            error.remove("message");
            error.remove("details");
            error.remove("stack");
        }
    }
}

fn pick_bounded(value: &Value, keys: &[&str], budget: usize) -> Value {
    let mut projected = json!({});
    for key in keys {
        if let Some(value) = value.get(*key) {
            insert_if_fits(&mut projected, key, value, budget);
        }
    }
    projected
}

fn insert_if_fits(object: &mut Value, key: &str, value: &Value, budget: usize) {
    if !fits(value, budget) {
        return;
    }
    object[key] = value.clone();
    if !fits(object, budget) {
        object.as_object_mut().unwrap().remove(key);
    }
}

struct ByteBudget(usize);

impl Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("inline byte budget"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fits(value: &Value, budget: usize) -> bool {
    serde_json::to_writer(ByteBudget(budget), value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Value {
        json!({
            "runId":"run-opaque-1","revision":17,"cursor":17,"checkpointId":"checkpoint-opaque-1",
            "status":"paused","reason":"outcome_unknown","originalTestVerdict":"inconclusive",
            "goalStatus":"not_requested","recoveryOccurred":false,"mayHaveEffects":true,
            "allowedNextActions":["get","reconcile","cancel"],"resourceIsolation":"quarantined",
            "blockedOutcome":{"execution":"outcome_unknown","verification":"inconclusive","effect":"possible","dispatch":{"requestId":"request-opaque-1"}},
            "evidenceRefs":["artifact-opaque-1"],"coverage":{"truncated":false,"businessPersistence":"unobserved","correlation":"state_only"}
        })
    }

    fn assert_safe_and_bounded(projected: &Value, budget: usize) {
        assert!(serde_json::to_vec(projected).unwrap().len() <= budget);
        assert_eq!(projected["runId"], "run-opaque-1");
        assert_eq!(projected["revision"], 17);
        assert_eq!(projected["status"], "paused");
        assert_eq!(projected["blockedOutcome"]["execution"], "outcome_unknown");
        assert_eq!(projected["blockedOutcome"]["effect"], "possible");
        assert_eq!(
            projected["blockedOutcome"]["dispatch"]["requestId"],
            "request-opaque-1"
        );
        assert_eq!(projected["evidenceRefs"][0], "artifact-opaque-1");
        assert_eq!(projected["coverage"]["truncated"], true);
    }

    #[test]
    fn small_reports_are_returned_exactly() {
        let original = report();
        assert_eq!(project(original.clone(), 4096), original);
    }

    #[test]
    fn blocked_values_evidence_and_error_bodies_cannot_escape_the_budget() {
        let mut original = report();
        original["blockedOutcome"]["data"] = json!("secret".repeat(100_000));
        original["blockedOutcome"]["error"] =
            json!({"code":"outcome_unknown","message":"error".repeat(100_000)});
        original["evidence"] = json!({"artifact-opaque-1":{"body":"evidence".repeat(100_000)}});
        let projected = project(original, 1024);
        assert_safe_and_bounded(&projected, 1024);
        assert!(projected.get("evidence").is_none());
        assert!(projected["blockedOutcome"].get("data").is_none());
        assert!(
            projected["blockedOutcome"]["error"]
                .get("message")
                .is_none()
        );
    }

    #[test]
    fn unknown_diagnostics_and_many_steps_fall_back_to_metadata() {
        let mut original = report();
        original["unrecognizedDiagnostic"] = json!("x".repeat(100_000));
        original["steps"] = json!((0..100).map(|index| json!({"id":format!("s{index}"),"outcome":{"execution":"completed","data":"x".repeat(1024)}})).collect::<Vec<_>>());
        assert_safe_and_bounded(&project(original, 1024), 1024);
    }

    #[test]
    fn long_reference_lists_keep_a_bounded_prefix_and_an_omission_count() {
        let mut original = report();
        let mut refs = vec![json!("artifact-opaque-1")];
        refs.extend((0..10_000).map(|index| json!(format!("artifact-{index:08}"))));
        original["evidenceRefs"] = json!(refs);
        let projected = project(original, 1024);
        assert_safe_and_bounded(&projected, 1024);
        let kept = projected["evidenceRefs"].as_array().unwrap().len();
        let omitted = projected["coverage"]["omittedEvidenceRefs"]
            .as_u64()
            .unwrap() as usize;
        assert_eq!(kept + omitted, 10_001);
    }

    #[test]
    fn byte_budget_accounts_for_unicode_and_json_escaping() {
        let mut original = report();
        original["blockedOutcome"]["data"] = json!("🔑\"\\\n".repeat(300));
        assert_safe_and_bounded(&project(original, 1024), 1024);
    }
}
