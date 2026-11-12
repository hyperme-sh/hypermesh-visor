//! Inverse MCP: expose visor desktop verbs as MCP tools for external agents.
//!
//! Stdio JSON-RPC. Tools: capabilities, view, mouse_move, mouse_click, mouse_scroll, type_text.
//! Talks to the visor's control socket by default. `HYPERMESH_VISOR_URL` selects
//! the opt-in TCP listener, with `HYPERMESH_VISOR_TOKEN` or the `api.token` file.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use hypermesh_visor::SCROLL_MAX_TICKS;
use serde_json::{json, Value};

const MAX_REQUEST_BYTES: usize = 512 * 1024;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 4096;
const MAX_COORD: i64 = 32767;
const MAX_OWNED_TASKS: usize = 16;

struct VisorResponse {
    status: u16,
    mime: String,
    body: Vec<u8>,
}

struct CallResult {
    content: Value,
    missing_task: Option<String>,
}

enum VisorEndpoint {
    Unix(PathBuf),
    Http { base: String, token: Option<String> },
}

impl VisorEndpoint {
    fn from_env() -> Result<Self, String> {
        match std::env::var("HYPERMESH_VISOR_URL") {
            Ok(url) if !url.trim().is_empty() => Ok(Self::Http {
                base: url,
                token: configured_token(),
            }),
            _ => {
                let dir = hypermesh_visor::runtime_dir().map_err(|err| err.to_string())?;
                Ok(Self::Unix(hypermesh_visor::control_socket_file(&dir)))
            }
        }
    }
}

fn configured_token() -> Option<String> {
    if let Ok(token) = std::env::var("HYPERMESH_VISOR_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    let Ok(dir) = hypermesh_visor::runtime_dir() else {
        return None;
    };
    hypermesh_visor::read_api_token(&dir).ok()
}

