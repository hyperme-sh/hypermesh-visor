//! Companion-only preference edits and host-owned, read-only previews.
use super::*;
use crate::permission_templates::{self as templates, Preferences, Selection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn local(link: CompanionLink) -> Result<(), ApiError> {
    if link.0 {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "Permissions require the local companion socket",
        ))
    }
}
fn directory(state: &AppState) -> Result<PathBuf, ApiError> {
    let path = state.mcp_dir.join("permission-templates");
    hypermesh_local_storage::create_dir(&path)
        .map_err(|_| ApiError::unavailable("Permission templates unavailable"))?;
    Ok(path)
}
pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Preferences>, ApiError> {
    local(link)?;
    tokio::task::spawn_blocking(move || {
        templates::load(&directory(&state)?)
            .map(Json)
            .map_err(ApiError::unavailable)
    })
    .await
    .map_err(|_| ApiError::unavailable("Permission templates unavailable"))?
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    expected: Preferences,
    selection: Selection,
}
pub(super) async fn save(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(edit): Json<Edit>,
) -> Result<Json<Preferences>, ApiError> {
    local(link)?;
    tokio::task::spawn_blocking(move || {
        let mut next = edit.expected.clone();
        next.select(edit.selection).map_err(ApiError::bad)?;
        templates::save(&directory(&state)?, &edit.expected, &next).map_err(ApiError::conflict)?;
        Ok(Json(next))
    })
    .await
    .map_err(|_| ApiError::unavailable("Permission templates unavailable"))?
}
pub(super) async fn context(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
) -> Result<Json<Value>, ApiError> {
    local(link)?;
    let bridge = state
        .host_bridge
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Permission preview needs an assigned host bridge"))?;
    bridge
        .permission_context()
        .await
        .map(Json)
        .map_err(|_| ApiError::unavailable("Permission preview context unavailable"))
}
fn scope_matches(selection: &Selection, context: &Value) -> bool {
    context["version"] == 1
        && context["configured_only"] == true
        && context["device_id"].as_str() == Some(selection.scope.device.to_string().as_str())
        && context["binding"]["organization_id"].as_str()
            == Some(selection.scope.organization.to_string().as_str())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preview {
    selection: Selection,
    operation: templates::Operation,
    target: String,
    max_bytes: u64,
    timeout_seconds: u32,
}
pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Extension(link): Extension<CompanionLink>,
    Json(body): Json<Preview>,
) -> Result<Json<Value>, ApiError> {
    local(link)?;
    body.selection.validate().map_err(ApiError::bad)?;
    if body.target.is_empty()
        || body.target.len() > 4096
        || body.max_bytes == 0
        || body.max_bytes > 64 * 1024 * 1024
        || body.timeout_seconds == 0
        || body.timeout_seconds > 300
    {
        return Err(ApiError::bad("Invalid preview limits or target"));
    }
    let bridge = state
        .host_bridge
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Permission preview needs an assigned host bridge"))?;
    let context = bridge
        .permission_context()
        .await
        .map_err(|_| ApiError::unavailable("Permission preview context unavailable"))?;
    if !scope_matches(&body.selection, &context) {
        return Err(ApiError::forbidden(
            "Selected organization or device differs from assigned host",
        ));
    }
    let rules:Vec<Value>=body.selection.rules.iter().map(|r|json!({"operation":r.operation,"target":r.target,"limits":{"max_bytes":r.max_bytes,"timeout_seconds":r.timeout_seconds}})).collect();
    let request = json!({"project":body.selection.scope.project,"template_version":body.selection.version,"template":rules,
        "action":{"operation":body.operation,"target":body.target,"arguments_sha256":format!("{:x}",Sha256::digest(b"{}")),"limits":{"max_bytes":body.max_bytes,"timeout_seconds":body.timeout_seconds}}});
    let result = bridge
        .permission_preview(request)
        .await
        .map_err(|_| ApiError::unavailable("Permission preview unavailable"))?;
    if result["preview_only"] != true
        || result["identity"]["organization_id"] != body.selection.scope.organization.to_string()
        || result["identity"]["device_id"] != body.selection.scope.device.to_string()
    {
        return Err(ApiError::unavailable("Permission preview binding differs"));
    }
    Ok(Json(result))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_org_device_and_network_clients_are_refused() {
        let scope = templates::Scope {
            organization: Uuid::new_v4(),
            device: Uuid::new_v4(),
            project: std::env::temp_dir().to_str().unwrap().into(),
        };
        let selection = Selection::new(
            scope.clone(),
            templates::Preset::LocalInspection,
            scope.project,
        )
        .unwrap();
        let mut context = json!({"version":1,"configured_only":true,"device_id":scope.device,"binding":{"organization_id":scope.organization}});
        assert!(scope_matches(&selection, &context));
        context["binding"]["organization_id"] = Uuid::new_v4().to_string().into();
        assert!(!scope_matches(&selection, &context));
        assert!(local(CompanionLink(false)).is_err());
    }
}
