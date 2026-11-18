//! Local companion read entrypoint; all file access uses the Visor user's UID.
use super::*;
use crate::local_reads as reads;
use serde_json::{json, Value};

static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
use crate::access_actions::Input as ReadRequest;
#[cfg(unix)]
fn execute(
    state: Arc<AppState>,
    body: ReadRequest,
    cancel: Arc<AtomicBool>,
    consent: tasks::Consent,
    rt: tokio::runtime::Handle,
) -> Result<Value, &'static str> {
    let session = body.session_id;
    let valid = || {
        !cancel.load(Ordering::SeqCst)
            && state.has_session(session)
            && consent.matches(&lock(&state.automation))
    };
    if !valid() {
        return Err(reads::ERROR);
    }
    let dir = super::local_reads::directory(&state)?;
    let bridge = state.host_bridge.as_ref().ok_or(reads::ERROR)?;
    let context = rt
        .block_on(bridge.machine_context())
        .map_err(|_| reads::ERROR)?;
    let identity: Value = [
        "tenant_id",
        "owner_id",
        "principal",
        "organization_id",
        "device_id",
        "bot_id",
        "assignment_id",
        "session_id",
    ]
    .into_iter()
    .map(|k| (k.to_owned(), context["identity"][k].clone()))
    .collect::<serde_json::Map<String, Value>>()
    .into();
    crate::access_actions::perform(
        &dir,
        body,
        &identity,
        context["workspace"].as_str().ok_or(reads::ERROR)?,
        |request, mode| {
            let future = async {
                match mode {
                    0 => bridge.permission_action(request, false).await,
                    1 => bridge.permission_action(request, true).await,
                    2 => bridge.permission_service_context().await,
                    _ => bridge.permission_service_execute(request).await,
                }
            };
            rt.block_on(async {
                tokio::pin!(future);
                let mut timer = tokio::time::interval(Duration::from_millis(50));
                loop {
                    tokio::select! {
                        result = &mut future => return result.map_err(|_| reads::ERROR),
                        _ = timer.tick() => if !valid() { return Err(reads::ERROR); }
                    }
                }
            })
        },
        valid,
    )
}
#[cfg(not(unix))]
fn execute(
    _: Arc<AppState>,
    _: ReadRequest,
    _: Arc<AtomicBool>,
    _: tasks::Consent,
    _: tokio::runtime::Handle,
) -> Result<Value, &'static str> {
    Err(reads::ERROR)
}

pub(super) async fn run(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<ReadRequest>,
) -> Result<Response, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    body.selection.validate().map_err(ApiError::bad)?;
    if body.request_id.is_nil()
        || body.session_id.is_nil()
        || body.target.len() > 4096
        || body.max_bytes == 0
        || body.max_bytes > 1048576
        || body.timeout_seconds == 0
        || body.timeout_seconds > 30
    {
        return Err(ApiError::bad(
            "Action limits must be at most 1 MiB and 30 seconds",
        ));
    }
    let consent = tasks::Consent::capture(&lock(&state.automation), body.session_id);
    if !consent.matches(&lock(&state.automation)) {
        return Err(ApiError::forbidden(reads::ERROR));
    }
    let permit = SLOTS
        .try_acquire()
        .map_err(|_| ApiError::unavailable(reads::ERROR))?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        struct Cancel(Arc<AtomicBool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _cancel = Cancel(cancelled.clone());
        let rt = tokio::runtime::Handle::current();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            execute(state, body, cancelled, consent, rt)
        });
        tokio::select! {
            _=tx.closed()=>{},
            result=tokio::time::timeout(Duration::from_secs(65),work)=>{
                let value=match result{Ok(Ok(Ok(value)))=>value,_=>json!({"error":"Action did not confirm completion; outcome may be unknown. Do not automatically retry."})};
                let _=tx.send(Ok::<Bytes,std::convert::Infallible>(Bytes::from(value.to_string()))).await;
            }
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(ReceiverStream::new(rx)),
    )
        .into_response())
}
