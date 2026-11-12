//! Explicit, session-owned MCP prompts; returned text does not grant authority.
use super::*;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PromptRequest {
    server: String,
    action: Action,
    decision: Decision,
    #[serde(default)]
    params: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    List,
    Get,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Decision {
    Approve,
    Deny,
}

pub(super) async fn request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PromptRequest>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&id)?;
    if body.server.is_empty() || body.server.len() > 512 {
        return Err(ApiError::bad("Invalid MCP server name"));
    }
    let method = match body.action {
        Action::List => "prompts/list",
        Action::Get => "prompts/get",
    };
    let params = if body.params.is_null() {
        json!({})
    } else {
        body.params
    };
    let outcome = tokio::task::spawn_blocking(move || {
        let mut attached = {
            let mut sessions = lock(&state.sessions);
            let session = sessions.get_mut(&id).ok_or_else(ApiError::missing)?;
            let server = session
                .mcp()
                .servers
                .iter()
                .find(|s| s.name == body.server)
                .ok_or_else(|| ApiError::bad("MCP server not attached"))?;
            if matches!(body.decision, Decision::Deny) {
                drop(sessions);
                lock(&state.mcp_audit).record(McpAuditRow {
                    server: body.server,
                    tool: format!("@{method}"),
                    decision: "deny".into(),
                    outcome: "denied".into(),
                });
                return Ok(json!({"outcome":"denied","result":null}));
            }
            if server.refreshing {
                return Err(ApiError::conflict("MCP server operation in progress"));
            }
            session
                .mcp_mut()
                .take_for_refresh(&body.server)
                .map_err(ApiError::from_mcp)?
        };
        let result = attached.prompt_request(method, params);
        lock(&state.mcp_audit).record(McpAuditRow {
            server: body.server,
            tool: format!("@{method}"),
            decision: "approve".into(),
            outcome: if result.is_ok() { "ok" } else { "failed" }.into(),
        });
        {
            let mut sessions = lock(&state.sessions);
            let session = sessions.get_mut(&id).ok_or_else(ApiError::missing)?;
            session.mcp_mut().restore_after_refresh(attached);
        }
        Ok(json!({"outcome":"ok","result":result.map_err(ApiError::from_mcp)?}))
    })
    .await
    .map_err(|_| ApiError::internal("MCP prompt task failed"))??;
    Ok(Json(outcome))
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    async fn send(state: &Arc<AppState>, method: &str, path: &str, body: Value) -> (u16, Value) {
        let response = router(Arc::clone(state))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let code = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn prompt_only_server_requires_approval_and_keeps_content_out_of_audit() {
        let dir = std::env::temp_dir().join(format!("visor-prompts-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let script = dir.join("server.py");
        std::fs::write(&script, r#"
import json, pathlib, sys, time
root=pathlib.Path(__file__).parent
for line in sys.stdin:
    message=json.loads(line); method=message.get('method'); params=message.get('params',{})
    if method=='initialize':
        result={'protocolVersion':'2024-11-05','capabilities':{'prompts':{}},'serverInfo':{'name':'prompt-only','version':'1'}}
    elif method=='prompts/list':
        result={'prompts':[{'name':'entry','arguments':[{'name':'code','required':True}]}], 'nextCursor':'next'}
    elif method=='prompts/get':
        if params['name']=='private://entry': assert params.get('arguments')=={'code':'private input'}
        if params['name']=='private://slow':
            (root/'entered').write_text('ready')
            deadline=time.monotonic()+5
            while not (root/'finish').exists() and time.monotonic()<deadline: time.sleep(.01)
        result={'messages':[{'role':'user','content':{'type':'text','text':'private content'}}]}
    else: continue
    if method.startswith('prompts/'):
        with (root/'calls').open('a') as out: out.write(method+'\n')
    print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::write(
            dir.join("mcp.json"),
            json!({"mcpServers":{"fixture":{"command":"python3","args":[script],"type":"stdio"}}})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("mcp-profiles.json"),
            r#"{"active":"default","profiles":{"default":{"servers":["fixture"]}}}"#,
        )
        .unwrap();
        let mut state = AppState::new(
            Arc::new(crate::desktop::HeadlessDesktop),
            Duration::from_millis(250),
        );
        state.mcp_dir = dir.clone();
        let state = Arc::new(state);
        let (code, session) = send(&state, "POST", "/session", json!({"purpose":"prompt contract","recipes":[],"agent":{"name":"reader","instructions":"get"},"skills":[]})).await;
        assert_eq!(code, 201, "{session}");
        let path = format!("/session/{}/mcp/prompts", session["id"].as_str().unwrap());
        let (code, result) = send(&state, "POST", &path, json!({"server":"fixture","action":"get","decision":"deny","params":{"name":"private://entry"}})).await;
        assert_eq!(code, 200);
        assert_eq!(result["outcome"], "denied");
        assert!(!dir.join("calls").exists());
        let (code, _) = send(
            &state,
            "POST",
            &path,
            json!({"server":"fixture","action":"list"}),
        )
        .await;
        assert_eq!(code, 422);
        for action in ["list", "get"] {
            let params = if action == "get" {
                json!({"name":"private://entry","arguments":{"code":"private input"}})
            } else {
                json!({})
            };
            let (code, result) = send(
                &state,
                "POST",
                &path,
                json!({"server":"fixture","action":action,"decision":"approve","params":params}),
            )
            .await;
            assert_eq!(code, 200, "{result}");
            assert_eq!(result["outcome"], "ok");
            if action == "get" {
                assert_eq!(
                    result["result"]["messages"][0]["content"]["text"],
                    "private content"
                );
            }
        }
        let (code, _) = send(
            &state,
            "POST",
            &format!("/session/{}/mcp/prompts", Uuid::new_v4()),
            json!({"server":"fixture","action":"list","decision":"approve"}),
        )
        .await;
        assert_eq!(code, 404);
        {
            let id = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
            let mut sessions = lock(&state.sessions);
            sessions.get_mut(&id).unwrap().mcp_mut().servers[0]
                .negotiated
                .as_mut()
                .unwrap()
                .server_capabilities = json!({});
        }
        let (code, _) = send(
            &state,
            "POST",
            &path,
            json!({"server":"fixture","action":"list","decision":"approve"}),
        )
        .await;
        assert_eq!(code, 502, "unadvertised prompts must not dispatch");
        let (code, audit) = send(&state, "GET", "/mcp-audit", Value::Null).await;
        assert_eq!(code, 200);
        assert_eq!(audit.as_array().unwrap().len(), 4);
        assert!(!audit.to_string().contains("private://"));
        assert!(!audit.to_string().contains("private content"));
        assert!(!audit.to_string().contains("private input"));
        let (_, inference_audit) = send(&state, "GET", "/audit", Value::Null).await;
        assert_eq!(inference_audit, json!([]), "retrieval never dispatches inference");
        assert_eq!(
            std::fs::read_to_string(dir.join("calls")).unwrap(),
            "prompts/list\nprompts/get\n"
        );
        {
            let id = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
            let mut sessions = lock(&state.sessions);
            sessions.get_mut(&id).unwrap().mcp_mut().servers[0]
                .negotiated
                .as_mut()
                .unwrap()
                .server_capabilities = json!({"prompts":{}});
        }
        let pending_state = Arc::clone(&state);
        let pending_path = path.clone();
        let pending = tokio::spawn(async move {
            send(&pending_state, "POST", &pending_path, json!({"server":"fixture","action":"get","decision":"approve","params":{"name":"private://slow"}})).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dir.join("entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let (code, _) = send(
            &state,
            "DELETE",
            &format!("/session/{}", session["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(code, 204);
        std::fs::write(dir.join("finish"), "").unwrap();
        let (code, result) = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code, 404);
        assert!(!result.to_string().contains("private content"));
        let (_, audit) = send(&state, "GET", "/mcp-audit", Value::Null).await;
        assert_eq!(audit.as_array().unwrap().len(), 5);
        assert_eq!(audit[4]["outcome"], "ok");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
