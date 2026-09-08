//! Strict, GUI-independent workflow v1 protocol and expression evaluator.
mod spec;
mod validate;
mod value_expr;
pub use crate::outcome::WorkflowError;
pub use spec::*;
pub use validate::{
    MAX_BINDING_BYTES, MAX_CONDITION_DEPTH, MAX_CONDITION_NODES, MAX_DEADLINE_MS,
    MAX_LOCATOR_DEPTH, MAX_SPEC_BYTES, MAX_STEPS,
};
pub use value_expr::{
    resolve_condition, resolve_step, validate_pointer, InputRef, StepRef, ValueExpr,
};
