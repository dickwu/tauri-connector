use super::{Condition, Locator, Step, StepOp, WorkflowError};
use crate::outcome::ExecutionOutcome;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ValueExpr {
    Literal(Value),
    FromInput(InputRef),
    FromStep(StepRef),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRef {
    pub key: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepRef {
    pub step_id: String,
    pub pointer: String,
}
impl Serialize for ValueExpr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Literal(v) => json!({"literal":v}),
            Self::FromInput(v) => json!({"fromInput":v}),
            Self::FromStep(v) => json!({"fromStep":v}),
        }
        .serialize(s)
    }
}
impl<'de> Deserialize<'de> for ValueExpr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        match value {
            Value::String(_) | Value::Bool(_) | Value::Number(_) => Ok(Self::Literal(value)),
            Value::Object(object) if object.len() == 1 => {
                let (key, value) = object.into_iter().next().expect("length checked");
                match key.as_str() {
                    "literal" => Ok(Self::Literal(value)),
                    "fromInput" => serde_json::from_value(value)
                        .map(Self::FromInput)
                        .map_err(serde::de::Error::custom),
                    "fromStep" => {
                        let reference: StepRef =
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                        validate_pointer(&reference.pointer).map_err(serde::de::Error::custom)?;
                        Ok(Self::FromStep(reference))
                    }
                    _ => Err(serde::de::Error::custom(
                        "expression must have one of literal, fromInput, fromStep",
                    )),
                }
            }
            _ => Err(serde::de::Error::custom(
                "expression must be a scalar or an object with exactly one expression key",
            )),
        }
    }
}
impl ValueExpr {
    pub fn literal(value: Value) -> Self {
        Self::Literal(value)
    }
    pub fn as_literal(&self) -> Option<&Value> {
        match self {
            Self::Literal(value) => Some(value),
            _ => None,
        }
    }
    pub fn resolve(
        &self,
        inputs: &BTreeMap<String, Value>,
        outcomes: &BTreeMap<String, ExecutionOutcome>,
    ) -> Result<Value, WorkflowError> {
        let value = match self {
            Self::Literal(v) => v,
            Self::FromInput(reference) => inputs.get(&reference.key).ok_or_else(|| {
                WorkflowError::new(
                    "binding_missing",
                    "binding",
                    "Required workflow input is missing",
                )
            })?,
            Self::FromStep(reference) => {
                validate_pointer(&reference.pointer)?;
                let outcome = outcomes
                    .get(&reference.step_id)
                    .filter(|outcome| outcome.is_success(false))
                    .ok_or_else(|| {
                        WorkflowError::new(
                            "binding_missing",
                            "binding",
                            "Referenced step has not succeeded",
                        )
                    })?;
                outcome.data.pointer(&reference.pointer).ok_or_else(|| {
                    WorkflowError::new(
                        "binding_missing",
                        "binding",
                        "Referenced result path is missing",
                    )
                })?
            }
        };
        if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > super::MAX_BINDING_BYTES) {
            return Err(WorkflowError::new(
                "invalid_spec",
                "binding",
                "Resolved binding exceeds the value size limit",
            ));
        }
        Ok(value.clone())
    }
}

