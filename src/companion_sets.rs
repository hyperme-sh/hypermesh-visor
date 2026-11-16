//! Private saved companion configuration. Credentials are never part of this schema.
use crate::companion_manager::{Error, Resources, Scope};
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
const LIMIT: u64 = 1024 * 1024;
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    companion: Uuid,
    session: Uuid,
    generation: Uuid,
}
impl Reference {
    pub fn new(scope: Scope) -> Self {
        Self {
            companion: scope.companion(),
            session: scope.session(),
            generation: scope.generation(),
        }
    }
    pub fn scope(self) -> Scope {
        Scope::recovered(self.companion, self.session, self.generation)
    }
    fn valid(self) -> bool {
        !self.companion.is_nil() && !self.session.is_nil() && !self.generation.is_nil()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub memory_mib: u32,
    pub cpu_millis: u32,
    pub instructions: String,
    pub grant: Option<Reference>,
}
impl Member {
    fn valid(&self) -> bool {
        (512..=32768).contains(&self.memory_mib)
            && (100..=32000).contains(&self.cpu_millis)
            && valid_instructions(&self.instructions)
            && self.grant.is_none_or(Reference::valid)
    }
    pub fn resources(&self) -> Resources {
        Resources {
            memory_mib: self.memory_mib,
            cpu_millis: self.cpu_millis,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSet {
    pub id: Uuid,
    pub name: String,
    pub members: Vec<Member>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    reference: Reference,
    instructions: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    sets: Vec<SavedSet>,
    profiles: Vec<Profile>,
}
pub struct Store {
    path: PathBuf,
    data: Data,
    poisoned: bool,
}
pub fn valid_instructions(value: &str) -> bool {
    value.len() <= 4096 && !value.contains('\0')
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("companion-sets.json");
        let data: Data = match storage::open(&path, storage::Open::Read { private: true }) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Backend)?;
                if bytes.len() as u64 > LIMIT {
                    return Err(Error::Invalid);
                }
                serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(_) => return Err(Error::Backend),
        };
        let mut ids = std::collections::HashSet::new();
        if data.sets.len() > 32
            || data.profiles.len() > 128
            || data.sets.iter().any(|s| {
                s.id.is_nil()
                    || !ids.insert(s.id)
                    || !valid_name(&s.name)
                    || s.members.is_empty()
                    || s.members.len() > 3
                    || s.members.iter().any(|m| !m.valid())
            })
            || data.profiles.iter().any(|p| {
                !p.reference.valid()
                    || !ids.insert(p.reference.generation)
                    || !valid_instructions(&p.instructions)
            })
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            path,
            data,
            poisoned: false,
        })
    }
    fn save(&mut self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        let bytes = serde_json::to_vec(&self.data).map_err(|_| Error::Backend)?;
        if bytes.len() as u64 > LIMIT {
            self.poisoned = true;
            return Err(Error::Capacity);
        }
        let temporary = self.path.with_extension(Uuid::new_v4().to_string());
        let result = (|| -> std::io::Result<()> {
            let mut file = storage::open(&temporary, storage::Open::New)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            storage::replace(&temporary, &self.path)?;
            storage::sync_dir(self.path.parent().unwrap())
        })();
        let _ = std::fs::remove_file(temporary);
        if result.is_err() {
            self.poisoned = true;
            return Err(Error::Backend);
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<(Uuid, String, usize, bool)>, Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        Ok(self
            .data
            .sets
            .iter()
            .map(|s| {
                (
                    s.id,
                    s.name.clone(),
                    s.members.len(),
                    s.members.iter().any(|m| m.grant.is_some()),
                )
            })
            .collect())
    }
    pub fn get(&self, id: Uuid) -> Result<SavedSet, Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        self.data
            .sets
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or(Error::Stale)
    }
    pub fn create(&mut self, name: String, members: Vec<Member>) -> Result<Uuid, Error> {
        if !valid_name(&name)
            || members.is_empty()
            || members.len() > 3
            || members.iter().any(|m| !m.valid())
        {
            return Err(Error::Invalid);
        }
        if self.data.sets.len() >= 32 {
            return Err(Error::Capacity);
        }
        let id = Uuid::new_v4();
        self.data.sets.push(SavedSet { id, name, members });
        self.save()?;
        Ok(id)
    }
    pub fn delete(&mut self, id: Uuid) -> Result<(), Error> {
        self.data.sets.retain(|s| s.id != id);
        self.save()
    }
    pub fn instructions(&self, scope: Scope) -> Result<String, Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        Ok(self
            .data
            .profiles
            .iter()
            .find(|p| p.reference.scope() == scope)
            .map(|p| p.instructions.clone())
            .unwrap_or_default())
    }
    pub fn configure(&mut self, scope: Scope, instructions: String) -> Result<(), Error> {
        if !valid_instructions(&instructions) {
            return Err(Error::Invalid);
        }
        if let Some(profile) = self
            .data
            .profiles
            .iter_mut()
            .find(|p| p.reference.scope() == scope)
        {
            profile.instructions = instructions;
        } else {
            if self.data.profiles.len() >= 128 {
                return Err(Error::Capacity);
            }
            self.data.profiles.push(Profile {
                reference: Reference::new(scope),
                instructions,
            });
        }
        self.save()
    }
    pub fn retain(&mut self, scopes: &[Scope]) -> Result<(), Error> {
        let size = self.data.profiles.len();
        self.data
            .profiles
            .retain(|p| scopes.contains(&p.reference.scope()));
        if size != self.data.profiles.len() {
            self.save()?;
        }
        Ok(())
    }
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GuestProfile {
    reference: Reference,
    instructions: String,
}
pub fn digest(instructions: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(instructions.as_bytes()))
}
pub fn write_guest(directory: &Path, scope: Scope, instructions: &str) -> Result<(), Error> {
    if !valid_instructions(instructions) {
        return Err(Error::Invalid);
    }
    let bytes = serde_json::to_vec(&GuestProfile {
        reference: Reference::new(scope),
        instructions: instructions.into(),
    })
    .map_err(|_| Error::Invalid)?;
    let path = directory.join(format!(".profile-{}", Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = storage::open(&path, storage::Open::New)?;
        file.write_all(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        }
        drop(file);
        storage::replace(&path, &directory.join("profile.json"))
    })();
    let _ = std::fs::remove_file(path);
    result.map_err(|_| Error::Backend)
}
pub fn read_guest(path: &Path, scope: Scope) -> Result<String, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => return Err(Error::Backend),
    };
    if !file.metadata().map_err(|_| Error::Backend)?.is_file() {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Backend)?;
    if bytes.len() > 32768 {
        return Err(Error::Invalid);
    }
    let value: GuestProfile = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if value.reference.scope() != scope || !valid_instructions(&value.instructions) {
        return Err(Error::Stale);
    }
    Ok(value.instructions)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sets-unit-{}", Uuid::new_v4()));
            storage::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scope() -> Scope {
        Scope::recovered(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
    }
    fn member(source: Scope) -> Member {
        Member {
            memory_mib: 2048,
            cpu_millis: 1000,
            instructions: "Review the assigned change".into(),
            grant: Some(Reference::new(source)),
        }
    }
    #[test]
    fn persisted_sets_keep_configuration_and_references_without_copying_credentials() {
        let temp = Temp::new();
        std::fs::write(
            temp.0.join("companion-broker.json"),
            "credential-canary-do-not-copy",
        )
        .unwrap();
        let source = scope();
        let mut store = Store::open(&temp.0).unwrap();
        store
            .configure(source, "source instructions".into())
            .unwrap();
        let id = store
            .create("Reviewers".into(), vec![member(source)])
            .unwrap();
        drop(store);
        let mut store = Store::open(&temp.0).unwrap();
        let saved = store.get(id).unwrap();
        assert_eq!(saved.members[0].grant.unwrap().scope(), source);
        assert_eq!(saved.members[0].instructions, "Review the assigned change");
        let bytes = std::fs::read_to_string(temp.0.join("companion-sets.json")).unwrap();
        assert!(!bytes.contains("credential-canary"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(temp.0.join("companion-sets.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        store.retain(&[]).unwrap();
        assert_eq!(store.instructions(source).unwrap(), "");
        assert!(store.get(id).is_ok());
        store.configure(source, "still here".into()).unwrap();
        store.delete(id).unwrap();
        assert_eq!(store.instructions(source).unwrap(), "still here");
        assert!(store.list().unwrap().is_empty());
    }
    #[test]
    fn credentials_unknown_fields_and_invalid_configuration_are_refused() {
        let temp = Temp::new();
        let source = scope();
        let mut value = serde_json::to_value(member(source)).unwrap();
        value["credential"] = "secret".into();
        assert!(serde_json::from_value::<Member>(value).is_err());
        let mut store = Store::open(&temp.0).unwrap();
        assert!(store.create(" ".into(), vec![member(source)]).is_err());
        assert!(store.create("empty".into(), vec![]).is_err());
        assert!(store
            .create("too many".into(), vec![member(source); 4])
            .is_err());
        let mut invalid = member(source);
        invalid.instructions = "x".repeat(4097);
        assert!(store.create("oversize".into(), vec![invalid]).is_err());
        assert!(store.configure(source, "bad\0text".into()).is_err());
        assert!(store.list().unwrap().is_empty());
    }
    #[test]
    fn guest_profile_is_bounded_and_bound_to_the_exact_generation() {
        let temp = Temp::new();
        let source = scope();
        let path = temp.0.join("profile.json");
        assert_eq!(read_guest(&path, source).unwrap(), "");
        write_guest(&temp.0, source, "specific instructions").unwrap();
        assert_eq!(read_guest(&path, source).unwrap(), "specific instructions");
        assert!(read_guest(&path, scope()).is_err());
        std::fs::write(&path, vec![b' '; 32769]).unwrap();
        assert!(read_guest(&path, source).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink("missing", &path).unwrap();
            assert!(read_guest(&path, source).is_err());
        }
    }
}
