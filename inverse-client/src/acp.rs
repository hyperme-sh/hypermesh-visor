//! ACP v1 admission boundary. Client metadata never grants local authority.
use serde_json::{json, Value};

pub const MAX_FRAME: usize = 512 * 1024;
pub const MAX_PROMPT: usize = 64 * 1024;

pub mod transport;

#[derive(Debug, PartialEq)]
pub enum Action {
    Reply(Value),
    Open { id: Value },
    Prompt { id: Value, text: String },
    Cancel,
    Ignore,
}

pub struct Protocol {
    initialized: bool,
    session: Option<String>,
    opening: Option<Value>,
    active: Option<Value>,
    cancelled: bool,
}

fn error(id: Value, code: i32, message: &str) -> Action {
    Action::Reply(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}))
}

fn reply(id: Value, result: Value) -> Action {
    Action::Reply(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn fields(value: &Value, names: &[&str]) -> bool {
    value.as_object().is_some_and(|o| {
        o.keys()
            .all(|k| names.contains(&k.as_str()) || k == "_meta")
    })
}

fn prompt_text(value: &Value) -> Option<String> {
    let blocks = value
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 64)?;
    let mut text = String::new();
    for block in blocks {
        let part = match block["type"].as_str()? {
            "text" => {
                if !fields(block, &["type", "text", "annotations"]) {
                    return None;
                }
                block["text"].as_str()?.to_owned()
            }
            "resource_link" => {
                // References are untrusted prompt text, never read or fetched here.
                if !fields(
                    block,
                    &[
                        "type",
                        "uri",
                        "name",
                        "description",
                        "mimeType",
                        "size",
                        "title",
                        "annotations",
                    ],
                ) {
                    return None;
                }
                let uri = block["uri"].as_str()?;
                let name = block["name"].as_str()?;
                if uri.is_empty()
                    || name.is_empty()
                    || uri.len() > 8192
                    || name.len() > 1024
                    || uri.chars().any(char::is_control)
                    || name.chars().any(char::is_control)
                {
                    return None;
                }
                format!(
                    "Resource reference (not loaded): {}",
                    json!({"name":name,"uri":uri})
                )
            }
            _ => return None,
        };
        if !text.is_empty() {
            text.push('\n');
        }
        if text.len().checked_add(part.len())? > MAX_PROMPT {
            return None;
        }
        text.push_str(&part);
    }
    (!text.trim().is_empty() && !text.contains('\0')).then_some(text)
}

impl Default for Protocol {
    fn default() -> Self {
        Self {
            initialized: false,
            session: None,
            opening: None,
            active: None,
            cancelled: false,
        }
    }
}

impl Protocol {
    pub fn receive(&mut self, frame: &str) -> Action {
        if frame.len() > MAX_FRAME {
            return error(Value::Null, -32600, "Request too large");
        }
        let Ok(request) = serde_json::from_str::<Value>(frame) else {
            return error(Value::Null, -32700, "Parse error");
        };
        let id = request.get("id").cloned();
        if request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || !fields(&request, &["jsonrpc", "id", "method", "params"])
            || id.as_ref().is_some_and(|v| {
                !(v.is_i64() || v.is_u64() || v.as_str().is_some_and(|s| s.len() <= 128))
            })
        {
            return error(Value::Null, -32600, "Invalid request");
        }
        let method = request["method"].as_str().unwrap();
        let params = request.get("params").cloned().unwrap_or(json!({}));
        if id.is_none() {
            if method == "session/cancel"
                && fields(&params, &["sessionId"])
                && self
                    .session
                    .as_deref()
                    .is_some_and(|s| params["sessionId"] == s)
                && self.active.is_some()
            {
                self.cancelled = true;
                return Action::Cancel;
            }
            if method == "$/cancel_request"
                && fields(&params, &["requestId"])
                && self
                    .active
                    .as_ref()
                    .or(self.opening.as_ref())
                    .is_some_and(|id| params.get("requestId") == Some(id))
            {
                self.cancelled = true;
                return Action::Cancel;
            }
            return Action::Ignore;
        }
        let id = id.unwrap();
        // Never emit a second response for the active ID or accept a duplicate turn.
        if self.active.as_ref() == Some(&id) || self.opening.as_ref() == Some(&id) {
            return Action::Ignore;
        }
        if !params.is_object() {
            return error(id, -32602, "Invalid params");
        }
        if method == "initialize" {
            if self.initialized {
                return error(id, -32600, "Already initialized");
            }
            if !fields(
                &params,
                &["protocolVersion", "clientCapabilities", "clientInfo"],
            ) || params["protocolVersion"].as_u64().is_none()
                || params
                    .get("clientCapabilities")
                    .is_some_and(|v| !v.is_object())
                || params.get("clientInfo").is_some_and(|v| !v.is_object())
            {
                return error(id, -32602, "Invalid initialization");
            }
            self.initialized = true;
            return reply(
                id,
                json!({"protocolVersion":1,"agentCapabilities":{"loadSession":false,"promptCapabilities":{},"mcpCapabilities":{}},"agentInfo":{"name":"hypermesh-visor","version":env!("CARGO_PKG_VERSION")},"authMethods":[]}),
            );
        }
        if !self.initialized {
            return error(id, -32000, "Initialize first");
        }
        match method {
            "session/new" => {
                if self.session.is_some() || self.opening.is_some() {
                    return error(id, -32000, "One assigned session per connection");
                }
                let cwd = params["cwd"].as_str().unwrap_or("");
                if !fields(&params, &["cwd", "mcpServers"])
                    || cwd.is_empty()
                    || cwd.len() > 4096
                    || cwd.chars().any(char::is_control)
                    || !std::path::Path::new(cwd).is_absolute()
                    || params["mcpServers"]
                        .as_array()
                        .is_none_or(|v| !v.is_empty())
                {
                    return error(id, -32602, "Absolute cwd and empty mcpServers required");
                }
                // The caller must validate the assigned machine/session before open succeeds.
                self.opening = Some(id.clone());
                self.cancelled = false;
                Action::Open { id }
            }
            "session/prompt" => {
                if !fields(&params, &["sessionId", "prompt"])
                    || !self
                        .session
                        .as_deref()
                        .is_some_and(|s| params["sessionId"] == s)
                {
                    return error(id, -32602, "Unknown session");
                }
                if self.active.is_some() {
                    return error(id, -32000, "Prompt already active");
                }
                let Some(text) = prompt_text(&params["prompt"]) else {
                    return error(id, -32602, "Unsupported or oversized prompt content");
                };
                self.active = Some(id.clone());
                self.cancelled = false;
                Action::Prompt { id, text }
            }
            _ => error(id, -32601, "Method not found"),
        }
    }

    pub fn opened(&mut self, id: Value, session: uuid::Uuid) -> Action {
        if self.opening.as_ref() != Some(&id) {
            return Action::Ignore;
        }
        self.opening = None;
        if self.cancelled {
            return error(id, -32800, "Request cancelled");
        }
        let session = session.to_string();
        self.session = Some(session.clone());
        reply(id, json!({"sessionId":session}))
    }

    pub fn open_failed(&mut self, id: Value) -> Action {
        if self.opening.as_ref() != Some(&id) {
            return Action::Ignore;
        }
        self.opening = None;
        if self.cancelled {
            error(id, -32800, "Request cancelled")
        } else {
            error(
                id,
                -32000,
                "Assigned machine session unavailable or refused",
            )
        }
    }

    pub fn finish(&mut self, result: Result<&str, ()>, cancelled: bool) -> Vec<Value> {
        let Some(id) = self.active.take() else {
            return Vec::new();
        };
        if cancelled || self.cancelled {
            return vec![json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"cancelled"}})];
        }
        match result {
            Ok(text) if text.len() <= MAX_PROMPT => vec![
                json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":self.session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}),
                json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}),
            ],
            _ => vec![
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Assigned prompt route unavailable or refused"}}),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(protocol: &mut Protocol, id: i32, method: &str, params: Value) -> Action {
        protocol
            .receive(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string())
    }
    fn session() -> (Protocol, String) {
        let mut p = Protocol::default();
        request(&mut p, 1, "initialize", json!({"protocolVersion":1}));
        request(
            &mut p,
            2,
            "session/new",
            json!({"cwd":std::env::temp_dir(),"mcpServers":[]}),
        );
        let id = uuid::Uuid::new_v4();
        p.opened(json!(2), id);
        (p, id.to_string())
    }
    #[test]
    fn requires_setup_and_never_imports_client_commands() {
        let mut p = Protocol::default();
        assert!(
            matches!(request(&mut p, 1, "session/new", json!({})), Action::Reply(v) if v["error"]["code"] == -32000)
        );
        assert!(
            matches!(request(&mut p, 2, "initialize", json!({"protocolVersion":999})), Action::Reply(v) if v["result"]["protocolVersion"] == 1)
        );
        for params in [
            json!({"cwd":"relative","mcpServers":[]}),
            json!({"cwd":std::env::temp_dir(),"mcpServers":[{"command":"private-command"}]}),
            json!({"cwd":std::env::temp_dir(),"mcpServers":[],"token":"private-secret"}),
        ] {
            let Action::Reply(value) = request(&mut p, 3, "session/new", params) else {
                panic!()
            };
            assert!(value.get("error").is_some());
            assert!(!value.to_string().contains("private"));
        }
        assert!(matches!(
            request(
                &mut p,
                4,
                "session/new",
                json!({"cwd":std::env::temp_dir(),"mcpServers":[]})
            ),
            Action::Open { .. }
        ));
    }
    #[test]
    fn session_binding_single_turn_and_cancel_output() {
        let (mut p, s) = session();
        let prompt = json!([{"type":"text","text":"hello"}]);
        assert!(matches!(
            request(
                &mut p,
                3,
                "session/prompt",
                json!({"sessionId":"other","prompt":prompt})
            ),
            Action::Reply(_)
        ));
        assert!(matches!(
            request(
                &mut p,
                3,
                "session/prompt",
                json!({"sessionId":s,"prompt":prompt})
            ),
            Action::Prompt { .. }
        ));
        assert_eq!(
            request(
                &mut p,
                3,
                "session/prompt",
                json!({"sessionId":s,"prompt":prompt})
            ),
            Action::Ignore
        );
        assert!(matches!(
            request(
                &mut p,
                4,
                "session/prompt",
                json!({"sessionId":s,"prompt":prompt})
            ),
            Action::Reply(_)
        ));
        assert_eq!(
            p.receive(
                &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"other"}})
                    .to_string()
            ),
            Action::Ignore
        );
        assert_eq!(
            p.receive(
                &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":s}})
                    .to_string()
            ),
            Action::Cancel
        );
        let finished = p.finish(Ok("late private output"), false);
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0]["result"]["stopReason"], "cancelled");
        assert!(!finished[0].to_string().contains("private"));
        assert!(p.finish(Ok("duplicate"), false).is_empty());
    }
    #[test]
    fn admission_completion_and_cancellation_are_bound_to_the_request() {
        let mut p = Protocol::default();
        assert_eq!(p.opened(json!(1), uuid::Uuid::nil()), Action::Ignore);
        request(&mut p, 1, "initialize", json!({"protocolVersion":1}));
        let setup = json!({"cwd":std::env::temp_dir(),"mcpServers":[]});
        assert!(matches!(
            request(&mut p, 2, "session/new", setup.clone()),
            Action::Open { .. }
        ));
        assert_eq!(p.opened(json!(99), uuid::Uuid::nil()), Action::Ignore);
        assert!(
            matches!(request(&mut p, 3, "session/new", setup.clone()), Action::Reply(v) if v.get("error").is_some())
        );
        assert_eq!(
            p.receive(
                &json!({"jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":2}})
                    .to_string()
            ),
            Action::Cancel
        );
        assert!(
            matches!(p.opened(json!(2), uuid::Uuid::nil()), Action::Reply(v) if v["error"]["code"] == -32800)
        );
        assert!(matches!(
            request(&mut p, 4, "session/new", setup),
            Action::Open { .. }
        ));
        assert!(
            matches!(p.open_failed(json!(4)), Action::Reply(v) if v["error"]["code"] == -32000)
        );
        assert!(p.session.is_none());
    }

    #[test]
    fn malformed_envelopes_never_admit_work() {
        for input in [
            "{private-invalid-json",
            "[]",
            r#"{"jsonrpc":"2.0","id":{},"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","result":{}}"#,
        ] {
            let Action::Reply(value) = Protocol::default().receive(input) else {
                panic!()
            };
            assert!(value.get("error").is_some());
            assert!(!value.to_string().contains("private"));
        }
    }

    #[test]
    fn prompt_content_bounds_and_untrusted_links() {
        assert!(prompt_text(&json!([{"type":"image","data":"anything"}])).is_none());
        assert!(prompt_text(&json!([{"type":"text","text":"x".repeat(MAX_PROMPT+1)}])).is_none());
        assert!(prompt_text(&json!([{"type":"text","text":"\u{0}"}])).is_none());
        let text = prompt_text(
            &json!([{"type":"resource_link","name":"input","uri":"file:///private/example"}]),
        )
        .unwrap();
        assert!(text.contains("not loaded"));
        let (mut p, s) = session();
        request(
            &mut p,
            3,
            "session/prompt",
            json!({"sessionId":s,"prompt":[{"type":"text","text":"hello"}]}),
        );
        let output = p.finish(Ok("answer"), false);
        assert_eq!(output[0]["params"]["sessionId"], s);
        assert_eq!(output[1]["id"], 3);
    }
}
