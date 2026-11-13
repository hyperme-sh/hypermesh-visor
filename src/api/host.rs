//! Host tools use the existing authenticated API transports.
use super::*;
use crate::host_bridge::{tool_name, Call, Entry};
use serde_json::{json, Value};

fn refused() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":"host capability unavailable or refused"})),
    )
        .into_response()
}
pub(super) async fn catalog(State(state): State<Arc<AppState>>) -> Response {
    let Some(bridge) = &state.host_bridge else {
        return refused();
    };
    match bridge.catalog().await {
        Ok(entries) => Json(json!({"capabilities":entries})).into_response(),
        Err(()) => refused(),
    }
}
pub(super) async fn call(State(state): State<Arc<AppState>>, Json(call): Json<Call>) -> Response {
    let Some(bridge) = &state.host_bridge else {
        return refused();
    };
    match bridge.call(call).await {
        Ok(result) => Json(result).into_response(),
        Err(()) => refused(),
    }
}
fn tool(entry: &Entry) -> Result<Value, ()> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    if let Some(inputs) = entry.definition.get("inputs").and_then(Value::as_object) {
        for (name, input) in inputs {
            let kind = input["type"].as_str().ok_or(())?;
            let mut schema = match kind {
                "integer" | "number" | "boolean" | "string" => json!({"type":kind}),
                "vector3" => json!({"type":"array", "minItems":3,"maxItems":3}),
                _ => return Err(()),
            };
            for (source, target) in [
                ("minimum", "minimum"),
                ("maximum", "maximum"),
                ("max_length", "maxLength"),
                ("enum", "enum"),
            ] {
                if let Some(value) = input.get(source) {
                    schema[target] = value.clone();
                }
            }
            if kind == "vector3" {
                let axes = input["axes"]
                    .as_array()
                    .filter(|axes| axes.len() == 3)
                    .ok_or(())?;
                schema["prefixItems"] = Value::Array(axes.iter().map(|axis| json!({"type":"number","minimum":axis["minimum"],"maximum":axis["maximum"]})).collect());
            }
            if input["required"] == true {
                required.push(name.clone());
            }
            properties.insert(name.clone(), schema);
        }
    }
    Ok(
        json!({"name":tool_name(entry)?,"title":entry.definition["id"],
        "description":"Owner-enabled machine capability. Supply a fresh call_id; never retry an uncertain call.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["call_id","arguments"],"properties":{
            "call_id":{"type":"string","format":"uuid"},
            "arguments":{"type":"object","additionalProperties":false,"properties":properties,"required":required}
        }}}),
    )
}
fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}
pub(super) async fn mcp(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Value>,
) -> Response {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request["jsonrpc"] != "2.0"
        || !(id.is_null()
            || id.is_i64()
            || id.is_u64()
            || id.as_str().is_some_and(|s| s.len() <= 128))
    {
        return rpc_error(Value::Null, -32600, "Invalid request");
    }
    let method = request["method"].as_str().unwrap_or("");
    if method == "notifications/initialized"
        && !request.as_object().is_some_and(|r| r.contains_key("id"))
    {
        return StatusCode::ACCEPTED.into_response();
    }
    if id.is_null() {
        return rpc_error(id, -32600, "Request ID required");
    }
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":crate::mcp::protocol::VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"hypermesh-host-tools","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" | "tools/call" => {
            let Some(bridge) = &state.host_bridge else {
                return rpc_error(id, -32000, "Host capabilities disabled");
            };
            let entries = match bridge.catalog().await {
                Ok(v) => v,
                Err(()) => return rpc_error(id, -32000, "Host capabilities unavailable"),
            };
            if method == "tools/list" {
                let tools: Result<Vec<_>, _> = entries.iter().map(tool).collect();
                match tools {
                    Ok(tools) => json!({"tools":tools}),
                    Err(()) => return rpc_error(id, -32000, "Host catalog invalid"),
                }
            } else {
                let params = &request["params"];
                let Some(entry) = entries
                    .iter()
                    .find(|entry| tool_name(entry).ok().as_deref() == params["name"].as_str())
                else {
                    return rpc_error(id, -32602, "Unknown host tool");
                };
                let args = &params["arguments"];
                if args.as_object().is_none_or(|a| a.len() != 2) {
                    return rpc_error(id, -32602, "Invalid arguments");
                }
                let (Some(call_id), Some(scope), Some(capability_id), Some(arguments)) = (
                    args["call_id"].as_str(),
                    entry.definition["scope"].as_str(),
                    entry.definition["id"].as_str(),
                    args.get("arguments"),
                ) else {
                    return rpc_error(id, -32602, "Invalid arguments");
                };
                let call = Call {
                    scope: scope.into(),
                    capability_id: capability_id.into(),
                    call_id: call_id.into(),
                    arguments: arguments.clone(),
                };
                match bridge.call(call).await {
                    Ok(result) => {
                        json!({"content":[{"type":"text","text":result.to_string()}],"isError":false})
                    }
                    Err(()) => {
                        json!({"content":[{"type":"text","text":"Host capability refused or outcome uncertain; do not retry."}],"isError":true})
                    }
                }
            }
        }
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_schema_preserves_typed_bounds_and_call_id() {
        let entry = Entry {
            definition: json!({"scope":"org/a/site/b","id":"clock/read","inputs":{"axis":{"type":"number","required":true,"minimum":-1,"maximum":1}}}),
            source_hash: "a".repeat(64),
            graph_hash: "b".repeat(64),
        };
        let tool = tool(&entry).unwrap();
        assert_eq!(
            tool["inputSchema"]["properties"]["arguments"]["properties"]["axis"]["minimum"],
            -1
        );
        assert_eq!(
            tool["inputSchema"]["required"],
            json!(["call_id", "arguments"])
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["name"].as_str().unwrap().len(), 69);
    }
}
