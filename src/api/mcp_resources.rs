//! Explicit, session-owned MCP resources; returned text does not grant authority.
use super::*;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceRequest {
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
    Templates,
    Read,
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
    Json(body): Json<ResourceRequest>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&id)?;
    if body.server.is_empty() || body.server.len() > 512 {
        return Err(ApiError::bad("Invalid MCP server name"));
    }
    let method = match body.action {
        Action::List => "resources/list",
        Action::Templates => "resources/templates/list",
        Action::Read => "resources/read",
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
        let result = attached.resource_request(method, params);
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
    .map_err(|_| ApiError::internal("MCP resource task failed"))??;
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
    async fn resource_only_server_requires_approval_and_keeps_content_out_of_audit() {
        let dir = std::env::temp_dir().join(format!("visor-resources-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let script = dir.join("server.py");
        std::fs::write(&script, r#"
import json, pathlib, sys, time
root=pathlib.Path(__file__).parent
for line in sys.stdin:
    message=json.loads(line); method=message.get('method'); params=message.get('params',{})
    if method=='initialize':
        result={'protocolVersion':'2024-11-05','capabilities':{'resources':{}},'serverInfo':{'name':'resource-only','version':'1'}}
    elif method=='resources/list':
        result={'resources':[{'uri':'private://entry','name':'entry'}], 'nextCursor':'next'}
    elif method=='resources/templates/list':
        result={'resourceTemplates':[{'uriTemplate':'private://{id}','name':'entries'}]}
    elif method=='resources/read':
        if params['uri']=='private://slow':
            (root/'entered').write_text('ready')
            deadline=time.monotonic()+5
            while not (root/'finish').exists() and time.monotonic()<deadline: time.sleep(.01)
        result={'contents':[{'uri':params['uri'],'text':'private content'}]}
    else: continue
    if method.startswith('resources/'):
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
        let (code, session) = send(&state, "POST", "/session", json!({"purpose":"resource contract","recipes":[],"agent":{"name":"reader","instructions":"read"},"skills":[]})).await;
        assert_eq!(code, 201, "{session}");
        let path = format!("/session/{}/mcp/resources", session["id"].as_str().unwrap());
        let (code, result) = send(&state, "POST", &path, json!({"server":"fixture","action":"read","decision":"deny","params":{"uri":"private://entry"}})).await;
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
        for action in ["list", "templates", "read"] {
            let params = if action == "read" {
                json!({"uri":"private://entry"})
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
            if action == "read" {
                assert_eq!(result["result"]["contents"][0]["text"], "private content");
            }
        }
        let (code, _) = send(
            &state,
            "POST",
            &format!("/session/{}/mcp/resources", Uuid::new_v4()),
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
        assert_eq!(code, 502, "unadvertised resources must not dispatch");
        let (code, audit) = send(&state, "GET", "/mcp-audit", Value::Null).await;
        assert_eq!(code, 200);
        assert_eq!(audit.as_array().unwrap().len(), 5);
        assert!(!audit.to_string().contains("private://"));
        assert!(!audit.to_string().contains("private content"));
        assert_eq!(
            std::fs::read_to_string(dir.join("calls")).unwrap(),
            "resources/list\nresources/templates/list\nresources/read\n"
        );
        {
            let id = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
            let mut sessions = lock(&state.sessions);
            sessions.get_mut(&id).unwrap().mcp_mut().servers[0]
                .negotiated
                .as_mut()
                .unwrap()
                .server_capabilities = json!({"resources":{}});
        }
        let pending_state = Arc::clone(&state);
        let pending_path = path.clone();
        let pending = tokio::spawn(async move {
            send(&pending_state, "POST", &pending_path, json!({"server":"fixture","action":"read","decision":"approve","params":{"uri":"private://slow"}})).await
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
        assert_eq!(audit.as_array().unwrap().len(), 6);
        assert_eq!(audit[5]["outcome"], "ok");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
