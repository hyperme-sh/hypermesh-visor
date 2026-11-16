//! Host-owned bot credentials and current Plane bindings for isolated guests.
//! No credential, owner session or network configuration is sent to a guest.
use crate::companion_manager::{Error, Scope};
use hypermesh_local_storage as storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const LIMIT: u64 = 262144;
fn now() -> Result<f64, Error> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Stale)?
        .as_secs_f64())
}
fn origin(value: &str) -> Result<String, Error> {
    let url = url::Url::parse(value).map_err(|_| Error::Invalid)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::Invalid);
    }
    Ok(url.origin().ascii_serialization())
}
fn credential(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("hm_bot_")
        && value[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    companion: Uuid,
    session: Uuid,
    generation: Uuid,
    pub bot: Uuid,
    tenant: Uuid,
    owner: Uuid,
    origin: String,
    secret: String,
    assignment: Uuid,
    #[serde(skip, default = "Uuid::new_v4")]
    connection: Uuid,
}
impl Drop for Binding {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.secret.zeroize();
    }
}
impl Binding {
    fn scope(&self) -> Scope {
        Scope::recovered(self.companion, self.session, self.generation)
    }
    fn valid(&self) -> bool {
        [
            self.companion,
            self.session,
            self.generation,
            self.bot,
            self.tenant,
            self.owner,
        ]
        .iter()
        .all(|id| !id.is_nil())
            && credential(&self.secret)
            && origin(&self.origin).is_ok()
    }
    fn matches(&self, status: &Value) -> bool {
        status["bot_id"] == self.bot.to_string()
            && status["tenant_id"] == self.tenant.to_string()
            && status["owner_id"] == self.owner.to_string()
            && status["mode"] == "container"
            && status["platform"] == "linux"
            && status["revoked"] == false
            && status["assignment"]["id"] == self.assignment.to_string()
            && status["assignment"]["session_id"] == self.session.to_string()
    }
    fn agent(&self, action: &str, body: Value) -> Result<Value, Error> {
        post(
            &self.origin,
            self.tenant,
            &self.secret,
            &format!("/{}/agent/{action}", self.bot),
            body,
        )
    }
}
pub struct Broker {
    path: PathBuf,
    bindings: Vec<Binding>,
    poisoned: bool,
}
impl Broker {
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("broker.json");
        let bindings: Vec<Binding> =
            match storage::open(&path, storage::Open::Read { private: true }) {
                Ok(file) => {
                    let mut bytes = Zeroizing::new(Vec::new());
                    file.take(LIMIT + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|_| Error::Backend)?;
                    if bytes.len() as u64 > LIMIT {
                        return Err(Error::Invalid);
                    }
                    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(_) => return Err(Error::Backend),
            };
        let mut ids = std::collections::HashSet::new();
        if bindings.len() > 128
            || bindings
                .iter()
                .any(|b| !b.valid() || !ids.insert(b.generation) || !ids.insert(b.bot))
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            path,
            bindings,
            poisoned: false,
        })
    }
    fn save(&mut self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&self.bindings).map_err(|_| Error::Backend)?);
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Capacity);
        }
        let temp = self.path.with_extension(format!("{}", Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut file = storage::open(&temp, storage::Open::New)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            storage::replace(&temp, &self.path)?;
            storage::sync_dir(self.path.parent().unwrap())
        })();
        let _ = std::fs::remove_file(temp);
        if result.is_err() {
            self.poisoned = true;
            return Err(Error::Backend);
        }
        Ok(())
    }
    fn binding(&self, scope: Scope) -> Result<&Binding, Error> {
        if self.poisoned {
            return Err(Error::Backend);
        }
        self.bindings
            .iter()
            .find(|b| b.scope() == scope)
            .ok_or(Error::Stale)
    }
    pub fn retain(&mut self, scopes: &[Scope]) -> Result<(), Error> {
        let count = self.bindings.len();
        self.bindings.retain(|b| scopes.contains(&b.scope()));
        if self.bindings.len() != count {
            self.save()?;
        }
        Ok(())
    }
    pub fn remove(&mut self, scope: Scope) -> Result<(), Error> {
        let scopes = self
            .bindings
            .iter()
            .filter(|b| b.scope() != scope)
            .map(Binding::scope)
            .collect::<Vec<_>>();
        self.retain(&scopes)
    }
    pub fn task_live(status: &Value, task: Uuid) -> bool {
        status["tasks"]
            .as_array()
            .and_then(|tasks| tasks.iter().find(|t| t["id"] == task.to_string()))
            .is_some_and(|t| {
                t["status"] == "delivered" && t.get("cancel_requested").is_none_or(|v| v == false)
            })
    }
    pub fn disconnect(&mut self, scope: Scope) {
        if let Some(binding) = self.bindings.iter_mut().find(|b| b.scope() == scope) {
            binding.connection = Uuid::new_v4();
        }
    }
    pub fn enrolled(&self, scope: Scope) -> bool {
        self.bindings.iter().any(|b| b.scope() == scope)
    }
    pub fn bot(&self, scope: Scope) -> Option<Uuid> {
        self.bindings
            .iter()
            .find(|b| b.scope() == scope)
            .map(|b| b.bot)
    }
    pub fn enroll(
        &mut self,
        scope: Scope,
        service: &str,
        tenant: Uuid,
        owner_token: &str,
    ) -> Result<(), Error> {
        if self.bindings.len() >= 128 || self.enrolled(scope) {
            return Err(Error::Capacity);
        }
        let service = origin(service)?;
        let reply = post(
            &service,
            tenant,
            owner_token,
            "",
            json!({"mode":"container", "platform":"linux", "capabilities":["desktop"]}),
        )?;
        let bot = parse_id(&reply["bot"]["bot_id"]).map_err(|_| Error::Backend)?;
        // Persist the paused identity before the second remote operation. An
        // uncertain assignment can never lose its credential or enroll twice.
        self.install(scope, service.clone(), tenant, reply.clone())
            .map_err(|_| Error::Backend)?;
        let assignment = Uuid::new_v4();
        let assigned = post(
            &service,
            tenant,
            owner_token,
            &format!("/{bot}/assignment"),
            json!({"assignment_id":assignment,"session_id":scope.session(),"expires_at":now()? as u64 + 3600}),
        )?;
        let binding = self
            .bindings
            .iter_mut()
            .find(|b| b.scope() == scope)
            .ok_or(Error::Stale)?;
        binding.assignment = assignment;
        if !binding.matches(&assigned) {
            self.poisoned = true;
            return Err(Error::Stale);
        }
        self.save()
    }
    pub fn recreate(
        &mut self,
        source: Scope,
        target: Scope,
        action: &str,
        operation: Uuid,
        owner_token: &str,
    ) -> Result<(), Error> {
        if self.bindings.len() >= 128
            || self.enrolled(target)
            || !matches!(action, "clone" | "restore")
        {
            return Err(Error::Invalid);
        }
        let old = self.binding(source)?;
        let reply = post(
            &old.origin,
            old.tenant,
            owner_token,
            &format!("/{}/recreate", old.bot),
            json!({"operation_id":operation,"action":action,"session_id":target.session()}),
        )?;
        if reply["bot"]["owner_id"] != old.owner.to_string() {
            return Err(Error::Stale);
        }
        let service = old.origin.clone();
        let tenant = old.tenant;
        self.install(target, service, tenant, reply)
    }
    fn install(
        &mut self,
        scope: Scope,
        service: String,
        tenant: Uuid,
        reply: Value,
    ) -> Result<(), Error> {
        let status = &reply["bot"];
        let binding = Binding {
            companion: scope.companion(),
            session: scope.session(),
            generation: scope.generation(),
            bot: parse_id(&status["bot_id"])?,
            tenant,
            owner: parse_id(&status["owner_id"])?,
            origin: service,
            secret: reply["credential"].as_str().ok_or(Error::Invalid)?.into(),
            assignment: if status["assignment"].is_null() {
                Uuid::nil()
            } else {
                parse_id(&status["assignment"]["id"])?
            },
            connection: Uuid::new_v4(),
        };
        if !binding.valid()
            || (!binding.assignment.is_nil() && !binding.matches(status))
            || status["tenant_id"] != tenant.to_string()
            || status["mode"] != "container"
            || status["platform"] != "linux"
            || status["revoked"] != false
            || status["desired"] != "paused"
            || status["local_consent"] != false
            || self
                .bindings
                .iter()
                .any(|b| b.bot == binding.bot || b.secret == binding.secret)
        {
            return Err(Error::Invalid);
        }
        self.bindings.push(binding);
        self.save()
    }
    pub fn control(&self, scope: Scope, owner_token: &str, action: &str) -> Result<(), Error> {
        if !matches!(action, "resume" | "stop" | "revoke") {
            return Err(Error::Invalid);
        }
        let b = self.binding(scope)?;
        let status = post(
            &b.origin,
            b.tenant,
            owner_token,
            &format!("/{}/control", b.bot),
            json!({"action":action}),
        )?;
        if status["bot_id"] != b.bot.to_string() || status["tenant_id"] != b.tenant.to_string() {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub fn heartbeat(&self, scope: Scope, consent: bool) -> Result<Value, Error> {
        let b = self.binding(scope)?;
        let status = b.agent(
            "heartbeat",
            json!({"connection_id":b.connection,"local_consent":consent}),
        )?;
        if !b.matches(&status) {
            return Err(Error::Stale);
        }
        Ok(status)
    }
    pub fn authorized(&self, scope: Scope, status: &Value) -> Result<Duration, Error> {
        let b = self.binding(scope)?;
        let time = now()?;
        let expiry = status["assignment"]["expires_at"]
            .as_f64()
            .ok_or(Error::Stale)?;
        if !b.matches(status)
            || status["desired"] != "running"
            || status["local_consent"] != true
            || status["connection_id"] != b.connection.to_string()
            || !expiry.is_finite()
            || expiry <= time
        {
            return Err(Error::Stale);
        }
        Ok(Duration::from_secs_f64((expiry - time).min(5.0)))
    }
    pub fn claim(&self, scope: Scope) -> Result<Option<(Uuid, Value, f64)>, Error> {
        let b = self.binding(scope)?;
        let reply = b.agent("claim", json!({"connection_id":b.connection}))?;
        self.authorized(scope, &reply["bot"])?;
        if reply["task"].is_null() {
            return Ok(None);
        }
        let task = &reply["task"];
        let until = reply["authorized_until"].as_f64().ok_or(Error::Stale)?;
        let time = now()?;
        if !until.is_finite()
            || until <= time
            || until - time > 15.0
            || task["capability"] != "desktop"
            || task["session_id"] != b.session.to_string()
            || task["assignment_id"] != b.assignment.to_string()
            || task["connection_id"] != b.connection.to_string()
            || task["status"] != "delivered"
        {
            return Err(Error::Stale);
        }
        Ok(Some((
            parse_id(&task["id"])?,
            task["payload"].clone(),
            until,
        )))
    }
    pub fn complete(&self, scope: Scope, task: Uuid, status: &str) -> Result<(), Error> {
        if !matches!(status, "completed" | "failed" | "cancelled") {
            return Err(Error::Invalid);
        }
        let b = self.binding(scope)?;
        b.agent(
            "complete",
            json!({"connection_id":b.connection,"task_id":task,"status":status}),
        )?;
        Ok(())
    }
}
fn parse_id(value: &Value) -> Result<Uuid, Error> {
    let id = Uuid::parse_str(value.as_str().ok_or(Error::Invalid)?).map_err(|_| Error::Invalid)?;
    if id.is_nil() {
        Err(Error::Invalid)
    } else {
        Ok(id)
    }
}
fn post(service: &str, tenant: Uuid, token: &str, path: &str, body: Value) -> Result<Value, Error> {
    if token.is_empty() || token.len() > 16384 || token.bytes().any(|b| b <= 0x20 || b >= 0x7f) {
        return Err(Error::Invalid);
    }
    let auth = Zeroizing::new(format!("Bearer {token}"));
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .build();
    let bytes = Zeroizing::new(serde_json::to_vec(&body).map_err(|_| Error::Invalid)?);
    let response = agent
        .post(&format!("{service}/api/v1/hypermesh/bots{path}"))
        .set("Authorization", auth.as_str())
        .set("X-Tenant-ID", &tenant.to_string())
        .set("Content-Type", "application/json")
        .send_bytes(&bytes)
        .map_err(|error| match error {
            ureq::Error::Status(400..=499, _) => Error::Denied,
            _ => Error::Backend,
        })?;
    if !matches!(response.status(), 200 | 201) {
        return Err(Error::Backend);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .into_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Backend)?;
    if bytes.len() as u64 > LIMIT {
        return Err(Error::Invalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Scope {
        Scope::recovered(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
    }
    fn reply(scope: Scope, tenant: Uuid) -> Value {
        json!({"credential":format!("hm_bot_{}", "a".repeat(64)), "bot":{
            "bot_id":Uuid::new_v4(),"owner_id":Uuid::new_v4(),"tenant_id":tenant,
            "mode":"container","platform":"linux","revoked":false,"desired":"paused","local_consent":false,
            "assignment":{"id":Uuid::new_v4(),"session_id":scope.session(),"expires_at":now().unwrap()+60.0}}})
    }
    #[test]
    fn private_store_reopens_with_fresh_connection_and_never_reuses_identity() {
        let directory = std::env::temp_dir().join(format!("broker-unit-{}", Uuid::new_v4()));
        storage::create_dir(&directory).unwrap();
        let scope = scope();
        let tenant = Uuid::new_v4();
        let mut broker = Broker::open(&directory).unwrap();
        let response = reply(scope, tenant);
        broker
            .install(scope, "https://plane.test".into(), tenant, response.clone())
            .unwrap();
        let old = broker.binding(scope).unwrap().connection;
        assert!(broker
            .install(scope, "https://plane.test".into(), tenant, response)
            .is_err());
        let recovered = Broker::open(&directory).unwrap();
        assert_ne!(old, recovered.binding(scope).unwrap().connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(directory.join("broker.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn authorization_requires_exact_owner_scope_connection_consent_and_unexpired_grant() {
        let scope = scope();
        let tenant = Uuid::new_v4();
        let response = reply(scope, tenant);
        let status = &response["bot"];
        let binding = Binding {
            companion: scope.companion(),
            session: scope.session(),
            generation: scope.generation(),
            bot: parse_id(&status["bot_id"]).unwrap(),
            owner: parse_id(&status["owner_id"]).unwrap(),
            tenant,
            origin: "https://plane.test".into(),
            secret: response["credential"].as_str().unwrap().into(),
            assignment: parse_id(&status["assignment"]["id"]).unwrap(),
            connection: Uuid::new_v4(),
        };
        let mut current = status.clone();
        current["desired"] = json!("running");
        current["local_consent"] = json!(true);
        current["connection_id"] = json!(binding.connection);
        let broker = Broker {
            path: PathBuf::new(),
            bindings: vec![binding],
            poisoned: false,
        };
        assert!(broker.authorized(scope, &current).unwrap() <= Duration::from_secs(5));
        for key in ["bot_id", "tenant_id", "owner_id", "connection_id"] {
            let mut bad = current.clone();
            bad[key] = json!(Uuid::new_v4());
            assert!(broker.authorized(scope, &bad).is_err());
        }
        for (key, value) in [
            ("desired", json!("paused")),
            ("local_consent", json!(false)),
            ("revoked", json!(true)),
        ] {
            let mut bad = current.clone();
            bad[key] = value;
            assert!(broker.authorized(scope, &bad).is_err());
        }
        let mut bad = current.clone();
        bad["assignment"]["expires_at"] = json!(now().unwrap() - 1.0);
        assert!(broker.authorized(scope, &bad).is_err());
        bad = current.clone();
        bad["assignment"]["session_id"] = json!(Uuid::new_v4());
        assert!(broker.authorized(scope, &bad).is_err());
    }
    #[test]
    fn cancellation_missing_or_unknown_task_refuses_continued_execution() {
        let id = Uuid::new_v4();
        let good = json!({"tasks":[{"id":id,"status":"delivered","cancel_requested":false}]});
        assert!(Broker::task_live(&good, id));
        let mut absent = good.clone();
        absent["tasks"][0]
            .as_object_mut()
            .unwrap()
            .remove("cancel_requested");
        assert!(Broker::task_live(&absent, id));
        for status in ["unknown", "cancelled", "completed", "queued"] {
            let mut bad = good.clone();
            bad["tasks"][0]["status"] = json!(status);
            assert!(!Broker::task_live(&bad, id));
        }
        let mut bad = good.clone();
        bad["tasks"][0]["cancel_requested"] = json!(true);
        assert!(!Broker::task_live(&bad, id));
        assert!(!Broker::task_live(&good, Uuid::new_v4()));
        assert!(!Broker::task_live(&json!({}), id));
    }
    #[test]
    fn deleting_binding_preserves_other_scope_and_reclaims_persisted_capacity() {
        let directory = std::env::temp_dir().join(format!("broker-delete-unit-{}", Uuid::new_v4()));
        storage::create_dir(&directory).unwrap();
        let mut broker = Broker::open(&directory).unwrap();
        let first = scope();
        let second = scope();
        let tenant = Uuid::new_v4();
        broker
            .install(
                first,
                "https://plane.test".into(),
                tenant,
                reply(first, tenant),
            )
            .unwrap();
        let mut other = reply(second, tenant);
        other["credential"] = json!(format!("hm_bot_{}", "b".repeat(64)));
        broker
            .install(second, "https://plane.test".into(), tenant, other)
            .unwrap();
        broker.remove(first).unwrap();
        let mut reopened = Broker::open(&directory).unwrap();
        assert!(!reopened.enrolled(first));
        assert!(reopened.enrolled(second));
        reopened.retain(&[]).unwrap();
        assert!(Broker::open(&directory).unwrap().bindings.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn origins_cannot_redirect_owner_credentials_to_plaintext_or_url_credentials() {
        for value in [
            "http://localhost",
            "https://user:password@plane.test",
            "https://plane.test/path",
            "https://plane.test?token=x",
        ] {
            assert!(origin(value).is_err());
        }
        assert_eq!(origin("https://plane.test/").unwrap(), "https://plane.test");
    }
}
