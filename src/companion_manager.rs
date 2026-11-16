//! Local allocation and revocation core for the companion manager.
//!
//! This is not a remote authorization service. A trusted UI/controller must
//! authenticate session assignment before invoking it. Backends must enforce
//! permits at action boundaries and cancellation checkpoints. Docker execution,
//! durable recovery, the host input observer and UI wiring are separate adapters.
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const MAX_COMPANIONS: usize = 4;
const MONITOR_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Denied,
    Capacity,
    Limit,
    MonitorUnavailable,
    Stale,
    NotPaused,
    Backend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resources {
    pub memory_mib: u32,
    pub cpu_millis: u32,
}
impl Resources {
    fn valid(self) -> bool {
        self.memory_mib > 0 && self.cpu_millis > 0
    }
}

/// Opaque local routing identity, not an enrolled bot principal or a grant.
/// The generation also scopes the home/profile and control channel allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    companion: Uuid,
    session: Uuid,
    generation: Uuid,
}
impl Scope {
    pub(crate) fn recovered(companion: Uuid, session: Uuid, generation: Uuid) -> Self {
        Self {
            companion,
            session,
            generation,
        }
    }
    pub fn companion(self) -> Uuid {
        self.companion
    }
    pub fn session(self) -> Uuid {
        self.session
    }
    pub fn generation(self) -> Uuid {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Native,
    Container,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Paused,
    Running,
    Quarantined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub scope: Scope,
    pub kind: Kind,
    pub resources: Resources,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub allocation: Allocation,
    pub phase: Phase,
}

/// A preview of one paused generation. The trusted UI retains it while asking
/// the user, then consumes it on confirmation. Input or monitor loss invalidates
/// outstanding previews even when the desktop was already paused.
pub struct ResumeOffer {
    scope: Scope,
    pause: Uuid,
    observer_epoch: Uuid,
}
impl ResumeOffer {
    pub fn scope(&self) -> Scope {
        self.scope
    }
}

type Monitor = Arc<Mutex<Option<Instant>>>;
fn fresh(monitor: &Monitor) -> bool {
    monitor
        .lock()
        .ok()
        .and_then(|last| *last)
        .is_some_and(|last| last.elapsed() < MONITOR_TIMEOUT)
}

/// Fast host observer path, independent of the manager's backend operations.
/// Only the authenticated native observer may hold this handle. Updating it
/// never waits for Docker, cancellation, or desktop acknowledgements.
#[derive(Clone)]
pub struct Observer {
    monitor: Monitor,
    epoch: Arc<Mutex<Uuid>>,
}
impl Observer {
    fn epoch(&self) -> Result<Uuid, Error> {
        self.epoch
            .lock()
            .map(|epoch| *epoch)
            .map_err(|_| Error::MonitorUnavailable)
    }
    pub fn physical_input(&self) -> Result<(), Error> {
        *self.epoch.lock().map_err(|_| Error::MonitorUnavailable)? = Uuid::new_v4();
        Ok(())
    }
    pub fn poll(&self, available: bool) -> Result<(), Error> {
        self.poll_at(available, Instant::now())
    }
    fn poll_at(&self, available: bool, sampled: Instant) -> Result<(), Error> {
        let mut last = self.monitor.lock().map_err(|_| Error::MonitorUnavailable)?;
        let timely =
            last.is_some_and(|last| sampled.saturating_duration_since(last) < MONITOR_TIMEOUT);
        if !available || !timely {
            // Revoke before renewing: a worker cannot revive an old permit by
            // waiting for this monitor lock while a late poll is processed.
            self.physical_input()?;
        }
        *last = available.then_some(sampled);
        Ok(())
    }
}

/// An in-memory revocable route. It cannot be deserialized or constructed by a
/// caller. Copying it neither changes its target nor extends the observer lease.
#[derive(Clone)]
pub struct Permit {
    scope: Scope,
    enabled: Arc<AtomicBool>,
    monitor: Monitor,
    observer: Observer,
    observer_epoch: Uuid,
}
impl Permit {
    pub fn check(&self, target: Scope) -> Result<(), Error> {
        self.check_with_lease(target, || fresh(&self.monitor))
    }
    // The lease read can wait behind an observer update. Its completion must
    // never revive a route that was revoked while that read was pending.
    fn check_with_lease(&self, target: Scope, lease: impl FnOnce() -> bool) -> Result<(), Error> {
        if self.scope != target
            || !self.enabled.load(Ordering::Acquire)
            || self.observer.epoch()? != self.observer_epoch
        {
            return Err(Error::Stale);
        }
        if !lease() {
            return Err(Error::MonitorUnavailable);
        }
        if !self.enabled.load(Ordering::Acquire) || self.observer.epoch()? != self.observer_epoch {
            return Err(Error::Stale);
        }
        Ok(())
    }
}

/// Operations must be bounded and retry-safe for the exact scope. `start`
/// creates a paused desktop, never work. `cancel` acknowledges that active work
/// can no longer act; `stop` acknowledges that its desktop has stopped. These
/// acknowledgements are distinct, including when an earlier call failed.
/// No backend may widen an allocation into arbitrary mounts, devices or network.
pub trait Backend {
    fn start(&mut self, allocation: Allocation) -> Result<(), Error>;
    fn resume(&mut self, scope: Scope, permit: Permit) -> Result<(), Error>;
    fn pause(&mut self, scope: Scope) -> Result<(), Error>;
    fn cancel(&mut self, scope: Scope) -> Result<(), Error>;
    fn stop(&mut self, scope: Scope) -> Result<(), Error>;
}

struct Slot {
    pause: Uuid,
    snapshot: Snapshot,
    enabled: Arc<AtomicBool>,
}
impl Slot {
    fn revoke(&mut self) {
        self.enabled.store(false, Ordering::Release);
        self.pause = Uuid::new_v4();
    }
}

pub struct Manager<B: Backend> {
    backend: B,
    slots: BTreeMap<Uuid, Slot>,
    budget: Resources,
    monitor: Monitor,
    observer: Observer,
    applied_epoch: Uuid,
}
impl<B: Backend> Manager<B> {
    /// Adopt exactly one existing native session, initially paused. The caller
    /// must already have paused it; adoption does not create another desktop.
    /// Restart must reconcile previously owned allocations before constructing
    /// a replacement manager. This core does not persist or discover desktops.
    pub fn new(
        backend: B,
        session: Uuid,
        native: Resources,
        budget: Resources,
    ) -> Result<Self, Error> {
        if session.is_nil() || !native.valid() || !budget.valid() {
            return Err(Error::Invalid);
        }
        if native.memory_mib > budget.memory_mib || native.cpu_millis > budget.cpu_millis {
            return Err(Error::Capacity);
        }
        let scope = Scope {
            companion: Uuid::new_v4(),
            session,
            generation: Uuid::new_v4(),
        };
        let monitor = Arc::new(Mutex::new(None));
        let applied_epoch = Uuid::new_v4();
        let observer = Observer {
            monitor: monitor.clone(),
            epoch: Arc::new(Mutex::new(applied_epoch)),
        };
        let mut manager = Self {
            backend,
            slots: BTreeMap::new(),
            budget,
            monitor,
            observer,
            applied_epoch,
        };
        manager.insert(Allocation {
            scope,
            kind: Kind::Native,
            resources: native,
        });
        Ok(manager)
    }
    fn insert(&mut self, allocation: Allocation) {
        self.slots.insert(
            allocation.scope.companion,
            Slot {
                pause: Uuid::new_v4(),
                snapshot: Snapshot {
                    allocation,
                    phase: Phase::Paused,
                },
                enabled: Arc::new(AtomicBool::new(false)),
            },
        );
    }
    pub(crate) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn snapshots(&self) -> Vec<Snapshot> {
        self.slots.values().map(|slot| slot.snapshot).collect()
    }
    /// Only the trusted host observer may refresh this lease. Guest virtual
    /// input and remote task traffic must never call it. A restored observer
    /// cannot resume a paused companion or revive an old permit.
    pub fn observer_poll(&mut self, available: bool) -> Result<(), Error> {
        self.observer_poll_at(available, Instant::now())
    }
    pub fn observer(&self) -> Observer {
        self.observer.clone()
    }
    fn observer_poll_at(&mut self, available: bool, sampled: Instant) -> Result<(), Error> {
        self.observer.poll_at(available, sampled)?;
        self.tick()?;
        if available && !fresh(&self.monitor) {
            return Err(Error::MonitorUnavailable);
        }
        Ok(())
    }
    /// The controller calls this periodically even with no incoming requests.
    /// Permits independently reject host input and observer loss between ticks.
    pub fn tick(&mut self) -> Result<(), Error> {
        let epoch = self.observer.epoch()?;
        if self.applied_epoch != epoch || !fresh(&self.monitor) {
            self.applied_epoch = epoch;
            self.pause_all()
        } else {
            Ok(())
        }
    }
    pub fn physical_input(&mut self) -> Result<(), Error> {
        self.observer.physical_input()?;
        self.tick()
    }

