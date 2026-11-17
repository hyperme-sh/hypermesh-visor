//! OS-account location and OS-managed service entry. Never an interactive daemon fallback.
use crate::{refused, Owner};
use std::{io, path::PathBuf};

#[cfg(unix)]
pub fn home() -> io::Result<PathBuf> {
    use std::{
        ffi::{CStr, OsStr},
        os::unix::ffi::OsStrExt,
    };
    let mut bytes = vec![0u8; 65536];
    let mut user: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut user,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || user.pw_dir.is_null() {
        return Err(refused());
    }
    Ok(PathBuf::from(OsStr::from_bytes(
        unsafe { CStr::from_ptr(user.pw_dir) }.to_bytes(),
    )))
}
#[cfg(windows)]
pub fn home() -> io::Result<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::TOKEN_QUERY,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    #[link(name = "userenv")]
    extern "system" {
        fn GetUserProfileDirectoryW(token: HANDLE, directory: *mut u16, size: *mut u32) -> i32;
    }
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0u16; 32768];
        let mut size = bytes.len() as u32;
        let result = GetUserProfileDirectoryW(token, bytes.as_mut_ptr(), &mut size);
        CloseHandle(token);
        if result == 0 || size == 0 || size as usize > bytes.len() {
            return Err(refused());
        }
        Ok(PathBuf::from(OsString::from_wide(
            &bytes[..size as usize - 1],
        )))
    }
}
#[cfg(unix)]
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn stopped(_: i32) {
    STOP.store(true, std::sync::atomic::Ordering::Release);
}
#[cfg(unix)]
pub fn serve() -> io::Result<()> {
    if Owner::open()?.authority() != crate::Authority::Visor {
        return Err(refused());
    }
    verify_service(std::process::id())?;
    unsafe {
        libc::signal(libc::SIGTERM, stopped as *const () as usize);
        libc::signal(libc::SIGINT, stopped as *const () as usize);
    }
    Owner::open()?.service(|| STOP.load(std::sync::atomic::Ordering::Acquire))
}
#[cfg(windows)]
pub fn serve() -> io::Result<()> {
    use windows_sys::Win32::System::Services::*;
    let mut name: Vec<u16> = "HypermeshCrashRetention"
        .encode_utf16()
        .chain([0])
        .collect();
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(windows)]
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(windows)]
unsafe extern "system" fn control(code: u32) {
    use windows_sys::Win32::System::Services::*;
    if code == SERVICE_CONTROL_STOP || code == SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true, std::sync::atomic::Ordering::Release);
    }
}
#[cfg(windows)]
unsafe extern "system" fn service_main(_: u32, _: *mut *mut u16) {
    use windows_sys::Win32::System::Services::*;
    let name: Vec<u16> = "HypermeshCrashRetention"
        .encode_utf16()
        .chain([0])
        .collect();
    let handle = RegisterServiceCtrlHandlerW(name.as_ptr(), Some(control));
    if handle.is_null() {
        return;
    }
    let mut status: SERVICE_STATUS = std::mem::zeroed();
    status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    status.dwCurrentState = SERVICE_RUNNING;
    status.dwControlsAccepted = SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN;
    if SetServiceStatus(handle, &status) == 0 {
        return;
    }
    let result = Owner::open()
        .and_then(|owner| owner.service(|| STOP.load(std::sync::atomic::Ordering::Acquire)));
    status.dwCurrentState = SERVICE_STOPPED;
    status.dwControlsAccepted = 0;
    status.dwWin32ExitCode = if result.is_ok() { 0 } else { 1 };
    SetServiceStatus(handle, &status);
}

