//! Linux host runtime for private, locally managed companion desktops.
use crate::{
    companion_docker::{Docker, LocalRunner},
    companion_lease::Lease,
    companion_manager::{
        Allocation, Backend, Error, Kind, Manager, Observer, Permit, Resources, ResumeOffer, Scope,
    },
    companion_recovery::{Journal, Owned},
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
}
type Controls = Arc<Mutex<HashMap<Uuid, Control>>>;
fn write_lease(control: &Control) -> Result<(), Error> {
    let run = control
        .running
        .as_ref()
        .filter(|(_, p)| p.check(control.scope).is_ok())
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
        };
        write_lease(&control)?;
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
        let controls: Controls = Arc::new(Mutex::new(HashMap::new()));
        let backend = DesktopBackend {
            journal,
            docker,
            controls: controls.clone(),
            retain: true,
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
        Ok(runtime)
    }
    pub fn observer(&self) -> &Observer {
        &self.observer
    }
    pub fn request(&self, body: Value) -> Result<Value, Error> {
        let action = body["action"].as_str().ok_or(Error::Invalid)?;
        let mut manager = self.manager.lock().map_err(|_| Error::Backend)?;
        // Refresh pending host revocations before showing or acting on state.
        let _ = manager.tick();
        if action == "list" {
            let retained = manager.backend_mut().journal.retained().iter().map(|a| json!({"companion":a.companion,"session":a.session,"generation":a.generation})).collect::<Vec<_>>();
            return Ok(
                json!({"retained":retained,"companions":manager.snapshots().iter().map(|s| json!({
                "companion":s.allocation.scope.companion(),"session":s.allocation.scope.session(),
                "generation":s.allocation.scope.generation(),"native":s.allocation.kind == Kind::Native,
                "phase":format!("{:?}",s.phase)
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
            if available < (u64::from(resources.memory_mib) + 1024) * 1024 {
                return Err(Error::Capacity);
            }
            let scope = manager.add_container(resources)?;
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
            manager.resume(offer)?;
            return Ok(json!({}));
        }
        let scope = request_scope(&body)?;
        if action == "delete_home" {
            let backend = manager.backend_mut();
            backend
                .journal
                .delete_retained(&mut backend.docker, scope)?;
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
                manager.backend_mut().retain =
                    body["retain_home"].as_bool().ok_or(Error::Invalid)?;
                manager.stop(scope)?;
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
