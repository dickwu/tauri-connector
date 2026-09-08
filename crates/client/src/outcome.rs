//! Execution facts, verification, and effects are independent from business JSON.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    NotDispatched,
    Completed,
    Failed,
    OutcomeUnknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    NotRequested,
    Passed,
    Failed,
    Inconclusive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    None,
    Possible,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowError {
    pub code: String,
    pub stage: String,
    pub message: String,
    #[serde(default)]
    pub retryable_before_dispatch: bool,
}
impl WorkflowError {
    pub fn new(
        code: impl Into<String>,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            stage: stage.into(),
            message: message.into(),
            retryable_before_dispatch: false,
        }
    }
}
impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for WorkflowError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutcome {
    pub execution: ExecutionStatus,
    pub verification: VerificationStatus,
    pub effect: EffectStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_scope: Option<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkflowError>,
    #[serde(default)]
    pub timing: Value,
    #[serde(default)]
    pub dispatch: Value,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub coverage: Value,
    #[serde(default)]
    pub warnings: Vec<String>,
}
impl ExecutionOutcome {
    pub fn completed(data: Value, effect: EffectStatus) -> Self {
        Self {
            execution: ExecutionStatus::Completed,
            verification: VerificationStatus::NotRequested,
            effect,
            effect_scope: None,
            data,
            error: None,
            timing: json!({}),
            dispatch: json!({}),
            evidence_refs: Vec::new(),
            coverage: json!({"businessPersistence":"unobserved"}),
            warnings: Vec::new(),
        }
    }
    pub fn not_dispatched(error: WorkflowError) -> Self {
        Self {
            execution: ExecutionStatus::NotDispatched,
            error: Some(error),
            ..Self::completed(Value::Null, EffectStatus::None)
        }
    }
    pub fn failed(error: WorkflowError, effect: EffectStatus) -> Self {
        Self {
            execution: ExecutionStatus::Failed,
            error: Some(error),
            ..Self::completed(Value::Null, effect)
        }
    }
    pub fn unknown(error: WorkflowError) -> Self {
        Self {
            execution: ExecutionStatus::OutcomeUnknown,
            error: Some(error),
            ..Self::completed(Value::Null, EffectStatus::Possible)
        }
    }
    /// Required verification must pass; business data never decides tool success.
    pub fn is_success(&self, expect: bool) -> bool {
        self.execution == ExecutionStatus::Completed
            && self.error.is_none()
            && if expect {
                self.verification == VerificationStatus::Passed
            } else {
                matches!(
                    self.verification,
                    VerificationStatus::NotRequested | VerificationStatus::Passed
                )
            }
    }
}

/// Stable machine-readable error codes understood by workflow v1.
pub const ERROR_CODES: &[&str] = &[
    "invalid_spec",
    "capability_unavailable",
    "unsupported_feature",
    "unsupported_condition",
    "target_not_found",
    "ambiguous_target",
    "target_changed",
    "stale_ref",
    "not_actionable",
    "binding_missing",
    "binding_type_mismatch",
    "precondition_failed",
    "postcondition_failed",
    "condition_timeout",
    "execution_failed",
    "outcome_unknown",
    "run_key_conflict",
    "run_not_found",
    "run_expired",
    "stale_checkpoint",
    "resume_not_safe",
    "app_instance_changed",
    "resume_requires_inputs",
    "observation_failed",
    "cancel_requested",
    "cancelled_before_dispatch",
    "persistence_unavailable",
    "evidence_persistence_failed",
    "resource_busy",
    "capture_incomplete",
    "protocol_mismatch",
    "unauthorized",
];

/// Adapter for known legacy tool contracts. It deliberately does not inspect
/// arbitrary query results for business `error`, `ok`, or `found` fields.
pub fn legacy_outcome(tool: &str, value: Value) -> ExecutionOutcome {
    let canonical = tool.strip_prefix("webview_").unwrap_or(tool);
    let mut out = ExecutionOutcome::completed(value, EffectStatus::Possible);
    if matches!(canonical, "wait_for" | "wait") {
        out.effect = EffectStatus::None;
        if out.data.get("timeout").and_then(Value::as_bool) == Some(true)
            && out.data.get("found").and_then(Value::as_bool) == Some(false)
        {
            out.execution = ExecutionStatus::Failed;
            out.error = Some(WorkflowError::new(
                "condition_timeout",
                "waiting",
                "Wait condition timed out",
            ));
        }
    }
    if canonical == "act_and_verify" {
        match out.data.get("verdict").and_then(Value::as_str) {
            Some("failed") => {
                out.verification = VerificationStatus::Failed;
                out.error = Some(WorkflowError::new(
                    "postcondition_failed",
                    "verifying",
                    "Action verification failed",
                ));
            }
            Some("passed") => out.verification = VerificationStatus::Passed,
            Some("inconclusive") => out.verification = VerificationStatus::Inconclusive,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn business_json_does_not_decide_success() {
        for data in [
            json!({"error":"saved text"}),
            json!({"found":false,"timeout":true}),
            json!({"verdict":"failed"}),
            json!({"ok":false}),
        ] {
            assert!(legacy_outcome("query", data).is_success(false));
        }
    }
    #[test]
    fn known_wait_and_verification_failures_propagate() {
        assert!(
            !legacy_outcome("webview_wait_for", json!({"found":false,"timeout":true}))
                .is_success(false)
        );
        let outcome = legacy_outcome("webview_act_and_verify", json!({"verdict":"failed"}));
        assert_eq!(outcome.execution, ExecutionStatus::Completed);
        assert_eq!(outcome.verification, VerificationStatus::Failed);
        assert!(!outcome.is_success(false));
    }
    #[test]
    fn undispatched_verified_state_is_not_success() {
        let mut out = ExecutionOutcome::not_dispatched(WorkflowError::new(
            "not_actionable",
            "preparing",
            "disabled",
        ));
        out.verification = VerificationStatus::Passed;
        assert!(!out.is_success(true));
    }
    #[test]
    fn error_codes_are_stable_and_unique() {
        let unique: std::collections::HashSet<_> = ERROR_CODES.iter().collect();
        assert_eq!(unique.len(), ERROR_CODES.len());
        assert!(ERROR_CODES.contains(&"outcome_unknown"));
        assert_eq!(
            serde_json::to_value(ExecutionStatus::OutcomeUnknown).unwrap(),
            json!("outcome_unknown")
        );
    }
}
