//! Consent ownership inside the native handler, before Crashpad starts writing.
#[allow(dead_code)]
#[path = "../../../../src/health_consent.rs"]
mod health_consent;
use health_consent::{Scope, Store};
use hypermesh_local_storage as storage;
use std::{
    ffi::{c_char, c_void, CStr},
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Guard {
    file: File,
    finished: Arc<AtomicBool>,
    monitor: Option<thread::JoinHandle<()>>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
        drop(storage::UnlockOnDrop(&self.file));
    }
}
fn refused() -> io::Error {
    io::Error::other("native capture refused")
}

fn prepare(database: &Path) -> io::Result<(Store, PathBuf, Vec<u8>)> {
    if !database.is_absolute() || database.file_name().and_then(|s| s.to_str()) != Some("database")
    {
        return Err(refused());
    }
    storage::check_dir(database)?;
    let run = database.parent().ok_or_else(refused)?;
    storage::check_dir(run)?;
    let name = run
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(refused)?;
    let id = Uuid::parse_str(name).map_err(|_| refused())?;
    if id.is_nil() || id.to_string() != name {
        return Err(refused());
    }
    let root = run.parent().ok_or_else(refused)?;
    if root.file_name().and_then(|s| s.to_str()) != Some("crash-runs") {
        return Err(refused());
    }
    storage::check_dir(root)?;
    let directory = root.parent().ok_or_else(refused)?;
    storage::check_dir(directory)?;
    let mut bytes = Vec::new();
    storage::open(
        &run.join("ticket.json"),
        storage::Open::Read { private: true },
    )?
    .take(4097)
    .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(refused());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| refused())?;
    let scope: Scope = serde_json::from_value(value["scope"].clone()).map_err(|_| refused())?;
    let store = Store::open_existing(&directory.join("health-consent"), scope)?;
    if store.restore_crash_ticket(&bytes)?.is_none() {
        return Err(refused());
    }
    Ok((store, run.to_owned(), bytes))
}

fn acquire(database: &Path) -> io::Result<(File, PathBuf)> {
    let (store, run, bytes) = prepare(database)?;
    // The application still owns the capture lock while the SDK prepares its
    // database. This signal lets it release that lock while SDK initialization
    // waits for the handler's normal startup handshake.
    let mut ready = storage::open(&run.join("handler-ready"), storage::Open::New)?;
    ready.write_all(b"visor-crash-guard-v1")?;
    ready.sync_all()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        match store.capture_lock() {
            Ok(file) => break file,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Err(_) => return Err(refused()),
        }
    };
    // Withdrawal or a scope reset may have won the lock handoff. Refuse before
    // entering Crashpad; never restore or grant a category from the run record.
    if store.restore_crash_ticket(&bytes)?.is_none() {
        return Err(refused());
    }
    // The application may have canceled and purged a delayed startup while we
    // waited for the lock. Never recreate that run or write after its cleanup.
    if prepare(database)?.2 != bytes || stop_requested(&run)? {
        return Err(refused());
    }
    Ok((file, run))
}