/// Consult the OS manager at admission, independently of account-writable files.
/// Cache only successful checks for at most one second; the lease and heartbeat
/// and live IPC are still checked on every write. A different PID never uses a cached result.
pub fn verify_service(pid: u32) -> io::Result<()> {
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };
    static CHECK: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
    let mut checked = CHECK.lock().map_err(|_| refused())?;
    if checked
        .as_ref()
        .is_some_and(|(old, at)| *old == pid && at.elapsed() < Duration::from_secs(1))
    {
        return Ok(());
    }
    *checked = None;
    verify_service_uncached(pid)?;
    *checked = Some((pid, Instant::now()));
    Ok(())
}
#[cfg(unix)]
fn verify_service_uncached(pid: u32) -> io::Result<()> {
    use std::process::Command;
    let uid = unsafe { libc::geteuid() };
    if uid == 0 || pid == 0 {
        return Err(refused());
    }
    #[cfg(target_os = "linux")]
    let valid = {
        use std::os::unix::fs::MetadataExt;
        let result = manager_output(Command::new("/usr/bin/systemctl").args([
            "show",
            &format!("hypermesh-crash-retention@{uid}.service"),
            "--property=MainPID",
            "--value",
        ]))?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout).trim() == pid.to_string()
            && std::fs::metadata(format!("/proc/{pid}"))?.uid() == uid
    };
    #[cfg(target_os = "macos")]
    let valid = {
        let result = manager_output(Command::new("/bin/launchctl").args([
            "print",
            &format!("system/sh.hyperme.visor.crash-retention.{uid}"),
        ]))?;
        let owner =
            manager_output(Command::new("/bin/ps").args(["-o", "uid=", "-p", &pid.to_string()]))?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout)
                .lines()
                .any(|line| line.trim() == format!("pid = {pid}"))
            && owner.status.success()
            && String::from_utf8_lossy(&owner.stdout).trim() == uid.to_string()
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let valid = false;
    if valid {
        Ok(())
    } else {
        Err(refused())
    }
}
#[cfg(windows)]
fn verify_service_uncached(pid: u32) -> io::Result<()> {
    use hypermesh_local_storage::{windows_process_user_sid, windows_user_sid};
    use std::ptr::null;
    use windows_sys::Win32::System::Services::*;
    if pid == 0 {
        return Err(refused());
    }
    let sid = windows_user_sid()?;
    if ["S-1-5-18", "S-1-5-19", "S-1-5-20"].contains(&sid.as_str())
        || windows_process_user_sid(pid)? != sid
    {
        return Err(refused());
    }
    let name: Vec<u16> = format!("HypermeshCrashRetention-{sid}")
        .encode_utf16()
        .chain([0])
        .collect();
    unsafe {
        let manager = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err(io::Error::last_os_error());
        }
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        CloseServiceHandle(manager);
        if service.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut needed = 0;
        let ok = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of_val(&status) as u32,
            &mut needed,
        );
        CloseServiceHandle(service);
        if ok == 0 || status.dwCurrentState != SERVICE_RUNNING || status.dwProcessId != pid {
            return Err(refused());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn manager_output(command: &mut std::process::Command) -> io::Result<std::process::Output> {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(refused());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Separate Host service identity; the Visor verifier still refuses root.
pub fn verify_host_service(pid: u32) -> io::Result<()> {
    crate::host::require_root()?;
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };
    static CHECK: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
    let mut checked = CHECK.lock().map_err(|_| refused())?;
    if checked
        .as_ref()
        .is_some_and(|(old, at)| *old == pid && at.elapsed() < Duration::from_secs(1))
    {
        return Ok(());
    }
    *checked = None;
    verify_host_uncached(pid)?;
    *checked = Some((pid, Instant::now()));
    Ok(())
}
#[cfg(unix)]
fn verify_host_uncached(pid: u32) -> io::Result<()> {
    use std::process::Command;
    if pid == 0 {
        return Err(refused());
    }
    #[cfg(target_os = "linux")]
    let valid = {
        use std::os::unix::fs::MetadataExt;
        let result = manager_output(Command::new("/usr/bin/systemctl").args([
            "show",
            "hypermesh-host-crash-retention.service",
            "--property=MainPID",
            "--value",
        ]))?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout).trim() == pid.to_string()
            && std::fs::metadata(format!("/proc/{pid}"))?.uid() == 0
    };
    #[cfg(target_os = "macos")]
    let valid = {
        let result = manager_output(
            Command::new("/bin/launchctl")
                .args(["print", "system/sh.hyperme.host.crash-retention"]),
        )?;
        let owner =
            manager_output(Command::new("/bin/ps").args(["-o", "uid=", "-p", &pid.to_string()]))?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout)
                .lines()
                .any(|line| line.trim() == format!("pid = {pid}"))
            && owner.status.success()
            && String::from_utf8_lossy(&owner.stdout).trim() == "0"
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let valid = false;
    if valid {
        Ok(())
    } else {
        Err(refused())
    }
}
#[cfg(not(unix))]
fn verify_host_uncached(_: u32) -> io::Result<()> {
    Err(refused())
}
#[cfg(unix)]
pub fn serve_host() -> io::Result<()> {
    crate::host::require_root()?;
    let owner = Owner::open()?;
    if owner.authority() != crate::Authority::Host {
        return Err(refused());
    }
    verify_host_service(std::process::id())?;
    unsafe {
        libc::signal(libc::SIGTERM, stopped as *const () as usize);
        libc::signal(libc::SIGINT, stopped as *const () as usize);
    }
    owner.service(|| STOP.load(std::sync::atomic::Ordering::Acquire))
}
#[cfg(not(unix))]
pub fn serve_host() -> io::Result<()> {
    Err(refused())
}
