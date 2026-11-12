//! Local tool contracts. Schema references never authorize file or network access.
use super::McpError;
use serde_json::Value;

fn invalid() -> McpError {
    // Validator errors contain instance values, property names and schema URIs.
    McpError::message("MCP tool data does not satisfy its local schema contract")
}

fn bounded(value: &Value, depth: usize, remaining: &mut usize) -> bool {
    if depth > 32 || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    match value {
        Value::Object(values) => values.values().all(|v| bounded(v, depth + 1, remaining)),
        Value::Array(values) => values.iter().all(|v| bounded(v, depth + 1, remaining)),
        _ => true,
    }
}

pub(super) fn validate(schema: &Value, instance: &Value) -> Result<(), McpError> {
    if schema.to_string().len() > 64 * 1024
        || instance.to_string().len() > 1024 * 1024
        || !bounded(schema, 0, &mut 4096)
        || !bounded(instance, 0, &mut 16384)
    {
        return Err(invalid());
    }
    let validator = jsonschema::options()
        .offline()
        .with_pattern_options(jsonschema::PatternOptions::fancy_regex()
            .backtrack_limit(10_000).size_limit(1024 * 1024).dfa_size_limit(1024 * 1024))
        .build(schema)
        .map_err(|_| invalid())?;
    if validator.is_valid(instance) { Ok(()) } else { Err(invalid()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constraints_and_local_definitions_are_enforced_without_returning_values() {
        let schema = json!({"type":"object", "required":["secret"], "additionalProperties":false,
            "$defs":{"entry":{"type":"string","minLength":3}},
            "properties":{"secret":{"$ref":"#/$defs/entry"}}});
        validate(&schema, &json!({"secret":"valid"})).unwrap();
        for instance in [json!({}), json!({"secret":1}), json!({"secret":"x"}), json!({"secret":"valid","extra":true})] {
            let error = validate(&schema, &instance).unwrap_err().to_string();
            assert!(!error.contains("secret") && !error.contains("valid"));
        }
    }

    #[test]
    fn external_references_and_invalid_or_excessive_schemas_are_refused() {
        for reference in ["https://127.0.0.1:9/private", "file:///private", "unknown://private"] {
            assert!(validate(&json!({"$ref":reference}), &json!({})).is_err());
        }
        assert!(validate(&json!({"type":"not-a-type"}), &json!({})).is_err());
        assert!(validate(&json!({}), &json!("x".repeat(1024*1024))).is_err());
        let mut deep = json!({});
        for _ in 0..34 { deep = json!({"properties":{"nested":deep}}); }
        assert!(validate(&deep, &json!({})).is_err());
    }
}
