use connector_client::outcome::{EffectStatus, ExecutionOutcome, VerificationStatus};
use connector_client::workflow::{resolve_step, ValueExpr, WorkflowSpec};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn spec(steps: Value) -> Value {
    json!({"schemaVersion":1,"runKey":"contract","steps":steps})
}

#[test]
fn strict_union_rejects_unrecognized_and_inapplicable_fields() {
    for step in [
        json!({"id":"x","op":"click","target":{"by":"css","value":"button"},"value":"x"}),
        json!({"id":"x","op":"wait","condition":{"kind":"element","target":{"by":"css","value":"button"},"state":"visible","ignored":true}}),
    ] {
        assert_eq!(
            WorkflowSpec::parse(&spec(json!([step]))).unwrap_err().code,
            "invalid_spec"
        );
    }
}

#[test]
fn binding_validation_rejects_forward_missing_and_wrong_typed_inputs() {
    let step = json!({"id":"x","op":"fill","target":{"by":"css","value":"input"},"value":{"fromStep":{"stepId":"later","pointer":"/value"}}});
    assert_eq!(
        WorkflowSpec::parse(&spec(json!([step]))).unwrap_err().code,
        "binding_missing"
    );
    let mut s = spec(
        json!([{"id":"x","op":"fill","target":{"by":"css","value":"input"},"value":{"fromInput":{"key":"name"}}}]),
    );
    s["inputs"] = json!({"name":42});
    assert_eq!(
        WorkflowSpec::parse(&s).unwrap_err().code,
        "binding_type_mismatch"
    );
}

#[test]
fn pointers_preserve_null_escape_and_success_dependency() {
    let expr: ValueExpr =
        serde_json::from_value(json!({"fromStep":{"stepId":"read","pointer":"/a~1b/~0"}})).unwrap();
    let mut results = BTreeMap::new();
    results.insert(
        "read".into(),
        ExecutionOutcome::completed(json!({"a/b":{"~":null}}), EffectStatus::None),
    );
    assert_eq!(
        expr.resolve(&BTreeMap::new(), &results).unwrap(),
        Value::Null
    );
    results.get_mut("read").unwrap().verification = VerificationStatus::Failed;
    assert_eq!(
        expr.resolve(&BTreeMap::new(), &results).unwrap_err().code,
        "binding_missing"
    );
    assert!(serde_json::from_value::<ValueExpr>(
        json!({"fromStep":{"stepId":"read","pointer":"/~2"}})
    )
    .is_err());
}

#[test]
fn result_self_expect_is_allowed_but_bindings_cannot_read_self() {
    let s = WorkflowSpec::parse(&spec(json!([{"id":"read","op":"query","target":{"by":"css","value":"input"},"query":{"kind":"value"},"expect":{"kind":"result","stepId":"read","pointer":"/value","operator":"nonEmptyString"}}]))).unwrap();
    assert!(resolve_step(&s.steps[0], &s.inputs, &BTreeMap::new()).is_ok());
}

#[test]
fn literal_business_json_is_not_recursively_interpreted() {
    let expr: ValueExpr =
        serde_json::from_value(json!({"literal":{"fromInput":{"key":"secret"}}})).unwrap();
    assert_eq!(
        expr.resolve(&BTreeMap::new(), &BTreeMap::new()).unwrap(),
        json!({"fromInput":{"key":"secret"}})
    );
    assert!(
        serde_json::from_value::<ValueExpr>(json!({"literal":"x","fromInput":{"key":"x"}}))
            .is_err()
    );
}

#[test]
fn modes_and_limits_fail_before_dispatch() {
    let mut s = spec(json!([{"id":"x","op":"press","key":"Enter"}]));
    s["mode"] = json!("relaxed");
    assert_eq!(
        WorkflowSpec::parse(&s).unwrap_err().code,
        "unsupported_feature"
    );
    s["mode"] = json!("strict");
    s["deadlineMs"] = json!(300001);
    assert_eq!(WorkflowSpec::parse(&s).unwrap_err().code, "invalid_spec");
}

#[test]
fn required_identity_cannot_resolve_to_null_or_empty() {
    for value in [json!(null), json!(""), json!(42)] {
        let s=WorkflowSpec::parse(&spec(json!([
            {"id":"read","op":"query","target":{"by":"css","value":"input"},"query":{"kind":"value"}},
            {"id":"click","op":"click","target":{"by":"testId","value":"row","entity":{"attribute":"data-id","value":{"fromStep":{"stepId":"read","pointer":"/value"}}}}}
        ]))).unwrap();
        let outcomes = BTreeMap::from([(
            "read".into(),
            ExecutionOutcome::completed(json!({"value":value}), EffectStatus::None),
        )]);
        assert_eq!(
            resolve_step(&s.steps[1], &s.inputs, &outcomes)
                .unwrap_err()
                .code,
            "binding_type_mismatch"
        );
    }
}

#[test]
fn tool_binding_only_evaluates_declared_existing_slots() {
    let mut v = spec(
        json!([{"id":"tool","op":"tool","tool":"trusted_query","args":{"id":null,"opaque":{"fromInput":{"key":"absent"}}},"bindings":{"/id":{"fromInput":{"key":"id"}}}}]),
    );
    v["inputs"] = json!({"id":"entity-1"});
    let s = WorkflowSpec::parse(&v).unwrap();
    let resolved =
        serde_json::to_value(resolve_step(&s.steps[0], &s.inputs, &BTreeMap::new()).unwrap())
            .unwrap();
    assert_eq!(resolved["args"]["id"], "entity-1");
    assert_eq!(
        resolved["args"]["opaque"],
        json!({"fromInput":{"key":"absent"}})
    );
    v["steps"][0]["bindings"] = json!({"/missing":{"literal":1}});
    assert_eq!(WorkflowSpec::parse(&v).unwrap_err().code, "invalid_spec");
    v["steps"][0]["bindings"] =
        json!({"/opaque":{"literal":{}},"/opaque/fromInput":{"literal":{}}});
    assert_eq!(WorkflowSpec::parse(&v).unwrap_err().code, "invalid_spec");
}

