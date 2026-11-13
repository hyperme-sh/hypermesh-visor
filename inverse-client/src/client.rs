//! Inverse MCP: expose visor desktop verbs as MCP tools for external agents.
//!
//! Stdio JSON-RPC. Tools: capabilities, view, mouse_move, mouse_click, mouse_scroll, type_text.
//! Talks to the visor's control socket by default. `HYPERMESH_VISOR_URL` selects
//! the opt-in TCP listener, with `HYPERMESH_VISOR_TOKEN` or the `api.token` file.

use std::cell::RefCell;
use std::io::{self, BufRead, Read, Write};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Weak,
};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use crate::input::SCROLL_MAX_TICKS;
use serde_json::{json, Value};

const MAX_REQUEST_BYTES: usize = 512 * 1024;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 4096;
const MAX_COORD: i64 = 32767;
const MAX_OWNED_TASKS: usize = 16;

const MAX_PENDING_REQUESTS: usize = 128;
thread_local! {
    static REQUEST_CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

struct CancellationScope(Option<Arc<AtomicBool>>);
impl CancellationScope {
    fn enter(cancel: Option<Arc<AtomicBool>>) -> Self {
        Self(REQUEST_CANCEL.with(|current| current.replace(cancel)))
    }
}
impl Drop for CancellationScope {
    fn drop(&mut self) {
        REQUEST_CANCEL.with(|current| {
            current.replace(self.0.take());
        });
    }
}
fn request_cancelled() -> bool {
    REQUEST_CANCEL.with(|current| {
        current
            .borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    })
}
struct Incoming {
    line: String,
    cancel: Arc<AtomicBool>,
}

fn incoming_requests() -> (mpsc::Receiver<Incoming>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::sync_channel(MAX_PENDING_REQUESTS);
    let overflow = Arc::new(AtomicBool::new(false));
    let stopped = overflow.clone();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        read_requests(stdin.lock(), tx, stopped);
    });
    (rx, overflow)
}

