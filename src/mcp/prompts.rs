//! Explicit prompt-template retrieval. Returned content is never executed.
use super::McpError;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashSet;

fn invalid() -> McpError {
    McpError::message("Invalid or oversized MCP prompt data")
}
fn name(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s.len() <= 1024 && !s.contains('\0'))
}
fn cursor(value: &Value) -> bool {
    value.as_str().is_some_and(|s| s.len() <= 4096)
}
fn optional_text(value: &Value, key: &str) -> bool {
    value.get(key).is_none_or(Value::is_string)
}

pub(super) fn validate_request(method: &str, params: &Value) -> Result<(), McpError> {
    let object = params.as_object().ok_or_else(invalid)?;
    if params.to_string().len() > 64 * 1024 {
        return Err(invalid());
    }
    match method {
        "prompts/list" => {
            if object.keys().any(|key| key != "cursor")
                || object.get("cursor").is_some_and(|v| !cursor(v))
            {
                return Err(invalid());
            }
        }
        "prompts/get" => {
            if !name(object.get("name"))
                || object.keys().any(|key| key != "name" && key != "arguments")
            {
                return Err(invalid());
            }
            if let Some(arguments) = object.get("arguments") {
                let arguments = arguments
                    .as_object()
                    .filter(|a| a.len() <= 128)
                    .ok_or_else(invalid)?;
                for (key, value) in arguments {
                    if key.is_empty()
                        || key.len() > 1024
                        || key.contains('\0')
                        || !value.is_string()
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

pub(super) fn validate_result(method: &str, value: &Value) -> Result<(), McpError> {
    if !value.is_object() || value.to_string().len() > 1024 * 1024 {
        return Err(invalid());
    }
    match method {
        "prompts/list" => {
            if value.get("nextCursor").is_some_and(|v| !cursor(v)) {
                return Err(invalid());
            }
            let rows = value["prompts"]
                .as_array()
                .filter(|rows| rows.len() <= 256)
                .ok_or_else(invalid)?;
            let mut names = HashSet::new();
            for row in rows {
                if !row.is_object()
                    || !name(row.get("name"))
                    || !optional_text(row, "description")
                    || !optional_text(row, "title")
                    || !names.insert(row["name"].as_str().unwrap())
                {
                    return Err(invalid());
                }
                if let Some(arguments) = row.get("arguments") {
                    let arguments = arguments
                        .as_array()
                        .filter(|a| a.len() <= 128)
                        .ok_or_else(invalid)?;
                    let mut names = HashSet::new();
                    for argument in arguments {
                        if !argument.is_object()
                            || !name(argument.get("name"))
                            || !optional_text(argument, "description")
                            || !optional_text(argument, "title")
                            || argument.get("required").is_some_and(|v| !v.is_boolean())
                            || !names.insert(argument["name"].as_str().unwrap())
                        {
                            return Err(invalid());
                        }
                    }
                }
            }
        }
        "prompts/get" => {
            if !optional_text(value, "description") || value.get("nextCursor").is_some() {
                return Err(invalid());
            }
            let messages = value["messages"]
                .as_array()
                .filter(|rows| rows.len() <= 256)
                .ok_or_else(invalid)?;
            for message in messages {
                if !matches!(message["role"].as_str(), Some("user" | "assistant")) {
                    return Err(invalid());
                }
                let content = &message["content"];
                match content["type"].as_str() {
                    Some("text") if content["text"].is_string() => {}
                    Some("image" | "audio") => {
                        if !name(content.get("mimeType")) {
                            return Err(invalid());
                        }
                        let data = content["data"].as_str().ok_or_else(invalid)?;
                        base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .map_err(|_| invalid())?;
                    }
                    Some("resource") => super::resources::validate_result(
                        "resources/read",
                        &json!({"contents":[content["resource"]]}),
                    )
                    .map_err(|_| invalid())?,
                    Some("resource_link") => super::resources::validate_result(
                        "resources/list",
                        &json!({"resources":[content]}),
                    )
                    .map_err(|_| invalid())?,
                    _ => return Err(invalid()),
                }
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_bound_arguments_and_preserve_opaque_cursors() {
        validate_request("prompts/list", &json!({"cursor":""})).unwrap();
        validate_request(
            "prompts/get",
            &json!({"name":"review","arguments":{"code":"private\ncode"}}),
        )
        .unwrap();
        for params in [
            json!({}),
            json!({"name":""}),
            json!({"name":"review","arguments":{"code":1}}),
            json!({"name":"review","arguments":{"code":"x".repeat(65536)}}),
            json!({"name":"review","extra":true}),
        ] {
            assert!(validate_request("prompts/get", &params).is_err());
        }
        assert!(validate_request("prompts/list", &json!({"cursor":null})).is_err());
        assert!(validate_request("completion/complete", &json!({})).is_err());
    }
    #[test]
    fn results_validate_catalog_and_all_content_kinds_without_fetching() {
        validate_result("prompts/list", &json!({"prompts":[{"name":"review","arguments":[{"name":"code","required":true}]}],"nextCursor":"next"})).unwrap();
        for content in [
            json!({"type":"text","text":""}),
            json!({"type":"image","data":"AA==","mimeType":"image/png"}),
            json!({"type":"audio","data":"AA==","mimeType":"audio/wav"}),
            json!({"type":"resource","resource":{"uri":"private://entry","text":"private content"}}),
            json!({"type":"resource_link","uri":"file:///private","name":"entry"}),
        ] {
            validate_result(
                "prompts/get",
                &json!({"messages":[{"role":"assistant","content":content}]}),
            )
            .unwrap();
        }
        for content in [
            json!({"type":"text","text":null}),
            json!({"type":"image","data":"private","mimeType":"image/png"}),
            json!({"type":"resource","resource":{"uri":"relative","text":"private"}}),
            json!({"type":"unknown","text":"private"}),
        ] {
            let error = validate_result(
                "prompts/get",
                &json!({"messages":[{"role":"user","content":content}]}),
            )
            .unwrap_err()
            .to_string();
            assert!(!error.contains("private"));
        }
        assert!(validate_result(
            "prompts/get",
            &json!({"messages":[{"role":"system","content":{"type":"text","text":"private"}}]})
        )
        .is_err());
        assert!(validate_result(
            "prompts/list",
            &json!({"prompts":[{"name":"x"},{"name":"x"}]})
        )
        .is_err());
        assert!(validate_result(
            "prompts/list",
            &json!({"prompts":[{"name":"x","arguments":[{"name":"a","required":"yes"}]}]})
        )
        .is_err());
        assert!(validate_result("prompts/get", &json!({"messages":vec![json!({"role":"user","content":{"type":"text","text":""}});257]})).is_err());
    }
}
