use super::*;
use serde_json::{json, Value};
static SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
pub(super) async fn history(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Value>, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    let bridge = state
        .host_bridge
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Host unavailable"))?;
    let context = bridge
        .machine_context()
        .await
        .map_err(|_| ApiError::unavailable("Current context unavailable"))?;
    let dir = local_reads::directory(&state).map_err(ApiError::unavailable)?;
    let journal = tokio::task::spawn_blocking(move || crate::permission_dream::load(&dir))
        .await
        .map_err(|_| ApiError::unavailable("Review history unavailable"))?
        .map_err(ApiError::unavailable)?;
    Ok(Json(
        json!({"version":1,"history":journal.history.into_iter().filter(|h|h["identity"]["organization_id"]==context["identity"]["organization_id"] && h["identity"]["principal"]==context["identity"]["principal"] && h["project"]==context["workspace"]).collect::<Vec<_>>(),"pending":journal.pending.is_some()}),
    ))
}
pub(super) async fn run(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Value>, ApiError> {
    if !link.0 {
        return Err(ApiError::forbidden("Local companion socket required"));
    }
    let _slot = SLOT
        .try_acquire()
        .map_err(|_| ApiError::unavailable("Review already running"))?;
    let bridge = state
        .host_bridge
        .clone()
        .ok_or_else(|| ApiError::unavailable("Host unavailable"))?;
    let context = bridge
        .machine_context()
        .await
        .map_err(|_| ApiError::unavailable("Active machine context unavailable"))?;
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
    let project = context["workspace"]
        .as_str()
        .ok_or_else(|| ApiError::unavailable("Workspace unavailable"))?
        .to_owned();
    let dir = local_reads::directory(&state).map_err(ApiError::unavailable)?;
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        crate::permission_dream::perform(&dir, &identity, &project, |request| {
            rt.block_on(bridge.permission_dream(request))
                .map_err(|_| "Permission review unavailable")
        })
    })
    .await
    .map_err(|_| ApiError::unavailable("Review unavailable"))?
    .map(Json)
    .map_err(ApiError::unavailable)
}
