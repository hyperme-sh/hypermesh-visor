//! Local editor admission through the owner queue; machine execution stays in the host.
use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;

static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
fn unavailable() -> ApiError {
    ApiError::unavailable("Assigned editor session unavailable")
}
fn permitted(state: &AppState, session: Uuid) -> bool {
    state.has_session(session) && {
        let gate = lock(&state.automation);
        gate.phase() == Phase::Running && gate.session() == Some(session)
    }
}
pub(super) fn binding(value: &Value, workspace: &str) -> Result<Uuid, ()> {
    let identity = &value["identity"];
    let session = Uuid::parse_str(identity["session_id"].as_str().ok_or(())?).map_err(|_| ())?;
    if session.is_nil() || value["workspace"].as_str() != Some(workspace) {
        return Err(());
    }
    for key in [
        "tenant_id",
        "device_id",
        "bot_id",
        "assignment_id",
        "organization_id",
        "owner_id",
    ] {
        let id = Uuid::parse_str(identity[key].as_str().ok_or(())?).map_err(|_| ())?;
        if id.is_nil() {
            return Err(());
        }
    }
    Ok(session)
}
pub(super) async fn context(
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
    let session = binding(&value, value["workspace"].as_str().ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    if !permitted(&state, session) {
        return Err(unavailable());
    }
    Ok(Json(
        context_view(&value,session),
    ))
}

fn context_view(value:&Value,session:Uuid)->Value{
 let identity:Value=["tenant_id","owner_id","principal","organization_id","device_id","bot_id","assignment_id","session_id"].into_iter().map(|key|(key.to_owned(),value["identity"][key].clone())).collect::<serde_json::Map<String,Value>>().into();
 json!({"session_id":session,"workspace":value["workspace"],"identity":identity})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prompt {
    session_id: Uuid,
    workspace: String,
    text: String,
}
fn submission(value: &Value, body: &Prompt) -> Result<Vec<u8>, ()> {
    if binding(value, &body.workspace)? != body.session_id
        || body.text.is_empty()
        || body.text.len() > 65536
    {
        return Err(());
    }
    let mut request = serde_json::Map::new();
    request.insert("version".into(), json!(1));
    for key in [
        "tenant_id",
        "device_id",
        "bot_id",
        "assignment_id",
        "session_id",
    ] {
        request.insert(key.into(), value["identity"][key].clone());
    }
    for key in ["class", "destination", "max_output_tokens"] {
        request.insert(key.into(), value[key].clone());
    }
    request.insert("task_id".into(), json!(Uuid::new_v4()));
    request.insert("operation_id".into(), json!(Uuid::new_v4()));
    request.insert("prompt".into(), json!(body.text));
    let raw = serde_json::to_vec(&request).map_err(|_| ())?;
    if raw.len() > 512 * 1024 {
        return Err(());
    }
    Ok(raw)
}
fn admit(value: Value, raw: Vec<u8>, allowed: impl Fn() -> bool) -> Result<(), ()> {
    if !allowed() {
        return Err(());
    }
    let origin = value["plane"].as_str().ok_or(())?;
    let url = url::Url::parse(origin).map_err(|_| ())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(());
    }
    let request: Value = serde_json::from_slice(&raw).map_err(|_| ())?;
    let tokens = hypermesh_session::refresh_session(
        &hypermesh_session::KeyringStore,
        &hypermesh_session::Endpoints::panopticon(),
    )
    .map_err(|_| ())?;
    if !allowed() {
        return Err(());
    }
    let nonce = Uuid::new_v4();
    let digest = format!("{:x}", Sha256::digest(&raw));
    let payload = json!({"version":1,"request_id":nonce,"device_id":request["device_id"],"assignment_id":request["assignment_id"],"session_id":request["session_id"],"task_id":request["task_id"],"request_sha256":digest});
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .build()
        .post(&format!(
            "{}/api/v1/hypermesh/bots/{}/owner-prompt",
            origin.trim_end_matches('/'),
            request["bot_id"].as_str().ok_or(())?
        ))
        .set("Authorization", &format!("Bearer {}", tokens.access_token))
        .set("X-Tenant-ID", request["tenant_id"].as_str().ok_or(())?)
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string())
        .map_err(|_| ())?;
    if response.status() != 200 {
        return Err(());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > 8192 {
        return Err(());
    }
    let fact: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if fact["version"] != 1
        || fact["request_id"] != nonce.to_string()
        || fact["task_id"] != request["task_id"]
        || fact["assignment_id"] != request["assignment_id"]
        || fact["session_id"] != request["session_id"]
        || fact["request_sha256"] != digest
    {
        return Err(());
    }
    Ok(())
}
type Capacity = Arc<tokio::sync::SemaphorePermit<'static>>;
async fn bounded_admission(
    permit: Capacity,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<(), ()> + Send + 'static,
) -> Result<(), ()> {
    struct Cancel(Arc<AtomicBool>);
    impl Drop for Cancel {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = Cancel(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if cancelled.load(Ordering::SeqCst) {
            return Err(());
        }
        work(cancelled)
    })
    .await
    .map_err(|_| ())?
}
fn current(state: &AppState, consent: tasks::Consent) -> bool {
    state.has_session(consent.session) && consent.matches(&lock(&state.automation))
}
async fn execute(state: Arc<AppState>, body: Prompt, permit: Capacity) -> Result<Value, ()> {
    if !permitted(&state, body.session_id) {
        return Err(());
    }
    let consent = tasks::Consent::capture(&lock(&state.automation), body.session_id);
    if !current(&state, consent) {
        return Err(());
    }
    let bridge = state.host_bridge.as_ref().ok_or(())?;
    let value = bridge.machine_context().await?;
    let raw = submission(&value, &body)?;
    let copy = raw.clone();
    let check = state.clone();
    bounded_admission(permit, move |cancelled| {
        admit(value, copy, || {
            !cancelled.load(Ordering::SeqCst) && current(&check, consent)
        })
    })
    .await?;
    if !current(&state, consent) {
        return Err(());
    }
    let request = bridge.machine_prompt(raw);
    tokio::pin!(request);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let result = loop {
        tokio::select! {
            value=&mut request => break value?,
            _=interval.tick() => { if !current(&state,consent) { return Err(()); } }
        }
    };
    if !current(&state, consent) {
        return Err(());
    }
    let text = result["choices"][0]["message"]["content"]
        .as_str()
        .ok_or(())?;
    if text.len() > 65536 {
        return Err(());
    }
    Ok(json!({"session_id":body.session_id,"text":text}))
}
pub(super) async fn prompt(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Prompt>,
) -> Result<Response, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    if body.text.is_empty() || body.text.len() > 65536 || body.workspace.len() > 4096 {
        return Err(ApiError::bad("Invalid editor prompt"));
    }
    let permit = Arc::new(SLOTS.try_acquire().map_err(|_| unavailable())?);
    Ok(streamed(permit.clone(), execute(state, body, permit)))
}
fn streamed(
    permit: Capacity,
    work: impl std::future::Future<Output = Result<Value, ()>> + Send + 'static,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    // Returning a streaming body makes disconnect observable before model output.
    // Dropping it cancels the upstream future and closes the host connection.
    tokio::spawn(async move {
        let _permit = permit;
        tokio::select! {
            _=tx.closed()=>{},
            result=tokio::time::timeout(Duration::from_secs(340),work)=>{
                let value=match result { Ok(Ok(value))=>value,_=>json!({"error":"Assigned editor session unavailable; not retried"}) };
                let _=tx.send(Ok::<Bytes,std::convert::Infallible>(Bytes::from(value.to_string()))).await;
            }
        }
    });
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(ReceiverStream::new(rx)),
    )
        .into_response()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn disconnect_drops_active_upstream_without_retry() {
        let ended = Arc::new(AtomicBool::new(false));
        let flag = ended.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let response = streamed(Arc::new(SLOTS.try_acquire().unwrap()), async move {
            let _guard = Guard(flag);
            let _ = started.send(());
            std::future::pending::<Result<Value, ()>>().await
        });
        ready.await.unwrap();
        drop(response);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !ended.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn renewed_consent_cannot_revive_an_old_editor_turn() {
        let mut gate = automation::Gate::default();
        let session = Uuid::new_v4();
        gate.record_type(session, None, gate.generation(), 0);
        let consent = tasks::Consent::capture(&gate, session);
        assert!(consent.matches(&gate));
        gate.control(Some(session), Control::Pause).unwrap();
        gate.control(Some(session), Control::Resume).unwrap();
        assert!(!consent.matches(&gate));
    }
    #[tokio::test]
    async fn cancelled_blocking_worker_retains_capacity_and_cannot_admit() {
        static ONE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let permit = Arc::new(ONE.try_acquire().unwrap());
        let (start, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let admitted = Arc::new(AtomicBool::new(false));
        let flag = admitted.clone();
        let job = tokio::spawn(bounded_admission(permit, move |cancelled| {
            let _ = start.send(());
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            if !cancelled.load(Ordering::SeqCst) {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(())
        }));
        started.await.unwrap();
        job.abort();
        let _ = job.await;
        assert!(ONE.try_acquire().is_err());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while ONE.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!admitted.load(Ordering::SeqCst));
    }
    #[test]
    fn project_session_and_route_are_bound_before_owner_admission() {
        let session = Uuid::new_v4();
        let mut identity = json!({"session_id":session});
        for key in [
            "tenant_id",
            "device_id",
            "bot_id",
            "assignment_id",
            "organization_id",
            "owner_id",
        ] {
            identity[key] = json!(Uuid::new_v4());
        }
        let value = json!({"identity":identity,"workspace":"/project","class":"coding","destination":"owner-route","max_output_tokens":1024});
        let view=context_view(&value,session);
        assert_eq!(view["identity"]["device_id"],value["identity"]["device_id"]);
        assert_eq!(view["identity"]["organization_id"],value["identity"]["organization_id"]);
        assert_eq!(view["session_id"],session.to_string());
        assert_eq!(view["workspace"],"/project");
        assert!(view.get("class").is_none());
        let mut body = Prompt {
            session_id: session,
            workspace: "/project".into(),
            text: "untrusted prompt".into(),
        };
        let raw = submission(&value, &body).unwrap();
        let request: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(request["destination"], "owner-route");
        assert_eq!(request["prompt"], body.text);
        body.text = "\n".repeat(50000);
        assert!(submission(&value, &body).unwrap().len() > 96 * 1024);
        body.workspace = "/other".into();
        assert!(submission(&value, &body).is_err());
        body.workspace = "/project".into();
        body.session_id = Uuid::new_v4();
        assert!(submission(&value, &body).is_err());
    }
}
