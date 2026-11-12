//! Prompt settings for the permission-bearing native app.
use crate::prompt::{ConductorDoor, DoorConfig};
use std::ffi::OsString;

pub(crate) fn configuration(
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Result<DoorConfig, String> {
    if environment("HYPERMESH_CONDUCTOR_CONFIG").is_some() {
        return Err("native app requires an independently started Conductor until child isolation is available".into());
    }
    if environment("HYPERMESH_SUPERVISOR_URL").is_some()
        || environment("HYPERMESH_SUPERVISOR_URL_2").is_some()
    {
        return Err("native app prompt routing requires Conductor".into());
    }
    let mut text = |name| {
        environment(name)
            .map(|value| value.into_string().map_err(|_| "invalid native prompt configuration".to_string()))
            .transpose()
    };
    let mut config = DoorConfig::default();
    config.conductor_addr = text("HYPERMESH_CONDUCTOR_ADDR")?;
    config.conductor_lease = text("HYPERMESH_CONDUCTOR_LEASE_ID")?
        .map(|value| uuid::Uuid::parse_str(&value).map_err(|_| "invalid Conductor lease ID".to_string()))
        .transpose()?;
    if let Some(model) = text("HYPERMESH_DEFAULT_MODEL")? {
        let model = model.trim();
        if model.len() > 1024 || model.chars().any(char::is_control) {
            return Err("invalid native default model".into());
        }
        if !model.is_empty() { config.default_model = model.into(); }
    }
    match &config.conductor_addr {
        Some(address) => { ConductorDoor::connect(address, config.conductor_lease)?; }
        None if config.conductor_lease.is_some() => return Err("Conductor lease ID requires a Conductor route".into()),
        None => {}
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured(values: &[(&str, &str)]) -> Result<DoorConfig, String> {
        configuration(|key| values.iter().find(|(name, _)| *name == key).map(|(_, value)| OsString::from(value)))
    }
    #[test]
    fn native_route_retains_model_and_task_scope() {
        let lease = "11111111-1111-4111-8111-111111111111";
        let config = configured(&[("HYPERMESH_CONDUCTOR_ADDR", "127.0.0.1:12345"),
            ("HYPERMESH_CONDUCTOR_LEASE_ID", lease), ("HYPERMESH_DEFAULT_MODEL", " custom-model ")]).unwrap();
        assert_eq!(config.conductor_addr.as_deref(), Some("127.0.0.1:12345"));
        assert_eq!(config.conductor_lease.unwrap().to_string(), lease);
        assert_eq!(config.default_model, "custom-model");
        assert!(config.primary_url.is_none() && config.second_url.is_none());
    }
    #[test]
    fn missing_route_stays_unconfigured_and_invalid_settings_fail_closed() {
        assert!(configured(&[]).unwrap().conductor_addr.is_none());
        for (key, value) in [
            ("HYPERMESH_CONDUCTOR_ADDR", "192.0.2.1:80"),
            ("HYPERMESH_CONDUCTOR_ADDR", "127.0.0.1:0"),
            ("HYPERMESH_CONDUCTOR_ADDR", ""),
            ("HYPERMESH_CONDUCTOR_LEASE_ID", "private-invalid-lease"),
            ("HYPERMESH_CONDUCTOR_LEASE_ID", "11111111-1111-4111-8111-111111111111"),
            ("HYPERMESH_CONDUCTOR_CONFIG", "private-path"),
            ("HYPERMESH_SUPERVISOR_URL", "https://private.example"),
            ("HYPERMESH_SUPERVISOR_URL_2", "https://private.example"),
        ] {
            let error = configured(&[(key, value)]).unwrap_err();
            assert!(!error.contains("private"));
        }
    }
}