/// Validate RFC 6901 escapes instead of silently accepting malformed pointers.
pub fn validate_pointer(pointer: &str) -> Result<(), WorkflowError> {
    if pointer.len() > 4096 || (!pointer.is_empty() && !pointer.starts_with('/')) {
        return Err(WorkflowError::new(
            "invalid_spec",
            "validating",
            "JSON Pointer must be empty or begin with / and fit within 4096 bytes",
        ));
    }
    let mut chars = pointer.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err(WorkflowError::new(
                "invalid_spec",
                "validating",
                "JSON Pointer has an invalid escape",
            ));
        }
    }
    Ok(())
}
fn resolve_expr(
    expr: &mut ValueExpr,
    inputs: &BTreeMap<String, Value>,
    outcomes: &BTreeMap<String, ExecutionOutcome>,
    string: bool,
    nonempty: bool,
) -> Result<(), WorkflowError> {
    let value = expr.resolve(inputs, outcomes)?;
    if string && !value.is_string() {
        return Err(WorkflowError::new(
            "binding_type_mismatch",
            "binding",
            "Expression requires a string value",
        ));
    }
    if nonempty && value.as_str().is_some_and(|v| v.trim().is_empty()) {
        return Err(WorkflowError::new(
            "binding_type_mismatch",
            "binding",
            "Identity expression requires a non-empty string",
        ));
    }
    *expr = ValueExpr::Literal(value);
    Ok(())
}
fn resolve_locator(
    target: &mut Locator,
    inputs: &BTreeMap<String, Value>,
    outcomes: &BTreeMap<String, ExecutionOutcome>,
) -> Result<(), WorkflowError> {
    resolve_expr(&mut target.value, inputs, outcomes, true, true)?;
    if let Some(name) = &mut target.name {
        resolve_expr(name, inputs, outcomes, true, false)?;
    }
    if let Some(scope) = &mut target.scope {
        resolve_locator(scope, inputs, outcomes)?;
    }
    if let Some(entity) = &mut target.entity {
        resolve_expr(&mut entity.value, inputs, outcomes, true, true)?;
    }
    Ok(())
}
pub fn resolve_condition(
    condition: &Condition,
    inputs: &BTreeMap<String, Value>,
    outcomes: &BTreeMap<String, ExecutionOutcome>,
) -> Result<Condition, WorkflowError> {
    let mut condition = condition.clone();
    match &mut condition {
        Condition::Element { target, .. } => resolve_locator(target, inputs, outcomes)?,
        Condition::ValueEquals { target, expected }
        | Condition::TextContains { target, expected }
        | Condition::AttributeEquals {
            target, expected, ..
        } => {
            resolve_locator(target, inputs, outcomes)?;
            resolve_expr(expected, inputs, outcomes, true, false)?;
        }
        Condition::Result { expected, .. } => {
            if let Some(expected) = expected {
                resolve_expr(expected, inputs, outcomes, false, false)?;
            }
        }
        Condition::All { conditions } | Condition::Any { conditions } => {
            for item in conditions {
                *item = resolve_condition(item, inputs, outcomes)?;
            }
        }
    }
    Ok(condition)
}
pub fn resolve_step(
    step: &Step,
    inputs: &BTreeMap<String, Value>,
    outcomes: &BTreeMap<String, ExecutionOutcome>,
) -> Result<Step, WorkflowError> {
    let mut step = step.clone();
    match &mut step.op {
        StepOp::Click { target } | StepOp::Query { target, .. } => {
            resolve_locator(target, inputs, outcomes)?
        }
        StepOp::Fill { target, value } | StepOp::Type { target, value } => {
            resolve_locator(target, inputs, outcomes)?;
            resolve_expr(value, inputs, outcomes, true, false)?;
        }
        StepOp::Press { target, key } => {
            if let Some(target) = target {
                resolve_locator(target, inputs, outcomes)?;
            }
            resolve_expr(key, inputs, outcomes, true, true)?;
        }
        StepOp::Wait { condition } => *condition = resolve_condition(condition, inputs, outcomes)?,
        StepOp::Tool { args, bindings, .. } => {
            for (pointer, expr) in bindings.iter() {
                validate_pointer(pointer)?;
                if pointer.is_empty() {
                    return Err(WorkflowError::new(
                        "invalid_spec",
                        "binding",
                        "Tool binding cannot replace the entire arguments object",
                    ));
                }
                let value = expr.resolve(inputs, outcomes)?;
                let slot = args.pointer_mut(pointer).ok_or_else(|| {
                    WorkflowError::new(
                        "binding_missing",
                        "binding",
                        "Tool argument binding path does not exist",
                    )
                })?;
                *slot = value;
            }
            bindings.clear();
        }
    }
    if let Some(expect) = &step.expect {
        step.expect = Some(resolve_condition(expect, inputs, outcomes)?);
    }
    if serde_json::to_vec(&step).map_or(true, |bytes| bytes.len() > super::MAX_SPEC_BYTES) {
        return Err(WorkflowError::new(
            "invalid_spec",
            "binding",
            "Resolved step exceeds the 256 KiB size limit",
        ));
    }
    Ok(step)
}
