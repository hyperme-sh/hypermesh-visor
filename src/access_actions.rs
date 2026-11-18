//! Bound typed executors. No shell, redirects, overwrites or privileged file access.
use crate::local_reads::ERROR;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    time::Duration,
};

pub fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub request_id: uuid::Uuid,
    pub session_id: uuid::Uuid,
    pub selection: crate::permission_templates::Selection,
    pub operation: crate::permission_templates::Operation,
    pub target: String,
    pub max_bytes: u64,
    pub timeout_seconds: u32,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub content_base64: Option<String>,
}
pub fn action(operation: &str, target: &str, arguments: Value, max: u64, seconds: u32) -> Value {
    let raw = serde_json::to_string(&arguments).unwrap();
    json!({"operation":operation,"target":target,"arguments_sha256":digest(raw.as_bytes()),"arguments_json":raw,"limits":{"max_bytes":max,"timeout_seconds":seconds}})
}
pub struct Network {
    pub url: String,
    pub method: String,
    pub addresses: Vec<SocketAddr>,
    pub body: Vec<u8>,
}
impl Network {
    pub fn prepare(target: &str, method: &str, body: Vec<u8>) -> Result<Self, &'static str> {
        let url = url::Url::parse(target).map_err(|_| ERROR)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.as_str() != target
            || target.contains(['%', '*', '\\'])
            || !matches!(method, "GET" | "POST")
            || method == "GET" && !body.is_empty()
        {
            return Err(ERROR);
        }
        let addresses: Vec<_> = (url.host_str().unwrap(), 443)
            .to_socket_addrs()
            .map_err(|_| ERROR)?
            .collect();
        Self::bound(target.into(), method.into(), body, addresses)
    }
    fn bound(
        url: String,
        method: String,
        body: Vec<u8>,
        mut addresses: Vec<SocketAddr>,
    ) -> Result<Self, &'static str> {
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty()
            || addresses.len() > 16
            || addresses.iter().any(|a| !public(a.ip()) || a.port() != 443)
        {
            return Err(ERROR);
        }
        Ok(Self {
            url,
            method,
            addresses,
            body,
        })
    }
    pub fn arguments(&self) -> Value {
        json!({"method":self.method,"addresses":self.addresses.iter().map(|a|a.ip().to_string()).collect::<Vec<_>>(),"body_sha256":digest(&self.body),"body_bytes":self.body.len()})
    }
    pub fn execute(
        self,
        max: u64,
        seconds: u32,
        valid: impl Fn() -> bool,
    ) -> Result<Value, &'static str> {
        if !valid() {
            return Err(ERROR);
        }
        let url = url::Url::parse(&self.url).map_err(|_| ERROR)?;
        let authority = format!("{}:443", url.host_str().ok_or(ERROR)?);
        let addresses = self.addresses.clone();
        let agent = ureq::AgentBuilder::new()
            .https_only(true)
            .try_proxy_from_env(false)
            .redirects(0)
            .timeout(Duration::from_secs(seconds as u64))
            .resolver(move |host: &str| {
                if host == authority {
                    Ok(addresses.clone())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Unreviewed destination",
                    ))
                }
            })
            .build();
        if !valid() {
            return Err(ERROR);
        }
        let response = agent
            .request(&self.method, &self.url)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(&self.body)
            .map_err(|_| ERROR)?;
        finish_response(response, max, valid)
    }
}
fn finish_response(
    response: ureq::Response,
    max: u64,
    valid: impl Fn() -> bool,
) -> Result<Value, &'static str> {
    if !(200..300).contains(&response.status()) || !valid() {
        return Err(ERROR);
    }
    let mut output = Vec::new();
    response
        .into_reader()
        .take(max + 1)
        .read_to_end(&mut output)
        .map_err(|_| ERROR)?;
    if output.len() as u64 > max || !valid() {
        return Err(ERROR);
    }
    use base64::Engine;
    Ok(
        json!({"completed":true,"content_base64":base64::engine::general_purpose::STANDARD.encode(output)}),
    )
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_unspecified()
                && a != 0
                && a < 224
                && !(a == 100 && (64..128).contains(&b))
                && !(a == 192 && b == 0)
                && !(a == 198 && (b == 18 || b == 19))
                && !ip.is_documentation()
        }
        IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && ip.to_ipv4_mapped().is_none()
                && ip.segments()[0] & 0xe000 == 0x2000
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0xdb8)
        }
    }
}

