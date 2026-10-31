use crate::{Value, json};
use axum::http::{HeaderMap, header};

fn error(kind: &str, field: Option<&str>, message: &str, input: &Value) -> Value {
    let mut location = vec![json!("body")];
    if let Some(field) = field {
        location.push(json!(field));
    }
    json!({"type":kind,"loc":location,"msg":message,"input":input})
}

pub(super) fn request(
    bytes: &[u8],
    headers: &HeaderMap,
    realtime: bool,
) -> Result<Value, Vec<Value>> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    let media = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let value = if bytes.is_empty() {
        Value::Null
    } else if media == "application/json"
        || (media.starts_with("application/") && media.ends_with("+json"))
    {
        serde_json::from_slice(bytes).map_err(|e| {
            // Native parser wording and byte offsets differ from Python's JSON decoder.
            vec![json!({"type":"json_invalid","loc":["body",e.column().saturating_sub(1)],"msg":"JSON decode error","input":{},"ctx":{"error":e.to_string()}})]
        })?
    } else {
        json!(String::from_utf8_lossy(bytes))
    };
    if value.is_null() {
        return Err(vec![error("missing", None, "Field required", &Value::Null)]);
    }
    let class = if realtime { "Job" } else { "DefaultRequest" };
    if !value.is_object() {
        let mut detail = error(
            "dataclass_type",
            None,
            &format!("Input should be a dictionary or an instance of {class}"),
            &value,
        );
        detail["ctx"] = json!({"class_name":class});
        return Err(vec![detail]);
    }
    let mut details = Vec::new();
    if realtime {
        match value.get("id") {
            None => details.push(error("missing", Some("id"), "Field required", &value)),
            Some(id) if !id.is_string() => details.push(error(
                "string_type",
                Some("id"),
                "Input should be a valid string",
                id,
            )),
            _ => {}
        }
    }
    match value.get("input") {
        None => details.push(error("missing", Some("input"), "Field required", &value)),
        Some(input) if !realtime && !input.is_object() => details.push(error(
            "dict_type",
            Some("input"),
            "Input should be a valid dictionary",
            input,
        )),
        Some(input) if realtime && input.is_null() => {
            for (kind, label, noun) in [
                ("dict", "dict[str,any]", "dictionary"),
                ("list", "list[any]", "list"),
                ("string", "str", "string"),
                ("int", "int", "integer"),
                ("float", "float", "number"),
                ("bool", "bool", "boolean"),
            ] {
                let mut detail = error(
                    &format!("{kind}_type"),
                    Some("input"),
                    &format!("Input should be a valid {noun}"),
                    input,
                );
                detail["loc"].as_array_mut().unwrap().push(json!(label));
                details.push(detail);
            }
        }
        _ => {}
    }
    if !realtime
        && let Some(webhook) = value.get("webhook")
        && !webhook.is_null()
        && !webhook.is_string()
    {
        details.push(error(
            "string_type",
            Some("webhook"),
            "Input should be a valid string",
            webhook,
        ));
    }
    if details.is_empty() {
        Ok(value)
    } else {
        Err(details)
    }
}
