//! Regression checks for the JSON-only short runtime packet adapter.
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    #[test]
    fn command_arguments_round_trip_without_source_interpolation() {
        let command = json!({"cmd":"execute","step":{"op":"fill","value":"quotes\" slash\\ newline\n 中文 ${danger} `ticks`\u{2028}\u{2029}"}});
        let arguments = crate::runtime::json(&command);
        assert_eq!(serde_json::from_str::<Value>(&arguments).unwrap(), command);
        assert!(!arguments.contains('\u{2028}'));
        assert!(!arguments.contains('\u{2029}'));
        let packet = json!({"module":"workflow","args":command});
        let script = crate::runtime::dispatch_script(&packet);
        assert!(script.len() < 2048);
        assert!(!script.contains("const observations = new Map()"));
        assert!(!script.contains("function resolve("));
    }
}
