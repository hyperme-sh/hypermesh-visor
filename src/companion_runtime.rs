//! Linux host runtime for private, locally managed companion desktops.
use crate::{
    companion_broker::Broker,
    companion_docker::{Docker, LocalRunner},
    companion_lease::Lease,
    companion_manager::{
        Allocation, Backend, Error, Kind, Manager, Observer, Permit, Resources, ResumeOffer, Scope,
    },
    companion_recovery::{Journal, Owned},
    companion_sets::{self, Member, Reference, Store as Sets},
};
use hypermesh_local_storage as storage;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Control {
    scope: Scope,
    path: PathBuf,
    running: Option<(Uuid, Permit)>,
    remote: bool,
    remote_until: Option<Instant>,
}
type Controls = Arc<Mutex<HashMap<Uuid, Control>>>;
fn write_lease(control: &Control) -> Result<(), Error> {
    let run = control
        .running
        .as_ref()
        .filter(|(_, p)| {
            p.check(control.scope).is_ok()
                && (!control.remote || control.remote_until.is_some_and(|t| t > Instant::now()))
        })
        .map(|(id, _)| *id);
    let bytes = serde_json::to_vec(&Lease::new(control.scope, run)?).map_err(|_| Error::Backend)?;
    let temp = control.path.join(format!(".lease-{}", Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = storage::open(&temp, storage::Open::New)?;
        file.write_all(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        }
        std::fs::rename(&temp, control.path.join("lease.json"))
    })();
    let _ = std::fs::remove_file(temp);
    result.map_err(|_| Error::Backend)
}
struct DesktopBackend {
    journal: Journal,
    docker: Docker<LocalRunner>,
    controls: Controls,
    retain: bool,
    profiles: Arc<Mutex<Sets>>,
}
impl DesktopBackend {
    fn owned(&self, scope: Scope) -> Result<Owned, Error> {
        self.journal
            .active()
            .iter()
            .find(|a| a.scope() == scope)
            .copied()
            .ok_or(Error::Stale)
    }
    fn revoke(&self, scope: Scope) -> Result<(), Error> {
        let mut controls = self.controls.lock().map_err(|_| Error::Backend)?;
        if let Some(control) = controls.get_mut(&scope.generation()) {
            control.running = None;
            write_lease(control)?;
        }
        Ok(())
    }
}
impl Backend for DesktopBackend {
    fn start(&mut self, allocation: Allocation) -> Result<(), Error> {
        let a = self.journal.reserve(allocation)?;
        let path = self.journal.directory().join(a.generation.to_string());
        if !storage::create_dir(&path).map_err(|_| Error::Backend)? {
            return Err(Error::Backend);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(|_| Error::Backend)?;
        }
        let control = Control {
            scope: a.scope(),
            path: path.clone(),
            running: None,
            remote: false,
            remote_until: None,
        };
        write_lease(&control)?;
        let instructions = self
            .profiles
            .lock()
            .map_err(|_| Error::Backend)?
            .instructions(a.scope())?;
        companion_sets::write_guest(&path, a.scope(), &instructions)?;
        self.controls
            .lock()
            .map_err(|_| Error::Backend)?
            .insert(a.generation, control);
        self.docker.create(a, &path)?;
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Ok(status) = self.docker.guest(a, "status") {
                if status["version"] != 1
                    || status["companion"] != a.companion.to_string()
                    || status["session"] != a.session.to_string()
                    || status["generation"] != a.generation.to_string()
                {
                    return Err(Error::Backend);
                }
                if !profile_matches(&status, &instructions) {
                    return Err(Error::Backend);
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Backend);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    fn resume(&mut self, scope: Scope, permit: Permit) -> Result<(), Error> {
        let a = self.owned(scope)?;
        permit.check(scope)?;
        let instructions = self
            .profiles
            .lock()
            .map_err(|_| Error::Backend)?
            .instructions(scope)?;
        if !profile_matches(&self.docker.guest(a, "status")?, &instructions) {
            return Err(Error::Stale);
        }
        {
            let mut controls = self.controls.lock().map_err(|_| Error::Backend)?;
            let control = controls.get_mut(&scope.generation()).ok_or(Error::Stale)?;
            control.running = Some((Uuid::new_v4(), permit.clone()));
            write_lease(control)?;
        }
        self.docker.guest(a, "resume")?;
        permit.check(scope)
    }
    fn pause(&mut self, scope: Scope) -> Result<(), Error> {
        self.revoke(scope)
    }
    fn cancel(&mut self, scope: Scope) -> Result<(), Error> {
        self.revoke(scope)?;
        let a = match self.owned(scope) {
            Ok(a) => a,
            Err(Error::Stale) => return Ok(()),
            Err(e) => return Err(e),
        };
        if !self.docker.present(a)? {
            return Ok(());
        }
        self.docker.guest(a, "pause")?;
        Ok(())
    }
    fn stop(&mut self, scope: Scope) -> Result<(), Error> {
        self.revoke(scope)?;
        if self.owned(scope).is_ok() {
            self.journal.stop(&mut self.docker, scope, self.retain)?;
        }
        self.controls
            .lock()
            .map_err(|_| Error::Backend)?
            .remove(&scope.generation());
        // The only files here are host-generated control records. Keep homes
        // according to the explicit caller choice; this is not a home mount.
        let _ = std::fs::remove_dir_all(
            self.journal
                .directory()
                .join(scope.generation().to_string()),
        );
        Ok(())
    }
}

pub struct Runtime {
    manager: Mutex<Manager<DesktopBackend>>,
    observer: Observer,
    offers: Mutex<HashMap<Uuid, (Instant, ResumeOffer)>>,
    stopped: Arc<AtomicBool>,
    broker: Mutex<Broker>,
    profiles: Arc<Mutex<Sets>>,
}
impl Runtime {
    pub fn open(directory: &Path, image: String, native_session: Uuid) -> Result<Arc<Self>, Error> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Backend);
        }
        let mut journal = Journal::open(directory)?;
        let mut docker = Docker::new(LocalRunner::new(directory.into()), journal.owner(), image)?;
        docker.claim()?;
        journal.recover(&mut docker)?;
        let mut broker = Broker::open(directory)?;
        let scopes = journal
            .active()
            .iter()
            .chain(journal.retained())
            .map(|a| a.scope())
            .collect::<Vec<_>>();
        broker.retain(&scopes)?;
        let mut profiles = Sets::open(directory)?;
        profiles.retain(&scopes)?;
        let profiles = Arc::new(Mutex::new(profiles));
        let controls: Controls = Arc::new(Mutex::new(HashMap::new()));
        let backend = DesktopBackend {
            journal,
            docker,
            controls: controls.clone(),
            retain: true,
            profiles: profiles.clone(),
        };
        // Reserve one native desktop and at most three 2 GiB / 1 CPU guests.
        // Individual additions also check currently available host RAM.
        let budget = Resources {
            memory_mib: 8192,
            cpu_millis: std::thread::available_parallelism()
                .map_err(|_| Error::Capacity)?
                .get()
                .min(4) as u32
                * 1000,
        };
        let manager = Manager::new(
            backend,
            native_session,
            Resources {
                memory_mib: 2048,
                cpu_millis: 1000,
            },
            budget,
        )?;
        let observer = manager.observer();
        let stopped = Arc::new(AtomicBool::new(false));
        let runtime = Arc::new(Self {
            broker: Mutex::new(broker),
            profiles,
            manager: Mutex::new(manager),
            observer,
            offers: Mutex::new(HashMap::new()),
            stopped: stopped.clone(),
        });
        let weak = Arc::downgrade(&runtime);
        std::thread::Builder::new()
            .name("visor-companion-leases".into())
            .spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    if let Ok(controls) = controls.lock() {
                        for control in controls.values() {
                            let _ = write_lease(control);
                        }
                    }
                    let Some(runtime) = weak.upgrade() else {
                        break;
                    };
                    drop(runtime);
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|_| Error::Backend)?;
        let weak = Arc::downgrade(&runtime);
        std::thread::Builder::new()
            .name("visor-companion-watch".into())
            .spawn(move || {
                while let Some(runtime) = weak.upgrade() {
                    if let Ok(mut manager) = runtime.manager.try_lock() {
                        let _ = manager.tick();
                    }
                    drop(runtime);
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|_| Error::Backend)?;
        let weak = Arc::downgrade(&runtime);
        std::thread::Builder::new()
            .name("visor-companion-broker".into())
            .spawn(move || {
                let mut active = HashMap::<Uuid, (Uuid, Uuid)>::new();
                while let Some(runtime) = weak.upgrade() {
                    runtime.poll_bots(&mut active);
                    drop(runtime);
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .map_err(|_| Error::Backend)?;
        Ok(runtime)
    }
    fn poll_bots(&self, active: &mut HashMap<Uuid, (Uuid, Uuid)>) {
        let Ok(mut manager) = self.manager.try_lock() else {
            return;
        };
        let Ok(mut broker) = self.broker.lock() else {
            return;
        };
        for snapshot in manager.snapshots() {
            let scope = snapshot.allocation.scope;
            if !broker.enrolled(scope) {
                continue;
            }
            let running = snapshot.phase == crate::companion_manager::Phase::Running;
            let result = (|| -> Result<(), Error> {
                if running {
                    // A missed grant never silently revives an old local permit.
                    let controls = manager
                        .backend_mut()
                        .controls
                        .lock()
                        .map_err(|_| Error::Backend)?;
                    if !controls
                        .get(&scope.generation())
                        .is_some_and(|c| c.remote_until.is_some_and(|t| t > Instant::now()))
                    {
                        return Err(Error::Stale);
                    }
                }
                let status = broker.heartbeat(scope, running)?;
                if !running {
                    if active.remove(&scope.generation()).is_some() {
                        broker.disconnect(scope);
                    }
                    return Ok(());
                }
                if let Some((cloud, _)) = active.get(&scope.generation()) {
                    if !Broker::task_live(&status, *cloud) {
                        return Err(Error::Stale);
                    }
                }
                let duration = broker.authorized(scope, &status)?;
                {
                    let mut controls = manager
                        .backend_mut()
                        .controls
                        .lock()
                        .map_err(|_| Error::Backend)?;
                    let control = controls.get_mut(&scope.generation()).ok_or(Error::Stale)?;
                    if !control.remote_until.is_some_and(|t| t > Instant::now()) {
                        return Err(Error::Stale);
                    }
                    control.remote_until = Some(Instant::now() + duration);
                }
                let backend = manager.backend_mut();
                let owned = backend.owned(scope)?;
                if let Some((cloud, local)) = active.get(&scope.generation()).copied() {
                    let result = backend
                        .docker
                        .task(owned, json!({"action":"status","id":local}))?;
                    if let Some(status @ ("completed" | "failed" | "cancelled")) =
                        result["status"].as_str()
                    {
                        broker.complete(scope, cloud, status)?;
                        active.remove(&scope.generation());
                    }
                } else if let Some((cloud, payload, until)) = broker.claim(scope)? {
                    let result = backend.docker.task(owned, json!({"action":"submit","payload":payload,"until_ms":(until*1000.0).floor() as u64}))?;
                    let local = result["id"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .ok_or(Error::Invalid)?;
                    active.insert(scope.generation(), (cloud, local));
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = manager.pause(scope);
                active.remove(&scope.generation());
                broker.disconnect(scope);
            }
        }
    }
    pub fn observer(&self) -> &Observer {
        &self.observer
    }
    pub fn request(&self, body: Value) -> Result<Value, Error> {
        let action = body["action"].as_str().ok_or(Error::Invalid)?;
        let mut manager = self.manager.lock().map_err(|_| Error::Backend)?;
        // Refresh pending host revocations before showing or acting on state.
        let _ = manager.tick();
        let mut broker = self.broker.lock().map_err(|_| Error::Backend)?;
        if action == "sets" {
            let sets = self.profiles.lock().map_err(|_| Error::Backend)?.list()?;
            return Ok(
                json!({"sets":sets.into_iter().map(|(id,name,count,requires_owner)|json!({"id":id,"name":name,"members":count,"requires_owner":requires_owner})).collect::<Vec<_>>()}),
            );
        }
        if action == "save_set" {
            let members = manager
                .snapshots()
                .into_iter()
                .filter(|s| s.allocation.kind == Kind::Container)
                .map(|s| {
                    let scope = s.allocation.scope;
                    Ok(Member {
                        memory_mib: s.allocation.resources.memory_mib,
                        cpu_millis: s.allocation.resources.cpu_millis,
                        instructions: self
                            .profiles
                            .lock()
                            .map_err(|_| Error::Backend)?
                            .instructions(scope)?,
                        grant: broker.enrolled(scope).then(|| Reference::new(scope)),
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let id = self
                .profiles
                .lock()
                .map_err(|_| Error::Backend)?
                .create(body["name"].as_str().ok_or(Error::Invalid)?.into(), members)?;
            return Ok(json!({"id":id}));
        }
        if matches!(action, "load_set" | "delete_set") {
            let id = body["id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or(Error::Invalid)?;
            if action == "delete_set" {
                self.profiles
                    .lock()
                    .map_err(|_| Error::Backend)?
                    .delete(id)?;
                return Ok(json!({}));
            }
            let saved = self.profiles.lock().map_err(|_| Error::Backend)?.get(id)?;
            let mut results = Vec::new();
            for (index, member) in saved.members.into_iter().enumerate() {
                let result = (|| {
                    available_memory(member.memory_mib)?;
                    let scope = manager.add_checked(member.resources(), |target| {
                        if let Some(reference) = member.grant {
                            if !broker.enrolled(reference.scope()) {
                                return Err(Error::Denied);
                            }
                            broker.recreate(
                                reference.scope(),
                                target,
                                "clone",
                                Uuid::new_v4(),
                                body["owner_token"].as_str().ok_or(Error::Denied)?,
                            )?;
                        }
                        self.profiles
                            .lock()
                            .map_err(|_| Error::Backend)?
                            .configure(target, member.instructions.clone())
                    })?;
                    if member.grant.is_some() {
                        let mut controls = manager
                            .backend_mut()
                            .controls
                            .lock()
                            .map_err(|_| Error::Backend)?;
                        controls
                            .get_mut(&scope.generation())
                            .ok_or(Error::Stale)?
                            .remote = true;
                    }
                    Ok::<_, Error>(scope)
                })();
                // Keep uncertain starts, but discard preparation that never reserved a guest.
                let mut scopes = manager
                    .snapshots()
                    .iter()
                    .map(|s| s.allocation.scope)
                    .collect::<Vec<_>>();
                scopes.extend(
                    manager
                        .backend_mut()
                        .journal
                        .retained()
                        .iter()
                        .map(|a| a.scope()),
                );
                broker.retain(&scopes)?;
                self.profiles
                    .lock()
                    .map_err(|_| Error::Backend)?
                    .retain(&scopes)?;
                results.push(match result {
                    Ok(scope)=>json!({"member":index,"status":"paused","companion":scope.companion(),"session":scope.session(),"generation":scope.generation()}),
                    Err(error)=>json!({"member":index,"status":"refused","reason":format!("{error:?}")}),
                });
            }
            return Ok(json!({"members":results}));
        }
        if action == "list" {
            let retained = manager.backend_mut().journal.retained().iter().map(|a| json!({"companion":a.companion,"session":a.session,"generation":a.generation,"enrolled":broker.enrolled(a.scope())})).collect::<Vec<_>>();
            return Ok(
                json!({"retained":retained,"companions":manager.snapshots().iter().map(|s| json!({
                "companion":s.allocation.scope.companion(),"session":s.allocation.scope.session(),
                "generation":s.allocation.scope.generation(),"native":s.allocation.kind == Kind::Native,
                "enrolled":broker.enrolled(s.allocation.scope),"phase":format!("{:?}",s.phase)
            })).collect::<Vec<_>>() }),
            );
        }
        if matches!(action, "add" | "clone" | "restore") {
            let resources = if action == "add" {
                Resources {
                    memory_mib: 2048,
                    cpu_millis: 1000,
                }
            } else {
                let scope = request_scope(&body)?;
                let snapshots = manager.snapshots();
                let retained = manager.backend_mut().journal.retained();
                recreation_resources(action, scope, &snapshots, retained)?
            };
            available_memory(resources.memory_mib)?;
            let source = if action == "add" {
                None
            } else {
                Some(request_scope(&body)?)
            };
            let owner = if source.is_some_and(|s| broker.enrolled(s)) {
                Some(body["owner_token"].as_str().ok_or(Error::Invalid)?)
            } else {
                None
            };
            let instructions = source
                .map(|scope| {
                    self.profiles
                        .lock()
                        .map_err(|_| Error::Backend)?
                        .instructions(scope)
                })
                .transpose()?
                .unwrap_or_default();
            let scope = manager.add_checked(resources, |target| {
                self.profiles
                    .lock()
                    .map_err(|_| Error::Backend)?
                    .configure(target, instructions)
            })?;
            if let Some(token) = owner {
                let source = source.ok_or(Error::Invalid)?;
                let result = broker.recreate(source, scope, action, Uuid::new_v4(), token);
                if result.is_err() {
                    let _ = manager.stop(scope);
                    return Err(Error::Backend);
                }
                let mut controls = manager
                    .backend_mut()
                    .controls
                    .lock()
                    .map_err(|_| Error::Backend)?;
                controls
                    .get_mut(&scope.generation())
                    .ok_or(Error::Stale)?
                    .remote = true;
            }
            return Ok(
                json!({"companion":scope.companion(), "session":scope.session(), "generation":scope.generation()}),
            );
        }
        if action == "resume" {
            let token = body["offer"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or(Error::Invalid)?;
            let (when, offer) = self
                .offers
                .lock()
                .map_err(|_| Error::Backend)?
                .remove(&token)
                .ok_or(Error::Stale)?;
            if when.elapsed() > Duration::from_secs(30) {
                return Err(Error::Stale);
            }
            let scope = offer.scope();
            if broker.enrolled(scope) {
                let owner = body["owner_token"].as_str().ok_or(Error::Invalid)?;
                broker.control(scope, owner, "resume")?;
                broker.heartbeat(scope, false)?;
                let status = broker.heartbeat(scope, true)?;
                let duration = broker.authorized(scope, &status)?;
                let mut controls = manager
                    .backend_mut()
                    .controls
                    .lock()
                    .map_err(|_| Error::Backend)?;
                let control = controls.get_mut(&scope.generation()).ok_or(Error::Stale)?;
                control.remote = true;
                control.remote_until = Some(Instant::now() + duration);
            }
            manager.resume(offer)?;
            return Ok(json!({}));
        }
        let scope = request_scope(&body)?;
        if action == "delete_home" {
            let backend = manager.backend_mut();
            backend
                .journal
                .delete_retained(&mut backend.docker, scope)?;
            broker.remove(scope)?;
            let backend = manager.backend_mut();
            let scopes = backend
                .journal
                .active()
                .iter()
                .chain(backend.journal.retained())
                .map(|a| a.scope())
                .collect::<Vec<_>>();
            self.profiles
                .lock()
                .map_err(|_| Error::Backend)?
                .retain(&scopes)?;
            return Ok(json!({}));
        }
        let snapshot = manager
            .snapshots()
            .into_iter()
            .find(|s| s.allocation.scope == scope)
            .ok_or(Error::Stale)?;
        if snapshot.allocation.kind == Kind::Native {
            return Err(Error::Invalid);
        }
        match action {
            "profile" => Ok(
                json!({"instructions":self.profiles.lock().map_err(|_|Error::Backend)?.instructions(scope)?}),
            ),
            "configure" => {
                let instructions = body["instructions"].as_str().ok_or(Error::Invalid)?;
                if !companion_sets::valid_instructions(instructions) {
                    return Err(Error::Invalid);
                }
                manager.pause(scope)?;
                self.profiles
                    .lock()
                    .map_err(|_| Error::Backend)?
                    .configure(scope, instructions.into())?;
                let backend = manager.backend_mut();
                let owned = backend.owned(scope)?;
                let path = backend
                    .journal
                    .directory()
                    .join(scope.generation().to_string());
                companion_sets::write_guest(&path, scope, instructions)?;
                let status = backend.docker.guest(owned, "configure")?;
                if status["profile_hash"] != companion_sets::digest(instructions) {
                    return Err(Error::Stale);
                }
                Ok(json!({}))
            }
            "enroll" => {
                manager.pause(scope)?;
                let tenant = body["tenant"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or(Error::Invalid)?;
                let result = broker.enroll(
                    scope,
                    body["origin"].as_str().ok_or(Error::Invalid)?,
                    tenant,
                    body["owner_token"].as_str().ok_or(Error::Invalid)?,
                );
                let required = enrollment_requires_remote(broker.enrolled(scope), &result);
                {
                    let mut controls = manager
                        .backend_mut()
                        .controls
                        .lock()
                        .map_err(|_| Error::Backend)?;
                    let control = controls.get_mut(&scope.generation()).ok_or(Error::Stale)?;
                    control.remote |= required;
                }
                result?;
                Ok(json!({}))
            }
            "offer" => {
                let offer = manager.offer_resume(scope)?;
                let mut offers = self.offers.lock().map_err(|_| Error::Backend)?;
                offers.retain(|_, (when, _)| when.elapsed() < Duration::from_secs(30));
                if offers.len() >= 8 {
                    return Err(Error::Capacity);
                }
                let token = Uuid::new_v4();
                offers.insert(token, (Instant::now(), offer));
                Ok(json!({"offer":token}))
            }
            "stop" => {
                // Pause revokes permits before attempting guest communication.
                // Cleanup must still run if that guest or Plane is unavailable.
                let _ = manager.pause(scope);
                if broker.enrolled(scope) {
                    if let Some(token) = body["owner_token"].as_str() {
                        let _ = broker.control(scope, token, "stop");
                    }
                }
                manager.backend_mut().retain =
                    body["retain_home"].as_bool().ok_or(Error::Invalid)?;
                manager.stop(scope)?;
                if !manager.backend_mut().retain {
                    broker.remove(scope)?;
                    let backend = manager.backend_mut();
                    let scopes = backend
                        .journal
                        .active()
                        .iter()
                        .chain(backend.journal.retained())
                        .map(|a| a.scope())
                        .collect::<Vec<_>>();
                    self.profiles
                        .lock()
                        .map_err(|_| Error::Backend)?
                        .retain(&scopes)?;
                }
                Ok(json!({}))
            }
            "view" => {
                let backend = manager.backend_mut();
                let a = backend.owned(scope)?;
                backend.docker.guest(a, "view")
            }
            _ => Err(Error::Invalid),
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.observer.physical_input();
    }
}

fn profile_matches(status: &Value, instructions: &str) -> bool {
    match status.get("profile_hash") {
        // Compatibility with guests created before instruction support.
        None => instructions.is_empty(),
        Some(value) => value.as_str() == Some(companion_sets::digest(instructions).as_str()),
    }
}

fn available_memory(memory_mib: u32) -> Result<(), Error> {
    let memory = std::fs::read_to_string("/proc/meminfo").map_err(|_| Error::Capacity)?;
    let available = memory
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemAvailable:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .ok_or(Error::Capacity)?;
    if available < (u64::from(memory_mib) + 1024) * 1024 {
        return Err(Error::Capacity);
    }
    Ok(())
}
fn enrollment_requires_remote(enrolled: bool, result: &Result<(), Error>) -> bool {
    enrolled
        || !matches!(
            result,
            Err(Error::Invalid | Error::Capacity | Error::Denied)
        )
}

fn request_scope(body: &Value) -> Result<Scope, Error> {
    let parse = |key: &str| {
        body[key]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(Error::Invalid)
    };
    Ok(Scope::recovered(
        parse("companion")?,
        parse("session")?,
        parse("generation")?,
    ))
}

/// Only numeric execution configuration crosses into a new desktop. Homes,
/// identity, credentials, assignment and running permits never cross this path.
fn recreation_resources(
    action: &str,
    scope: Scope,
    active: &[crate::companion_manager::Snapshot],
    retained: &[Owned],
) -> Result<Resources, Error> {
    match action {
        "clone" => active
            .iter()
            .find(|s| s.allocation.scope == scope && s.allocation.kind == Kind::Container)
            .map(|s| s.allocation.resources)
            .ok_or(Error::Stale),
        "restore" => retained
            .iter()
            .find(|a| a.scope() == scope)
            .map(|a| Resources {
                memory_mib: a.memory_mib,
                cpu_millis: a.cpu_millis,
            })
            .ok_or(Error::Stale),
        _ => Err(Error::Invalid),
    }
}

#[cfg(test)]
mod recreation_tests {
    #[test]
    fn clearing_instructions_cannot_resume_a_guest_with_stale_text() {
        assert!(super::profile_matches(&serde_json::json!({}), ""));
        assert!(!super::profile_matches(&serde_json::json!({}), "new text"));
        assert!(!super::profile_matches(
            &serde_json::json!({"profile_hash":crate::companion_sets::digest("old text")}),
            ""
        ));
        assert!(super::profile_matches(
            &serde_json::json!({"profile_hash":crate::companion_sets::digest("")}),
            ""
        ));
        assert!(!super::profile_matches(
            &serde_json::json!({"profile_hash":null}),
            ""
        ));
    }

    use super::*;
    use crate::companion_manager::{Phase, Snapshot};
    struct EmptyBackend;
    impl Backend for EmptyBackend {
        fn start(&mut self, _: Allocation) -> Result<(), Error> {
            Ok(())
        }
        fn resume(&mut self, _: Scope, _: Permit) -> Result<(), Error> {
            Ok(())
        }
        fn pause(&mut self, _: Scope) -> Result<(), Error> {
            Ok(())
        }
        fn cancel(&mut self, _: Scope) -> Result<(), Error> {
            Ok(())
        }
        fn stop(&mut self, _: Scope) -> Result<(), Error> {
            Ok(())
        }
    }
    #[test]
    fn expired_remote_grant_removes_guest_lease_without_waiting_for_manager() {
        let unit = Resources {
            memory_mib: 512,
            cpu_millis: 1000,
        };
        let mut manager = Manager::new(
            EmptyBackend,
            Uuid::new_v4(),
            unit,
            Resources {
                memory_mib: 4096,
                cpu_millis: 4000,
            },
        )
        .unwrap();
        manager.observer_poll(true).unwrap();
        let scope = manager.add_container(unit).unwrap();
        let offer = manager.offer_resume(scope).unwrap();
        let permit = manager.resume(offer).unwrap();
        let path = std::env::temp_dir().join(format!("remote-lease-unit-{}", Uuid::new_v4()));
        storage::create_dir(&path).unwrap();
        let mut control = Control {
            scope,
            path: path.clone(),
            running: Some((Uuid::new_v4(), permit)),
            remote: true,
            remote_until: Some(Instant::now() + Duration::from_secs(2)),
        };
        write_lease(&control).unwrap();
        let read = || {
            serde_json::from_slice::<Lease>(&std::fs::read(path.join("lease.json")).unwrap())
                .unwrap()
        };
        assert!(read().run.is_some());
        control.remote_until = Some(Instant::now() - Duration::from_secs(1));
        write_lease(&control).unwrap();
        assert!(read().run.is_none());
        control.remote_until = None;
        write_lease(&control).unwrap();
        assert!(read().run.is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn definitive_enrollment_refusal_keeps_local_mode_but_uncertainty_does_not() {
        for error in [Error::Invalid, Error::Capacity, Error::Denied] {
            assert!(!enrollment_requires_remote(false, &Err(error)));
            assert!(enrollment_requires_remote(true, &Err(error)));
        }
        assert!(enrollment_requires_remote(false, &Err(Error::Backend)));
        assert!(enrollment_requires_remote(true, &Ok(())));
    }
    fn saved() -> Owned {
        Owned {
            companion: Uuid::new_v4(),
            session: Uuid::new_v4(),
            generation: Uuid::new_v4(),
            memory_mib: 1024,
            cpu_millis: 1000,
        }
    }
    #[test]
    fn recreation_copies_only_resources_and_requires_exact_source() {
        let old = saved();
        let resources = Resources {
            memory_mib: old.memory_mib,
            cpu_millis: old.cpu_millis,
        };
        let active = Snapshot {
            allocation: Allocation {
                scope: old.scope(),
                resources,
                kind: Kind::Container,
            },
            phase: Phase::Running,
        };
        assert_eq!(
            recreation_resources("clone", old.scope(), &[active], &[]),
            Ok(resources)
        );
        assert_eq!(
            recreation_resources("restore", old.scope(), &[], &[old]),
            Ok(resources)
        );
        assert_eq!(
            recreation_resources("restore", saved().scope(), &[], &[old]),
            Err(Error::Stale)
        );
        assert_eq!(
            recreation_resources("clone", old.scope(), &[], &[old]),
            Err(Error::Stale)
        );
        let native = Snapshot {
            allocation: Allocation {
                kind: Kind::Native,
                ..active.allocation
            },
            ..active
        };
        assert_eq!(
            recreation_resources("clone", old.scope(), &[native], &[]),
            Err(Error::Stale)
        );
    }
    #[test]
    fn restored_configuration_starts_paused_and_cannot_reuse_old_authority() {
        let old = saved();
        let resources = recreation_resources("restore", old.scope(), &[], &[old]).unwrap();
        let mut manager = Manager::new(
            EmptyBackend,
            Uuid::new_v4(),
            resources,
            Resources {
                memory_mib: 4096,
                cpu_millis: 4000,
            },
        )
        .unwrap();
        // A missing/revoked observer authorization cannot create a desktop.
        assert_eq!(
            manager.add_container(resources),
            Err(Error::MonitorUnavailable)
        );
        manager.observer_poll(true).unwrap();
        let new = manager.add_container(resources).unwrap();
        assert_ne!(new.companion(), old.companion);
        assert_ne!(new.session(), old.session);
        assert_ne!(new.generation(), old.generation);
        assert_eq!(
            manager
                .snapshots()
                .iter()
                .find(|s| s.allocation.scope == new)
                .unwrap()
                .phase,
            Phase::Paused
        );
        let offer = manager.offer_resume(new).unwrap();
        manager.physical_input().unwrap();
        assert!(manager.resume(offer).is_err());
        assert!(manager.offer_resume(old.scope()).is_err());
    }
}