fn main() {
    let _reports = hypermesh_visor::telemetry::init(
        hypermesh_visor::telemetry::Component::InverseMcp,
        env!("CARGO_PKG_VERSION"),
    );
    let visor = match VisorEndpoint::from_env() {
        Ok(visor) => visor,
        Err(err) => {
            eprintln!("hypermesh-desktop-mcp: {err}");
            std::process::exit(1);
        }
    };
    let session = std::env::var("HYPERMESH_SESSION_ID").unwrap_or_default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut initialized = false;
    let mut ready = false;
    let mut owned_tasks = std::collections::VecDeque::<String>::new();
    let mut input = stdin.lock();
    loop {
        let mut line = String::new();
        match (&mut input)
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_line(&mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_REQUEST_BYTES {
            rpc_error(&mut stdout, Value::Null, -32600, "request exceeds 512 KiB");
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            rpc_error(&mut stdout, Value::Null, -32700, "parse error");
            continue;
        };
        let id = msg
            .get("id")
            .filter(|id| id.is_string() || id.is_i64() || id.is_u64())
            .cloned();
        if !msg.is_object()
            || msg["jsonrpc"] != "2.0"
            || !msg["method"].is_string()
            || (msg.get("id").is_some() && id.is_none())
        {
            rpc_error(
                &mut stdout,
                id.unwrap_or(Value::Null),
                -32600,
                "invalid request",
            );
            continue;
        }
        // Notifications never receive responses and cannot invoke tools.
        if id.is_none() {
            if msg["method"] == "notifications/initialized" && initialized {
                ready = true;
            }
            // Cancellation for completed/non-cancellable calls is ignored. Queued
            // desktop jobs have explicit session-bound task_cancel semantics.
            continue;
        }
        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
            match method {
                "initialize" => {
                    if initialized {
                        rpc_error(&mut stdout, id.unwrap(), -32600, "already initialized");
                        continue;
                    }
                    if !valid_initialize(msg.get("params")) {
                        rpc_error(
                            &mut stdout,
                            id.unwrap(),
                            -32602,
                            "invalid initialize parameters",
                        );
                        continue;
                    }
                    initialized = true;
                    reply(
                        &mut stdout,
                        msg.get("id").cloned(),
                        json!({
                            "protocolVersion": "2024-11-05",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "hypermesh-desktop", "version": "0.1.0"}
                        }),
                    );
                }
                "ping" => reply(&mut stdout, id, json!({})),
                _ if !ready => {
                    rpc_error(
                        &mut stdout,
                        id.unwrap(),
                        -32002,
                        "initialize and send notifications/initialized first",
                    );
                }
                "tools/list" => {
                    reply(&mut stdout, msg.get("id").cloned(), tool_list());
                }
                "tools/call" => {
                    // A tool invocation is a request, never an executable notification.
                    if let Some(id) = msg.get("id").filter(|id| id.is_string() || id.is_number()) {
                        let result = if msg.pointer("/params/name").and_then(Value::as_str)
                            == Some("task_submit")
                            && owned_tasks.len() >= MAX_OWNED_TASKS
                        {
                            // Never discard ownership of a possibly active job. The
                            // daemon's queue limit does not bound jobs this client has
                            // submitted over time, including externally cancelled jobs.
                            Ok(CallResult {
                                content: failure("task ownership limit reached; use task_status or task_cancel to confirm a tracked task is terminal or missing before submitting another".to_owned()),
                                missing_task: None,
                            })
                        } else {
                            handle_call(&visor, &session, msg.get("params"))
                        };
                        if let Ok(result) = &result {
                            if let Some(id) = &result.missing_task {
                                owned_tasks.retain(|known| known != id);
                            }
                            let result = &result.content;
                            let name = msg.pointer("/params/name").and_then(Value::as_str);
                            if matches!(name, Some("task_submit" | "task_status" | "task_cancel"))
                                && result["isError"] == false
                            {
                                if let Some(text) =
                                    result.pointer("/content/0/text").and_then(Value::as_str)
                                {
                                    if let Ok(task) = serde_json::from_str::<Value>(text) {
                                        if let Some(id) = task
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                                        {
                                            let terminal = task
                                                .get("status")
                                                .and_then(Value::as_str)
                                                .is_some_and(|status| {
                                                    ["completed", "failed", "cancelled"]
                                                        .contains(&status)
                                                });
                                            if terminal {
                                                owned_tasks.retain(|known| known != id);
                                            } else if name == Some("task_submit")
                                                && !owned_tasks.iter().any(|known| known == id)
                                            {
                                                owned_tasks.push_back(id.to_owned());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let envelope = match result {
                            Ok(result) => {
                                json!({"jsonrpc":"2.0", "id":id, "result":result.content})
                            }
                            Err(message) => json!({"jsonrpc":"2.0", "id":id,
                                "error":{"code":-32602,"message":message}}),
                        };
                        let _ = writeln!(stdout, "{envelope}");
                        let _ = stdout.flush();
                    }
                }
                _ => {
                    if msg.get("id").is_some() {
                        let _ = writeln!(
                            stdout,
                            "{}",
                            json!({
                                "jsonrpc": "2.0",
                                "id": msg.get("id"),
                                "error": {"code": -32601, "message": "method not found"}
                            })
                        );
                        let _ = stdout.flush();
                    }
                }
            }
        }
    }
    // Cancel only jobs submitted by this connection. Abrupt process loss is
    // additionally bounded by the daemon's task deadline.
    std::thread::scope(|scope| {
        for id in &owned_tasks {
            let visor = &visor;
            let session = &session;
            scope.spawn(move || {
                let _ = handle_call(
                    visor,
                    session,
                    Some(&json!({"name":"task_cancel","arguments":{"task_id":id}})),
                );
            });
        }
    });
}

fn rpc_error(stdout: &mut impl Write, id: Value, code: i64, message: &str) {
    let body = json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}});
    let _ = writeln!(stdout, "{body}");
    let _ = stdout.flush();
}

fn valid_initialize(params: Option<&Value>) -> bool {
    let Some(params) = params.and_then(Value::as_object) else {
        return false;
    };
    params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
        && params.get("capabilities").is_some_and(Value::is_object)
        && params.get("clientInfo").is_some_and(|client| {
            client
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
                && client.get("version").and_then(Value::as_str).is_some()
        })
}

fn reply(stdout: &mut impl Write, id: Option<Value>, result: Value) {
    let mut body = json!({"jsonrpc": "2.0", "result": result});
    if let Some(id) = id {
        body["id"] = id;
    }
    let _ = writeln!(stdout, "{body}");
    let _ = stdout.flush();
}

fn tool_list() -> Value {
    let coordinate = json!({"type":"integer", "minimum":0, "maximum":MAX_COORD});
    let delta = json!({"type":"integer","minimum":-SCROLL_MAX_TICKS,"maximum":SCROLL_MAX_TICKS});
    let mut catalog = json!({"tools":[
        {"name":"capabilities", "description":"Read the authenticated visor capability contract; does not grant desktop access", "inputSchema":
            {"type":"object", "properties":{}, "additionalProperties":false}},
        {"name":"view", "description":"Capture the desktop as a PNG image", "inputSchema":
            {"type":"object", "properties":{}, "additionalProperties":false}},
        {"name":"mouse_move", "description":"Move the pointer in root-window pixels", "inputSchema":
            {"type":"object", "properties":{"x":coordinate,"y":coordinate}, "required":["x","y"], "additionalProperties":false}},
        {"name":"mouse_click", "description":"Click in root-window pixels", "inputSchema":
            {"type":"object", "properties":{"x":coordinate,"y":coordinate,
                "button":{"type":"string","enum":["left","right","middle"],"default":"left"}},
             "required":["x","y"],"additionalProperties":false}},
        {"name":"mouse_scroll", "description":"Scroll at root-window x/y in wheel ticks: positive right/down; 1 to 128 total ticks, horizontal then vertical", "inputSchema":
            {"type":"object", "properties":{"x":coordinate,"y":coordinate,"delta_x":delta,"delta_y":delta},
             "required":["x","y","delta_x","delta_y"],"additionalProperties":false}},
        {"name":"type_text", "description":"Type text; the visor also validates key support and the planned keystroke limit", "inputSchema":
            {"type":"object", "properties":{"text":{"type":"string","minLength":1,"maxLength":MAX_TEXT_CHARS}},
             "required":["text"],"additionalProperties":false}}
    ]});
    catalog["tools"].as_array_mut().unwrap().push(json!({
        "name":"mouse_drag", "description":"Drag between root-window pixel coordinates using the existing user-pause gate",
        "inputSchema":{"type":"object","properties":{"x":coordinate,"y":coordinate,"to_x":coordinate,"to_y":coordinate,
            "button":{"type":"string","enum":["left","right","middle"],"default":"left"}},
            "required":["x","y","to_x","to_y"],"additionalProperties":false}
    }));
    catalog["tools"].as_array_mut().unwrap().push(json!({
        "name":"key_chord", "description":"Press 1 to 8 named keys together, then release them in reverse order; preserves user pause",
        "inputSchema":{"type":"object","properties":{"keys":{"type":"array","minItems":1,"maxItems":8,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":32}}},
        "required":["keys"],"additionalProperties":false}
    }));
    let steps: Vec<Value> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["name"] != "capabilities")
        .map(|tool| {
            json!({"type":"object","properties":{
            "tool":{"const":tool["name"]},"arguments":tool["inputSchema"]},
            "required":["tool","arguments"],"additionalProperties":false})
        })
        .collect();
    catalog["tools"].as_array_mut().unwrap().extend([
        json!({"name":"task_submit","description":"Queue up to 16 desktop steps for the assigned session; preserves user pause",
            "inputSchema":{"type":"object","properties":{"steps":{"type":"array","minItems":1,"maxItems":16,"items":{"oneOf":steps}}},"required":["steps"],"additionalProperties":false}}),
        json!({"name":"task_status","description":"Read status and bounded results for a task in the assigned session",
            "inputSchema":{"type":"object","properties":{"task_id":{"type":"string","format":"uuid"}},"required":["task_id"],"additionalProperties":false}}),
        json!({"name":"task_cancel","description":"Cancel a queued task or request interruption of its active input; inspect status until terminal",
            "inputSchema":{"type":"object","properties":{"task_id":{"type":"string","format":"uuid"}},"required":["task_id"],"additionalProperties":false}}),
    ]);
    catalog
}

fn handle_call(
    visor: &VisorEndpoint,
    session: &str,
    params: Option<&Value>,
) -> Result<CallResult, String> {
    let _attribution = hypermesh_visor::telemetry::UpstreamScope::enter(
        hypermesh_visor::telemetry::Service::Visor,
    );
    let params = params
        .and_then(Value::as_object)
        .ok_or("params must be an object")?;
    let tool = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("name must be a string")?;
    let empty = json!({});
    let args = params.get("arguments").unwrap_or(&empty);
    // Validate before connecting, so malformed input can never drive the desktop.
    let (method, path, body) = tool_request(session, tool, args)?;
    let response = match visor {
        VisorEndpoint::Unix(socket) => unix_exchange(socket, method, &path, body.as_deref()),
        VisorEndpoint::Http { base, token } => http_exchange(
            &format!("{}{path}", base.trim_end_matches('/')),
            method,
            body.as_deref(),
            token.as_deref(),
        ),
    };
    let mut missing_task = None;
    let content = match response {
        Ok(response) => {
            // The daemon evicts only terminal records. A confirmed absent record
            // (or closed session) cannot contain active work owned by this client.
            // Do not reclaim on proxy 404s, denial, or uncertain transport errors.
            if matches!(tool, "task_status" | "task_cancel")
                && response.status == 404
                && response
                    .mime
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
                && serde_json::from_slice::<Value>(&response.body)
                    .ok()
                    .is_some_and(|body| body["error"] == "session closed or unknown")
            {
                missing_task = args["task_id"]
                    .as_str()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .map(|id| id.to_string());
            }
            tool_result(tool, response)
        }
        Err(error) => failure(format!("{tool}: {error}")),
    };
    Ok(CallResult {
        content,
        missing_task,
    })
}

fn tool_request(
    session: &str,
    tool: &str,
    args: &Value,
) -> Result<(&'static str, String, Option<String>), String> {
    let allowed: &[&str] = match tool {
        "capabilities" | "view" => &[],
        "task_submit" => &["steps"],
        "task_status" | "task_cancel" => &["task_id"],
        "mouse_move" => &["x", "y"],
        "mouse_click" => &["x", "y", "button"],
        "mouse_drag" => &["x", "y", "to_x", "to_y", "button"],
        "mouse_scroll" => &["x", "y", "delta_x", "delta_y"],
        "type_text" => &["text"],
        "key_chord" => &["keys"],
        _ => return Err("unknown tool".into()),
    };
    let args = args.as_object().ok_or("arguments must be an object")?;
    if args.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unexpected argument".into());
    }
    if tool == "capabilities" {
        return Ok(("GET", "/capabilities".into(), None));
    }
    let session =
        uuid::Uuid::parse_str(session).map_err(|_| "HYPERMESH_SESSION_ID must be a UUID")?;
    let coord = |name: &str| -> Result<i64, String> {
        args.get(name)
            .and_then(Value::as_i64)
            .filter(|value| (0..=MAX_COORD).contains(value))
            .ok_or_else(|| format!("{name} must be an integer from 0 to {MAX_COORD}"))
    };
    match tool {
        "task_submit" => {
            let steps = args
                .get("steps")
                .and_then(Value::as_array)
                .filter(|steps| (1..=16).contains(&steps.len()))
                .ok_or("steps must contain 1 to 16 operations")?;
            for step in steps {
                let object = step.as_object().ok_or("each step must be an object")?;
                if object
                    .keys()
                    .any(|key| !["tool", "arguments"].contains(&key.as_str()))
                {
                    return Err("unexpected step field".into());
                }
                let name = object
                    .get("tool")
                    .and_then(Value::as_str)
                    .ok_or("step tool is required")?;
                if ![
                    "view",
                    "mouse_move",
                    "mouse_click",
                    "mouse_drag",
                    "mouse_scroll",
                    "type_text",
                    "key_chord",
                ]
                .contains(&name)
                {
                    return Err("unsupported task tool".into());
                }
                tool_request(
                    &session.to_string(),
                    name,
                    object
                        .get("arguments")
                        .ok_or("step arguments are required")?,
                )?;
            }
            Ok((
                "POST",
                format!("/session/{session}/tasks"),
                Some(json!({"steps":steps}).to_string()),
            ))
        }
        "task_status" | "task_cancel" => {
            let task = args
                .get("task_id")
                .and_then(Value::as_str)
                .ok_or("task_id is required")?;
            let task = uuid::Uuid::parse_str(task).map_err(|_| "task_id must be a UUID")?;
            Ok((
                if tool == "task_cancel" {
                    "DELETE"
                } else {
                    "GET"
                },
                format!("/session/{session}/tasks/{task}"),
                None,
            ))
        }
        "view" => Ok(("POST", format!("/session/{session}/view"), None)),
        "mouse_move" | "mouse_click" | "mouse_drag" => {
            let x = coord("x")?;
            let y = coord("y")?;
            let mut body = json!({"action":if tool == "mouse_move" {"move"} else if tool == "mouse_drag" {"drag"} else {"click"},"x":x,"y":y});
            if tool == "mouse_drag" {
                body["to_x"] = json!(coord("to_x")?);
                body["to_y"] = json!(coord("to_y")?);
            }
            if tool != "mouse_move" {
                let button = match args.get("button") {
                    None => "left",
                    Some(value) => value
                        .as_str()
                        .filter(|b| ["left", "right", "middle"].contains(b))
                        .ok_or("button must be left, right, or middle")?,
                };
                body["button"] = json!(button);
            }
            Ok((
                "POST",
                format!("/session/{session}/mouse"),
                Some(body.to_string()),
            ))
        }
        "mouse_scroll" => {
            let x = coord("x")?;
            let y = coord("y")?;
            let delta = |name: &str| -> Result<i64, String> {
                args.get(name).and_then(Value::as_i64).filter(|n| (-SCROLL_MAX_TICKS..=SCROLL_MAX_TICKS).contains(n))
                    .ok_or_else(|| format!("{name} must be an integer from -{SCROLL_MAX_TICKS} to {SCROLL_MAX_TICKS}"))
            };
            let dx = delta("delta_x")?;
            let dy = delta("delta_y")?;
            if !(1..=SCROLL_MAX_TICKS).contains(&(dx.abs() + dy.abs())) {
                return Err("scroll must contain 1 to 128 total ticks".into());
            }
            Ok((
                "POST",
                format!("/session/{session}/mouse"),
                Some(json!({"action":"scroll","x":x,"y":y,"delta_x":dx,"delta_y":dy}).to_string()),
            ))
        }
        "key_chord" => {
            let body =
                hypermesh_visor::key_chord_request(args.get("keys").ok_or("keys are required")?)?;
            Ok((
                "POST",
                format!("/session/{session}/type"),
                Some(body.to_string()),
            ))
        }
        "type_text" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("text must be a string")?;
            if text.is_empty() || text.chars().count() > MAX_TEXT_CHARS {
                return Err(format!(
                    "text must contain 1 to {MAX_TEXT_CHARS} characters"
                ));
            }
            Ok((
                "POST",
                format!("/session/{session}/type"),
                Some(json!({"text":text}).to_string()),
            ))
        }
        _ => unreachable!(),
    }
}

fn failure(message: String) -> Value {
    json!({"content":[{"type":"text","text":message}],"isError":true})
}

fn tool_result(tool: &str, response: VisorResponse) -> Value {
    if !(200..300).contains(&response.status) {
        let error = serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned));
        let detail = error
            .map(|s| format!(": {}", s.chars().take(2048).collect::<String>()))
            .unwrap_or_default();
        return failure(format!("{tool} status {}{detail}", response.status));
    }
    if matches!(
        tool,
        "capabilities" | "task_submit" | "task_status" | "task_cancel"
    ) {
        let is_json = response
            .mime
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
        let contract = serde_json::from_slice::<Value>(&response.body);
        return match contract {
            Ok(mut contract) if is_json && contract.is_object() => {
                let mut content = vec![json!({"type":"text","text":""})];
                if tool == "task_status" {
                    if let Some(results) = contract.get_mut("results").and_then(Value::as_array_mut)
                    {
                        for result in results {
                            if result["type"] == "image" && result["mimeType"] == "image/png" {
                                let valid = result
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .and_then(|data| {
                                        base64::engine::general_purpose::STANDARD.decode(data).ok()
                                    })
                                    .is_some_and(|bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
                                if !valid {
                                    return failure("task returned an invalid PNG result".into());
                                }
                                let index = content.len();
                                content.push(result.clone());
                                *result = json!({"type":"image","mimeType":"image/png","content_index":index});
                            }
                        }
                    }
                }
                content[0]["text"] = json!(contract.to_string());
                json!({"content":content,"isError":false})
            }
            _ => failure(format!("{tool} returned an invalid JSON object")),
        };
    }
    if tool == "view" {
        if !response
            .mime
            .split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("image/png"))
            || !response.body.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return failure("view returned an invalid PNG response".into());
        }
        return json!({"content":[{"type":"image", "mimeType":"image/png",
            "data":base64::engine::general_purpose::STANDARD.encode(response.body)}], "isError":false});
    }
    let text = match tool {
        "mouse_move" => "moved",
        "mouse_click" => "clicked",
        "mouse_drag" => "dragged",
        "mouse_scroll" => "scrolled",
        "type_text" => "typed",
        "key_chord" => "pressed and released keys",
        _ => "done",
    };
    json!({"content":[{"type":"text","text":text}],"isError":false})
}

