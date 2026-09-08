use super::*;
use serde_json::Value;
use std::collections::HashSet;

pub const MAX_SPEC_BYTES: usize = 256 * 1024;
pub const MAX_BINDING_BYTES: usize = 64 * 1024;
pub const MAX_STEPS: usize = 100;
pub const MAX_DEADLINE_MS: u64 = 300_000;
pub const MAX_CONDITION_DEPTH: usize = 8;
pub const MAX_CONDITION_NODES: usize = 100;
pub const MAX_LOCATOR_DEPTH: usize = 4;

fn invalid(message: &str) -> WorkflowError {
    WorkflowError::new("invalid_spec", "validating", message)
}
fn nonempty(value: &str, label: &str) -> Result<(), WorkflowError> {
    if value.trim().is_empty() || value.len() > 1024 {
        Err(invalid(&format!(
            "{label} must be non-empty and at most 1024 bytes"
        )))
    } else {
        Ok(())
    }
}
impl WorkflowSpec {
    pub fn parse(value: &Value) -> Result<Self, WorkflowError> {
        if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_SPEC_BYTES) {
            return Err(invalid("Workflow spec exceeds 256 KiB"));
        }
        // Unsupported modes/condition kinds are capability failures, not fields
        // accepted and silently ignored. Inspect only schema-owned locations.
        if value
            .get("schemaVersion")
            .and_then(Value::as_u64)
            .is_some_and(|version| version != 1)
        {
            return Err(WorkflowError::new(
                "unsupported_feature",
                "validating",
                "Only schemaVersion 1 is supported",
            ));
        }
        for (field, supported) in [("mode", "strict"), ("schedule", "sequential")] {
            if value
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|v| v != supported)
            {
                return Err(WorkflowError::new(
                    "unsupported_feature",
                    "validating",
                    format!("Only {supported} {field} is supported"),
                ));
            }
        }
        if let Some(steps) = value.get("steps").and_then(Value::as_array) {
            if steps.len() > MAX_STEPS {
                return Err(invalid("Workflow cannot exceed 100 steps"));
            }
            for step in steps {
                if let Some(target) = step.get("target") {
                    check_locator_shape(target, 1)?;
                }
                if let Some(op) = step.get("op").and_then(Value::as_str) {
                    if !["click", "fill", "type", "press", "wait", "query", "tool"].contains(&op) {
                        return Err(WorkflowError::new(
                            "unsupported_feature",
                            "validating",
                            "Unsupported workflow operation",
                        ));
                    }
                }
                for field in ["expect", "condition"] {
                    if let Some(condition) = step.get(field) {
                        check_condition_shape(condition, 1)?;
                    }
                }
            }
        }
        if let Some(goal) = value.get("goal") {
            check_condition_shape(goal, 1)?;
        }
        let spec: Self = serde_json::from_value(value.clone())
            .map_err(|_| invalid("Workflow fields do not match the strict v1 schema"))?;
        spec.validate()?;
        Ok(spec)
    }
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.schema_version != 1 || self.mode != "strict" || self.schedule != "sequential" {
            return Err(WorkflowError::new(
                "unsupported_feature",
                "validating",
                "Only strict sequential workflow schemaVersion 1 is supported",
            ));
        }
        nonempty(&self.run_key, "runKey")?;
        nonempty(&self.window_id, "windowId")?;
        if self.steps.is_empty() || self.steps.len() > MAX_STEPS {
            return Err(invalid("Workflow needs between 1 and 100 steps"));
        }
        budget(self.deadline_ms, MAX_DEADLINE_MS, "deadlineMs")?;
        budget(
            self.defaults.step_timeout_ms,
            MAX_DEADLINE_MS,
            "stepTimeoutMs",
        )?;
        budget(
            self.defaults.locator_timeout_ms,
            MAX_DEADLINE_MS,
            "locatorTimeoutMs",
        )?;
        budget(self.defaults.poll_interval_ms, 1_000, "pollIntervalMs")?;
        if self.defaults.poll_interval_ms < 10 {
            return Err(invalid("pollIntervalMs must be at least 10"));
        }
        if self.defaults.failure_evidence_grace_ms > 2_000 {
            return Err(invalid("Failure evidence grace cannot exceed 2000 ms"));
        }
        evidence(&self.evidence)?;
        for (key, value) in &self.inputs {
            nonempty(key, "input key")?;
            if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_BINDING_BYTES) {
                return Err(invalid("Input exceeds binding size limit"));
            }
        }
        let mut seen = HashSet::new();
        for step in &self.steps {
            nonempty(&step.id, "step id")?;
            if seen.contains(step.id.as_str()) {
                return Err(invalid("Step IDs must be unique"));
            }
            if let Some(window) = &step.window_id {
                nonempty(window, "step windowId")?;
            }
            if let Some(timeout) = step.timeout_ms {
                budget(timeout, MAX_DEADLINE_MS, "timeoutMs")?;
            }
            if let Some(policy) = &step.evidence {
                evidence(policy)?;
            }
            match &step.op {
                StepOp::Click { target } | StepOp::Query { target, .. } => {
                    locator(target, self, &seen, 1)?
                }
                StepOp::Fill { target, value } | StepOp::Type { target, value } => {
                    locator(target, self, &seen, 1)?;
                    expr(value, self, &seen, true, false)?;
                }
                StepOp::Press { target, key } => {
                    if let Some(target) = target {
                        locator(target, self, &seen, 1)?;
                    }
                    expr(key, self, &seen, true, true)?;
                }
                StepOp::Wait {
                    condition: wait_condition,
                } => condition(wait_condition, self, &seen, None, 1, &mut 0)?,
                StepOp::Tool {
                    tool,
                    args,
                    bindings,
                } => {
                    nonempty(tool, "tool")?;
                    if !args.is_object() {
                        return Err(invalid("tool args must be an object"));
                    }
                    for (pointer, value) in bindings {
                        validate_pointer(pointer)?;
                        if pointer.is_empty() || args.pointer(pointer).is_none() {
                            return Err(invalid(
                                "Tool bindings require existing, non-root argument pointers",
                            ));
                        }
                        expr(value, self, &seen, false, false)?;
                    }
                    let keys: Vec<_> = bindings.keys().collect();
                    for (i, key) in keys.iter().enumerate() {
                        if keys
                            .iter()
                            .skip(i + 1)
                            .any(|other| other.starts_with(&format!("{key}/")))
                        {
                            return Err(invalid("Tool argument binding paths cannot overlap"));
                        }
                    }
                }
            }
            if let StepOp::Query {
                query: QueryKind::Attribute { name },
                ..
            } = &step.op
            {
                attribute(name)?;
            }
            if let Some(expect) = &step.expect {
                condition(expect, self, &seen, Some(&step.id), 1, &mut 0)?;
            }
            seen.insert(step.id.as_str());
        }
        if let Some(goal) = &self.goal {
            condition(goal, self, &seen, None, 1, &mut 0)?;
        }
        Ok(())
    }
}
fn check_condition_shape(value: &Value, depth: usize) -> Result<(), WorkflowError> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(invalid("Condition depth exceeds 8"));
    }
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        if ![
            "element",
            "valueEquals",
            "textContains",
            "attributeEquals",
            "result",
            "all",
            "any",
        ]
        .contains(&kind)
        {
            return Err(WorkflowError::new(
                "unsupported_condition",
                "validating",
                "Unsupported workflow condition",
            ));
        }
    }
    for field in ["transition", "stabilityMs", "correlation", "invocation"] {
        if value.get(field).is_some() {
            return Err(WorkflowError::new(
                "unsupported_condition",
                "validating",
                format!("Condition {field} is not supported"),
            ));
        }
    }
    if let Some(children) = value.get("conditions").and_then(Value::as_array) {
        for child in children {
            check_condition_shape(child, depth + 1)?;
        }
    }
    Ok(())
}
fn check_locator_shape(value: &Value, depth: usize) -> Result<(), WorkflowError> {
    if depth > MAX_LOCATOR_DEPTH {
        return Err(invalid("Locator scope depth exceeds 4"));
    }
    if let Some(scope) = value.get("scope") {
        check_locator_shape(scope, depth + 1)?;
    }
    Ok(())
}
fn budget(value: u64, max: u64, name: &str) -> Result<(), WorkflowError> {
    if value == 0 || value > max {
        Err(invalid(&format!("{name} must be between 1 and {max}")))
    } else {
        Ok(())
    }
}
fn evidence(policy: &EvidencePolicy) -> Result<(), WorkflowError> {
    if policy.success != "summary" || policy.failure != "scoped" {
        return Err(WorkflowError::new(
            "unsupported_feature",
            "validating",
            "Only summary success and scoped failure evidence are supported",
        ));
    }
    if !(1024..=65536).contains(&policy.max_inline_bytes) {
        return Err(invalid("maxInlineBytes must be between 1024 and 65536"));
    }
    Ok(())
}
fn expr(
    value: &ValueExpr,
    spec: &WorkflowSpec,
    seen: &HashSet<&str>,
    string: bool,
    non_empty: bool,
) -> Result<(), WorkflowError> {
    let literal = match value {
        ValueExpr::Literal(value) => Some(value),
        ValueExpr::FromInput(reference) => {
            nonempty(&reference.key, "input key")?;
            Some(spec.inputs.get(&reference.key).ok_or_else(|| {
                WorkflowError::new(
                    "binding_missing",
                    "validating",
                    "Referenced input does not exist",
                )
            })?)
        }
        ValueExpr::FromStep(reference) => {
            validate_pointer(&reference.pointer)?;
            if !seen.contains(reference.step_id.as_str()) {
                return Err(WorkflowError::new(
                    "binding_missing",
                    "validating",
                    "Step binding must reference a previous step",
                ));
            }
            None
        }
    };
    if let Some(value) = literal {
        if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_BINDING_BYTES) {
            return Err(invalid("Literal exceeds binding size limit"));
        }
        if string && !value.is_string() {
            return Err(WorkflowError::new(
                "binding_type_mismatch",
                "validating",
                "Expression requires a string value",
            ));
        }
        if non_empty && value.as_str().is_some_and(|v| v.trim().is_empty()) {
            return Err(WorkflowError::new(
                "binding_type_mismatch",
                "validating",
                "Identity expression requires a non-empty string",
            ));
        }
    }
    Ok(())
}
fn attribute(name: &str) -> Result<(), WorkflowError> {
    if name.is_empty()
        || name.len() > 256
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
    {
        return Err(invalid("Invalid attribute name"));
    }
    Ok(())
}
fn locator(
    target: &Locator,
    spec: &WorkflowSpec,
    seen: &HashSet<&str>,
    depth: usize,
) -> Result<(), WorkflowError> {
    if depth > MAX_LOCATOR_DEPTH {
        return Err(invalid("Locator scope depth exceeds 4"));
    }
    expr(&target.value, spec, seen, true, true)?;
    if let Some(name) = &target.name {
        expr(name, spec, seen, true, false)?;
    }
    if let Some(scope) = &target.scope {
        locator(scope, spec, seen, depth + 1)?;
    }
    if let Some(entity) = &target.entity {
        attribute(&entity.attribute)?;
        expr(&entity.value, spec, seen, true, true)?;
    }
    Ok(())
}
fn condition(
    value: &Condition,
    spec: &WorkflowSpec,
    seen: &HashSet<&str>,
    current: Option<&str>,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), WorkflowError> {
    *nodes += 1;
    if depth > MAX_CONDITION_DEPTH || *nodes > MAX_CONDITION_NODES {
        return Err(invalid("Condition exceeds depth or node limit"));
    }
    match value {
        Condition::Element { target, .. } => locator(target, spec, seen, 1)?,
        Condition::ValueEquals { target, expected }
        | Condition::TextContains { target, expected }
        | Condition::AttributeEquals {
            target, expected, ..
        } => {
            locator(target, spec, seen, 1)?;
            expr(expected, spec, seen, true, false)?;
        }
        Condition::Result {
            step_id,
            pointer,
            operator,
            expected,
        } => {
            validate_pointer(pointer)?;
            if !seen.contains(step_id.as_str()) && current != Some(step_id.as_str()) {
                return Err(WorkflowError::new(
                    "binding_missing",
                    "validating",
                    "Result condition must reference an available step",
                ));
            }
            if (*operator == ResultOperator::Eq) != expected.is_some() {
                return Err(invalid(
                    "Only eq result conditions require an expected expression",
                ));
            }
            if let Some(expected) = expected {
                expr(expected, spec, seen, false, false)?;
            }
        }
        Condition::All { conditions } | Condition::Any { conditions } => {
            if conditions.is_empty() {
                return Err(invalid("all/any conditions must be non-empty"));
            }
            for child in conditions {
                condition(child, spec, seen, current, depth + 1, nodes)?;
            }
        }
    }
    if let Condition::AttributeEquals { name, .. } = value {
        attribute(name)?;
    }
    Ok(())
}