    pub fn add_container(&mut self, resources: Resources) -> Result<Scope, Error> {
        self.add_checked(resources, |_| Ok(()))
    }
    pub fn add_checked(
        &mut self,
        resources: Resources,
        check: impl FnOnce(Scope) -> Result<(), Error>,
    ) -> Result<Scope, Error> {
        self.tick()?;
        if !fresh(&self.monitor) {
            self.tick()?;
            return Err(Error::MonitorUnavailable);
        }
        if !resources.valid() {
            return Err(Error::Invalid);
        }
        if self.slots.len() >= MAX_COMPANIONS {
            return Err(Error::Limit);
        }
        let memory: u64 = self
            .slots
            .values()
            .map(|s| u64::from(s.snapshot.allocation.resources.memory_mib))
            .sum();
        let cpu: u64 = self
            .slots
            .values()
            .map(|s| u64::from(s.snapshot.allocation.resources.cpu_millis))
            .sum();
        if memory + u64::from(resources.memory_mib) > u64::from(self.budget.memory_mib)
            || cpu + u64::from(resources.cpu_millis) > u64::from(self.budget.cpu_millis)
        {
            return Err(Error::Capacity);
        }
        let scope = Scope {
            companion: Uuid::new_v4(),
            session: Uuid::new_v4(),
            generation: Uuid::new_v4(),
        };
        let allocation = Allocation {
            scope,
            kind: Kind::Container,
            resources,
        };
        // A missing grant/resource must not start a guest, even a paused one.
        check(scope)?;
        if !fresh(&self.monitor) {
            return Err(Error::MonitorUnavailable);
        }
        // Reserve before starting. Even an uncertain start owns a slot until
        // explicit cancellation and stop both succeed.
        self.insert(allocation);
        if self.backend.start(allocation).is_err() {
            self.slots.get_mut(&scope.companion).unwrap().snapshot.phase = Phase::Quarantined;
            return Err(Error::Backend);
        }
        Ok(scope)
    }
    fn slot(&mut self, scope: Scope) -> Result<&mut Slot, Error> {
        self.slots
            .get_mut(&scope.companion)
            .filter(|s| s.snapshot.allocation.scope == scope)
            .ok_or(Error::Stale)
    }
    pub fn offer_resume(&mut self, scope: Scope) -> Result<ResumeOffer, Error> {
        self.tick()?;
        let observer_epoch = self.observer.epoch()?;
        let slot = self.slot(scope)?;
        if slot.snapshot.phase != Phase::Paused {
            return Err(Error::NotPaused);
        }
        Ok(ResumeOffer {
            scope,
            pause: slot.pause,
            observer_epoch,
        })
    }
    /// Called only after explicit local confirmation of this offer and current
    /// assignment checks. A scope alone is never sufficient to resume.
    pub fn resume(&mut self, offer: ResumeOffer) -> Result<Permit, Error> {
        self.tick()?;
        if !fresh(&self.monitor) {
            self.tick()?;
            return Err(Error::MonitorUnavailable);
        }
        if offer.observer_epoch != self.observer.epoch()? {
            return Err(Error::Stale);
        }
        let scope = offer.scope;
        let slot = self.slot(scope)?;
        if slot.pause != offer.pause {
            return Err(Error::Stale);
        }
        if slot.snapshot.phase != Phase::Paused {
            return Err(Error::NotPaused);
        }
        // Allocate a fresh gate: re-enabling the previous atomic would revive
        // requests that were captured before pause or revocation.
        slot.enabled = Arc::new(AtomicBool::new(true));
        let enabled = Arc::clone(&slot.enabled);
        let permit = Permit {
            scope,
            enabled,
            monitor: Arc::clone(&self.monitor),
            observer: self.observer.clone(),
            observer_epoch: offer.observer_epoch,
        };
        if self.backend.resume(scope, permit.clone()).is_err() || permit.check(scope).is_err() {
            self.slot(scope)?.revoke();
            let _ = self.backend.pause(scope);
            let _ = self.backend.cancel(scope);
            self.slot(scope)?.snapshot.phase = Phase::Quarantined;
            return Err(Error::Backend);
        }
        self.slot(scope)?.snapshot.phase = Phase::Running;
        Ok(permit)
    }
    pub fn pause(&mut self, scope: Scope) -> Result<(), Error> {
        let slot = self.slot(scope)?;
        slot.revoke();
        slot.snapshot.phase = Phase::Paused;
        let paused = self.backend.pause(scope).is_ok();
        let cancelled = self.backend.cancel(scope).is_ok();
        if !paused || !cancelled {
            self.slot(scope)?.snapshot.phase = Phase::Quarantined;
            return Err(Error::Backend);
        }
        Ok(())
    }
    fn pause_all(&mut self) -> Result<(), Error> {
        let scopes: Vec<_> = self
            .slots
            .values_mut()
            .filter_map(|slot| {
                slot.revoke();
                if slot.snapshot.phase != Phase::Running {
                    return None;
                }
                slot.snapshot.phase = Phase::Paused;
                Some(slot.snapshot.allocation.scope)
            })
            .collect();
        // Revoke EVERY route before any potentially failing backend operation.
        let mut failed = false;
        for scope in scopes {
            let paused = self.backend.pause(scope).is_ok();
            let cancelled = self.backend.cancel(scope).is_ok();
            if !paused || !cancelled {
                self.slot(scope)?.snapshot.phase = Phase::Quarantined;
                failed = true;
            }
        }
        if failed {
            Err(Error::Backend)
        } else {
            Ok(())
        }
    }
    /// Stop always removes local execution authority before contacting the
    /// backend. Failed cleanup retains both capacity and identity for retry.
    pub fn stop(&mut self, scope: Scope) -> Result<(), Error> {
        self.slot(scope)?.revoke();
        let cancelled = self.backend.cancel(scope).is_ok();
        let stopped = self.backend.stop(scope).is_ok();
        if !cancelled || !stopped {
            self.slot(scope)?.snapshot.phase = Phase::Quarantined;
            return Err(Error::Backend);
        }
        self.slots.remove(&scope.companion);
        Ok(())
    }
}
impl<B: Backend> Drop for Manager<B> {
    fn drop(&mut self) {
        // Dropping Rust state revokes routes; it does not establish OS cleanup.
        for slot in self.slots.values_mut() {
            slot.revoke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Op {
        Start,
        Resume,
        Pause,
        Cancel,
        Stop,
    }
    #[derive(Default)]
    struct Calls {
        calls: Vec<(Op, Scope)>,
        permits: Vec<Permit>,
        fail: Option<Op>,
        all_revoked: bool,
    }
    #[derive(Clone)]
    struct Fake(Arc<Mutex<Calls>>);
    impl Fake {
        fn call(&self, op: Op, scope: Scope) -> Result<(), Error> {
            let mut state = self.0.lock().unwrap();
            if matches!(op, Op::Pause | Op::Cancel | Op::Stop) {
                for permit in &state.permits {
                    if state.all_revoked || permit.scope == scope {
                        assert!(
                            permit.check(permit.scope).is_err(),
                            "authority survived revocation"
                        );
                    }
                }
            }
            state.calls.push((op, scope));
            if state.fail == Some(op) {
                Err(Error::Backend)
            } else {
                Ok(())
            }
        }
    }
    impl Backend for Fake {
        fn start(&mut self, a: Allocation) -> Result<(), Error> {
            self.call(Op::Start, a.scope)
        }
        fn resume(&mut self, scope: Scope, permit: Permit) -> Result<(), Error> {
            permit.check(scope)?;
            self.0.lock().unwrap().permits.push(permit);
            self.call(Op::Resume, scope)
        }
        fn pause(&mut self, scope: Scope) -> Result<(), Error> {
            self.call(Op::Pause, scope)
        }
        fn cancel(&mut self, scope: Scope) -> Result<(), Error> {
            self.call(Op::Cancel, scope)
        }
        fn stop(&mut self, scope: Scope) -> Result<(), Error> {
            self.call(Op::Stop, scope)
        }
    }
    const UNIT: Resources = Resources {
        memory_mib: 1024,
        cpu_millis: 1000,
    };
    fn fixture() -> (Manager<Fake>, Arc<Mutex<Calls>>, Scope) {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let mut manager = Manager::new(
            Fake(calls.clone()),
            Uuid::new_v4(),
            UNIT,
            Resources {
                memory_mib: 4096,
                cpu_millis: 4000,
            },
        )
        .unwrap();
        let native = manager.snapshots()[0].allocation.scope;
        manager.observer_poll(true).unwrap();
        (manager, calls, native)
    }
    fn resume(manager: &mut Manager<Fake>, scope: Scope) -> Result<Permit, Error> {
        let offer = manager.offer_resume(scope)?;
        manager.resume(offer)
    }
    fn expire(manager: &Manager<Fake>) {
        *manager.monitor.lock().unwrap() = Some(Instant::now() - MONITOR_TIMEOUT);
    }
    #[test]
    fn default_one_and_distinct_four_scopes_refuse_fifth_before_start() {
        let (mut manager, calls, native) = fixture();
        assert_eq!(manager.snapshots().len(), 1);
        assert_eq!(manager.snapshots()[0].phase, Phase::Paused);
        let mut scopes = vec![native];
        for _ in 0..3 {
            scopes.push(manager.add_container(UNIT).unwrap());
        }
        assert_eq!(manager.add_container(UNIT), Err(Error::Limit));
        assert_eq!(calls.lock().unwrap().calls.len(), 3);
        for field in [Scope::companion, Scope::session, Scope::generation] {
            assert_eq!(
                scopes
                    .iter()
                    .copied()
                    .map(field)
                    .collect::<HashSet<_>>()
                    .len(),
                4
            );
        }
        assert!(manager.snapshots().iter().all(|s| s.phase == Phase::Paused));
    }
    #[test]
    fn preflight_refusals_never_start_or_reserve_a_guest() {
        let (mut manager, calls, _) = fixture();
        assert_eq!(
            manager.add_checked(UNIT, |_| Err(Error::Denied)),
            Err(Error::Denied)
        );
        assert_eq!(manager.snapshots().len(), 1);
        assert!(calls.lock().unwrap().calls.is_empty());
        assert_eq!(
            manager.add_checked(
                Resources {
                    memory_mib: u32::MAX,
                    ..UNIT
                },
                |_| panic!("capacity must be checked first")
            ),
            Err(Error::Capacity)
        );
        let scope = manager.add_checked(UNIT, |_| Ok(())).unwrap();
        assert_eq!(
            manager
                .snapshots()
                .iter()
                .find(|s| s.allocation.scope == scope)
                .unwrap()
                .phase,
            Phase::Paused
        );
        assert_eq!(calls.lock().unwrap().calls, vec![(Op::Start, scope)]);
    }
    #[test]
    fn capacity_and_invalid_sizes_do_not_reserve_or_start() {
        let (mut manager, calls, _) = fixture();
        for resources in [
            Resources {
                memory_mib: 0,
                ..UNIT
            },
            Resources {
                cpu_millis: 0,
                ..UNIT
            },
        ] {
            assert_eq!(manager.add_container(resources), Err(Error::Invalid));
        }
        for resources in [
            Resources {
                memory_mib: u32::MAX,
                ..UNIT
            },
            Resources {
                cpu_millis: u32::MAX,
                ..UNIT
            },
        ] {
            assert_eq!(manager.add_container(resources), Err(Error::Capacity));
        }
        assert!(calls.lock().unwrap().calls.is_empty());
        assert_eq!(manager.snapshots().len(), 1);
    }
    #[test]
    fn stop_revokes_before_cancel_then_stops_only_its_desktop() {
        let (mut manager, calls, native) = fixture();
        let second = manager.add_container(UNIT).unwrap();
        let native_permit = resume(&mut manager, native).unwrap();
        let second_permit = resume(&mut manager, second).unwrap();
        calls.lock().unwrap().calls.clear();
        manager.stop(second).unwrap();
        assert_eq!(
            calls.lock().unwrap().calls,
            vec![(Op::Cancel, second), (Op::Stop, second)]
        );
        assert_eq!(second_permit.check(second), Err(Error::Stale));
        assert!(native_permit.check(native).is_ok());
        assert_eq!(manager.snapshots().len(), 1);
        assert_eq!(manager.stop(second), Err(Error::Stale));
    }
    #[test]
    fn failed_cancel_or_stop_retains_capacity_until_both_acknowledge() {
        for failure in [Op::Cancel, Op::Stop] {
            let (mut manager, calls, native) = fixture();
            for _ in 0..3 {
                manager.add_container(UNIT).unwrap();
            }
            let permit = resume(&mut manager, native).unwrap();
            calls.lock().unwrap().fail = Some(failure);
            assert_eq!(manager.stop(native), Err(Error::Backend));
            assert_eq!(permit.check(native), Err(Error::Stale));
            assert_eq!(manager.add_container(UNIT), Err(Error::Limit));
            assert!(matches!(
                resume(&mut manager, native),
                Err(Error::NotPaused)
            ));
            calls.lock().unwrap().fail = None;
            manager.stop(native).unwrap();
            let replacement = manager.add_container(UNIT).unwrap();
            assert_ne!(native, replacement);
            assert!(permit.check(replacement).is_err());
        }
    }
    #[test]
    fn uncertain_start_keeps_slot_for_explicit_cleanup() {
        let (mut manager, calls, _) = fixture();
        calls.lock().unwrap().fail = Some(Op::Start);
        assert_eq!(manager.add_container(UNIT), Err(Error::Backend));
        let failed = manager
            .snapshots()
            .into_iter()
            .find(|s| s.phase == Phase::Quarantined)
            .unwrap();
        assert_eq!(manager.snapshots().len(), 2);
        assert!(matches!(
            resume(&mut manager, failed.allocation.scope),
            Err(Error::NotPaused)
        ));
        calls.lock().unwrap().fail = None;
        manager.stop(failed.allocation.scope).unwrap();
        assert_eq!(manager.snapshots().len(), 1);
    }
    #[test]
    fn physical_activity_revokes_every_permit_before_any_backend_callback() {
        let (mut manager, calls, native) = fixture();
        let second = manager.add_container(UNIT).unwrap();
        let first_permit = resume(&mut manager, native).unwrap();
        let second_permit = resume(&mut manager, second).unwrap();
        calls.lock().unwrap().all_revoked = true;
        calls.lock().unwrap().fail = Some(Op::Pause);
        assert_eq!(manager.physical_input(), Err(Error::Backend));
        assert!(first_permit.check(native).is_err());
        assert!(second_permit.check(second).is_err());
        assert!(manager
            .snapshots()
            .iter()
            .all(|s| s.phase == Phase::Quarantined));
        let recorded = &calls.lock().unwrap().calls;
        assert_eq!(
            recorded.iter().filter(|(op, _)| *op == Op::Cancel).count(),
            2
        );
    }
    #[test]
    fn pause_and_resume_never_revive_old_permits() {
        let (mut manager, _, native) = fixture();
        let old = resume(&mut manager, native).unwrap();
        manager.physical_input().unwrap();
        let current = resume(&mut manager, native).unwrap();
        assert_eq!(old.check(native), Err(Error::Stale));
        assert!(current.check(native).is_ok());
        let second = manager.add_container(UNIT).unwrap();
        assert_eq!(current.check(second), Err(Error::Stale));
        assert!(matches!(
            resume(&mut manager, native),
            Err(Error::NotPaused)
        ));
    }
    #[test]
    fn expired_observer_refuses_without_a_tick_and_late_poll_cannot_revive() {
        let (mut manager, _, native) = fixture();
        let old = resume(&mut manager, native).unwrap();
        expire(&manager);
        assert_eq!(old.check(native), Err(Error::MonitorUnavailable));
        manager.observer_poll(true).unwrap();
        assert_eq!(old.check(native), Err(Error::Stale));
        assert_eq!(manager.snapshots()[0].phase, Phase::Paused);
        let current = resume(&mut manager, native).unwrap();
        assert!(current.check(native).is_ok());
    }
    #[test]
    fn missing_observer_blocks_start_and_resume_and_tick_cancels_work() {
        let (mut manager, calls, native) = fixture();
        let permit = resume(&mut manager, native).unwrap();
        expire(&manager);
        manager.tick().unwrap();
        assert_eq!(permit.check(native), Err(Error::Stale));
        assert!(matches!(
            resume(&mut manager, native),
            Err(Error::MonitorUnavailable)
        ));
        assert_eq!(manager.add_container(UNIT), Err(Error::MonitorUnavailable));
        assert_eq!(manager.snapshots().len(), 1);
        assert!(calls
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(op, _)| *op == Op::Cancel));
        manager.observer_poll(true).unwrap();
        let permit = resume(&mut manager, native).unwrap();
        manager.observer_poll(false).unwrap();
        assert!(permit.check(native).is_err());
    }
    #[test]
    fn failed_resume_revokes_retained_backend_permit_and_quarantines() {
        let (mut manager, calls, native) = fixture();
        calls.lock().unwrap().fail = Some(Op::Resume);
        assert!(matches!(resume(&mut manager, native), Err(Error::Backend)));
        assert!(calls.lock().unwrap().permits[0].check(native).is_err());
        assert_eq!(manager.snapshots()[0].phase, Phase::Quarantined);
        assert_eq!(
            &calls.lock().unwrap().calls,
            &[
                (Op::Resume, native),
                (Op::Pause, native),
                (Op::Cancel, native)
            ]
        );
    }
    #[test]
    fn wrong_generation_or_session_cannot_control_an_existing_slot() {
        let (mut manager, calls, native) = fixture();
        for scope in [
            Scope {
                session: Uuid::new_v4(),
                ..native
            },
            Scope {
                generation: Uuid::new_v4(),
                ..native
            },
        ] {
            assert_eq!(manager.stop(scope), Err(Error::Stale));
            assert!(matches!(resume(&mut manager, scope), Err(Error::Stale)));
        }
        assert!(calls.lock().unwrap().calls.is_empty());
    }
    #[test]
    fn physical_input_or_monitor_loss_invalidates_an_already_paused_offer() {
        for loss in [false, true] {
            let (mut manager, calls, native) = fixture();
            let offer = manager.offer_resume(native).unwrap();
            if loss {
                manager.observer_poll(false).unwrap();
                manager.observer_poll(true).unwrap();
            } else {
                manager.physical_input().unwrap();
            }
            assert!(matches!(manager.resume(offer), Err(Error::Stale)));
            assert!(calls.lock().unwrap().calls.is_empty());
            assert!(resume(&mut manager, native).unwrap().check(native).is_ok());
        }
    }
    #[test]
    fn timely_observer_poll_preserves_running_route_but_not_duplicate_offer() {
        let (mut manager, _, native) = fixture();
        let first = manager.offer_resume(native).unwrap();
        let duplicate = manager.offer_resume(native).unwrap();
        let permit = manager.resume(first).unwrap();
        manager.observer_poll(true).unwrap();
        assert!(permit.check(native).is_ok());
        assert!(matches!(manager.resume(duplicate), Err(Error::NotPaused)));
        manager.physical_input().unwrap();
        assert_eq!(permit.check(native), Err(Error::Stale));
    }
    #[test]
    fn delayed_lease_check_cannot_authorize_a_revoked_route_after_renewal() {
        let (mut manager, _, native) = fixture();
        let old = resume(&mut manager, native).unwrap();
        expire(&manager);
        // Deterministically interleave observer renewal between the worker's
        // first revocation read and its shared lease read.
        assert_eq!(
            old.check_with_lease(native, || {
                manager.observer_poll(true).unwrap();
                fresh(&old.monitor)
            }),
            Err(Error::Stale)
        );
    }
    #[test]
    fn observer_callbacks_cannot_restamp_an_expired_sample() {
        let (mut manager, calls, native) = fixture();
        let old = resume(&mut manager, native).unwrap();
        // Model a poll delayed by callbacks without a wall-clock sleep.
        let sampled = Instant::now() - MONITOR_TIMEOUT * 2;
        *manager.monitor.lock().unwrap() = Some(sampled - MONITOR_TIMEOUT);
        assert_eq!(
            manager.observer_poll_at(true, sampled),
            Err(Error::MonitorUnavailable)
        );
        assert_eq!(*manager.monitor.lock().unwrap(), Some(sampled));
        assert_eq!(old.check(native), Err(Error::Stale));
        assert!(calls
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(op, _)| *op == Op::Cancel));
        assert!(matches!(
            resume(&mut manager, native),
            Err(Error::MonitorUnavailable)
        ));
        manager.observer_poll(true).unwrap();
        assert!(resume(&mut manager, native).unwrap().check(native).is_ok());
    }
    #[test]
    fn fast_observer_revokes_while_actor_is_unavailable() {
        let (mut manager, _, native) = fixture();
        let permit = resume(&mut manager, native).unwrap();
        let observer = manager.observer();
        // The actor is owned exclusively here; the observer does not need it.
        observer.physical_input().unwrap();
        assert_eq!(permit.check(native), Err(Error::Stale));
        observer.poll(true).unwrap();
        assert_eq!(permit.check(native), Err(Error::Stale));
        manager.tick().unwrap();
        assert_eq!(manager.snapshots()[0].phase, Phase::Paused);
        assert!(resume(&mut manager, native).unwrap().check(native).is_ok());
    }
    #[test]
    fn fast_observer_invalidates_offer_without_actor_tick() {
        let (mut manager, _, native) = fixture();
        let offer = manager.offer_resume(native).unwrap();
        manager.observer().physical_input().unwrap();
        assert!(matches!(manager.resume(offer), Err(Error::Stale)));
    }
    #[test]
    fn fast_late_poll_cannot_restore_authority_without_actor_tick() {
        let (mut manager, _, native) = fixture();
        let permit = resume(&mut manager, native).unwrap();
        expire(&manager);
        manager.observer().poll(true).unwrap();
        assert_eq!(permit.check(native), Err(Error::Stale));
    }

    #[test]
    fn dropping_manager_revokes_all_cloned_routes() {
        let (mut manager, _, native) = fixture();
        let permit = resume(&mut manager, native).unwrap();
        let clone = permit.clone();
        drop(manager);
        assert_eq!(permit.check(native), Err(Error::Stale));
        assert_eq!(clone.check(native), Err(Error::Stale));
    }
}
