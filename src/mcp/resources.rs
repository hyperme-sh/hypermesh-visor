//! Explicit resource requests. URIs stay opaque; Visor never fetches them itself.
use super::McpError;
use base64::Engine;
use serde_json::Value;

fn invalid() -> McpError {
    McpError::message("Invalid or oversized MCP resource data")
}
fn text(value: Option<&Value>, limit: usize) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s.len() <= limit && !s.contains('\0'))
}

fn cursor(value: &Value) -> bool {
    value.as_str().is_some_and(|s| s.len() <= 4096)
}

fn uri(value: Option<&Value>) -> bool {
    text(value, 4096)
        && value.and_then(Value::as_str).is_some_and(|s| {
            !s.chars().any(|c| c.is_control() || c.is_whitespace()) && url::Url::parse(s).is_ok()
        })
}

pub(super) fn validate_request(method: &str, params: &Value) -> Result<(), McpError> {
    let object = params.as_object().ok_or_else(invalid)?;
    match method {
        "resources/list" | "resources/templates/list" => {
            if object.keys().any(|k| k != "cursor")
                || object.get("cursor").is_some_and(|v| !cursor(v))
            {
                return Err(invalid());
            }
        }
        "resources/read" => {
            if object.len() != 1 || !uri(object.get("uri")) {
                return Err(invalid());
            }
        }
        _ => return Err(McpError::message("Unsupported MCP resource operation")),
    }
    Ok(())
}

pub(super) fn validate_result(method: &str, value: &Value) -> Result<(), McpError> {
    if !value.is_object() || value.to_string().len() > 1024 * 1024 {
        return Err(invalid());
    }
    let field = match method {
        "resources/list" => "resources",
        "resources/templates/list" => "resourceTemplates",
        "resources/read" => "contents",
        _ => return Err(invalid()),
    };
    let rows = value[field]
        .as_array()
        .filter(|rows| rows.len() <= 256)
        .ok_or_else(invalid)?;
    if value
        .get("nextCursor")
        .is_some_and(|v| method == "resources/read" || !cursor(v))
    {
        return Err(invalid());
    }
    for row in rows {
        let identity = if field == "resourceTemplates" {
            "uriTemplate"
        } else {
            "uri"
        };
        if !row.is_object()
            || !(if field == "resourceTemplates" {
                text(row.get(identity), 4096)
            } else {
                uri(row.get(identity))
            })
        {
            return Err(invalid());
        }
        for key in ["mimeType", "description", "title"] {
            if row.get(key).is_some_and(|v| !v.is_string()) {
                return Err(invalid());
            }
        }
        if field != "contents" {
            if !text(row.get("name"), 1024) {
                return Err(invalid());
            }
        } else {
            match (row.get("text"), row.get("blob")) {
                (Some(Value::String(_)), None) => {}
                (None, Some(Value::String(blob))) => {
                    base64::engine::general_purpose::STANDARD
                        .decode(blob)
                        .map_err(|_| invalid())?;
                }
                _ => return Err(invalid()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn requests_are_bounded_and_never_expand_to_other_methods() {
        for method in ["resources/list", "resources/templates/list"] {
            validate_request(method, &json!({})).unwrap();
            validate_request(method, &json!({"cursor":""})).unwrap();
            validate_request(method, &json!({"cursor":"opaque"})).unwrap();
            assert!(validate_request(method, &json!({"uri":"file:///secret"})).is_err());
        }
        validate_request("resources/read", &json!({"uri":"custom://opaque/path"})).unwrap();
        for params in [
            json!({}),
            json!({"uri":""}),
            json!({"uri":"relative/path"}),
            json!({"uri":"https://example.test/a b"}),
            json!({"uri":"https://example.test/\n"}),
            json!({"uri":"memory://x","extra":true}),
            json!({"uri":"memory://x".repeat(4097)}),
        ] {
            assert!(validate_request("resources/read", &params).is_err());
        }
        assert!(validate_request("resources/subscribe", &json!({"uri":"memory://x"})).is_err());
    }
    #[test]
    fn results_validate_pagination_content_and_binary_encoding() {
        validate_result(
            "resources/list",
            &json!({"resources":[{"uri":"file:///x","name":"x"}],"nextCursor":"next"}),
        )
        .unwrap();
        validate_result(
            "resources/templates/list",
            &json!({"resourceTemplates":[{"uriTemplate":"custom://{id}","name":"entry"}]}),
        )
        .unwrap();
        for content in [
            json!({"uri":"memory://x","text":""}),
            json!({"uri":"memory://x","blob":"AAE="}),
        ] {
            validate_result("resources/read", &json!({"contents":[content]})).unwrap();
        }
        for content in [
            json!({"uri":"memory://x","blob":"not base64"}),
            json!({"uri":"memory://x","text":"private","blob":"AA=="}),
            json!({"uri":"memory://x","text":null}),
            json!({"text":"private"}),
        ] {
            let error = validate_result("resources/read", &json!({"contents":[content]}))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private"));
        }
        assert!(
            validate_result("resources/list", &json!({"resources":[],"nextCursor":null})).is_err()
        );
        assert!(validate_result(
            "resources/list",
            &json!({"resources":vec![json!({"uri":"memory://x","name":"x"});257]})
        )
        .is_err());
        assert!(validate_result(
            "resources/read",
            &json!({"contents":[{"uri":"memory://x","text":"x".repeat(1024*1024)}]})
        )
        .is_err());
    }
}
