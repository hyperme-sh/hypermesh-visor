//! Scoped local controls for Plane-owned Continue tasks.
use super::*;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    session_id: Uuid,
    workspace: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chunk {
    prompt: String,
    completion: String,
    depends_on: Vec<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    session_id: Uuid,
    workspace: String,
    run_id: Uuid,
    goal: String,
    completion: String,
    chunks: Vec<Chunk>,
    seconds: u64,
    tokens: u64,
    continue_enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    session_id: Uuid,
    workspace: String,
    run_id: Uuid,
    revision: u64,
    action: String,
}
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
fn unavailable() -> ApiError {
    ApiError::unavailable("Continue task unavailable; refresh owner status before retrying")
}
fn stable_id(run: Uuid, index: usize, kind: u8) -> Uuid {
    let mut h = Sha256::new();
    h.update(run.as_bytes());
    h.update((index as u64).to_be_bytes());
    h.update([kind]);
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 0x40;
    bytes[8] = (bytes[8] & 63) | 0x80;
    Uuid::from_bytes(bytes)
}
fn start_body(context: &Value, body: &Start) -> Result<Value, ()> {
    if body.run_id.is_nil()
        || body.goal.is_empty()
        || body.goal.len() > 4096
        || body.completion.is_empty()
        || body.completion.len() > 4096
        || body.chunks.is_empty()
        || body.chunks.len() > 16
        || !(1..=3600).contains(&body.seconds)
        || !(1..=1048576).contains(&body.tokens)
        || acp_machine::binding(context, &body.workspace)? != body.session_id
    {
        return Err(());
    }
    let identity = &context["identity"];
    let mut chunks = Vec::new();
    for (i, chunk) in body.chunks.iter().enumerate() {
        if chunk.prompt.is_empty()
            || chunk.prompt.len() > 32000
            || chunk.completion.is_empty()
            || chunk.completion.len() > 2048
            || chunk.depends_on.len() > 16
            || chunk.depends_on.iter().any(|d| *d >= i)
        {
            return Err(());
        }
        let id = stable_id(body.run_id, i, 0);
        let request = json!({"version":1,"tenant_id":identity["tenant_id"],"device_id":identity["device_id"],"bot_id":identity["bot_id"],"assignment_id":identity["assignment_id"],"session_id":identity["session_id"],"task_id":id,"operation_id":stable_id(body.run_id,i,1),"class":context["class"],"destination":context["destination"],"max_output_tokens":context["max_output_tokens"],"prompt":chunk.prompt});
        let raw = serde_json::to_vec(&request).map_err(|_| ())?;
        if raw.len() > 48000 {
            return Err(());
        }
        chunks.push(json!({"id":id,"depends_on":chunk.depends_on.iter().map(|d|stable_id(body.run_id,*d,0)).collect::<Vec<_>>(),"completion":chunk.completion,"submission_base64":base64::engine::general_purpose::STANDARD.encode(raw)}));
    }
    let mut result = scope_body(context);
    result["run_id"] = json!(body.run_id);
    result["continue_enabled"] = json!(body.continue_enabled);
    result["plan"] = json!({"version":1,"workspace":body.workspace,"goal":body.goal,"completion":body.completion,"chunks":chunks,"budgets":{"seconds":body.seconds,"tokens":body.tokens,"chunks":body.chunks.len()}});
    if result.to_string().len() > 524288 {
        return Err(());
    }
    Ok(result)
}
fn scope_body(context: &Value) -> Value {
    let id = &context["identity"];
    json!({"version":1,"request_id":Uuid::new_v4(),"device_id":id["device_id"],"assignment_id":id["assignment_id"],"session_id":id["session_id"],"workspace":context["workspace"]})
}
async fn context(state: &AppState, link: CompanionLink, scope: &Scope) -> Result<Value, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    let value = state
        .host_bridge
        .as_ref()
        .ok_or_else(unavailable)?
        .machine_context()
        .await
        .map_err(|_| unavailable())?;
    if acp_machine::binding(&value, &scope.workspace).map_err(|_| unavailable())?
        != scope.session_id
    {
        return Err(unavailable());
    }
    Ok(value)
}
fn owner_call(context: Value, path: &str, body: Value) -> Result<Value, ()> {
    let origin = context["plane"].as_str().ok_or(())?;
    let url = url::Url::parse(origin).map_err(|_| ())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    let tokens = hypermesh_session::refresh_session(
        &hypermesh_session::KeyringStore,
        &hypermesh_session::Endpoints::panopticon(),
    )
    .map_err(|_| ())?;
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .build()
        .post(&format!(
            "{}/api/v1/hypermesh/bots/{}/{}",
            origin.trim_end_matches('/'),
            context["identity"]["bot_id"].as_str().ok_or(())?,
            path
        ))
        .set("Authorization", &format!("Bearer {}", tokens.access_token))
        .set(
            "X-Tenant-ID",
            context["identity"]["tenant_id"].as_str().ok_or(())?,
        )
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|_| ())?;
    if response.status() != 200 {
        return Err(());
    }
    let mut raw = Vec::new();
    response
        .into_reader()
        .take(32769)
        .read_to_end(&mut raw)
        .map_err(|_| ())?;
    if raw.len() > 32768 {
        return Err(());
    }
    serde_json::from_slice(&raw).map_err(|_| ())
}
async fn call(context: Value, path: &'static str, body: Value) -> Result<Json<Value>, ApiError> {
    let slot = SLOTS.try_acquire().map_err(|_| unavailable())?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        owner_call(context, path, body)
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;
    Ok(Json(result))
}
pub(super) async fn view(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(scope): Json<Scope>,
) -> Result<Json<Value>, ApiError> {
    let value = context(&state, link, &scope).await?;
    let body = scope_body(&value);
    call(value, "continue-selected", body).await
}
pub(super) async fn start(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Start>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace.clone(),
    };
    let value = context(&state, link, &scope).await?;
    {
        let gate = lock(&state.automation);
        if gate.phase() != Phase::Running || gate.session() != Some(body.session_id) {
            return Err(unavailable());
        }
    }
    let mut payload =
        start_body(&value, &body).map_err(|_| ApiError::bad("Invalid bounded task plan"))?;
    payload.as_object_mut().unwrap().remove("workspace");
    call(value, "continue-tasks", payload).await
}
pub(super) async fn control(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Control>,
) -> Result<Json<Value>, ApiError> {
    if body.run_id.is_nil()
        || body.revision == 0
        || !matches!(
            body.action.as_str(),
            "continue-on" | "continue-off" | "pause" | "stop" | "resume"
        )
    {
        return Err(ApiError::bad("Invalid task control"));
    }
    let value = context(
        &state,
        link,
        &Scope {
            session_id: body.session_id,
            workspace: body.workspace,
        },
    )
    .await?;
    let mut payload = scope_body(&value);
    payload["run_id"] = json!(body.run_id);
    payload["revision"] = json!(body.revision);
    payload["action"] = json!(body.action);
    call(value, "continue-control", payload).await
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Start) {
        let session = Uuid::new_v4();
        let mut id = json!({"session_id":session});
        for key in [
            "tenant_id",
            "device_id",
            "bot_id",
            "assignment_id",
            "organization_id",
            "owner_id",
        ] {
            id[key] = json!(Uuid::new_v4())
        }
        let value = json!({"identity":id,"workspace":"/project","class":"coding","destination":"configured","max_output_tokens":512});
        let body = Start {
            session_id: session,
            workspace: "/project".into(),
            run_id: Uuid::new_v4(),
            goal: "goal".into(),
            completion: "two checked outputs".into(),
            chunks: vec![
                Chunk {
                    prompt: "one".into(),
                    completion: "text".into(),
                    depends_on: vec![],
                },
                Chunk {
                    prompt: "two".into(),
                    completion: "text".into(),
                    depends_on: vec![0],
                },
            ],
            seconds: 120,
            tokens: 2048,
            continue_enabled: true,
        };
        (value, body)
    }
    #[test]
    fn retry_keeps_operation_ids_and_declared_dependencies() {
        let (v, b) = fixture();
        let a = start_body(&v, &b).unwrap();
        let c = start_body(&v, &b).unwrap();
        assert_eq!(a["plan"], c["plan"]);
        assert_eq!(
            a["plan"]["chunks"][1]["depends_on"][0],
            a["plan"]["chunks"][0]["id"]
        );
        assert_eq!(a["continue_enabled"], true);
    }
    #[test]
    fn scope_and_limits_cannot_come_from_prompt() {
        let (v, mut b) = fixture();
        b.workspace = "/other".into();
        assert!(start_body(&v, &b).is_err());
        b.workspace = "/project".into();
        b.seconds = 0;
        assert!(start_body(&v, &b).is_err());
        b.seconds = 120;
        b.chunks[0].depends_on = vec![1];
        assert!(start_body(&v, &b).is_err());
    }
}
