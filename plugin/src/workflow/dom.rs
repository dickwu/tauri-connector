//! Safe JSON-only adapter for the isolated, page-local workflow executor.

use serde_json::Value;

const PAGE_RUNTIME: &str = include_str!("page.js");

/// Build a script using JSON serialization for every externally supplied value.
/// The page runtime accepts only declarative commands; it does not evaluate any
/// strings supplied in the command as JavaScript.
pub fn script(command: &Value) -> String {
    // U+2028/U+2029 escaping also works on older JavaScript content worlds.
    let arguments = command
        .to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("({PAGE_RUNTIME})({arguments})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_arguments_round_trip_without_source_interpolation() {
        let command = serde_json::json!({
            "cmd": "execute",
            "step": {"op": "fill", "value": "quotes\" slash\\ newline\n 中文 ${danger} `ticks`\u{2028}\u{2029}"}
        });
        let built = script(&command);
        let arguments = built
            .strip_prefix(&format!("({PAGE_RUNTIME})("))
            .and_then(|value| value.strip_suffix(')'))
            .expect("script wrapper");
        assert_eq!(serde_json::from_str::<Value>(arguments).unwrap(), command);
        assert!(!arguments.contains('\u{2028}'));
        assert!(!arguments.contains('\u{2029}'));
    }
}
