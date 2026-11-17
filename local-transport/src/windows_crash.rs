//! Separate LocalSystem crash transport. Same-user IPC policy is unchanged.
use std::{
    fs::OpenOptions,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, IntoRawHandle},
    },
    ptr::{null, null_mut},
    time::Duration,
};
use tokio::net::windows::named_pipe::NamedPipeClient;
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Pipes::*, Services::*, Threading::*},
};

pub const WINDOWS_CRASH_PIPE: &str = r"\\.\pipe\hypermesh-crash-registration-v1";
const SERVICE: &str = "HypermeshCrashRegistration";
// Specific data/attribute rights; FILE_APPEND_DATA would permit server creation.
const CLIENT_ACCESS: u32 = 0x12019b;
struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "untrusted crash registration service",
    )
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn status(service: SC_HANDLE) -> io::Result<SERVICE_STATUS_PROCESS> {
    let mut result: SERVICE_STATUS_PROCESS = unsafe { zeroed() };
    let mut needed = 0;
    if unsafe {
        QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            (&mut result as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    } == 0
        || result.dwCurrentState != SERVICE_RUNNING
        || result.dwServiceType != SERVICE_WIN32_OWN_PROCESS
        || result.dwProcessId == 0
    {
        return Err(refused());
    }
    Ok(result)
}
fn system_configuration(service: SC_HANDLE) -> io::Result<()> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfigW(service, null_mut(), 0, &mut needed);
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || needed < size_of::<QUERY_SERVICE_CONFIGW>() as u32
        || needed > 65536
    {
        return Err(refused());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe { QueryServiceConfigW(service, config, needed, &mut needed) } == 0 {
        return Err(refused());
    }
    let config = unsafe { &*config };
    if config.dwServiceType != SERVICE_WIN32_OWN_PROCESS {
        return Err(refused());
    }
    // The account string must be bounded by the returned configuration buffer.
    let start = config.lpServiceStartName as usize;
    let base = buffer.as_ptr() as usize;
    let end = base + buffer.len() * size_of::<usize>();
    if start < base || start >= end || start % 2 != 0 {
        return Err(refused());
    }
    let account =
        unsafe { std::slice::from_raw_parts(config.lpServiceStartName, (end - start) / 2) };
    let length = account.iter().position(|c| *c == 0).ok_or_else(refused)?;
    if account[..length] != wide("LocalSystem")[..11] {
        return Err(refused());
    }
    let mut sid: SERVICE_SID_INFO = unsafe { zeroed() };
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_SERVICE_SID_INFO,
            (&mut sid as *mut SERVICE_SID_INFO).cast(),
            size_of::<SERVICE_SID_INFO>() as u32,
            &mut needed,
        )
    } == 0
        || sid.dwServiceSidType != 0
    {
        return Err(refused());
    }
    Ok(())
}
fn verify(pipe: &NamedPipeClient) -> io::Result<()> {
    unsafe {
        let manager = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return Err(refused());
        }
        let manager = Service(manager);
        let service = OpenServiceW(
            manager.0,
            wide(SERVICE).as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
        );
        if service.is_null() {
            return Err(refused());
        }
        let service = Service(service);
        system_configuration(service.0)?;
        let before = status(service.0)?;
        let mut pid = 0;
        if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) == 0
            || pid != before.dwProcessId
        {
            return Err(refused());
        }
        // Holding a process handle prevents PID reuse during verification.
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(refused());
        }
        let process = Process(process);
        system_configuration(service.0)?;
        let after = status(service.0)?;
        let mut checked = 0;
        let mut exit = 0;
        if after.dwProcessId != pid
            || GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut checked) == 0
            || checked != pid
            || GetExitCodeProcess(process.0, &mut exit) == 0
            || exit != STILL_ACTIVE as u32
        {
            return Err(refused());
        }
    }
    Ok(())
}

/// Connect only to the fixed local endpoint and authenticate its SCM owner before
/// any request bytes are sent. The client permits identification, not delegation.
pub async fn connect_crash_registration() -> io::Result<NamedPipeClient> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .access_mode(CLIENT_ACCESS)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .security_qos_flags(SECURITY_IDENTIFICATION)
                .open(WINDOWS_CRASH_PIPE)
            {
                Ok(file) => file,
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                Err(_) => return Err(refused()),
            };
            let pipe = unsafe { NamedPipeClient::from_raw_handle(file.into_raw_handle()) }?;
            verify(&pipe)?;
            return Ok(pipe);
        }
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "crash registration service unavailable",
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::ServerOptions,
    };
    #[test]
    fn client_cannot_create_server_instances() {
        assert_eq!(CLIENT_ACCESS & FILE_APPEND_DATA, 0);
    }
    #[tokio::test]
    async fn same_user_pipe_impostor_is_refused_before_write() {
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(WINDOWS_CRASH_PIPE)
            .unwrap();
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut raw = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(3), server.read(&mut raw)).await {
                Ok(Ok(0)) | Ok(Err(_)) => (),
                other => panic!("unexpected bytes sent to impostor: {other:?}"),
            }
        });
        assert!(connect_crash_registration().await.is_err());
        task.await.unwrap();
    }
    #[tokio::test]
    #[ignore = "requires disposable CI LocalSystem service fixture"]
    async fn windows_crash_registration_service_contract() {
        assert_eq!(
            std::env::var("HYPERMESH_CRASH_SERVICE_CONTRACT").unwrap(),
            "1"
        );
        let mut pipe = connect_crash_registration().await.unwrap();
        pipe.write_all(b"unit metadata").await.unwrap();
        let mut raw = [0u8; 13];
        tokio::time::timeout(Duration::from_secs(3), pipe.read_exact(&mut raw))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&raw, b"unit accepted");
    }
}
