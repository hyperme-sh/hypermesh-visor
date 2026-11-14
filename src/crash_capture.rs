//! Native SDK ownership. The provisioned DSN never enters this module.
use crate::health_consent::Store;
use hypermesh_local_storage as storage;
use libloading::Library;
use std::{
    ffi::{c_char, c_void, CString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

static ATTEMPTED: AtomicBool = AtomicBool::new(false);
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
struct Active {
    sdk: &'static Sdk,
    store: Store,
    run: PathBuf,
}
type Options = *mut c_void;
type SetString = unsafe extern "C" fn(Options, *const c_char);
type SetInt = unsafe extern "C" fn(Options, i32);
struct Sdk {
    _library: Option<Library>,
    new: unsafe extern "C" fn() -> Options,
    init: unsafe extern "C" fn(Options) -> i32,
    close: unsafe extern "C" fn() -> i32,
    dsn: SetString,
    proxy: SetString,
    consent: SetInt,
    session: SetInt,
    debug: SetInt,
    cache: SetInt,
    breadcrumbs: unsafe extern "C" fn(Options, usize),
    #[cfg(unix)]
    database: SetString,
    #[cfg(unix)]
    handler: SetString,
    #[cfg(windows)]
    database: unsafe extern "C" fn(Options, *const u16),
    #[cfg(windows)]
    handler: unsafe extern "C" fn(Options, *const u16),
}
fn refused() -> io::Error {
    io::Error::other("native crash capture unavailable")
}
unsafe fn symbol<T: Copy>(library: &Library, name: &[u8]) -> io::Result<T> {
    library
        .get::<T>(name)
        .map(|value| *value)
        .map_err(|_| refused())
}
#[cfg(all(target_os = "macos", feature = "native-crash-static"))]
mod linked {
    use super::{c_char, Options};
    unsafe extern "C" {
        pub fn sentry_visor_capture_policy_version() -> i32;
        pub fn sentry_options_new() -> Options;
        pub fn sentry_init(options: Options) -> i32;
        pub fn sentry_close() -> i32;
        pub fn sentry_options_set_dsn(options: Options, value: *const c_char);
        pub fn sentry_options_set_proxy(options: Options, value: *const c_char);
        pub fn sentry_options_set_require_user_consent(options: Options, value: i32);
        pub fn sentry_options_set_auto_session_tracking(options: Options, value: i32);
        pub fn sentry_options_set_debug(options: Options, value: i32);
        pub fn sentry_options_set_cache_keep(options: Options, value: i32);
        pub fn sentry_options_set_max_breadcrumbs(options: Options, value: usize);
        pub fn sentry_options_set_database_path(options: Options, value: *const c_char);
        pub fn sentry_options_set_handler_path(options: Options, value: *const c_char);
    }
}
impl Sdk {
    #[cfg(all(target_os = "macos", feature = "native-crash-static"))]
    unsafe fn load(_path: &Path) -> io::Result<Self> {
        use linked::*;
        if sentry_visor_capture_policy_version() != 1 {
            return Err(refused());
        }
        Ok(Self {
            _library: None,
            new: sentry_options_new,
            init: sentry_init,
            close: sentry_close,
            dsn: sentry_options_set_dsn,
            proxy: sentry_options_set_proxy,
            consent: sentry_options_set_require_user_consent,
            session: sentry_options_set_auto_session_tracking,
            debug: sentry_options_set_debug,
            cache: sentry_options_set_cache_keep,
            breadcrumbs: sentry_options_set_max_breadcrumbs,
            database: sentry_options_set_database_path,
            handler: sentry_options_set_handler_path,
        })
    }
    #[cfg(not(all(target_os = "macos", feature = "native-crash-static")))]
    unsafe fn load(path: &Path) -> io::Result<Self> {
        let library = Library::new(path).map_err(|_| refused())?;
        let policy: unsafe extern "C" fn() -> i32 =
            symbol(&library, b"sentry_visor_capture_policy_version\0")?;
        if policy() != 1 {
            return Err(refused());
        }
        Ok(Self {
            new: symbol(&library, b"sentry_options_new\0")?,
            init: symbol(&library, b"sentry_init\0")?,
            close: symbol(&library, b"sentry_close\0")?,
            dsn: symbol(&library, b"sentry_options_set_dsn\0")?,
            proxy: symbol(&library, b"sentry_options_set_proxy\0")?,
            consent: symbol(&library, b"sentry_options_set_require_user_consent\0")?,
            session: symbol(&library, b"sentry_options_set_auto_session_tracking\0")?,
            debug: symbol(&library, b"sentry_options_set_debug\0")?,
            cache: symbol(&library, b"sentry_options_set_cache_keep\0")?,
            breadcrumbs: symbol(&library, b"sentry_options_set_max_breadcrumbs\0")?,
            #[cfg(unix)]
            database: symbol(&library, b"sentry_options_set_database_path\0")?,
            #[cfg(unix)]
            handler: symbol(&library, b"sentry_options_set_handler_path\0")?,
            #[cfg(windows)]
            database: symbol(&library, b"sentry_options_set_database_pathw\0")?,
            #[cfg(windows)]
            handler: symbol(&library, b"sentry_options_set_handler_pathw\0")?,
            _library: Some(library),
        })
    }
    unsafe fn initialize(&self, database: &Path, handler: &Path) -> io::Result<()> {
        #[cfg(unix)]
        let (database, handler) = {
            use std::os::unix::ffi::OsStrExt;
            (
                CString::new(database.as_os_str().as_bytes()).map_err(|_| refused())?,
                CString::new(handler.as_os_str().as_bytes()).map_err(|_| refused())?,
            )
        };
        #[cfg(windows)]
        let (database, handler) = {
            use std::os::windows::ffi::OsStrExt;
            let encode = |path: &Path| -> io::Result<Vec<u16>> {
                let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
                if value.contains(&0) {
                    return Err(refused());
                }
                value.push(0);
                Ok(value)
            };
            (encode(database)?, encode(handler)?)
        };
        let options = (self.new)();
        if options.is_null() {
            return Err(refused());
        }
        (self.dsn)(options, c"".as_ptr());
        (self.proxy)(options, c"".as_ptr());
        (self.consent)(options, 1);
        (self.session)(options, 0);
        (self.debug)(options, 0);
        (self.cache)(options, 0);
        (self.breadcrumbs)(options, 0);
        (self.database)(options, database.as_ptr());
        (self.handler)(options, handler.as_ptr());
        // sentry_init takes ownership of options, including on failure.
        if (self.init)(options) == 0 {
            Ok(())
        } else {
            Err(refused())
        }
    }
}

fn ready(run: &Path) -> io::Result<bool> {
    match storage::open(
        &run.join("handler-ready"),
        storage::Open::Read { private: true },
    ) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(64).read_to_end(&mut bytes)?;
            Ok(bytes == b"visor-crash-guard-v1")
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
fn request_stop(run: &Path) -> io::Result<()> {
    storage::check_dir(run)?;
    match storage::open(
        &run.join("stop-handler"),
        storage::Open::Read { private: true },
    ) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(5).read_to_end(&mut bytes)?;
            if bytes == b"stop" {
                return Ok(());
            }
            return Err(refused());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = storage::open(&run.join("stop-handler"), storage::Open::New)?;
    file.write_all(b"stop")?;
    file.sync_all()
}

fn stop_and_purge(store: &Store, run: &Path, withdraw: bool) -> io::Result<()> {
    request_stop(run)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let lock = loop {
        match store.capture_lock() {
            Ok(lock) => break lock,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Err(_) => return Err(refused()),
        }
    };
    let _unlock = storage::UnlockOnDrop(&lock);
    storage::check_dir(run)?;
    fs::remove_dir_all(run)?;
    storage::sync_dir(run.parent().ok_or_else(refused)?)?;
    if withdraw {
        store.withdraw_stopped_crash()?;
    }
    Ok(())
}

/// SDK and handler are packaged together next to the application executable.
pub(crate) fn start(store: Store, directory: &Path) -> io::Result<()> {
    let mut active = ACTIVE.lock().map_err(|_| refused())?;
    let Some(lease) = store.capture_lease()? else {
        return Ok(());
    };
    if ATTEMPTED.swap(true, Ordering::AcqRel) {
        return Err(refused());
    }
    let executable = std::env::current_exe()?;
    let executable_directory = executable.parent().ok_or_else(refused)?;
    #[cfg(target_os = "linux")]
    let sdk_root =
        if [Path::new("/usr/bin"), Path::new("/usr/local/bin")].contains(&executable_directory) {
            executable_directory
                .parent()
                .ok_or_else(refused)?
                .join("lib/hypermesh-visor/crash-sdk")
        } else {
            executable_directory.join("crash-sdk")
        };
    #[cfg(not(target_os = "linux"))]
    let sdk_root = executable_directory.join("crash-sdk");
    #[cfg(target_os = "linux")]
    let library = sdk_root.join("lib/libsentry.so");
    #[cfg(target_os = "macos")]
    let library = sdk_root.join("lib/libsentry.dylib");
    #[cfg(windows)]
    let library = sdk_root.join("bin/sentry.dll");
    #[cfg(unix)]
    let handler = sdk_root.join("bin/crashpad_handler");
    #[cfg(windows)]
    let handler = sdk_root.join("bin/crashpad_handler.exe");
    if !handler.is_file() {
        return Err(refused());
    }
    let sdk = unsafe { Sdk::load(&library)? };
    storage::check_dir(directory)?;
    let root = directory.join("crash-runs");
    storage::create_dir(&root)?;
    let run = root.join(Uuid::new_v4().to_string());
    storage::create_dir(&run)?;
    storage::create_dir(&run.join("database"))?;
    let mut ticket = storage::open(&run.join("ticket.json"), storage::Open::New)?;
    ticket.write_all(&serde_json::to_vec(lease.ticket()).map_err(|_| refused())?)?;
    ticket.sync_all()?;
    storage::sync_dir(&run)?;
    // Native OS callbacks may survive sentry_close. Retain the loaded library
    // until process exit even if initialization or later capture fails.
    let sdk = Box::leak(Box::new(sdk));
    let result = thread::scope(|scope| {
        let handoff = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready(&run)? {
                if Instant::now() >= deadline {
                    return Err(refused());
                }
                thread::sleep(Duration::from_millis(10));
            }
            drop(lease);
            Ok(())
        });
        let initialized = unsafe { sdk.initialize(&run.join("database"), &handler) };
        handoff.join().map_err(|_| refused())??;
        initialized
    });
    if result.is_err() {
        unsafe {
            (sdk.close)();
        }
        stop_and_purge(&store, &run, false)?;
        return Err(refused());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let activated = loop {
        if let Ok(file) = storage::open(
            &run.join("handler-active"),
            storage::Open::Read { private: true },
        ) {
            let mut bytes = Vec::new();
            if file.take(16).read_to_end(&mut bytes).is_ok() && bytes == b"active" {
                break true;
            }
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !activated {
        unsafe {
            (sdk.close)();
        }
        stop_and_purge(&store, &run, false)?;
        return Err(refused());
    }
    *active = Some(Active { sdk, store, run });
    Ok(())
}

/// Called only by an in-process local settings action. A timeout reports
/// failure; it must never be presented as successful withdrawal.
pub fn withdraw() -> io::Result<()> {
    let mut active = ACTIVE.lock().map_err(|_| refused())?;
    let capture = active.as_ref().ok_or_else(refused)?;
    unsafe {
        (capture.sdk.close)();
    }
    stop_and_purge(&capture.store, &capture.run, true)?;
    *active = None;
    Ok(())
}

/// Stop every configured run before changing the shared crash decision. This is
/// also used by the local CLI when the application is running in another process.
pub(crate) fn withdraw_configured(store: &Store, directory: &Path) -> io::Result<()> {
    let mut active = ACTIVE.lock().map_err(|_| refused())?;
    if let Some(capture) = active.as_ref() {
        if capture.run.parent().and_then(Path::parent) != Some(directory) {
            return Err(refused());
        }
        unsafe {
            (capture.sdk.close)();
        }
    }
    let root = directory.join("crash-runs");
    let mut runs = Vec::new();
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => {
            storage::check_dir(&root)?;
            for entry in fs::read_dir(&root)?.take(1025) {
                let path = entry?.path();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(refused)?;
                let id = Uuid::parse_str(name).map_err(|_| refused())?;
                if id.is_nil() || id.to_string() != name || runs.len() == 1024 {
                    return Err(refused());
                }
                storage::check_dir(&path)?;
                runs.push(path);
            }
        }
    }
    for run in &runs {
        request_stop(run)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        match store.capture_lock() {
            Ok(file) => break file,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Err(_) => return Err(refused()),
        }
    };
    let _unlock = storage::UnlockOnDrop(&file);
    for run in runs {
        fs::remove_dir_all(run)?;
    }
    if root.exists() {
        storage::sync_dir(&root)?;
    }
    store.withdraw_stopped_crash()?;
    *active = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_consent::{Category, Scope};
    #[test]
    fn disabled_category_never_loads_sdk_or_creates_capture_state() {
        let root = std::env::temp_dir().join(format!("visor-capture-off-{}", Uuid::new_v4()));
        storage::create_dir(&root).unwrap();
        let scope = Scope {
            subject: Uuid::new_v4(),
            tenant: Uuid::new_v4(),
            device: Uuid::new_v4(),
            notice_version: Uuid::new_v4(),
            purpose_version: Uuid::new_v4(),
        };
        let store = Store::open(&root.join("consent"), scope).unwrap();
        start(store, &root).unwrap();
        assert!(!root.join("crash-runs").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_requests_are_idempotent() {
        let root = std::env::temp_dir().join(format!("visor-stop-{}", Uuid::new_v4()));
        storage::create_dir(&root).unwrap();
        request_stop(&root).unwrap();
        request_stop(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_cleanup_precedes_successful_withdrawal() {
        let root = std::env::temp_dir().join(format!("visor-capture-{}", Uuid::new_v4()));
        storage::create_dir(&root).unwrap();
        let scope = Scope {
            subject: Uuid::new_v4(),
            tenant: Uuid::new_v4(),
            device: Uuid::new_v4(),
            notice_version: Uuid::new_v4(),
            purpose_version: Uuid::new_v4(),
        };
        let store = Store::open(&root.join("consent"), scope).unwrap();
        store.decide(Category::Crashes, true).unwrap();
        let run = root.join(Uuid::new_v4().to_string());
        storage::create_dir(&run).unwrap();
        storage::open(&run.join("raw"), storage::Open::New)
            .unwrap()
            .write_all(b"excluded")
            .unwrap();
        stop_and_purge(&store, &run, true).unwrap();
        assert!(!run.exists());
        assert!(store.capture(Category::Crashes).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
