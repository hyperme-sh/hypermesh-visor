//! Policy edits use the existing native sign-in and scoped-session exchange.
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Read;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    tenant_id: String,
    organization_id: String,
    #[serde(default)]
    mutation: Option<Value>,
}
fn id(value: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| "Invalid policy scope")?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err("Invalid policy scope".into());
    }
    Ok(())
}
pub fn request(body: Value) -> Result<Value, String> {
    let input: Input = serde_json::from_value(body).map_err(|_| "Invalid policy request")?;
    id(&input.tenant_id)?;
    id(&input.organization_id)?;
    let tokens = hypermesh_session::refresh_session(
        &hypermesh_session::KeyringStore,
        &hypermesh_session::Endpoints::panopticon(),
    )
    .map_err(|_| "Sign in again to change access policy")?;
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .redirects(0)
        .try_proxy_from_env(false)
        .timeout(std::time::Duration::from_secs(20))
        .build();
    transact(
        input,
        &tokens.access_token,
        |method, path, bearer, body, tenant| {
            let request = agent
                .request(method, &format!("{}{path}", crate::pages::API_BASE))
                .set("Authorization", &format!("Bearer {bearer}"))
                .set("X-Tenant-ID", tenant)
                .set("Content-Type", "application/json");
            let result = if let Some(body) = body {
                request.send_string(&body.to_string())
            } else {
                request.call()
            };
            let response = match result {
                Ok(r) if r.status() == 200 => r,
                Err(ureq::Error::Status(409, _)) => {
                    return Err("Policy changed; reload before editing".into())
                }
                _ => return Err("Policy request refused or unavailable".into()),
            };
            let mut raw = Vec::new();
            response
                .into_reader()
                .take(2097153)
                .read_to_end(&mut raw)
                .map_err(|_| "Policy response unavailable")?;
            if raw.len() > 2097152 {
                return Err("Policy response too large".into());
            }
            serde_json::from_slice(&raw).map_err(|_| "Invalid policy response".into())
        },
    )
}
fn transact(
    input: Input,
    native: &str,
    mut send: impl FnMut(&str, &str, &str, Option<Value>, &str) -> Result<Value, String>,
) -> Result<Value, String> {
    id(&input.tenant_id)?;
    id(&input.organization_id)?;
    if input
        .mutation
        .as_ref()
        .is_some_and(|m| m["organization_id"] != input.organization_id)
    {
        return Err("Organization differs".into());
    }
    let exchanged = send(
        "POST",
        "/auth/scoped-session",
        native,
        Some(json!({"tenant_id":input.tenant_id,"organization_id":input.organization_id})),
        &input.tenant_id,
    )?;
    let token = exchanged["access_token"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 16384)
        .ok_or("Scoped sign-in unavailable")?;
    if exchanged["token_type"] != "Bearer" || exchanged["expires_in"].as_u64().unwrap_or(0) == 0 {
        return Err("Scoped sign-in unavailable".into());
    }
    let result = if let Some(mutation) = input.mutation {
        send(
            "PUT",
            "/api/v1/aiuditor/access/policy",
            token,
            Some(mutation),
            &input.tenant_id,
        )?
    } else {
        send(
            "GET",
            &format!("/api/v1/aiuditor/access/policy/{}", input.organization_id),
            token,
            None,
            &input.tenant_id,
        )?
    };
    if result["version"] != 1 || result["organization_id"] != input.organization_id {
        return Err("Policy scope differs".into());
    }
    id(result["revision"]
        .as_str()
        .ok_or("Policy revision unavailable")?)?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_uses_scoped_token_and_bound_organization() {
        let tenant = uuid::Uuid::new_v4().to_string();
        let org = uuid::Uuid::new_v4().to_string();
        let revision = uuid::Uuid::new_v4().to_string();
        let mut calls = 0;
        let result = transact(
            Input {
                tenant_id: tenant.clone(),
                organization_id: org.clone(),
                mutation: None,
            },
            "native",
            |method, path, token, body, selected| {
                calls += 1;
                assert_eq!(selected, tenant);
                if calls == 1 {
                    assert_eq!(
                        (method, path, token),
                        ("POST", "/auth/scoped-session", "native")
                    );
                    assert_eq!(body.unwrap()["organization_id"], org);
                    Ok(json!({"access_token":"scoped","token_type":"Bearer","expires_in":300}))
                } else {
                    assert_eq!(method, "GET");
                    assert_eq!(token, "scoped");
                    assert!(path.ends_with(&org));
                    Ok(json!({"version":1,"organization_id":org,"revision":revision}))
                }
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result["revision"], revision);
    }
    #[test]
    fn mismatched_mutation_never_sends_credentials() {
        let result = transact(
            Input {
                tenant_id: uuid::Uuid::new_v4().to_string(),
                organization_id: uuid::Uuid::new_v4().to_string(),
                mutation: Some(json!({"organization_id":uuid::Uuid::new_v4()})),
            },
            "native",
            |_, _, _, _, _| panic!("must not send"),
        );
        assert!(result.is_err());
    }
}