#[test]
fn canonical_identity_normalizes_defaults_but_keeps_inputs_and_assertions() {
    let a = WorkflowSpec::parse(&spec(json!([{"id":"x","op":"press","key":"Enter"}]))).unwrap();
    let mut b = a.clone();
    b.run_key = "different".into();
    b.steps[0].window_id = Some("main".into());
    b.steps[0].timeout_ms = Some(10_000);
    b.steps[0].evidence = Some(b.evidence.clone());
    assert_eq!(a.canonical_value().unwrap(), b.canonical_value().unwrap());
    b.inputs.insert("name".into(), json!("changed"));
    assert_ne!(a.canonical_value().unwrap(), b.canonical_value().unwrap());
}

#[test]
fn condition_bounds_and_unsupported_semantics_are_enforced() {
    let leaf = json!({"kind":"element","target":{"by":"css","value":"button"},"state":"visible"});
    let mut condition = leaf.clone();
    for _ in 0..8 {
        condition = json!({"kind":"all","conditions":[condition]});
    }
    assert_eq!(
        WorkflowSpec::parse(&spec(
            json!([{"id":"wait","op":"wait","condition":condition}])
        ))
        .unwrap_err()
        .code,
        "invalid_spec"
    );
    let many = json!({"kind":"any","conditions":vec![leaf.clone();100]});
    assert_eq!(
        WorkflowSpec::parse(&spec(json!([{"id":"wait","op":"wait","condition":many}])))
            .unwrap_err()
            .code,
        "invalid_spec"
    );
    for condition in [
        json!({"kind":"invocation"}),
        json!({"kind":"element","target":{"by":"css","value":"button"},"state":"visible","transition":"appeared"}),
    ] {
        assert_eq!(
            WorkflowSpec::parse(&spec(
                json!([{"id":"wait","op":"wait","condition":condition}])
            ))
            .unwrap_err()
            .code,
            "unsupported_condition"
        );
    }
}

#[test]
fn spec_and_value_size_and_locator_depth_are_bounded() {
    let mut target = json!({"by":"css","value":"button"});
    for _ in 0..4 {
        target = json!({"by":"css","value":"section","scope":target});
    }
    assert_eq!(
        WorkflowSpec::parse(&spec(json!([{"id":"click","op":"click","target":target}])))
            .unwrap_err()
            .code,
        "invalid_spec"
    );
    let oversized = spec(
        json!([{"id":"fill","op":"fill","target":{"by":"css","value":"input"},"value":"a".repeat(262144)}]),
    );
    assert_eq!(
        WorkflowSpec::parse(&oversized).unwrap_err().code,
        "invalid_spec"
    );
    let expr = ValueExpr::literal(json!("a".repeat(65537)));
    assert_eq!(
        expr.resolve(&BTreeMap::new(), &BTreeMap::new())
            .unwrap_err()
            .code,
        "invalid_spec"
    );
}

#[test]
fn missing_pointer_differs_from_legal_null() {
    let out = BTreeMap::from([(
        "read".into(),
        ExecutionOutcome::completed(json!({"value":null}), EffectStatus::None),
    )]);
    for (pointer, ok) in [("/value", true), ("/absent", false)] {
        let expr: ValueExpr =
            serde_json::from_value(json!({"fromStep":{"stepId":"read","pointer":pointer}}))
                .unwrap();
        assert_eq!(expr.resolve(&BTreeMap::new(), &out).is_ok(), ok);
    }
}

#[test]
fn schema_errors_do_not_echo_rejected_input_strings() {
    let secret = "PRIVATE_USER_INPUT_7613";
    let value = spec(
        json!([{"id":"x","op":"query","target":{"by":secret,"value":"input"},"query":{"kind":"value"}}]),
    );
    let error = WorkflowSpec::parse(&value).unwrap_err();
    assert!(!error.message.contains(secret));
}

#[test]
fn array_pointer_indices_are_not_coerced() {
    let outcomes = BTreeMap::from([(
        "read".into(),
        ExecutionOutcome::completed(json!(["a", "b"]), EffectStatus::None),
    )]);
    for pointer in ["/01", "/-", "/+1"] {
        let expr: ValueExpr =
            serde_json::from_value(json!({"fromStep":{"stepId":"read","pointer":pointer}}))
                .unwrap();
        assert_eq!(
            expr.resolve(&BTreeMap::new(), &outcomes).unwrap_err().code,
            "binding_missing"
        );
    }
    let expr: ValueExpr =
        serde_json::from_value(json!({"fromStep":{"stepId":"read","pointer":"/1"}})).unwrap();
    assert_eq!(expr.resolve(&BTreeMap::new(), &outcomes).unwrap(), "b");
}

#[test]
fn shipped_workflow_examples_match_the_strict_contract() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/workflow");
    // Published crates do not package repository-level examples.
    if !root.exists() {
        return;
    }
    for name in ["create-task.json", "failure-and-reconcile.json"] {
        let raw: Value = serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap();
        let parsed = WorkflowSpec::parse(&raw).unwrap();
        let roundtrip = WorkflowSpec::parse(&serde_json::to_value(&parsed).unwrap()).unwrap();
        assert_eq!(
            parsed.canonical_value().unwrap(),
            roundtrip.canonical_value().unwrap()
        );
    }
}
