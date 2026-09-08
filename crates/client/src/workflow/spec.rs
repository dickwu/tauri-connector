use super::{ValueExpr, WorkflowError};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowSpec {
    pub schema_version: u32,
    pub run_key: String,
    #[serde(default = "strict")]
    pub mode: String,
    #[serde(default = "sequential")]
    pub schedule: String,
    #[serde(default = "main_window")]
    pub window_id: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default = "deadline")]
    pub deadline_ms: u64,
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    #[serde(default)]
    pub evidence: EvidencePolicy,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Condition>,
}
fn strict() -> String {
    "strict".into()
}
fn sequential() -> String {
    "sequential".into()
}
fn main_window() -> String {
    "main".into()
}
fn deadline() -> u64 {
    60_000
}
fn step_timeout() -> u64 {
    10_000
}
fn locator_timeout() -> u64 {
    3_000
}
fn poll_interval() -> u64 {
    100
}
fn evidence_grace() -> u64 {
    2_000
}
fn max_inline() -> usize {
    16_384
}
fn summary() -> String {
    "summary".into()
}
fn scoped() -> String {
    "scoped".into()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowDefaults {
    #[serde(default = "step_timeout")]
    pub step_timeout_ms: u64,
    #[serde(default = "locator_timeout")]
    pub locator_timeout_ms: u64,
    #[serde(default = "poll_interval")]
    pub poll_interval_ms: u64,
    #[serde(default = "evidence_grace")]
    pub failure_evidence_grace_ms: u64,
}
impl Default for WorkflowDefaults {
    fn default() -> Self {
        Self {
            step_timeout_ms: step_timeout(),
            locator_timeout_ms: locator_timeout(),
            poll_interval_ms: poll_interval(),
            failure_evidence_grace_ms: evidence_grace(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidencePolicy {
    #[serde(default = "summary")]
    pub success: String,
    #[serde(default = "scoped")]
    pub failure: String,
    #[serde(default = "max_inline")]
    pub max_inline_bytes: usize,
}
impl Default for EvidencePolicy {
    fn default() -> Self {
        Self {
            success: summary(),
            failure: scoped(),
            max_inline_bytes: max_inline(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect: Option<Condition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidencePolicy>,
    #[serde(flatten)]
    pub op: StepOp,
}
// Deserialize common fields separately so the op union can reject fields that
// belong to another op (serde flatten + deny_unknown_fields is not sufficient).
impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::<String, Value>::deserialize(deserializer)?;
        let id = fields
            .remove("id")
            .ok_or_else(|| serde::de::Error::missing_field("id"))?;
        let mut take = |name: &str| -> Result<Option<Value>, D::Error> { Ok(fields.remove(name)) };
        let window_id = take("windowId")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        let timeout_ms = take("timeoutMs")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        let expect = take("expect")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        let evidence = take("evidence")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            id: serde_json::from_value(id).map_err(serde::de::Error::custom)?,
            window_id,
            timeout_ms,
            expect,
            evidence,
            op: serde_json::from_value(Value::Object(fields)).map_err(serde::de::Error::custom)?,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum StepOp {
    Click {
        target: Locator,
    },
    Fill {
        target: Locator,
        value: ValueExpr,
    },
    Type {
        target: Locator,
        value: ValueExpr,
    },
    Press {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Locator>,
        key: ValueExpr,
    },
    Wait {
        condition: Condition,
    },
    Query {
        target: Locator,
        query: QueryKind,
    },
    Tool {
        tool: String,
        args: Value,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        bindings: BTreeMap<String, ValueExpr>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Locator {
    pub by: LocatorBy,
    pub value: ValueExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ValueExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Box<Locator>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityLocator>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocatorBy {
    Role,
    Label,
    TestId,
    Css,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityLocator {
    pub attribute: String,
    pub value: ValueExpr,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum QueryKind {
    Value,
    Text,
    Attribute { name: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Condition {
    Element {
        target: Locator,
        state: ElementState,
    },
    ValueEquals {
        target: Locator,
        expected: ValueExpr,
    },
    TextContains {
        target: Locator,
        expected: ValueExpr,
    },
    AttributeEquals {
        target: Locator,
        name: String,
        expected: ValueExpr,
    },
    Result {
        #[serde(rename = "stepId")]
        step_id: String,
        pointer: String,
        operator: ResultOperator,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected: Option<ValueExpr>,
    },
    All {
        conditions: Vec<Condition>,
    },
    Any {
        conditions: Vec<Condition>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElementState {
    Visible,
    Hidden,
    Attached,
    Detached,
    Enabled,
    Editable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResultOperator {
    Eq,
    Exists,
    NonEmptyString,
}
impl WorkflowSpec {
    pub fn canonical_value(&self) -> Result<Value, WorkflowError> {
        let mut normalized = self.clone();
        for step in &mut normalized.steps {
            step.window_id
                .get_or_insert_with(|| normalized.window_id.clone());
            step.timeout_ms
                .get_or_insert(normalized.defaults.step_timeout_ms);
            step.evidence
                .get_or_insert_with(|| normalized.evidence.clone());
        }
        let mut value = serde_json::to_value(normalized).map_err(|_| {
            WorkflowError::new("invalid_spec", "validating", "Could not serialize workflow")
        })?;
        if let Some(obj) = value.as_object_mut() {
            obj.remove("runKey");
        }
        Ok(value)
    }
}
