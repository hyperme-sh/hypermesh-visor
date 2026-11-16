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
    use std::process::Command;
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err(refused());
    }
    #[cfg(target_os = "linux")]
    let valid = {
        let result = Command::new("/usr/bin/systemctl")
            .args([
                "show",
                &format!("hypermesh-crash-retention@{uid}.service"),
                "--property=MainPID",
                "--value",
            ])
            .output()?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout).trim() == std::process::id().to_string()
    };
    #[cfg(target_os = "macos")]
    let valid = {
        let result = Command::new("/bin/launchctl")
            .args([
                "print",
                &format!("system/sh.hyperme.visor.crash-retention.{uid}"),
            ])
            .output()?;
        result.status.success()
            && String::from_utf8_lossy(&result.stdout)
                .lines()
                .any(|line| line.trim() == format!("pid = {}", std::process::id()))
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let valid = false;
    if !valid {
        return Err(refused());
    }
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