fn http_exchange(
    url: &str,
    method: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Result<VisorResponse, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(30))
        .build();
    let mut request = agent
        .request(method, url)
        .set("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let result = match body {
        Some(body) => request.send_string(body),
        None => request.call(),
    };
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err("visor HTTP transport failed".into()),
    };
    let status = response.status();
    let mime = response.header("content-type").unwrap_or("").to_owned();
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| "visor response read failed")?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err("visor response exceeds 32 MiB".into());
    }
    Ok(VisorResponse { status, mime, body })
}

fn unix_exchange(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<VisorResponse, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let stream = tokio::net::UnixStream::connect(socket)
                .await
                .map_err(|e| e.to_string())?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|e| e.to_string())?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            let request = hyper::Request::builder()
                .method(method)
                .uri(path)
                .header("host", "localhost")
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(body.unwrap_or("").to_owned())))
                .map_err(|e| e.to_string())?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let mime = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
                .collect()
                .await
                .map_err(|_| "visor response is incomplete or exceeds 32 MiB")?
                .to_bytes()
                .to_vec();
            Ok(VisorResponse { status, mime, body })
        })
        .await
        .map_err(|_| "visor request timed out".to_string())?
    })
}

#[cfg(test)]
mod capability_contract_tests {
    use super::*;

    #[test]
    fn queued_drag_schema_and_request_preserve_valid_endpoints() {
        let args = json!({"steps":[{"tool":"mouse_drag","arguments":{"x":0,"y":1,"to_x":32767,"to_y":2}}]});
        let (method, path, body) =
            tool_request("00000000-0000-0000-0000-000000000001", "task_submit", &args).unwrap();
        assert_eq!(method, "POST");
        assert!(path.ends_with("/tasks"));
        assert_eq!(serde_json::from_str::<Value>(&body.unwrap()).unwrap(), args);
        let catalog = tool_list();
        let submit = catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "task_submit")
            .unwrap();
        assert!(submit
            .pointer("/inputSchema/properties/steps/items/oneOf")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step.pointer("/properties/tool/const") == Some(&json!("mouse_drag"))));
    }

    #[test]
    fn discovery_needs_no_session_but_never_accepts_override_arguments() {
        let (method, path, body) = tool_request("", "capabilities", &json!({})).unwrap();
        assert_eq!((method, path.as_str()), ("GET", "/capabilities"));
        assert!(body.is_none());
        assert!(tool_request("", "view", &json!({})).is_err());
        assert!(tool_request("", "capabilities", &json!({"session":"other"})).is_err());
    }
}