fn stop_requested(run: &Path) -> io::Result<bool> {
    storage::check_dir(run)?;
    match storage::open(
        &run.join("stop-handler"),
        storage::Open::Read { private: true },
    ) {
        Ok(mut file) => {
            let mut bytes = [0; 5];
            Ok(file.read(&mut bytes)? == 4 && &bytes[..4] == b"stop")
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
fn monitor(run: &Path, finished: &AtomicBool, terminate: impl FnOnce()) {
    while !finished.load(Ordering::Acquire) {
        // Losing control of the private directory also stops capture.
        if stop_requested(run).unwrap_or(true) {
            terminate();
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn terminate() -> ! {
    #[cfg(unix)]
    unsafe {
        libc::_exit(0)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), 0);
        // TerminateProcess on the current process does not normally return.
        std::process::abort()
    }
}

/// Called by the handler entry point before any Crashpad database operation.
/// Null means it must exit without starting the native handler.
#[no_mangle]
pub unsafe extern "C" fn visor_capture_enter(database: *const c_char) -> *mut c_void {
    std::panic::catch_unwind(|| {
        if database.is_null() {
            return std::ptr::null_mut();
        }
        let Ok(path) = CStr::from_ptr(database).to_str() else {
            return std::ptr::null_mut();
        };
        let Ok((file, run)) = acquire(Path::new(path)) else {
            return std::ptr::null_mut();
        };
        let finished = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&finished);
        let active = run.join("handler-active");
        let Ok(monitor) = thread::Builder::new()
            .name("visor-capture-control".into())
            .spawn(move || monitor(&run, &signal, || terminate()))
        else {
            return std::ptr::null_mut();
        };
        let guard = Box::new(Guard {
            file,
            finished,
            monitor: Some(monitor),
        });
        let activated = (|| -> io::Result<()> {
            let mut file = storage::open(&active, storage::Open::New)?;
            file.write_all(b"active")?;
            file.sync_all()
        })();
        if activated.is_err() {
            return std::ptr::null_mut();
        }
        Box::into_raw(guard).cast()
    })
    .unwrap_or(std::ptr::null_mut())
}
#[no_mangle]
pub unsafe extern "C" fn visor_capture_leave(guard: *mut c_void) {
    if !guard.is_null() {
        drop(Box::from_raw(guard.cast::<Guard>()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use health_consent::Category;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Temp, Store, PathBuf) {
        let root = Temp(std::env::temp_dir().join(format!("visor-handler-{}", Uuid::new_v4())));
        storage::create_dir(&root.0).unwrap();
        let scope = Scope {
            subject: Uuid::new_v4(),
            tenant: Uuid::new_v4(),
            device: Uuid::new_v4(),
            notice_version: Uuid::new_v4(),
            purpose_version: Uuid::new_v4(),
        };
        let store = Store::open(&root.0.join("health-consent"), scope).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let runs = root.0.join("crash-runs");
        storage::create_dir(&runs).unwrap();
        let run = runs.join(Uuid::new_v4().to_string());
        storage::create_dir(&run).unwrap();
        storage::create_dir(&run.join("database")).unwrap();
        storage::open(&run.join("ticket.json"), storage::Open::New)
            .unwrap()
            .write_all(
                &serde_json::to_vec(&store.capture(Category::Crashes).unwrap().unwrap()).unwrap(),
            )
            .unwrap();
        (root, store, run)
    }
    #[test]
    fn handler_holds_capture_lock_and_refuses_stale_metadata() {
        let (_root, store, run) = fixture();
        let (file, _) = acquire(&run.join("database")).unwrap();
        assert!(store.decide(Category::Crashes, false).is_err());
        store.decide(Category::Sensors, true).unwrap();
        drop(storage::UnlockOnDrop(&file));
        store.decide(Category::Crashes, false).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        assert!(prepare(&run.join("database")).is_err());
    }
    #[test]
    fn withdrawal_refuses_before_creating_ready_signal() {
        let (_root, store, run) = fixture();
        store.decide(Category::Crashes, false).unwrap();
        assert!(acquire(&run.join("database")).is_err());
        assert!(!run.join("handler-ready").exists());
    }
    #[test]
    fn opening_wrong_scope_cannot_reset_existing_consent() {
        let (_root, store, run) = fixture();
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run.join("ticket.json")).unwrap()).unwrap();
        metadata["scope"]["device"] = serde_json::json!(Uuid::new_v4());
        let scope: Scope = serde_json::from_value(metadata["scope"].clone()).unwrap();
        let directory = run
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("health-consent");
        assert!(Store::open_existing(&directory, scope).is_err());
        assert!(store.capture(Category::Crashes).unwrap().is_some());
    }
    #[test]
    fn canceled_capture_never_enters_the_writer() {
        let (_root, store, run) = fixture();
        storage::open(&run.join("stop-handler"), storage::Open::New)
            .unwrap()
            .write_all(b"stop")
            .unwrap();
        assert!(acquire(&run.join("database")).is_err());
        store.decide(Category::Crashes, false).unwrap();
    }
    #[test]
    fn purged_handoff_cannot_recreate_the_run() {
        let (_root, store, run) = fixture();
        let lease = store.capture_lease().unwrap().unwrap();
        thread::scope(|scope| {
            let waiting = scope.spawn(|| acquire(&run.join("database")));
            let deadline = Instant::now() + Duration::from_secs(2);
            while !run.join("handler-ready").exists() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            std::fs::remove_dir_all(&run).unwrap();
            drop(lease);
            assert!(waiting.join().unwrap().is_err());
        });
        assert!(!run.exists());
    }
    #[test]
    fn private_stop_request_invokes_only_the_injected_termination() {
        let (_root, _store, run) = fixture();
        storage::open(&run.join("stop-handler"), storage::Open::New)
            .unwrap()
            .write_all(b"stop")
            .unwrap();
        let mut stopped = false;
        monitor(&run, &AtomicBool::new(false), || stopped = true);
        assert!(stopped);
    }
}
