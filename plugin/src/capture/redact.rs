//! Host-side second privacy boundary; only this output enters the capture store.
use serde_json::{Value, json};

pub fn sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "secret",
        "password",
        "passwd",
        "token",
        "cookie",
        "auth",
        "header",
        "credential",
        "privatekey",
        "private_key",
    ]
    .iter()
    .any(|part| key.contains(part))
}

pub fn preview(value: &Value, allowed_paths: &[String]) -> Value {
    fn visit(
        value: &Value,
        paths: &[String],
        path: &str,
        depth: usize,
        budget: &mut usize,
    ) -> Value {
        if depth > 4 || *budget < 32 {
            return json!({"type":"truncated"});
        }
        *budget = budget.saturating_sub(24);
        match value {
            Value::String(s)
                if path.rsplit('.').next() == Some("type")
                    && matches!(
                        s.as_str(),
                        "string"
                            | "bigint"
                            | "circular"
                            | "truncated"
                            | "unserializable"
                            | "undefined"
                            | "function"
                            | "symbol"
                            | "binary"
                            | "ArrayBuffer"
                            | "Map"
                            | "Set"
                            | "Response"
                            | "ReadableStream"
                            | "Error"
                            | "number"
                    ) =>
            {
                Value::String(s.clone())
            }
            Value::String(s) if s == "[redacted]" => Value::String(s.clone()),
            Value::String(s) if paths.iter().any(|p| p == path) => {
                let mut result = String::new();
                for ch in s.chars() {
                    if result.len() + ch.len_utf8() > 1024.min(*budget) {
                        break;
                    }
                    result.push(ch);
                }
                *budget = budget.saturating_sub(result.len());
                Value::String(result)
            }
            Value::String(s) => json!({"type":"string","length":s.chars().count()}),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .take(50)
                    .enumerate()
                    .map(|(i, v)| visit(v, paths, &format!("{path}.{i}"), depth + 1, budget))
                    .collect(),
            ),
            Value::Object(items) => {
                let mut result = serde_json::Map::new();
                for (key, v) in items.iter().take(50) {
                    if sensitive(key) || key.len() > 128 || *budget < 32 {
                        continue;
                    }
                    let next = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    result.insert(key.clone(), visit(v, paths, &next, depth + 1, budget));
                }
                Value::Object(result)
            }
            other => other.clone(),
        }
    }
    let result = visit(value, allowed_paths, "", 0, &mut 3500);
    if serde_json::to_vec(&result).is_ok_and(|v| v.len() <= 4096) {
        result
    } else {
        json!({"type":"truncated"})
    }
}
