//! Query one process handle, retaining it across SID and executable checks.
use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn token_user(process: HANDLE) -> Option<Vec<usize>> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = Handle(token);
        let mut size = 0;
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut size);
        if size == 0 || size > 65536 {
            return None;
        }
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            data.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return None;
        }
        Some(data)
    }
}
pub(crate) fn process_executable(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let process = Handle(process);
        let current = token_user(GetCurrentProcess())?;
        let target = token_user(process.0)?;
        let current = &*(current.as_ptr().cast::<TOKEN_USER>());
        let target = &*(target.as_ptr().cast::<TOKEN_USER>());
        if EqualSid(current.User.Sid, target.User.Sid) == 0 {
            return None;
        }
        let mut code = 0;
        if GetExitCodeProcess(process.0, &mut code) == 0 || code != STILL_ACTIVE as u32 {
            return None;
        }
        let mut length = 32768u32;
        let mut path = vec![0u16; length as usize];
        if QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) == 0 {
            return None;
        }
        let path = String::from_utf16(&path[..length as usize]).ok()?;
        std::path::Path::new(&path).is_absolute().then_some(path)
    }
}
pub(crate) fn unelevated() -> bool {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let token = Handle(token);
        let mut value = TOKEN_ELEVATION { TokenIsElevated: 1 };
        let mut size = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut value as *mut TOKEN_ELEVATION).cast::<c_void>(),
            std::mem::size_of_val(&value) as u32,
            &mut size,
        ) != 0
            && value.TokenIsElevated == 0
    }
}
