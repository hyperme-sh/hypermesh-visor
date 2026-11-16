//! Restricted local Docker adapter. It never installs a backend, pulls an
//! image, repairs permissions, or accepts an arbitrary container specification.
use crate::companion_manager::Error;
use crate::companion_recovery::{Cleanup, Owned};
use serde_json::Value;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

const CLAIM: &str = "hypermesh-companion-manager";
const OWNER: &str = "sh.hypermesh.companion.owner";
const COMPANION: &str = "sh.hypermesh.companion.id";
const SESSION: &str = "sh.hypermesh.companion.session";
const GENERATION: &str = "sh.hypermesh.companion.generation";
const OUTPUT_LIMIT: u64 = 1024 * 1024;

pub trait Runner {
    fn run(&mut self, arguments: &[String]) -> Result<Vec<u8>, Error>;
}
/// An explicit Unix engine address prevents inherited Docker context/TLS
/// variables from selecting a remote engine with different host mount paths.
pub struct LocalRunner {
    directory: PathBuf,
}
impl LocalRunner {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}
impl Runner for LocalRunner {
    fn run(&mut self, arguments: &[String]) -> Result<Vec<u8>, Error> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Backend);
        }
        let path = self.directory.join(format!(".docker-{}", Uuid::new_v4()));
        let result = (|| {
            let output = hypermesh_local_storage::open(&path, hypermesh_local_storage::Open::New)
                .map_err(|_| Error::Backend)?;
            let mut child = Command::new("/usr/bin/docker")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .arg("--host")
                .arg("unix:///var/run/docker.sock")
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::from(output.try_clone().map_err(|_| Error::Backend)?))
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| Error::Backend)?;
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                let over = output
                    .metadata()
                    .map(|m| m.len() > OUTPUT_LIMIT)
                    .unwrap_or(true);
                if over || Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Killing the CLI is not an acknowledgement from Docker.
                    // The journal must keep the uncertain allocation reserved.
                    return Err(Error::Backend);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Error::Backend);
                    }
                }
            };
            if !status.success() {
                return Err(Error::Backend);
            }
            let mut bytes = Vec::new();
            File::open(&path)
                .map_err(|_| Error::Backend)?
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::Backend)?;
            if bytes.len() as u64 > OUTPUT_LIMIT {
                return Err(Error::Backend);
            }
            Ok(bytes)
        })();
        let _ = std::fs::remove_file(path);
        result
    }
}
fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).into()).collect()
}
fn id(bytes: &[u8]) -> Result<String, Error> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Backend)?
        .trim();
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Backend);
    }
    Ok(text.into())
}
fn image_valid(image: &str) -> bool {
    // Select only this project's published image by channel/version, never an
    // arbitrary command image or digest. The installed image is used as-is.
    image
        .strip_prefix("ghcr.io/hyperme-sh/hypermesh-visor:")
        .is_some_and(|tag| {
            !tag.is_empty()
                && tag.len() <= 128
                && tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}
pub struct Docker<R: Runner> {
    runner: R,
    owner: Uuid,
    image: String,
}
impl<R: Runner> Docker<R> {
    pub fn new(runner: R, owner: Uuid, image: String) -> Result<Self, Error> {
        if owner.is_nil() || !image_valid(&image) {
            return Err(Error::Invalid);
        }
        Ok(Self {
            runner,
            owner,
            image,
        })
    }
    /// A fixed engine-wide claim prevents another configuration or OS account
    /// from admitting another four desktops on the same supported engine.
    pub fn claim(&mut self) -> Result<(), Error> {
        if let Some(found) = self.container(CLAIM)? {
            return if label(&found, OWNER) == Some(self.owner.to_string().as_str()) {
                Ok(())
            } else {
                Err(Error::Backend)
            };
        }
        let arguments = args(&[
            "container",
            "create",
            "--pull=never",
            "--name",
            CLAIM,
            "--label",
            &format!("{OWNER}={}", self.owner),
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--entrypoint=/bin/false",
            &self.image,
        ]);
        // Atomic Docker name creation wins or refuses. It is never started.
        id(&self.runner.run(&arguments)?)?;
        Ok(())
    }
    fn container(&mut self, name: &str) -> Result<Option<Value>, Error> {
        let output = self.runner.run(&args(&[
            "container",
            "ls",
            "--all",
            "--no-trunc",
            "--filter",
            &format!("name=^/{name}$"),
            "--format",
            "{{.ID}}",
        ]))?;
        if output.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        let exact = id(&output)?;
        let output = self.runner.run(&args(&["container", "inspect", &exact]))?;
        let rows: Vec<Value> = serde_json::from_slice(&output).map_err(|_| Error::Backend)?;
        let value = rows.into_iter().next().ok_or(Error::Backend)?;
        if value["Id"].as_str() != Some(exact.as_str())
            || value["Name"].as_str() != Some(format!("/{name}").as_str())
        {
            return Err(Error::Backend);
        }
        Ok(Some(value))
    }
    fn owned_container(&mut self, a: Owned) -> Result<Option<String>, Error> {
        let Some(value) = self.container(&a.container())? else {
            return Ok(None);
        };
        if !owned_labels(&value["Config"]["Labels"], self.owner, a) {
            return Err(Error::Backend);
        }
        Ok(Some(id(value["Id"]
            .as_str()
            .ok_or(Error::Backend)?
            .as_bytes())?))
    }
    pub fn create(&mut self, a: Owned, control: &Path) -> Result<(), Error> {
        if self.owned_container(a)?.is_some() {
            return Err(Error::Backend);
        }
        let mut volume = args(&["volume", "create", "--driver=local"]);
        labels(&mut volume, self.owner, a);
        volume.push(a.home());
        // A preexisting volume must never become a new companion's home.
        if self.home_exists(a)? {
            return Err(Error::Backend);
        }
        let created = self.runner.run(&volume)?;
        if std::str::from_utf8(&created).ok().map(str::trim) != Some(a.home().as_str()) {
            return Err(Error::Backend);
        }
        let command = create_arguments(self.owner, a, control, &self.image)?;
        let created = id(&self.runner.run(&command)?)?;
        if self.owned_container(a)?.as_deref() != Some(&created) {
            return Err(Error::Backend);
        }
        self.runner.run(&args(&["container", "start", &created]))?;
        Ok(())
    }
    pub fn present(&mut self, a: Owned) -> Result<bool, Error> {
        Ok(self.owned_container(a)?.is_some())
    }
    pub fn guest(&mut self, a: Owned, action: &str) -> Result<Value, Error> {
        if !matches!(action, "status" | "resume" | "pause" | "view") {
            return Err(Error::Invalid);
        }
        let exact = self.owned_container(a)?.ok_or(Error::Stale)?;
        let output = self.runner.run(&args(&[
            "container",
            "exec",
            "--user=1000:1000",
            &exact,
            "/usr/bin/python3",
            "-I",
            "-c",
            GUEST_BRIDGE,
            action,
            &a.session.to_string(),
        ]))?;
        serde_json::from_slice(&output).map_err(|_| Error::Backend)
    }
    fn home_exists(&mut self, a: Owned) -> Result<bool, Error> {
        let output = self.runner.run(&args(&[
            "volume",
            "ls",
            "--filter",
            &format!("name=^{}$", a.home()),
            "--format",
            "{{.Name}}",
        ]))?;
        let text = std::str::from_utf8(&output)
            .map_err(|_| Error::Backend)?
            .trim();
        if text.is_empty() {
            return Ok(false);
        }
        if text != a.home() {
            return Err(Error::Backend);
        }
        let output = self.runner.run(&args(&["volume", "inspect", &a.home()]))?;
        let rows: Vec<Value> = serde_json::from_slice(&output).map_err(|_| Error::Backend)?;
        if rows.len() != 1
            || rows[0]["Name"].as_str() != Some(a.home().as_str())
            || !owned_labels(&rows[0]["Labels"], self.owner, a)
        {
            return Err(Error::Backend);
        }
        Ok(true)
    }
}
impl<R: Runner> Cleanup for Docker<R> {
    fn remove_desktop(&mut self, owner: Uuid, a: Owned) -> Result<(), Error> {
        if owner != self.owner {
            return Err(Error::Stale);
        }
        if let Some(exact) = self.owned_container(a)? {
            // Remove by immutable full ID, never a reusable name. Docker rm -f
            // acknowledges process termination and removal together.
            self.runner
                .run(&args(&["container", "rm", "--force", &exact]))?;
            if self.owned_container(a)?.is_some() {
                return Err(Error::Backend);
            }
        }
        Ok(())
    }
    fn remove_home(&mut self, owner: Uuid, a: Owned) -> Result<(), Error> {
        if owner != self.owner {
            return Err(Error::Stale);
        }
        if self.owned_container(a)?.is_some() {
            return Err(Error::Backend);
        }
        if self.home_exists(a)? {
            self.runner.run(&args(&["volume", "rm", &a.home()]))?;
        }
        Ok(())
    }
}
fn label<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value["Config"]["Labels"][name].as_str()
}
fn owned_labels(value: &Value, owner: Uuid, a: Owned) -> bool {
    [
        (OWNER, owner),
        (COMPANION, a.companion),
        (SESSION, a.session),
        (GENERATION, a.generation),
    ]
    .iter()
    .all(|(key, id)| value[*key].as_str() == Some(id.to_string().as_str()))
}
fn labels(arguments: &mut Vec<String>, owner: Uuid, a: Owned) {
    for (key, value) in [
        (OWNER, owner),
        (COMPANION, a.companion),
        (SESSION, a.session),
        (GENERATION, a.generation),
    ] {
        arguments.push("--label".into());
        arguments.push(format!("{key}={value}"));
    }
}
fn create_arguments(
    owner: Uuid,
    a: Owned,
    control: &Path,
    image: &str,
) -> Result<Vec<String>, Error> {
    let source = control.to_str().ok_or(Error::Invalid)?;
    if !control.is_absolute()
        || source.contains([',', '\n', '\r'])
        || !image_valid(image)
        || a.memory_mib < 512
        || a.cpu_millis < 100
        || a.memory_mib > 32768
        || a.cpu_millis > 32000
    {
        return Err(Error::Invalid);
    }
    let memory = format!("{}m", a.memory_mib);
    let cpu = format!("{}.{:03}", a.cpu_millis / 1000, a.cpu_millis % 1000);
    let mut command = args(&[
        "container",
        "create",
        "--pull=never",
        "--name",
        &a.container(),
        "--user=1000:1000",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--network=none",
        "--restart=no",
        "--log-driver=none",
        "--pids-limit=512",
        "--shm-size=256m",
        "--memory",
        &memory,
        "--memory-swap",
        &memory,
        "--cpus",
        &cpu,
        "--tmpfs",
        "/run/visor:rw,nosuid,nodev,uid=1000,gid=1000,mode=0700,size=64m",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,mode=1777,size=256m",
        "--mount",
        &format!("type=volume,src={},dst=/home/bot", a.home()),
        "--mount",
        &format!("type=bind,src={source},dst=/run/manager,readonly,bind-propagation=rprivate"),
        "--env",
        &format!("HYPERMESH_MANAGED_COMPANION={}", a.companion),
        "--env",
        &format!("HYPERMESH_MANAGED_SESSION={}", a.session),
        "--env",
        &format!("HYPERMESH_MANAGED_GENERATION={}", a.generation),
    ]);
    labels(&mut command, owner, a);
    command.push(image.into());
    Ok(command)
}

// No shell, user code, bearer, prompt, or arbitrary URL crosses this bridge.
const GUEST_BRIDGE: &str = r#"
import base64,http.client,json,socket,sys
class Local(http.client.HTTPConnection):
 def connect(self):
  self.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
  self.sock.settimeout(2)
  self.sock.connect('/run/visor/hypermesh/companion.sock')
def call(method,path,body=None):
 c=Local('visor',timeout=2)
 try:
  c.request(method,path,None if body is None else json.dumps(body),{'Content-Type':'application/json'})
  r=c.getresponse(); raw=r.read(700001)
  if r.status not in (200,204) or len(raw)>700000: raise RuntimeError('managed desktop refused')
  if r.getheader('Content-Type','').startswith('image/'):
   return {'png':base64.b64encode(raw).decode('ascii')}
  return json.loads(raw) if raw else {}
 finally: c.close()
a,session=sys.argv[1:]
if a=='status': result=call('GET','/companion/managed')
elif a=='resume':
 status=call('GET','/companion/managed')
 if status['version']!=1 or status['session']!=session: raise RuntimeError('managed scope changed')
 result=call('POST','/companion/automation',{'action':'resume','expected_session':session,'expected_epoch':status['epoch']})
elif a=='pause': result=call('POST','/companion/managed/cancel')
elif a=='view': result=call('POST','/session/'+session+'/view',{})
else: raise RuntimeError('invalid operation')
print(json.dumps(result))
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        replies: std::collections::VecDeque<Result<Vec<u8>, Error>>,
        calls: Vec<Vec<String>>,
    }
    impl Runner for Fake {
        fn run(&mut self, a: &[String]) -> Result<Vec<u8>, Error> {
            self.calls.push(a.to_vec());
            self.replies.pop_front().unwrap_or(Err(Error::Backend))
        }
    }
    fn owned() -> Owned {
        Owned {
            companion: Uuid::new_v4(),
            session: Uuid::new_v4(),
            generation: Uuid::new_v4(),
            memory_mib: 2048,
            cpu_millis: 1500,
        }
    }
    const IMAGE: &str = "ghcr.io/hyperme-sh/hypermesh-visor:test";
    #[test]
    fn fixed_spec_has_private_scopes_limits_and_no_host_control() {
        let owner = Uuid::new_v4();
        let a = owned();
        let b = owned();
        let first = create_arguments(owner, a, Path::new("/private/a"), IMAGE).unwrap();
        let second = create_arguments(owner, b, Path::new("/private/b"), IMAGE).unwrap();
        for flag in [
            "--read-only",
            "--cap-drop=ALL",
            "--network=none",
            "--restart=no",
            "--pull=never",
            "--user=1000:1000",
        ] {
            assert!(first.iter().any(|a| a == flag));
        }
        assert!(first.contains(&"1.500".into()));
        assert!(!first.iter().any(|s| s.contains("docker.sock")
            || s.contains("/dev/uinput")
            || s.contains("X11-unix")));
        assert!(first.iter().any(|s| s.contains(&a.home())));
        assert!(!second
            .iter()
            .any(|s| s.contains(&a.home()) || s.contains(&a.session.to_string())));
        assert!(create_arguments(owner, a, Path::new("/bad,readonly=false"), IMAGE).is_err());
        assert!(create_arguments(owner, a, Path::new("relative"), IMAGE).is_err());
        assert!(create_arguments(owner, a, Path::new("/private"), "untrusted:image").is_err());
    }
    fn inspect(owner: Uuid, a: Owned, id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!([{"Id":id,"Name":format!("/{}",a.container()),"Config":{"Labels":{
            (OWNER):owner.to_string(),(COMPANION):a.companion.to_string(),(SESSION):a.session.to_string(),(GENERATION):a.generation.to_string()
        }}}])).unwrap()
    }
    #[test]
    fn cleanup_refuses_foreign_owner_and_never_removes_by_name() {
        let owner = Uuid::new_v4();
        let a = owned();
        let exact = "a".repeat(64);
        for foreign in [false, true] {
            let mut runner = Fake::default();
            runner.replies.push_back(Ok(exact.as_bytes().to_vec()));
            runner.replies.push_back(Ok(inspect(
                if foreign { Uuid::new_v4() } else { owner },
                a,
                &exact,
            )));
            runner.replies.push_back(Ok(Vec::new()));
            runner.replies.push_back(Ok(Vec::new()));
            let mut docker = Docker::new(runner, owner, IMAGE.into()).unwrap();
            assert_eq!(docker.remove_desktop(owner, a).is_err(), foreign);
            let removes: Vec<_> = docker
                .runner
                .calls
                .iter()
                .filter(|a| a.get(1).is_some_and(|s| s == "rm"))
                .collect();
            if foreign {
                assert!(removes.is_empty());
            } else {
                assert_eq!(removes[0].last(), Some(&exact));
            }
        }
    }
    #[test]
    fn unavailable_engine_is_not_mistaken_for_absent_desktop() {
        let owner = Uuid::new_v4();
        let mut docker = Docker::new(Fake::default(), owner, IMAGE.into()).unwrap();
        assert_eq!(docker.remove_desktop(owner, owned()), Err(Error::Backend));
    }
}
