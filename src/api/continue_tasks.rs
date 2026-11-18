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
    #[serde(default)]
    money: Option<Value>,
    #[serde(default)]
    execution: Option<Value>,
    continue_enabled: bool,
    #[serde(default)]
    tracking: Vec<String>,
    #[serde(default)]
    project: Option<Value>,
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
    if let Some(money) = &body.money {
        let object = money.as_object().ok_or(())?;
        let currency = money["currency"].as_str().ok_or(())?;
        if object.len() != 2
            || currency.len() != 3
            || !currency.bytes().all(|b| b.is_ascii_uppercase())
            || money["limit_micros"].as_u64().ok_or(())? > 1_000_000_000_000
        {
            return Err(());
        }
    }
    if let Some(execution) = &body.execution {
        let obj = execution.as_object().ok_or(())?;
        let files = execution["files"].as_array().ok_or(())?;
        let checks = execution["checks"].as_array().ok_or(())?;
        let configured = context["checks"].as_array().ok_or(())?;
        if obj.len() != 3
            || files.is_empty()
            || files.len() > 12
            || checks.is_empty()
            || checks.len() > 4
            || execution["max_repairs"].as_u64().ok_or(())? > 3
            || body.money.is_none()
            || checks
                .iter()
                .any(|c| !c.is_string() || !configured.contains(c))
        {
            return Err(());
        }
        for file in files {
            let path = file.as_str().ok_or(())?;
            if path.is_empty()
                || path.len() > 256
                || path.starts_with('/')
                || path
                    .split('/')
                    .any(|p| p.is_empty() || p == "." || p == ".." || p == ".git")
                || path.contains(['\\', '*', '?'])
                || path.chars().any(char::is_control)
            {
                return Err(());
            }
        }
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
    result["plan"] = json!({"version":1,"workspace":body.workspace,"goal":body.goal,"completion":body.completion,"chunks":chunks,"tracking":body.tracking,"budgets":{"seconds":body.seconds,"tokens":body.tokens,"chunks":body.chunks.len()}});
    if let Some(money) = &body.money {
        result["plan"]["budgets"]["money"] = money.clone();
    }
    if let Some(execution) = &body.execution {
        result["plan"]["execution"] = execution.clone();
    }
    if let Some(project) = &body.project {
        let mut project = project.clone();
        let units = project["units"].as_array_mut().ok_or(())?;
        if units.len() != body.chunks.len() {
            return Err(());
        }
        for (i, unit) in units.iter_mut().enumerate() {
            unit["chunk_id"] = json!(stable_id(body.run_id, i, 0));
            unit["assignment_id"] = identity["assignment_id"].clone();
        }
        result["plan"]["project"] = project;
    }
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
        .take(131073)
        .read_to_end(&mut raw)
        .map_err(|_| ())?;
    if raw.len() > 131072 {
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
pub(super) async fn current_context(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Value>, ApiError> {
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
    let session =
        acp_machine::binding(&value, value["workspace"].as_str().ok_or_else(unavailable)?)
            .map_err(|_| unavailable())?;
    let mut view = acp_machine::context_view(&value, session);
    view["checks"] = value["checks"].clone();
    let gate = lock(&state.automation);
    view["local_epoch"] = json!(gate.epoch());
    view["local_running"] =
        json!(gate.phase() == Phase::Running && gate.session() == Some(session));
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resume {
    session_id: Uuid,
    workspace: String,
    expected_epoch: u64,
}
pub(super) async fn local_resume(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Resume>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace,
    };
    context(&state, link, &scope).await?;
    state.control_automation_at(
        automation::Control::Resume,
        true,
        Some(body.session_id),
        Some(body.expected_epoch),
    )?;
    Ok(Json(json!({"acknowledged":true})))
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
            execution: None,
            money: Some(json!({"currency":"USD","limit_micros":5000000})),
            continue_enabled: true,
            tracking: vec![],
            project: None,
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
    fn edit_scope_requires_configured_checks_and_exact_money() {
        let (mut context, mut body) = fixture();
        body.execution = Some(json!({"files":["src/file.rs"],"checks":["unit"],"max_repairs":2}));
        assert!(start_body(&context, &body).is_err());
        context["checks"] = json!(["unit"]);
        let encoded = start_body(&context, &body).unwrap();
        assert_eq!(
            encoded["plan"]["execution"],
            body.execution.clone().unwrap()
        );
        assert_eq!(encoded["plan"]["budgets"]["money"]["limit_micros"], 5000000);
        body.money = Some(json!({"currency":"USD","limit_micros":true}));
        assert!(start_body(&context, &body).is_err());
        body.money = Some(json!({"currency":"USD","limit_micros":0}));
        body.execution.as_mut().unwrap()["files"] = json!(["../outside"]);
        assert!(start_body(&context, &body).is_err());
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    session_id: Uuid,
    workspace: String,
    plan: Value,
}
pub(super) async fn plan(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Plan>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace.clone(),
    };
    context(&state, link, &scope).await?;
    let result = state
        .host_bridge
        .as_ref()
        .ok_or_else(unavailable)?
        .continue_plan(json!({"workspace":body.workspace,"plan":body.plan}))
        .await
        .map_err(|_| unavailable())?;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Project {
    session_id: Uuid,
    workspace: String,
    run_id: Uuid,
    revision: u64,
    settings: Value,
}
pub(super) async fn project(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Project>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace,
    };
    let value = context(&state, link, &scope).await?;
    let mut payload = scope_body(&value);
    payload["run_id"] = json!(body.run_id);
    payload["revision"] = json!(body.revision);
    payload["settings"] = body.settings;
    call(value, "continue-project", payload).await
}

pub(super) async fn advice(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(scope): Json<Scope>,
) -> Result<Json<Value>, ApiError> {
    let value = context(&state, link, &scope).await?;
    let payload = scope_body(&value);
    let snapshot = call(value, "continue-selected", payload).await?.0;
    let run = &snapshot["run"];
    let plan =
        advice_input(run).map_err(|_| ApiError::bad("Select a task before requesting advice"))?;
    let result = state
        .host_bridge
        .as_ref()
        .ok_or_else(unavailable)?
        .continue_plan(json!({"workspace":scope.workspace,"plan":{"advice":plan}}))
        .await
        .map_err(|_| unavailable())?;
    Ok(Json(result))
}
fn advice_input(run: &Value) -> Result<Value, ()> {
    if !run.is_object() {
        return Err(());
    }
    let results = run["results"].as_object().ok_or(())?;
    let checks: Vec<String> = results
        .iter()
        .map(|(id, v)| {
            format!(
                "{}: {} ({})",
                id,
                v["state"].as_str().unwrap_or("unknown"),
                v["output_sha256"].as_str().unwrap_or("unknown")
            )
        })
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?
        .as_secs_f64();
    Ok(
        json!({"task_id":run["id"],"goal":run["goal"].as_str().unwrap_or("Goal unavailable"),"constraints":[format!("Workspace: {}",run["workspace"].as_str().unwrap_or("unknown")),run["completion"].as_str().unwrap_or("Completion unknown"),format!("Current state: {}; Continue: {}; spent tokens: {} of {}; deadline: {}. Advice does not resume or replace workers.",run["state"],run["continue_enabled"],run["used_tokens"],run["budgets"]["tokens"],run["deadline"])],"state":run["state"],"completed":results.values().filter(|v|v["state"]=="passed").count(),"total":run["chunks"].as_array().ok_or(())?.len(),"elapsed_seconds":((now-run["created_at"].as_f64().unwrap_or(now)).max(0.0).min(86400.0)) as u64,"checks":checks,"links":run["tracking"],"observations":run["observations"]}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observations {
    session_id: Uuid,
    workspace: String,
    run_id: Uuid,
    revision: u64,
    observations: Value,
}
pub(super) async fn observations(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Observations>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace,
    };
    let value = context(&state, link, &scope).await?;
    let mut payload = scope_body(&value);
    payload["run_id"] = json!(body.run_id);
    payload["revision"] = json!(body.revision);
    payload["observations"] = body.observations;
    call(value, "continue-observations", payload).await
}

#[cfg(test)]
mod advice_tests {
    use super::*;
    #[test]
    fn advice_uses_completed_results_without_inventing_missing_metrics() {
        let run = json!({"id":"task","goal":"goal","workspace":"/project","completion":"check","state":"paused","continue_enabled":false,"used_tokens":123,"budgets":{"tokens":1000},"results":{"first":{"state":"passed","output_sha256":"hash"}},"chunks":[{},{}],"tracking":[],"observations":null});
        let before = run.clone();
        let input = advice_input(&run).unwrap();
        assert_eq!(input["completed"], 1);
        assert!(input["observations"].is_null());
        assert_eq!(run, before);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Handoff {
    session_id: Uuid,
    workspace: String,
    run_id: Uuid,
    revision: u64,
    request_id: Uuid,
    action: String,
}
pub(super) async fn handoff(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Handoff>,
) -> Result<Json<Value>, ApiError> {
    if !["restart", "reassign", "retire"].contains(&body.action.as_str())
        || body.request_id.is_nil()
    {
        return Err(ApiError::bad("Explicit worker action required"));
    }
    let scope = Scope {
        session_id: body.session_id,
        workspace: body.workspace,
    };
    let value = context(&state, link, &scope).await?;
    let mut payload = scope_body(&value);
    payload["run_id"] = json!(body.run_id);
    payload["revision"] = json!(body.revision);
    payload["request_id"] = json!(body.request_id);
    payload["action"] = json!(body.action);
    call(value, "continue-handoff", payload).await
}
