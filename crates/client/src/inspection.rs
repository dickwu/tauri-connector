//! Shared inspection protocol contracts. This module holds no application state.
use serde_json::{json, Map, Value};

pub const INSPECTION_PROTOCOL_VERSION: u64 = 1;

#[derive(Debug, Clone)]
pub struct PickerRequest {
    pub action: String,
    pub convenience: bool,
    pub window_id: String,
    pub request_key: Option<String>,
    pub timeout_ms: u64,
    pub wait_ms: u64,
    pub picker_id: Option<String>,
    pub capture_screenshot: bool,
    pub screenshot_source: String,
    pub include_image: bool,
}

fn invalid(message: impl Into<String>) -> Value {
    json!({"code":"invalid_arguments","message":message.into()})
}

impl PickerRequest {
    pub fn parse(value: &Value) -> Result<Self, Value> {
        let fields = value
            .as_object()
            .ok_or_else(|| invalid("arguments must be an object"))?;
        let convenience = !fields.contains_key("action");
        let action = string(fields, "action")?.unwrap_or("start");
        let allowed: &[&str] = match action {
            "start" => &[
                "action",
                "authToken",
                "windowId",
                "requestKey",
                "timeoutMs",
                "timeout",
                "waitMs",
                "captureScreenshot",
                "screenshotSource",
                "includeImage",
            ],
            "get" => &["action", "authToken", "pickerId", "waitMs", "includeImage"],
            "cancel" => &["action", "authToken", "pickerId"],
            _ => return Err(invalid("action must be start, get, or cancel")),
        };
        for field in fields.keys() {
            if !allowed.contains(&field.as_str()) {
                return Err(invalid(format!("{field} is not allowed for {action}")));
            }
        }
        let _ = string(fields, "authToken")?;
        let window_id = string(fields, "windowId")?.unwrap_or("main");
        if window_id.is_empty() {
            return Err(invalid("windowId must not be empty"));
        }
        let request_key = string(fields, "requestKey")?;
        if request_key.is_some_and(|key| key.is_empty() || key.len() > 128) {
            return Err(invalid("requestKey must contain 1..128 UTF-8 bytes"));
        }
        let picker_id = string(fields, "pickerId")?;
        if action != "start" && picker_id.is_none_or(str::is_empty) {
            return Err(invalid("pickerId is required"));
        }
        let timeout_ms = integer(fields, "timeoutMs", 5000, 120000)?;
        let timeout_alias = integer(fields, "timeout", 5000, 120000)?;
        if timeout_ms.zip(timeout_alias).is_some_and(|(a, b)| a != b) {
            return Err(invalid("timeout and timeoutMs must match"));
        }
        let capture_screenshot = boolean(fields, "captureScreenshot")?.unwrap_or(true);
        let include_image = boolean(fields, "includeImage")?.unwrap_or(false);
        let screenshot_source = string(fields, "screenshotSource")?.unwrap_or("auto");
        if !["auto", "webview_native", "window_native", "dom_rendering"]
            .contains(&screenshot_source)
        {
            return Err(invalid("unsupported screenshotSource"));
        }
        if action == "start"
            && !capture_screenshot
            && (include_image || fields.contains_key("screenshotSource"))
        {
            return Err(invalid(
                "captureScreenshot:false conflicts with screenshotSource or includeImage:true",
            ));
        }
        Ok(Self {
            action: action.into(),
            convenience,
            window_id: window_id.into(),
            request_key: request_key.map(str::to_owned),
            timeout_ms: timeout_ms.or(timeout_alias).unwrap_or(60000),
            wait_ms: integer(fields, "waitMs", 0, 10000)?.unwrap_or(if convenience {
                10000
            } else {
                0
            }),
            picker_id: picker_id.map(str::to_owned),
            capture_screenshot,
            screenshot_source: screenshot_source.into(),
            include_image,
        })
    }

    /// Canonical logical selection request. Transport wait/image preferences do not alter it.
    pub fn fingerprint(&self) -> Value {
        json!({"windowId":self.window_id,"timeoutMs":self.timeout_ms,"captureScreenshot":self.capture_screenshot,"screenshotSource":self.screenshot_source})
    }
}