fn read_requests(
    mut input: impl BufRead,
    tx: mpsc::SyncSender<Incoming>,
    stopped: Arc<AtomicBool>,
) {
    let mut pending = Vec::<(Value, Weak<AtomicBool>)>::new();
    loop {
        let mut line = String::new();
        match (&mut input)
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_line(&mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        pending.retain(|(_, flag)| flag.strong_count() > 0);
        let parsed = serde_json::from_str::<Value>(&line).ok();
        let valid_id = |id: &Value| id.is_string() || id.is_i64() || id.is_u64();
        if let Some(message) = &parsed {
            if message["jsonrpc"] == "2.0"
                && message["method"] == "notifications/cancelled"
                && message.get("id").is_none()
            {
                if let Some(params) = message.get("params").and_then(Value::as_object) {
                    if message.get("result").is_none()
                        && message.get("error").is_none()
                        && params.get("reason").is_none_or(Value::is_string)
                    {
                        if let Some(id) = params.get("requestId").filter(|id| valid_id(id)) {
                            if let Some(flag) = pending.iter().find_map(|(known, flag)| {
                                (known == id).then(|| flag.upgrade()).flatten()
                            }) {
                                flag.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                continue;
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(message) = &parsed {
            if message["jsonrpc"] == "2.0"
                && message["method"].is_string()
                && message["method"] != "initialize"
            {
                if let Some(id) = message.get("id").filter(|id| valid_id(id)) {
                    pending.push((id.clone(), Arc::downgrade(&cancel)));
                }
            }
        }
        let oversized = line.len() > MAX_REQUEST_BYTES;
        if tx.try_send(Incoming { line, cancel }).is_err() {
            stopped.store(true, Ordering::SeqCst);
            for (_, flag) in pending {
                if let Some(flag) = flag.upgrade() {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            break;
        }
        if oversized {
            break;
        }
    }
}

struct ConnectionOutput<W> {
    inner: W,
    failed: bool,
}
impl<W: Write> Write for ConnectionOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.inner.write(bytes);
        self.failed |= result.is_err() || matches!(result, Ok(0)) && !bytes.is_empty();
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        let result = self.inner.flush();
        self.failed |= result.is_err();
        result
    }
}

fn send(stdout: &mut impl Write, body: Value) {
    if request_cancelled() {
        return;
    }
    let _ = writeln!(stdout, "{body}");
    let _ = stdout.flush();
}

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
    #[cfg(unix)]
    Unix(PathBuf),
    Http { base: String, token: Option<String> },
}

impl VisorEndpoint {
    fn from_env() -> Result<Self, String> {
        if let Some(endpoint) = configured_http(std::env::var("HYPERMESH_VISOR_URL").ok().as_deref(), configured_token(), cfg!(windows))? {
            return Ok(endpoint);
        }
        #[cfg(unix)]
        {
            let dir = crate::local_runtime::runtime_dir().map_err(|err| err.to_string())?;
            Ok(Self::Unix(crate::local_runtime::control_socket_file(&dir)))
        }
        #[cfg(not(unix))]
        { Err("Local Windows transport is not available; set HYPERMESH_VISOR_URL and HYPERMESH_VISOR_TOKEN".into()) }
    }
}

fn configured_http(url: Option<&str>, token: Option<String>, require_token: bool) -> Result<Option<VisorEndpoint>, String> {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else { return Ok(None); };
    if require_token && token.as_ref().is_none_or(|token| token.trim().is_empty()) {
        return Err("Windows client requires HYPERMESH_VISOR_TOKEN".into());
    }
    Ok(Some(VisorEndpoint::Http { base: url.into(), token }))
}

fn configured_token() -> Option<String> {
    if let Ok(token) = std::env::var("HYPERMESH_VISOR_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    #[cfg(unix)]
    let Ok(dir) = crate::local_runtime::runtime_dir() else {
        return None;
    };
    #[cfg(unix)]
    { crate::local_runtime::read_api_token(&dir).ok() }
    #[cfg(not(unix))]
    { None }
}

pub fn run() { run_with_version(env!("CARGO_PKG_VERSION")); }

pub fn run_with_version(version: &'static str) {
    let _reports = crate::telemetry::init(
        crate::telemetry::Component::InverseMcp,
        version,
    );
    let visor = match VisorEndpoint::from_env() {
        Ok(visor) => visor,
        Err(err) => {
            eprintln!("hypermesh-desktop-mcp: {err}");
            std::process::exit(1);
        }
    };
    let session = std::env::var("HYPERMESH_SESSION_ID").unwrap_or_default();
    let (incoming, overflow) = incoming_requests();
    let mut stdout = ConnectionOutput {
        inner: io::stdout(),
        failed: false,
    };
    let mut initialized = false;
    let mut ready = false;
    let mut owned_tasks = std::collections::VecDeque::<String>::new();
    loop {
        if stdout.failed {
            break;
        }
        let Ok(Incoming { line, cancel }) = incoming.recv() else {
            break;
        };
        if overflow.load(Ordering::SeqCst) {
            rpc_error(
                &mut stdout,
                Value::Null,
                -32000,
                "too many pending requests",
            );
            break;
        }
        if cancel.load(Ordering::SeqCst) {
            continue;
        }
        let _cancellation = CancellationScope::enter(Some(cancel));
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
            // The input reader handles cancellation without waiting for transport I/O.
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
                            "capabilities": {"tools": {}, "resources": {}},
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
                "resources/list" | "resources/templates/list" => {
                    if !resource_list_params(msg.get("params")) {
                        rpc_error(
                            &mut stdout,
                            id.unwrap(),
                            -32602,
                            "invalid resource list parameters",
                        );
                        continue;
                    }
                    let result = if method == "resources/list" {
                        let resources: Vec<Value> = owned_tasks.iter().filter_map(|task| {
                            task_resource_uri(&session, task).map(|uri| json!({"uri":uri,"name":"Desktop task","mimeType":"application/json"}))
                        }).collect();
                        json!({"resources":resources})
                    } else {
                        let templates: Vec<Value> = uuid::Uuid::parse_str(&session).ok().map(|session| {
                            json!({"uriTemplate":format!("visor-task://{session}/{{task_id}}"),"name":"Desktop task status and results","mimeType":"application/json"})
                        }).into_iter().collect();
                        json!({"resourceTemplates":templates})
                    };
                    reply(&mut stdout, id, result);
                }
                "resources/read" => match read_task_resource(&visor, &session, msg.get("params")) {
                    Ok((result, terminal)) => {
                        if let Some(task) = terminal {
                            owned_tasks.retain(|known| known != &task);
                        }
                        reply(&mut stdout, id, result);
                    }
                    Err((code, message, missing)) => {
                        if let Some(task) = missing {
                            owned_tasks.retain(|known| known != &task.to_string());
                        }
                        rpc_error(&mut stdout, id.unwrap(), code, message);
                    }
                },
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
                            let name = msg.pointer("/params/name").and_then(Value::as_str);
                            finish_task_call(&mut owned_tasks, name, result, &visor, &session);
                        }
                        let envelope = match result {
                            Ok(result) => {
                                json!({"jsonrpc":"2.0", "id":id, "result":result.content})
                            }
                            Err(message) => json!({"jsonrpc":"2.0", "id":id,
                                "error":{"code":-32602,"message":message}}),
                        };
                        send(&mut stdout, envelope);
                    }
                }
                _ => {
                    if msg.get("id").is_some() {
                        send(
                            &mut stdout,
                            json!({"jsonrpc":"2.0", "id":msg.get("id"),
                            "error":{"code":-32601,"message":"method not found"}}),
                        );
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
    send(stdout, body);
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
    send(stdout, body);
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

fn resource_list_params(params: Option<&Value>) -> bool {
    match params {
        None => true,
        Some(Value::Object(params)) => params
            .iter()
            .all(|(key, value)| key == "_meta" && value.is_object()),
        _ => false,
    }
}

fn task_resource_uri(session: &str, task: &str) -> Option<String> {
    Some(format!(
        "visor-task://{}/{}",
        uuid::Uuid::parse_str(session).ok()?,
        uuid::Uuid::parse_str(task).ok()?
    ))
}

fn read_task_resource(
    visor: &VisorEndpoint,
    session: &str,
    params: Option<&Value>,
) -> Result<(Value, Option<String>), (i64, &'static str, Option<uuid::Uuid>)> {
    let invalid = (-32602, "invalid task resource URI or parameters", None);
    let params = params.and_then(Value::as_object).ok_or(invalid)?;
    if params
        .iter()
        .any(|(key, value)| key != "uri" && !(key == "_meta" && value.is_object()))
    {
        return Err(invalid);
    }
    let uri = params.get("uri").and_then(Value::as_str).ok_or(invalid)?;
    let session = uuid::Uuid::parse_str(session).map_err(|_| invalid)?;
    let task = uri
        .strip_prefix(&format!("visor-task://{session}/"))
        .ok_or(invalid)?;
    let task_id = uuid::Uuid::parse_str(task).map_err(|_| invalid)?;
    let task = task_id.to_string();
    if task_resource_uri(&session.to_string(), &task).as_deref() != Some(uri) {
        return Err(invalid);
    }
    let response = visor_exchange(
        visor,
        "GET",
        &format!("/session/{session}/tasks/{task}"),
        None,
    )
    .map_err(|_| (-32603, "task resource transport failed", None))?;
    match response.status {
        404 => {
            return Err((
                -32002,
                "task resource not found",
                confirmed_missing_task(&response).then_some(task_id),
            ))
        }
        401 | 403 => return Err((-32001, "task resource access denied", None)),
        200 => {}
        _ => return Err((-32603, "task resource unavailable", None)),
    }
    if !response
        .mime
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err((-32603, "invalid task resource response", None));
    }
    let body: Value = serde_json::from_slice(&response.body)
        .map_err(|_| (-32603, "invalid task resource response", None))?;
    if body["id"] != task || !body.is_object() {
        return Err((-32603, "invalid task resource response", None));
    }
    let terminal = body["status"]
        .as_str()
        .filter(|state| ["completed", "failed", "cancelled"].contains(state))
        .map(|_| task);
    Ok((
        json!({"contents":[{"uri":uri,"mimeType":"application/json","text":body.to_string()}]}),
        terminal,
    ))
}

fn visor_exchange(
    visor: &VisorEndpoint,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<VisorResponse, String> {
    match visor {
        #[cfg(unix)]
        VisorEndpoint::Unix(socket) => unix_exchange(socket, method, path, body),
        VisorEndpoint::Http { base, token } => http_exchange(
            &format!("{}{path}", base.trim_end_matches('/')),
            method,
            body,
            token.as_deref(),
        ),
    }
}

fn finish_task_call(
    owned: &mut std::collections::VecDeque<String>,
    name: Option<&str>,
    result: &CallResult,
    visor: &VisorEndpoint,
    session: &str,
) {
    let submitted = track_task_result(owned, name, result);
    if request_cancelled() {
        if let Some(task) = submitted {
            // Recover the POST identity before requesting targeted cleanup.
            let _cleanup = CancellationScope::enter(None);
            if let Ok(result) = handle_call(
                visor,
                session,
                Some(&json!({"name":"task_cancel","arguments":{"task_id":task}})),
            ) {
                track_task_result(owned, Some("task_cancel"), &result);
            }
        }
    }
}

fn track_task_result(
    owned: &mut std::collections::VecDeque<String>,
    name: Option<&str>,
    result: &CallResult,
) -> Option<String> {
    if let Some(id) = &result.missing_task {
        owned.retain(|known| known != id);
    }
    if !matches!(name, Some("task_submit" | "task_status" | "task_cancel"))
        || result.content["isError"] != false
    {
        return None;
    }
    let text = result.content.pointer("/content/0/text")?.as_str()?;
    let task: Value = serde_json::from_str(text).ok()?;
    let id = uuid::Uuid::parse_str(task.get("id")?.as_str()?)
        .ok()?
        .to_string();
    let terminal = task
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| ["completed", "failed", "cancelled"].contains(&status));
    if terminal {
        owned.retain(|known| known != &id);
    } else if name == Some("task_submit") {
        if !owned.contains(&id) {
            owned.push_back(id.clone());
        }
        return Some(id);
    }
    None
}

fn confirmed_missing_task(response: &VisorResponse) -> bool {
    response.status == 404
        && response
            .mime
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        && serde_json::from_slice::<Value>(&response.body)
            .ok()
            .is_some_and(|body| body["error"] == "session closed or unknown")
}

fn handle_call(
    visor: &VisorEndpoint,
    session: &str,
    params: Option<&Value>,
) -> Result<CallResult, String> {
    let _attribution = crate::telemetry::UpstreamScope::enter(
        crate::telemetry::Service::Visor,
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
    if request_cancelled() {
        return Err("request cancelled".into());
    }
    // A submitted task needs its returned ID for targeted cleanup. Never abandon
    // a non-idempotent submission merely because its MCP request was cancelled.
    let _submission = (tool == "task_submit").then(|| CancellationScope::enter(None));
    let response = visor_exchange(visor, method, &path, body.as_deref());
    let mut missing_task = None;
    let content = match response {
        Ok(response) => {
            // The daemon evicts only terminal records. A confirmed absent record
            // (or closed session) cannot contain active work owned by this client.
            // Do not reclaim on proxy 404s, denial, or uncertain transport errors.
            if matches!(tool, "task_status" | "task_cancel") && confirmed_missing_task(&response) {
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
                crate::input::key_chord_request(args.get("keys").ok_or("keys are required")?)?;
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

fn with_transport<F>(future: F) -> Result<VisorResponse, String>
where
    F: std::future::Future<Output = Result<VisorResponse, String>>,
{
    let cancel = REQUEST_CANCEL.with(|current| current.borrow().clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "visor runtime failed")?;
    runtime.block_on(async {
        tokio::select! {
            biased;
            _ = async {
                if let Some(cancel) = cancel {
                    loop {
                        if cancel.load(Ordering::SeqCst) { break; }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                } else { std::future::pending::<()>().await; }
            } => Err("request cancelled".into()),
            result = tokio::time::timeout(Duration::from_secs(30), future) => result.map_err(|_| "visor request timed out".to_string())?,
        }
    })
}

async fn exchange_stream<S>(
    stream: S,
    method: &str,
    path: &str,
    host: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Result<VisorResponse, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| "visor HTTP handshake failed")?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut request = hyper::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
        .body(Full::new(Bytes::from(body.unwrap_or("").to_owned())))
        .map_err(|_| "invalid visor request")?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| "visor HTTP transport failed")?;
    let status = response.status().as_u16();
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|_| "visor response is incomplete or exceeds 32 MiB")?
        .to_bytes()
        .to_vec();
    Ok(VisorResponse { status, mime, body })
}

fn http_exchange(
    url: &str,
    method: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> Result<VisorResponse, String> {
    let url = url::Url::parse(url).map_err(|_| "invalid visor URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("visor URL requires HTTP(S) without embedded credentials".into());
    }
    let host = url.host_str().ok_or("visor URL requires a host")?;
    let port = url
        .port_or_known_default()
        .ok_or("visor URL requires a port")?;
    let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    with_transport(async {
        let tcp = tokio::net::TcpStream::connect((host.trim_matches(['[', ']']), port))
            .await
            .map_err(|_| "visor connection failed")?;
        if url.scheme() == "https" {
            use tokio_rustls::rustls;
            let roots =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name =
                rustls::pki_types::ServerName::try_from(host.trim_matches(['[', ']']).to_owned())
                    .map_err(|_| "invalid TLS server name")?;
            let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .map_err(|_| "visor TLS handshake failed")?;
            exchange_stream(stream, method, path, authority, body, token).await
        } else {
            exchange_stream(tcp, method, path, authority, body, token).await
        }
    })
}

#[cfg(unix)]
fn unix_exchange(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<VisorResponse, String> {
    with_transport(async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|_| "visor socket connection failed")?;
        exchange_stream(stream, method, path, "localhost", body, None).await
    })
}

#[cfg(test)]
mod capability_contract_tests {
    use super::*;

    #[test]
    fn native_http_selection_requires_explicit_endpoint_and_windows_token() {
        assert!(configured_http(None, Some("synthetic-token".into()), true).unwrap().is_none());
        assert!(configured_http(Some("  "), None, true).unwrap().is_none());
        for token in [None, Some("".into()), Some("  ".into())] {
            assert!(configured_http(Some("https://visor.example"), token, true).is_err());
        }
        match configured_http(Some("https://visor.example"), Some("synthetic-token".into()), true).unwrap().unwrap() {
            VisorEndpoint::Http { base, token } => {
                assert_eq!(base, "https://visor.example");
                assert_eq!(token.as_deref(), Some("synthetic-token"));
            }
            #[cfg(unix)]
            _ => panic!("unexpected local endpoint"),
        }
        assert!(configured_http(Some("http://127.0.0.1:9000"), None, false).unwrap().is_some());
    }

    #[test]
    fn reader_cancellation_is_scoped_and_malformed_notifications_are_ignored() {
        let messages = [
            json!({"jsonrpc":"2.0","id":"init","method":"initialize"}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call"}),
            json!({"jsonrpc":"2.0","id":2,"method":"resources/read"}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"init"}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":false}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2},"result":{}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2},"error":{"code":-32600,"message":"invalid"}}),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"unknown"}}),
        ];
        let input = messages
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>();
        let (tx, rx) = mpsc::sync_channel(MAX_PENDING_REQUESTS);
        let stopped = Arc::new(AtomicBool::new(false));
        read_requests(std::io::Cursor::new(input), tx, stopped.clone());
        let requests: Vec<_> = rx.into_iter().collect();
        assert_eq!(requests.len(), 3);
        assert!(!requests[0].cancel.load(Ordering::SeqCst));
        assert!(requests[1].cancel.load(Ordering::SeqCst));
        assert!(!requests[2].cancel.load(Ordering::SeqCst));
        assert!(!stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn input_overflow_is_bounded_and_cancels_admitted_requests() {
        let input = (0..3)
            .map(|id| {
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":"tools/call"})
                )
            })
            .collect::<String>();
        let (tx, rx) = mpsc::sync_channel(2);
        let stopped = Arc::new(AtomicBool::new(false));
        read_requests(std::io::Cursor::new(input), tx, stopped.clone());
        let requests: Vec<_> = rx.into_iter().collect();
        assert_eq!(requests.len(), 2);
        assert!(stopped.load(Ordering::SeqCst));
        assert!(requests
            .iter()
            .all(|request| request.cancel.load(Ordering::SeqCst)));
    }

    #[test]
    fn cancelled_submission_recovers_its_id_and_cancels_only_that_task() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = VisorEndpoint::Http {
            base: format!("http://{}", listener.local_addr().unwrap()),
            token: Some("test-bearer".into()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let server_cancel = cancel.clone();
        let task = "00000000-0000-0000-0000-000000000002";
        let session = "00000000-0000-0000-0000-000000000001";
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for (method, suffix, status) in [
                ("POST", "tasks".to_string(), "queued"),
                ("DELETE", format!("tasks/{task}"), "cancelled"),
            ] {
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "missing task request");
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                assert!(
                    header.starts_with(&format!("{method} /session/{session}/{suffix} HTTP/1.1"))
                );
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                if method == "POST" {
                    server_cancel.store(true, Ordering::SeqCst);
                }
                let body = json!({"id":task,"status":status}).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
                let mut rest = Vec::new();
                stream.read_to_end(&mut rest).unwrap();
            }
        });
        let _request = CancellationScope::enter(Some(cancel));
        let result = handle_call(&endpoint, session, Some(&json!({"name":"task_submit","arguments":{"steps":[{"tool":"view","arguments":{}}]}}))).unwrap_or_else(|_| panic!("submission lost its task identity"));
        let mut owned = std::collections::VecDeque::new();
        finish_task_call(&mut owned, Some("task_submit"), &result, &endpoint, session);
        assert!(owned.is_empty());
        let mut response = Vec::new();
        send(&mut response, result.content);
        assert!(response.is_empty());
        server.join().unwrap();
    }

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
