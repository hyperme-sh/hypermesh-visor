use super::{refused, Open};
use std::{
    ffi::c_void,
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{SystemServices::ACCESS_ALLOWED_ACE_TYPE, Threading::*, IO::OVERLAPPED},
};

struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(refused());
    }
    value.push(0);
    Ok(value)
}
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    if sid.is_null() || IsValidSid(sid) == 0 {
        return Err(refused());
    }
    let mut text = null_mut();
    if ConvertSidToStringSidW(sid, &mut text) == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = Allocation(text.cast());
    let mut count = 0;
    while *text.add(count) != 0 {
        count += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(text, count)).map_err(|_| refused())
}
fn current_user() -> io::Result<String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut size = 0;
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut size);
        if size == 0 || size > 65536 {
            return Err(refused());
        }
        // TOKEN_USER contains pointers; use aligned storage for the variable-size result.
        let mut bytes = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            bytes.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        sid_text((*(bytes.as_ptr().cast::<TOKEN_USER>())).User.Sid)
    }
}
fn descriptor() -> io::Result<Allocation> {
    let user = current_user()?;
    let sddl: Vec<u16> = format!("O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Allocation(descriptor))
}
fn validate(file: &File, directory: bool, private: bool) -> io::Result<()> {
    unsafe {
        let handle = file.as_raw_handle();
        if GetFileType(handle) != FILE_TYPE_DISK {
            return Err(refused());
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(handle, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks != 1)
        {
            return Err(refused());
        }
        let mut owner = null_mut();
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        let result = GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        );
        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let _allocation = Allocation(descriptor);
        let user = current_user()?;
        if sid_text(owner)? != user {
            return Err(refused());
        }
        if private {
            // A NULL DACL grants everyone access. Unknown ACE forms fail closed.
            if acl.is_null() || IsValidAcl(acl) == 0 {
                return Err(refused());
            }
            for index in 0..(*acl).AceCount as u32 {
                let mut ace = null_mut();
                if GetAce(acl, index, &mut ace) == 0 {
                    return Err(refused());
                }
                let header = &*ace.cast::<ACE_HEADER>();
                if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
                    || (header.AceSize as usize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
                {
                    return Err(refused());
                }
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid = std::ptr::addr_of!(allowed.SidStart) as PSID;
                if IsValidSid(sid) == 0
                    || GetLengthSid(sid) as usize
                        > header.AceSize as usize
                            - std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart)
                {
                    return Err(refused());
                }
                let trustee = sid_text(sid)?;
                if trustee != user && trustee != "S-1-5-18" && trustee != "S-1-5-32-544" {
                    return Err(refused());
                }
            }
        }
        Ok(())
    }
}
pub fn open(path: &Path, mode: Open) -> io::Result<File> {
    let path = wide(path)?;
    let descriptor = descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let (access, creation, private) = match mode {
        Open::Read { private } => (GENERIC_READ | READ_CONTROL, OPEN_EXISTING, private),
        Open::Lock => (
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
            OPEN_ALWAYS,
            true,
        ),
        Open::New => (
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
            CREATE_NEW,
            true,
        ),
    };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            creation,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate(&file, false, private)?;
    Ok(file)
}
pub fn check_dir(path: &Path) -> io::Result<()> {
    let path = wide(path)?;
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate(&file, true, true)
}
pub fn create_dir(path: &Path) -> io::Result<bool> {
    let name = wide(path)?;
    let descriptor = descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let created = if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } != 0 {
        true
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
            false
        } else {
            return Err(error);
        }
    };
    check_dir(path)?;
    Ok(created)
}
pub fn lock(file: &File) -> io::Result<()> {
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn unlock(file: &File) {
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    unsafe {
        UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped);
    }
}
pub fn replace(source: &Path, target: &Path) -> io::Result<()> {
    if unsafe {
        MoveFileExW(
            wide(source)?.as_ptr(),
            wide(target)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
// File contents are flushed before MoveFileExW's write-through replacement.
pub fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn broad_acl_and_directory_junction_are_refused() {
        let directory = crate::tests::Temp::new();
        let path = directory.0.join("data");
        let file = open(&path, Open::New).unwrap();
        let user = current_user().unwrap();
        let sddl: Vec<u16> = format!("D:P(A;;FA;;;{user})(A;;FR;;;WD)")
            .encode_utf16()
            .chain([0])
            .collect();
        unsafe {
            let mut sd = null_mut();
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    null_mut()
                ),
                0
            );
            let _allocation = Allocation(sd);
            let mut present = 0;
            let mut acl = null_mut();
            let mut defaulted = 0;
            assert_ne!(
                GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted),
                0
            );
            // The creating owner has WRITE_DAC, but this handle was deliberately
            // opened with data rights only. Reopen for the test's explicit ACL change.
            let handle = CreateFileW(
                wide(&path).unwrap().as_ptr(),
                WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            );
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            let handle = Handle(handle);
            assert_eq!(
                SetSecurityInfo(
                    handle.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    acl,
                    null_mut()
                ),
                ERROR_SUCCESS
            );
        }
        assert!(validate(&file, false, true).is_err());
        assert!(open(&path, Open::Read { private: true }).is_err());
        assert!(open(&path, Open::Read { private: false }).is_ok());
        let junction = directory.0.join("junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&directory.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "junction fixture could not be created"
        );
        assert!(check_dir(&junction).is_err());
        std::fs::remove_dir(junction).unwrap();
    }
}