fn string<'a>(fields: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, Value> {
    fields
        .get(key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid(format!("{key} must be a string")))
        })
        .transpose()
}
fn boolean(fields: &Map<String, Value>, key: &str) -> Result<Option<bool>, Value> {
    fields
        .get(key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| invalid(format!("{key} must be a boolean")))
        })
        .transpose()
}
fn integer(
    fields: &Map<String, Value>,
    key: &str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, Value> {
    fields
        .get(key)
        .map(|v| {
            v.as_u64()
                .filter(|v| (min..=max).contains(v))
                .ok_or_else(|| invalid(format!("{key} must be an integer in {min}..{max}")))
        })
        .transpose()
}

pub fn picker_exit_code(report: &Value, cancel_request: bool) -> i32 {
    if cancel_request
        && report.get("code").is_none()
        && matches!(
            report.get("status").and_then(Value::as_str),
            Some(
                "created"
                    | "installing"
                    | "awaiting_selection"
                    | "selected"
                    | "cancelled"
                    | "expired"
                    | "target_changed"
                    | "failed"
            )
        )
    {
        return 0;
    }
    match report.get("status").and_then(Value::as_str) {
        Some("selected") => 0,
        Some("created" | "installing" | "awaiting_selection") => 2,
        Some("cancelled") if cancel_request => 0,
        _ => 1,
    }
}

/// Render only at the MCP boundary. Image bytes never replace selection metadata.
pub fn picker_mcp_content(mut report: Value, include_image: bool) -> Value {
    let top_image = report.as_object_mut().and_then(|v| v.remove("image"));
    let mut image = None;
    if let Some(screenshot) = report.get_mut("screenshot").and_then(Value::as_object_mut) {
        let payload = top_image.or_else(|| {
            screenshot
                .get("image")
                .filter(|v| v.get("base64").is_some())
                .cloned()
        });
        if let Some(metadata) = screenshot.get_mut("image").and_then(Value::as_object_mut) {
            metadata.remove("base64");
        }
        let base64 = screenshot.remove("base64");
        let redacted = screenshot
            .get("redaction")
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            == Some("applied");
        if include_image && redacted {
            let payload = payload
                .unwrap_or_else(|| json!({"base64":base64,"mimeType":screenshot.get("mimeType")}));
            if let (Some(data), Some(mime)) = (
                payload.get("base64").and_then(Value::as_str),
                payload.get("mimeType").and_then(Value::as_str),
            ) {
                if ["image/png", "image/jpeg", "image/webp"].contains(&mime)
                    && data.len() <= 4 * 1024 * 1024
                {
                    image = Some(json!({"type":"image","data":data,"mimeType":mime}));
                }
            }
        }
        if include_image
            && image.is_none()
            && screenshot.get("status").and_then(Value::as_str) == Some("captured")
        {
            screenshot.insert(
                "imageOmissionReason".into(),
                json!("image_unavailable_or_output_budget"),
            );
        }
    }
    let mut content = vec![json!({"type":"text","text":report.to_string()})];
    if let Some(image) = image {
        content.push(image);
    }
    let is_error = report.get("status").and_then(Value::as_str) == Some("failed")
        && report.get("cancellationAccepted") != Some(&json!(true))
        || report.get("code").is_some();
    json!({"content":content,"structuredContent":report,"isError":is_error})
}

pub fn picker_schema() -> Value {
    let start = json!({"type":"object","additionalProperties":false,"properties":{
        "action":{"const":"start"},"authToken":{"type":"string"},
        "windowId":{"type":"string","minLength":1,"default":"main"},
        "requestKey":{"type":"string","minLength":1,"maxLength":128,"description":"At most 128 UTF-8 bytes. Reuse only for the same logical selection within retainedUntil."},
        "timeoutMs":{"type":"integer","minimum":5000,"maximum":120000,"default":60000},
        "timeout":{"type":"integer","minimum":5000,"maximum":120000,"description":"Milliseconds alias; if both given values must match"},
        "waitMs":{"type":"integer","minimum":0,"maximum":10000,"description":"Default 0 for explicit start, 10000 for omitted action; never renews the picker deadline"},
        "captureScreenshot":{"type":"boolean","default":true},
        "screenshotSource":{"enum":["auto","webview_native","window_native","dom_rendering"],"default":"auto"},
        "includeImage":{"type":"boolean","default":false}
    },"allOf":[{"if":{"properties":{"captureScreenshot":{"const":false}},"required":["captureScreenshot"]},"then":{"not":{"anyOf":[{"required":["screenshotSource"]},{"properties":{"includeImage":{"const":true}},"required":["includeImage"]}]}}}]});
    let get = json!({"type":"object","additionalProperties":false,"required":["action","pickerId"],"properties":{
        "action":{"const":"get"},"authToken":{"type":"string"},"pickerId":{"type":"string","minLength":1},
        "waitMs":{"type":"integer","minimum":0,"maximum":10000,"default":0},"includeImage":{"type":"boolean","default":false}
    }});
    let cancel = json!({"type":"object","additionalProperties":false,"required":["action","pickerId"],"properties":{
        "action":{"const":"cancel"},"authToken":{"type":"string"},"pickerId":{"type":"string","minLength":1}
    }});
    json!({"type":"object","oneOf":[start,get,cancel]})
}

pub fn new_request_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn protected_artifact_request(name: &str, args: &Value) -> bool {
    name.starts_with("artifact_")
        && (args.get("authToken").is_some()
            || [
                "artifactId",
                "artifact",
                "before",
                "after",
                "baselineId",
                "currentId",
            ]
            .iter()
            .any(|field| {
                args.get(field).and_then(Value::as_str).is_some_and(|id| {
                    id.starts_with("artifact-")
                        || id.contains(":artifact:")
                        || id.contains(":picker:") && id.ends_with(":image")
                })
            }))
}
pub fn protected_artifact_args(args: &Value) -> Result<Value, Value> {
    let mut args = args.clone();
    if let Some(fields) = args.as_object_mut() {
        for (legacy, current) in [
            ("artifact", "artifactId"),
            ("before", "baselineId"),
            ("after", "currentId"),
        ] {
            if let Some(value) = fields.remove(legacy) {
                if fields.get(current).is_some_and(|other| other != &value) {
                    return Err(invalid("conflicting artifact identity aliases"));
                }
                fields.entry(current).or_insert(value);
            }
        }
    }
    Ok(args)
}

pub fn image_mcp_content(mut report: Value, require_redaction: bool) -> Value {
    let bytes = report.as_object_mut().and_then(|v| v.remove("base64"));
    let allowed = !require_redaction
        || report
            .pointer("/redaction/status")
            .or_else(|| report.pointer("/artifact/redaction/status"))
            .and_then(Value::as_str)
            == Some("applied");
    let image = bytes
        .as_ref()
        .and_then(Value::as_str)
        .filter(|data| allowed && data.len() <= 4 * 1024 * 1024)
        .zip(report.get("mimeType").and_then(Value::as_str))
        .filter(|(_, mime)| ["image/png", "image/jpeg", "image/webp"].contains(mime))
        .map(|(data, mime)| json!({"type":"image","data":data,"mimeType":mime}));
    if bytes.is_some() && image.is_none() {
        report["imageOmissionReason"] = json!("redaction_or_output_budget");
    }
    let mut content = vec![json!({"type":"text","text":report.to_string()})];
    if let Some(image) = image {
        content.push(image);
    }
    json!({"content":content,"structuredContent":report})
}

/// Inspection locators are already-resolved data; workflow expressions are not evaluated here.
pub fn validate_static_locator(value: &Value) -> Result<(), Value> {
    fn visit(value: &Value, depth: usize) -> Result<(), Value> {
        if depth > 8 {
            return Err(invalid("locator scope exceeds depth budget"));
        }
        let object = value
            .as_object()
            .ok_or_else(|| invalid("target must be a locator object"))?;
        if object
            .keys()
            .any(|key| !["by", "value", "name", "scope", "entity"].contains(&key.as_str()))
        {
            return Err(invalid("unknown locator field"));
        }
        if !object
            .get("by")
            .and_then(Value::as_str)
            .is_some_and(|by| ["role", "label", "testId", "css"].contains(&by))
        {
            return Err(invalid("unsupported locator kind"));
        }
        for key in ["value", "name"] {
            if (key == "value" || object.contains_key(key))
                && !object
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty() && v.len() <= 4096)
            {
                return Err(invalid(
                    "locator identifiers must be nonempty bounded strings",
                ));
            }
        }
        if let Some(scope) = object.get("scope") {
            visit(scope, depth + 1)?;
        }
        if let Some(entity) = object.get("entity") {
            let entity = entity
                .as_object()
                .ok_or_else(|| invalid("entity must be an object"))?;
            if entity.len() != 2
                || !["attribute", "value"].iter().all(|key| {
                    entity
                        .get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|v| !v.is_empty() && v.len() <= 4096)
                })
            {
                return Err(invalid("entity requires bounded attribute/value strings"));
            }
        }
        Ok(())
    }
    visit(value, 0)
}

/// Apply negotiated MCP features only at the transport boundary.
/// Uninitialized/legacy clients retain the complete text and optional image representations.
pub fn shape_mcp_result(mut result: Value, protocol_version: Option<&str>) -> Value {
    if let Some(fields) = result
        .as_object_mut()
        .filter(|_| !matches!(protocol_version, Some("2025-06-18" | "2025-11-25")))
    {
        fields.remove("structuredContent");
    }
    result
}

pub const RICH_SCREENSHOT_FIELDS: &[&str] = &[
    "source",
    "target",
    "redaction",
    "allowWindowPreparation",
    "includeImage",
    "timeoutMs",
    "authToken",
];
pub fn is_rich_screenshot(args: &Value) -> bool {
    RICH_SCREENSHOT_FIELDS
        .iter()
        .any(|field| args.get(field).is_some())
}
