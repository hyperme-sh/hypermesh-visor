//! Native default-browser launch without a command interpreter.
use crate::AuthError;

fn unavailable() -> AuthError {
    AuthError::BrowserUnavailable("system browser could not be opened".into())
}

fn browser_url(value: &str) -> Result<String, AuthError> {
    if value.len() > 16_384 || value.chars().any(char::is_control) {
        return Err(unavailable());
    }
    let parsed = url::Url::parse(value).map_err(|_| unavailable())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none()
        || !parsed.username().is_empty() || parsed.password().is_some()
    {
        return Err(unavailable());
    }
    Ok(parsed.to_string())
}

pub(crate) fn open(value: &str) -> Result<(), AuthError> {
    let target = browser_url(value)?;
    launch(target)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn launch(target: String) -> Result<(), AuthError> {
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    // The validated URL is one argument and cannot be a command-line option.
    #[cfg(target_os = "linux")]
    {
        // Some xdg-open handlers stay alive with the browser. Preserve the
        // asynchronous handoff so callback processing can start immediately.
        let mut child = std::process::Command::new(program).arg(target)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|_| unavailable())?;
        let _ = std::thread::Builder::new().name("hypermesh-browser-reap".into()).spawn(move || { let _ = child.wait(); });
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // Wait only for open, never use its --wait-apps option.
        let status = std::process::Command::new(program).arg(target)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map_err(|_| unavailable())?;
        if status.success() { Ok(()) } else { Err(unavailable()) }
    }
}

#[cfg(target_os = "windows")]
fn launch(target: String) -> Result<(), AuthError> {
    // A dedicated STA avoids changing the caller's COM apartment. No cmd.exe,
    // PowerShell, executable path or command parameters are supplied.
    std::thread::Builder::new().name("hypermesh-browser".into()).spawn(move || {
        use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        unsafe {
            let initialized = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
            if initialized < 0 { return Err(unavailable()); }
            let result = ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(),
                std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) as isize;
            CoUninitialize();
            if result > 32 { Ok(()) } else { Err(unavailable()) }
        }
    }).map_err(|_| unavailable())?.join().map_err(|_| unavailable())?
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn launch(_: String) -> Result<(), AuthError> { Err(unavailable()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_target_preserves_oauth_query_as_data() {
        let value = "https://auth.example/authorize?state=a%26b&code_challenge=c-d_1&redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback";
        assert_eq!(browser_url(value).unwrap(), value);
        assert_eq!(browser_url("http://127.0.0.1:1234/authorize").unwrap(), "http://127.0.0.1:1234/authorize");
    }
    #[test]
    fn browser_target_refuses_shell_files_custom_schemes_and_credentials() {
        for value in ["file:///tmp/private", "javascript:alert(1)", "cmd.exe /c calc", "--help",
            "ms-settings:privacy", "https://user:secret@auth.example/", "https://auth.example/\nsecret",
            "https://auth.example/\0private", "not a URL"] {
            let error = browser_url(value).unwrap_err().to_string();
            assert!(!error.contains(value));
        }
        assert!(browser_url(&format!("https://auth.example/{}", "a".repeat(16_384))).is_err());
    }
}
