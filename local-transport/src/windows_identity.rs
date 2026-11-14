//! Owner ACLs do not distinguish a same-user low-integrity pipe impostor.
use std::{io, os::windows::io::AsRawHandle, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*,
    Security::*,
    System::{Pipes::*, Threading::*},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "untrusted local pipe server",
    )
}
fn token(process: HANDLE) -> io::Result<Handle> {
    let mut handle = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(handle))
}
fn information(
    token: HANDLE,
    kind: TOKEN_INFORMATION_CLASS,
    minimum: usize,
) -> io::Result<Vec<usize>> {
    unsafe {
        let mut size = 0;
        GetTokenInformation(token, kind, null_mut(), 0, &mut size);
        if GetLastError() != ERROR_INSUFFICIENT_BUFFER || (size as usize) < minimum || size > 65536
        {
            return Err(refused());
        }
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(token, kind, data.as_mut_ptr().cast(), size, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(data)
    }
}
fn integrity(token: HANDLE) -> io::Result<u32> {
    let data = information(
        token,
        TokenIntegrityLevel,
        std::mem::size_of::<TOKEN_MANDATORY_LABEL>(),
    )?;
    unsafe {
        let sid = (*(data.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())).Label.Sid;
        if sid.is_null()
            || IsValidSid(sid) == 0
            || *GetSidSubAuthorityCount(sid) != 1
            || (*GetSidIdentifierAuthority(sid)).Value != [0, 0, 0, 0, 0, 16]
        {
            return Err(refused());
        }
        Ok(*GetSidSubAuthority(sid, 0))
    }
}
fn trusted_token(server: HANDLE, current: HANDLE) -> io::Result<()> {
    let owner = information(server, TokenUser, std::mem::size_of::<TOKEN_USER>())?;
    let user = information(current, TokenUser, std::mem::size_of::<TOKEN_USER>())?;
    unsafe {
        if EqualSid(
            (*(owner.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid,
        ) == 0
        {
            return Err(refused());
        }
        SetLastError(0);
        if IsTokenRestricted(server) != 0 || GetLastError() != 0 {
            return Err(refused());
        }
    }
    let level = integrity(server)?;
    if level < 0x2000 || level < integrity(current)? {
        return Err(refused());
    }
    Ok(())
}
pub(super) fn check(pipe: &impl AsRawHandle) -> io::Result<()> {
    unsafe {
        let mut pid = 0;
        if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = Handle(process);
        trusted_token(token(process.0)?.0, token(GetCurrentProcess())?.0)?;
        let mut checked = 0;
        if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut checked) == 0
            || checked != pid
            || WaitForSingleObject(process.0, 0) != WAIT_TIMEOUT
        {
            return Err(refused());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Security::Authorization::ConvertStringSidToSidW, System::SystemServices::SE_GROUP_INTEGRITY,
    };
    #[test]
    fn same_user_low_integrity_token_is_refused() {
        unsafe {
            let mut original = null_mut();
            assert_ne!(
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_DUPLICATE | TOKEN_QUERY,
                    &mut original
                ),
                0
            );
            let original = Handle(original);
            let mut low = null_mut();
            assert_ne!(
                DuplicateTokenEx(
                    original.0,
                    TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
                    null_mut(),
                    SecurityImpersonation,
                    TokenPrimary,
                    &mut low
                ),
                0
            );
            let low = Handle(low);
            let text: Vec<u16> = "S-1-16-4096".encode_utf16().chain([0]).collect();
            let mut sid = null_mut();
            assert_ne!(ConvertStringSidToSidW(text.as_ptr(), &mut sid), 0);
            let label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: sid,
                    Attributes: SE_GROUP_INTEGRITY as u32,
                },
            };
            let result = SetTokenInformation(
                low.0,
                TokenIntegrityLevel,
                (&label as *const TOKEN_MANDATORY_LABEL).cast(),
                (std::mem::size_of_val(&label) + GetLengthSid(sid) as usize) as u32,
            );
            LocalFree(sid);
            assert_ne!(result, 0);
            assert_eq!(integrity(low.0).unwrap(), 4096);
            assert!(trusted_token(low.0, original.0).is_err());
            assert!(trusted_token(original.0, original.0).is_ok());
        }
    }
}
