//! Personal preview preferences. These records never authorize an operation.
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 128 * 1024;
const ERROR: &str = "Permission templates are invalid or unavailable";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    LocalInspection,
    ProjectDevelopment,
    NamedNetwork,
    ScopedAdministration,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Https,
    ServiceInspect,
    ServiceRestart,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub organization: Uuid,
    pub device: Uuid,
    pub project: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub operation: Operation,
    pub target: String,
    pub max_bytes: u64,
    pub timeout_seconds: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub version: u32,
    pub scope: Scope,
    pub preset: Preset,
    pub rules: Vec<Rule>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub selections: Vec<Selection>,
}
fn plain(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn local_target(value: &str) -> bool {
    // Keep the lexical target explicit. Resolving OS identities belongs to the
    // later executor; this preference is not a symlink or traversal guarantee.
    plain(value, 4096)
        && Path::new(value).is_absolute()
        && !value.split(['/', '\\']).any(|p| matches!(p, "." | ".."))
        && !value.contains(['*', '?', '\\'])
}
impl Selection {
    pub fn new(scope: Scope, preset: Preset, target: String) -> Result<Self, &'static str> {
        let operations = match preset {
            Preset::LocalInspection => vec![Operation::Read],
            Preset::ProjectDevelopment => vec![Operation::Read, Operation::Write],
            Preset::NamedNetwork => vec![Operation::Https],
            Preset::ScopedAdministration => {
                vec![Operation::ServiceInspect, Operation::ServiceRestart]
            }
        };
        let rules = operations
            .into_iter()
            .map(|operation| Rule {
                operation,
                target: target.clone(),
                max_bytes: 1024 * 1024,
                timeout_seconds: 30,
            })
            .collect();
        let result = Self {
            version: 1,
            scope,
            preset,
            rules,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || self.scope.organization.is_nil()
            || self.scope.device.is_nil()
            || !local_target(&self.scope.project)
            || self.rules.is_empty()
            || self.rules.len() > 32
        {
            return Err(ERROR);
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.max_bytes == 0
                || rule.max_bytes > 64 * 1024 * 1024
                || rule.timeout_seconds == 0
                || rule.timeout_seconds > 300
                || self.rules[..index]
                    .iter()
                    .any(|r| r.operation == rule.operation && r.target == rule.target)
            {
                return Err(ERROR);
            }
            let valid = match (&self.preset, &rule.operation) {
                (Preset::LocalInspection, Operation::Read)
                | (Preset::ProjectDevelopment, Operation::Read | Operation::Write) => {
                    local_target(&rule.target)
                }
                (Preset::NamedNetwork, Operation::Https) => url::Url::parse(&rule.target)
                    .is_ok_and(|u| {
                        u.scheme() == "https"
                            && matches!(u.host(), Some(url::Host::Domain(_)))
                            && u.port().is_none()
                            && u.username().is_empty()
                            && u.password().is_none()
                            && u.query().is_none()
                            && u.fragment().is_none()
                            && !rule.target.contains(['*', '\\', '%'])
                            && u.as_str() == rule.target
                    }),
                (
                    Preset::ScopedAdministration,
                    Operation::ServiceInspect | Operation::ServiceRestart,
                ) => {
                    plain(&rule.target, 128)
                        && rule
                            .target
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b))
                }
                _ => false,
            };
            if !valid {
                return Err(ERROR);
            }
        }
        Ok(())
    }
}
impl Preferences {
    pub fn empty() -> Self {
        Self {
            version: 1,
            selections: vec![],
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1 || self.selections.len() > 64 {
            return Err(ERROR);
        }
        for (i, selection) in self.selections.iter().enumerate() {
            selection.validate()?;
            if self.selections[..i]
                .iter()
                .any(|s| s.scope == selection.scope)
            {
                return Err(ERROR);
            }
        }
        Ok(())
    }
    pub fn select(&mut self, selection: Selection) -> Result<(), &'static str> {
        selection.validate()?;
        let mut next = self.clone();
        next.selections.retain(|s| s.scope != selection.scope);
        next.selections.push(selection);
        next.validate()?;
        *self = next;
        Ok(())
    }
}
fn data_path(dir: &Path) -> PathBuf {
    dir.join("selections.json")
}
pub fn load(dir: &Path) -> Result<Preferences, &'static str> {
    storage::check_dir(dir).map_err(|_| ERROR)?;
    let file = match storage::open(&data_path(dir), storage::Open::Read { private: true }) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::empty()),
        Err(_) => return Err(ERROR),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ERROR);
    }
    let value: Preferences = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    value.validate()?;
    Ok(value)
}
/// Compare under the private file lock to avoid losing another window's edits.
pub fn save(dir: &Path, expected: &Preferences, next: &Preferences) -> Result<(), &'static str> {
    expected.validate()?;
    next.validate()?;
    storage::check_dir(dir).map_err(|_| ERROR)?;
    let lock = storage::open(&dir.join("lock"), storage::Open::Lock).map_err(|_| ERROR)?;
    storage::lock(&lock).map_err(|_| "Permission templates are being edited; refresh")?;
    let _unlock = storage::UnlockOnDrop(&lock);
    if load(dir)? != *expected {
        return Err("Permission templates changed; refresh before saving");
    }
    let bytes = serde_json::to_vec(next).map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ERROR);
    }
    let temp = dir.join(format!(".{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = storage::open(&temp, storage::Open::New).map_err(|_| ERROR)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ERROR)?;
        storage::replace(&temp, &data_path(dir)).map_err(|_| ERROR)?;
        storage::sync_dir(dir).map_err(|_| ERROR)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Scope {
        Scope {
            organization: Uuid::new_v4(),
            device: Uuid::new_v4(),
            project: std::env::temp_dir()
                .join("project")
                .to_str()
                .unwrap()
                .into(),
        }
    }
    fn selection() -> Selection {
        let scope = scope();
        Selection::new(scope.clone(), Preset::LocalInspection, scope.project).unwrap()
    }
    #[test]
    fn four_presets_are_explicit_bounded_preferences() {
        for (preset, target, count) in [
            (
                Preset::LocalInspection,
                std::env::temp_dir().to_str().unwrap().to_owned(),
                1,
            ),
            (
                Preset::ProjectDevelopment,
                std::env::temp_dir().to_str().unwrap().to_owned(),
                2,
            ),
            (Preset::NamedNetwork, "https://example.test/api/".into(), 1),
            (Preset::ScopedAdministration, "example.service".into(), 2),
        ] {
            let s = Selection::new(scope(), preset, target).unwrap();
            assert_eq!(s.rules.len(), count);
            assert!(s.rules.iter().all(|r| r.timeout_seconds == 30));
            let json = serde_json::to_value(s).unwrap();
            assert!(json.get("allow").is_none());
            assert!(json.get("grants").is_none());
        }
    }
    #[test]
    fn invalid_or_ambiguous_targets_cannot_be_saved() {
        for target in [
            "http://example.test/",
            "https://user:pass@example.test/",
            "https://example.test/?key=secret",
            "https://example.test/a/../b",
            "https://*.test/",
            "https://example.test/#secret",
        ] {
            assert!(
                Selection::new(scope(), Preset::NamedNetwork, target.into()).is_err(),
                "{target}"
            );
        }
        let mut s = selection();
        s.rules[0].timeout_seconds = 0;
        assert!(s.validate().is_err());
        s.rules[0].timeout_seconds = 30;
        s.rules[0].operation = Operation::Write;
        assert!(s.validate().is_err());
        assert!(Selection::new(
            scope(),
            Preset::ScopedAdministration,
            "service; shell".into()
        )
        .is_err());
    }
    #[test]
    fn scopes_are_independent_and_old_edits_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("permission-templates-{}", Uuid::new_v4()));
        storage::create_dir(&dir).unwrap();
        let initial = load(&dir).unwrap();
        let mut next = initial.clone();
        let first = selection();
        next.select(first.clone()).unwrap();
        let mut second = first.clone();
        second.scope.organization = Uuid::new_v4();
        next.select(second).unwrap();
        save(&dir, &initial, &next).unwrap();
        assert_eq!(load(&dir).unwrap(), next);
        assert!(save(&dir, &initial, &Preferences::empty()).is_err());
        let mut changed = next.clone();
        let mut edited = first;
        edited.rules[0].max_bytes = 128;
        changed.select(edited).unwrap();
        assert_eq!(changed.selections.len(), 2);
        save(&dir, &next, &changed).unwrap();
        assert_eq!(load(&dir).unwrap(), changed);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unknown_schema_and_authority_fields_are_rejected() {
        let mut value = serde_json::to_value(selection()).unwrap();
        value["grants"] = serde_json::json!(["all"]);
        assert!(serde_json::from_value::<Selection>(value).is_err());
        let mut s = selection();
        s.version = 2;
        assert!(s.validate().is_err());
        s.version = 1;
        s.scope.device = Uuid::nil();
        assert!(s.validate().is_err());
    }
}