#[cfg(target_os = "linux")]
pub struct NewFile {
    parent: std::fs::File,
    project: String,
    target: String,
    name: std::ffi::CString,
    parent_mark: Value,
    content: Vec<u8>,
}
#[cfg(target_os = "linux")]
impl NewFile {
    pub fn prepare(project: &str, target: &str, content: Vec<u8>) -> Result<Self, &'static str> {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        if project == "/"
            || !project.starts_with('/')
            || project
                .split('/')
                .skip(1)
                .any(|s| s.is_empty() || s == "." || s == "..")
            || [project, target]
                .iter()
                .any(|s| s.contains(['*', '?', '\\']) || s.chars().any(char::is_control))
        {
            return Err(ERROR);
        }
        let path = std::path::Path::new(target);
        if path.parent() != Some(std::path::Path::new(project)) {
            return Err(ERROR);
        }
        let name = CString::new(path.file_name().and_then(|n| n.to_str()).ok_or(ERROR)?)
            .map_err(|_| ERROR)?;
        if name.as_bytes() == b"." || name.as_bytes() == b".." {
            return Err(ERROR);
        }
        let mut parent = std::fs::File::open("/").map_err(|_| ERROR)?;
        for part in project.split('/').skip(1) {
            let part = CString::new(part).map_err(|_| ERROR)?;
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    part.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(ERROR);
            }
            parent = unsafe { std::fs::File::from_raw_fd(fd) };
        }
        let parent_mark = directory_mark(&parent)?;
        let file = Self {
            parent,
            project: project.into(),
            target: target.into(),
            name,
            parent_mark,
            content,
        };
        file.absent()?;
        Ok(file)
    }
    fn absent(&self) -> Result<(), &'static str> {
        use std::os::fd::AsRawFd;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let r = unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
            return Err(ERROR);
        }
        Ok(())
    }
    pub fn arguments(&self) -> Value {
        json!({"content_sha256":digest(&self.content),"content_bytes":self.content.len(),"expected_object_sha256":digest(b"absent"),"parent_object_sha256":digest(&serde_json::to_vec(&self.parent_mark).unwrap())})
    }
    pub fn execute(self, valid: impl Fn() -> bool) -> Result<Value, &'static str> {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        let current = Self::prepare(&self.project, &self.target, Vec::new())?;
        if current.parent_mark != self.parent_mark || !valid() {
            return Err(ERROR);
        }
        let temporary = CString::new(format!(".visor-write-{}", uuid::Uuid::new_v4())).unwrap();
        let fd = unsafe {
            libc::openat(
                self.parent.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(ERROR);
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let result = (|| {
            file.write_all(&self.content)
                .and_then(|_| file.sync_all())
                .map_err(|_| ERROR)?;
            self.absent()?;
            if !valid() || directory_mark(&self.parent)? != self.parent_mark {
                return Err(ERROR);
            }
            let current = Self::prepare(&self.project, &self.target, Vec::new())?;
            if current.parent_mark != self.parent_mark {
                return Err(ERROR);
            }
            // Atomic create-only publication: an intervening file or symlink wins and
            // is never overwritten. The reviewed parent descriptor owns this operation.
            let r = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    self.parent.as_raw_fd(),
                    temporary.as_ptr(),
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if r != 0 {
                return Err(ERROR);
            }
            self.parent.sync_all().map_err(|_| ERROR)?;
            Ok(json!({"completed":true,"bytes_written":self.content.len()}))
        })();
        if result.is_err() {
            unsafe {
                libc::unlinkat(self.parent.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result
    }
}
#[cfg(target_os = "linux")]
fn directory_mark(file: &std::fs::File) -> Result<Value, &'static str> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_dir() {
        return Err(ERROR);
    }
    Ok(json!([m.dev(), m.ino(), m.uid(), m.gid(), m.mode()]))
}

/// All modes use the private Host bridge; admission consumes the request ID in
/// Conductor's persistent dispatch journal before any side effect is attempted.
#[cfg(unix)]
pub fn perform(
    dir: &std::path::Path,
    body: Input,
    identity: &Value,
    workspace: &str,
    mut review: impl FnMut(Value, u8) -> Result<Value, &'static str>,
    valid: impl Fn() -> bool,
) -> Result<Value, &'static str> {
    use crate::{
        local_reads as reads,
        permission_templates::{self, Operation},
    };
    use base64::Engine;
    use hypermesh_local_storage as storage;
    body.selection.validate()?;
    if body.request_id.is_nil()
        || body.session_id.is_nil()
        || body.max_bytes == 0
        || body.max_bytes > 1048576
        || body.timeout_seconds == 0
        || body.timeout_seconds > 30
        || !valid()
    {
        return Err(ERROR);
    }
    let guard = storage::open(&dir.join("lock"), storage::Open::Lock).map_err(|_| ERROR)?;
    storage::lock(&guard).map_err(|_| ERROR)?;
    let _unlock = storage::UnlockOnDrop(&guard);
    if !permission_templates::load(dir)?
        .selections
        .contains(&body.selection)
        || workspace != body.selection.scope.project
        || identity["session_id"] != body.session_id.to_string()
        || identity["organization_id"] != body.selection.scope.organization.to_string()
        || identity["device_id"] != body.selection.scope.device.to_string()
    {
        return Err(ERROR);
    }
    let content = match &body.content_base64 {
        Some(s) if s.len() <= 1398104 => base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|_| ERROR)?,
        None => Vec::new(),
        _ => return Err(ERROR),
    };
    if content.len() as u64 > body.max_bytes {
        return Err(ERROR);
    }
    enum Prepared {
        Network(Network),
        #[cfg(target_os = "linux")]
        File(NewFile),
        Service,
    }
    let (prepared, args, operation) = match body.operation {
        Operation::Https => {
            let network =
                Network::prepare(&body.target, body.method.as_deref().ok_or(ERROR)?, content)?;
            let args = network.arguments();
            (Prepared::Network(network), args, "https")
        }
        #[cfg(target_os = "linux")]
        Operation::Write => {
            if body.method.is_some() || body.content_base64.is_none() {
                return Err(ERROR);
            }
            let file = NewFile::prepare(workspace, &body.target, content)?;
            let args = file.arguments();
            (Prepared::File(file), args, "write")
        }
        Operation::ServiceRestart => {
            if body.method.is_some() || body.content_base64.is_some() {
                return Err(ERROR);
            }
            let context = review(Value::Null, 2)?;
            if !context["units"]
                .as_array()
                .ok_or(ERROR)?
                .contains(&json!(body.target))
            {
                return Err(ERROR);
            }
            (
                Prepared::Service,
                json!({"verb":"restart","unit":body.target,"uid":context["uid"],"executable":context["executable"],"executable_sha256":context["executable_sha256"],"controller_binding_sha256":context["controller_binding_sha256"]}),
                "service_restart",
            )
        }
        _ => return Err(ERROR),
    };
    let action = action(
        operation,
        &body.target,
        args,
        body.max_bytes,
        body.timeout_seconds,
    );
    let mut store = reads::load(dir)?;
    let previous = store
        .allows
        .iter()
        .find(|a| {
            a.identity == *identity
                && a.project == workspace
                && a.action == action
                && a.expires_at > reads::now()
        })
        .cloned();
    let rules:Vec<Value>=body.selection.rules.iter().map(|r|json!({"operation":r.operation,"target":r.target,"limits":{"max_bytes":r.max_bytes,"timeout_seconds":r.timeout_seconds}})).collect();
    let mut request = json!({"request_id":body.request_id,"project":workspace,"template_version":1,"template":rules,"action":action});
    if let Some(p) = &previous {
        request["expected_policy_revision"] = json!(p.policy_revision);
    }
    let first = review(request.clone(), 0)?;
    reads::verify(&first, &request, identity)?;
    if !valid() {
        return Err(ERROR);
    }
    request["expected_policy_revision"] = first["policy_revision"].clone();
    let (mut output, admitted) = match prepared {
        Prepared::Service => {
            let output = review(request.clone(), 3)?;
            let admitted = output["review"].clone();
            reads::verify(&admitted, &request, identity)?;
            if output["completed"] != true || admitted["reserved"] != true {
                return Err(ERROR);
            }
            (json!({"completed":true,"unit":body.target}), admitted)
        }
        other => {
            let admitted = review(request.clone(), 1)?;
            reads::verify(&admitted, &request, identity)?;
            if admitted["reserved"] != true || !valid() {
                return Err(ERROR);
            }
            let expires = admitted["expires_at"].as_f64().ok_or(ERROR)?;
            let current = || valid() && reads::now() < expires;
            let output = match other {
                Prepared::Network(network) => {
                    network.execute(body.max_bytes, body.timeout_seconds, current)?
                }
                #[cfg(target_os = "linux")]
                Prepared::File(file) => file.execute(current)?,
                Prepared::Service => unreachable!(),
            };
            (output, admitted)
        }
    };
    // Caching is optional after completion. Never misreport a completed write
    // as unperformed merely because its local allow metadata could not save.
    let learn = valid() && admitted["learning_eligible"] == true;
    let pending =
        admitted["decision"] == "advisory" || previous.as_ref().is_some_and(|a| a.review_pending);
    store
        .allows
        .retain(|a| !(a.identity == *identity && a.project == workspace && a.action == action));
    if learn {
        store.allows.push(reads::Learned {
            identity: identity.clone(),
            project: workspace.into(),
            action,
            policy_revision: admitted["policy_revision"].as_str().ok_or(ERROR)?.into(),
            origin: "conductor_aiuditor_verified".into(),
            last_use: reads::now(),
            expires_at: reads::now() + 86400.,
            review_pending: pending,
        });
    }
    let saved = reads::save(dir, &store).is_ok();
    output["request_id"] = json!(body.request_id);
    output["learned"] = json!(learn && saved);
    output["reused"] = json!(learn && saved && previous.is_some());
    output["review_pending"] = json!(pending);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("visor-actions-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn destinations_are_public_and_bound_in_arguments() {
        for ip in [
            "127.0.0.1",
            "10.1.1.1",
            "169.254.169.254",
            "100.64.1.2",
            "224.0.0.1",
            "::1",
            "::ffff:8.8.8.8",
            "fc00::1",
        ] {
            assert!(Network::bound(
                "https://example.com/".into(),
                "GET".into(),
                vec![],
                vec![SocketAddr::new(ip.parse().unwrap(), 443)]
            )
            .is_err());
        }
        let network = Network::bound(
            "https://example.com/".into(),
            "POST".into(),
            b"payload".to_vec(),
            vec!["8.8.8.8:443".parse().unwrap()],
        )
        .unwrap();
        assert_eq!(network.arguments()["addresses"], json!(["8.8.8.8"]));
        assert_eq!(network.arguments()["body_sha256"], digest(b"payload"));
        assert!(network.execute(100, 1, || false).is_err());
        for url in [
            "http://example.com/",
            "https://user:secret@example.com/",
            "https://example.com:444/",
            "https://example.com/%2e%2e/",
            "https://example.com/?secret=x",
        ] {
            assert!(Network::prepare(url, "GET", vec![]).is_err());
        }
    }
    #[test]
    fn named_http_response_contract_refuses_redirects_and_oversize() {
        let permitted =
            finish_response(ureq::Response::new(200, "OK", "hello").unwrap(), 5, || true).unwrap();
        assert_eq!(permitted["completed"], true);
        assert_eq!(permitted["content_base64"], "aGVsbG8=");
        for status in [301, 302, 303, 307, 308] {
            assert!(finish_response(
                ureq::Response::new(status, "Redirect", "").unwrap(),
                5,
                || true
            )
            .is_err());
        }
        assert!(finish_response(
            ureq::Response::new(200, "OK", "too large").unwrap(),
            5,
            || true
        )
        .is_err());
        assert!(
            finish_response(ureq::Response::new(200, "OK", "hello").unwrap(), 5, || {
                false
            })
            .is_err()
        );
    }
    #[test]
    fn arguments_bind_operation_target_and_content() {
        let a = action(
            "write",
            "/project/new",
            json!({"content_sha256":digest(b"a")}),
            100,
            2,
        );
        let b = action(
            "write",
            "/project/new",
            json!({"content_sha256":digest(b"b")}),
            100,
            2,
        );
        assert_ne!(a["arguments_sha256"], b["arguments_sha256"]);
        assert_eq!(
            a["arguments_sha256"],
            digest(a["arguments_json"].as_str().unwrap().as_bytes())
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn write_is_create_only_and_never_follows_links() {
        let dir = TempDir::new();
        let root = dir.path().canonicalize().unwrap();
        let project = root.to_str().unwrap();
        let target = root.join("new");
        let file = NewFile::prepare(project, target.to_str().unwrap(), b"one".to_vec()).unwrap();
        assert_eq!(file.execute(|| true).unwrap()["bytes_written"], 3);
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        assert!(NewFile::prepare(project, target.to_str().unwrap(), b"two".to_vec()).is_err());
        let race = root.join("race");
        let file = NewFile::prepare(project, race.to_str().unwrap(), b"bad".to_vec()).unwrap();
        std::os::unix::fs::symlink(&target, &race).unwrap();
        assert!(file.execute(|| true).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        let cancel = root.join("cancel");
        let file = NewFile::prepare(project, cancel.to_str().unwrap(), vec![1]).unwrap();
        assert!(file.execute(|| false).is_err());
        assert!(!cancel.exists());
        assert!(
            NewFile::prepare(project, root.join("../escape").to_str().unwrap(), vec![]).is_err()
        );
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert!(NewFile::prepare(
            alias.to_str().unwrap(),
            alias.join("newer").to_str().unwrap(),
            vec![]
        )
        .is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn parent_moved_during_write_is_refused() {
        let dir = TempDir::new();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let file = NewFile::prepare(
            project.to_str().unwrap(),
            project.join("new").to_str().unwrap(),
            b"content".to_vec(),
        )
        .unwrap();
        let calls = std::cell::Cell::new(0);
        assert!(file
            .execute(|| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    std::fs::rename(&project, root.join("moved")).unwrap();
                    std::fs::create_dir(&project).unwrap();
                }
                true
            })
            .is_err());
        assert!(!project.join("new").exists());
        assert!(!root.join("moved/new").exists());
        assert_eq!(std::fs::read_dir(root.join("moved")).unwrap().count(), 0);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn changed_parent_is_refused() {
        let dir = TempDir::new();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let file = NewFile::prepare(
            project.to_str().unwrap(),
            project.join("new").to_str().unwrap(),
            vec![1],
        )
        .unwrap();
        std::fs::rename(&project, root.join("old")).unwrap();
        std::fs::create_dir(&project).unwrap();
        assert!(file.execute(|| true).is_err());
        assert!(!project.join("new").exists());
        assert!(!root.join("old/new").exists());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod admission_contract {
    use super::*;
    use crate::{local_reads as reads, permission_templates::*};
    use hypermesh_local_storage as storage;
    #[test]
    fn write_requires_matching_current_admission_and_saves_only_metadata() {
        let root =
            std::env::temp_dir().join(format!("visor-action-contract-{}", uuid::Uuid::new_v4()));
        storage::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let dir = root.join("store");
        storage::create_dir(&dir).unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let target = project.join("new");
        let mut identity = json!({"principal":"owner"});
        for key in [
            "tenant_id",
            "owner_id",
            "organization_id",
            "device_id",
            "bot_id",
            "assignment_id",
            "session_id",
        ] {
            identity[key] = json!(uuid::Uuid::new_v4());
        }
        let selection = Selection::new(
            Scope {
                organization: uuid::Uuid::parse_str(identity["organization_id"].as_str().unwrap())
                    .unwrap(),
                device: uuid::Uuid::parse_str(identity["device_id"].as_str().unwrap()).unwrap(),
                project: project.to_str().unwrap().into(),
            },
            Preset::ProjectDevelopment,
            target.to_str().unwrap().into(),
        )
        .unwrap();
        let mut prefs = Preferences::empty();
        prefs.select(selection.clone()).unwrap();
        save(&dir, &Preferences::empty(), &prefs).unwrap();
        let input = Input {
            request_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::parse_str(identity["session_id"].as_str().unwrap()).unwrap(),
            selection,
            operation: Operation::Write,
            target: target.to_str().unwrap().into(),
            max_bytes: 1024,
            timeout_seconds: 10,
            method: None,
            content_base64: Some("cHJpdmF0ZSBjb250ZW50".into()),
        };
        let revision = uuid::Uuid::new_v4();
        let response = |r: &Value, reserved: bool| json!({"version":1,"request_id":r["request_id"],"project":r["project"],"action":r["action"],"limits":r["action"]["limits"],"preview_only":false,"identity":identity,"permitted":true,"decision":"advisory","learning_eligible":true,"policy_revision":revision,"expires_at":reads::now()+9.0,"reserved":reserved});
        let failed = perform(
            &dir,
            input.clone(),
            &identity,
            project.to_str().unwrap(),
            |r, _| Ok(response(&r, false)),
            || true,
        );
        assert!(failed.is_err());
        assert!(!target.exists());
        let mut modes = Vec::new();
        let result = perform(
            &dir,
            input,
            &identity,
            project.to_str().unwrap(),
            |r, mode| {
                modes.push(mode);
                if mode == 1 {
                    assert_eq!(r["expected_policy_revision"], json!(revision));
                }
                Ok(response(&r, mode == 1))
            },
            || true,
        )
        .unwrap();
        assert_eq!(modes, vec![0, 1]);
        assert_eq!(result["completed"], true);
        assert_eq!(result["learned"], true);
        assert_eq!(result["review_pending"], true);
        assert_eq!(std::fs::read(&target).unwrap(), b"private content");
        let store = reads::load(&dir).unwrap();
        assert!(store.requests.is_empty());
        assert_eq!(store.allows.len(), 1);
        assert!(!serde_json::to_string(&store)
            .unwrap()
            .contains("private content"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
