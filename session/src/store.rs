//! One keychain entry for the refresh token.
//!
//! Service `hypermesh`, account `session`. The Rust `keyring` crate writes that
//! entry to the macOS Keychain, Windows Credential Manager, or the Linux Secret
//! Service. The Go CLI uses the same service and account. Access tokens are
//! never stored here.

use std::fmt;
use std::sync::Mutex;

use crate::AuthError;

pub const SERVICE: &str = "hypermesh";
pub const ACCOUNT: &str = "session";

pub trait SessionStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError>;
    fn refresh_token(&self) -> Result<Option<String>, AuthError>;
    fn delete(&self) -> Result<(), AuthError>;
}

/// In-memory stand-in used by tests. It never touches a file or the OS keychain.
#[derive(Default)]
pub struct MemoryStore {
    token: Mutex<Option<String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let present = self
            .token
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        f.debug_struct("MemoryStore")
            .field("token", &if present { "present" } else { "absent" })
            .finish()
    }
}

impl SessionStore for MemoryStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        if token.is_empty() {
            return Err(AuthError::NoRefreshToken);
        }
        *self
            .token
            .lock()
            .map_err(|_| AuthError::message("session store lock"))? = Some(token.to_string());
        Ok(())
    }

    fn refresh_token(&self) -> Result<Option<String>, AuthError> {
        Ok(self
            .token
            .lock()
            .map_err(|_| AuthError::message("session store lock"))?
            .clone())
    }

    fn delete(&self) -> Result<(), AuthError> {
        *self
            .token
            .lock()
            .map_err(|_| AuthError::message("session store lock"))? = None;
        Ok(())
    }
}

/// OS keychain by default; a missing service never triggers a file fallback.
/// Builds without os-keyring require an explicitly configured private file.
pub struct KeyringStore;

#[cfg(feature = "os-keyring")]
struct KeyringEntryStore { entry: keyring::Entry }

#[cfg(feature = "os-keyring")]
fn os_store() -> Result<KeyringEntryStore, AuthError> {
    named_os_store(SERVICE, ACCOUNT)
}

#[cfg(feature = "os-keyring")]
fn named_os_store(service: &str, account: &str) -> Result<KeyringEntryStore, AuthError> {
    #[cfg(target_os = "windows")]
    let entry = keyring::Entry::new_with_target(&windows_credential_target(service, account), service, account);
    #[cfg(not(target_os = "windows"))]
    let entry = keyring::Entry::new(service, account);
    Ok(KeyringEntryStore { entry: entry.map_err(map_keyring)? })
}

// Match the bundled Go CLI's go-keyring Windows target and UTF-8 blob format.
#[cfg(any(target_os = "windows", test))]
fn windows_credential_target(service: &str, account: &str) -> String { format!("{service}:{account}") }
#[cfg(any(target_os = "windows", test))]
fn windows_token(bytes: Vec<u8>) -> Result<String, AuthError> {
    String::from_utf8(bytes).map_err(|_| AuthError::message("invalid Windows session encoding"))
}

#[cfg(feature = "os-keyring")]
impl SessionStore for KeyringStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        // Reject before even opening an OS entry.
        if token.is_empty() { return Err(AuthError::NoRefreshToken); }
        os_store()?.put_refresh_token(token)
    }
    fn refresh_token(&self) -> Result<Option<String>, AuthError> { os_store()?.refresh_token() }
    fn delete(&self) -> Result<(), AuthError> { os_store()?.delete() }
}

#[cfg(feature = "os-keyring")]
impl SessionStore for KeyringEntryStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        if token.is_empty() { return Err(AuthError::NoRefreshToken); }
        #[cfg(target_os = "windows")]
        { self.entry.set_secret(token.as_bytes()).map_err(map_keyring) }
        #[cfg(not(target_os = "windows"))]
        { self.entry.set_password(token).map_err(map_keyring) }
    }
    fn refresh_token(&self) -> Result<Option<String>, AuthError> {
        #[cfg(target_os = "windows")]
        let token = match self.entry.get_secret() {
            Ok(bytes) => Ok(windows_token(bytes)?),
            Err(err) => Err(err),
        };
        #[cfg(not(target_os = "windows"))]
        let token = self.entry.get_password();
        match token {
            Ok(token) if !token.is_empty() => Ok(Some(token)),
            Ok(_) => Ok(None),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_keyring(err)),
        }
    }
    fn delete(&self) -> Result<(), AuthError> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(map_keyring(err)),
        }
    }
}

#[cfg(feature = "os-keyring")]
fn map_keyring(err: keyring::Error) -> AuthError {
    match err {
        keyring::Error::NoEntry => AuthError::NoSession,
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            AuthError::NoKeychain
        }
        other => AuthError::message(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(target_os = "windows", feature = "os-keyring"))]
    #[test]
    #[ignore = "requires disposable hosted Windows CI"]
    fn windows_credential_store_contract() {
        for (name, expected) in [("GITHUB_ACTIONS", "true"), ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("RUNNER_OS", "Windows"), ("HYPERMESH_CREDENTIAL_CONTRACT", "1")] {
            assert_eq!(std::env::var(name).as_deref(), Ok(expected), "credential contract requires hosted Windows CI");
        }
        let service = format!("hypermesh-ci-{}", crate::new_state().unwrap());
        let store = named_os_store(&service, "fixture-session").unwrap();
        struct Cleanup<'a>(&'a KeyringEntryStore);
        impl Drop for Cleanup<'_> { fn drop(&mut self) { let _ = self.0.delete(); } }
        let _cleanup = Cleanup(&store);
        let helper = std::env::var_os("HYPERMESH_CREDENTIAL_HELPER").expect("native CLI contract helper required");
        let cli = |operation: &str| {
            assert!(std::process::Command::new(&helper).args([operation, &service]).status().unwrap().success());
        };
        cli("write");
        assert_eq!(store.refresh_token().unwrap().as_deref(), Some("go synthetic λ"));
        store.put_refresh_token("rust synthetic λ").unwrap();
        cli("read-rust");
        cli("delete");

        assert_eq!(store.refresh_token().unwrap(), None);
        store.delete().unwrap();
        assert!(matches!(store.put_refresh_token(""), Err(AuthError::NoRefreshToken)));
        assert_eq!(store.refresh_token().unwrap(), None);
        store.put_refresh_token("synthetic-refresh-one").unwrap();
        // Reopening proves this is the Windows store, not the Rust object's memory.
        let reopened = named_os_store(&service, "fixture-session").unwrap();
        assert_eq!(reopened.refresh_token().unwrap().as_deref(), Some("synthetic-refresh-one"));
        reopened.put_refresh_token("synthetic-refresh-two").unwrap();
        assert!(matches!(reopened.put_refresh_token(""), Err(AuthError::NoRefreshToken)));
        assert_eq!(store.refresh_token().unwrap().as_deref(), Some("synthetic-refresh-two"));
        reopened.delete().unwrap();
        assert_eq!(store.refresh_token().unwrap(), None);
        store.delete().unwrap();
    }

    #[test]
    fn windows_entry_matches_cli_name_and_utf8_format() {
        assert_eq!(windows_credential_target("hypermesh", "session"), "hypermesh:session");
        let fixture = "synthetic λ session";
        assert_eq!(windows_token(fixture.as_bytes().to_vec()).unwrap(), fixture);
        assert_eq!(windows_token(vec![]).unwrap(), "");
        assert!(windows_token(vec![0xff, 0xfe]).is_err());
    }

    #[test]
    fn memory_store_round_trips_and_deletes() {
        let store = MemoryStore::new();
        assert_eq!(store.refresh_token().unwrap(), None);
        store.put_refresh_token("refresh-1").unwrap();
        assert_eq!(store.refresh_token().unwrap().as_deref(), Some("refresh-1"));
        store.put_refresh_token("refresh-2").unwrap();
        assert_eq!(store.refresh_token().unwrap().as_deref(), Some("refresh-2"));
        store.delete().unwrap();
        assert_eq!(store.refresh_token().unwrap(), None);
        let rendered = format!("{store:?}");
        assert!(!rendered.contains("refresh"));
    }

    #[test]
    fn empty_refresh_token_is_refused() {
        let store = MemoryStore::new();
        assert!(store.put_refresh_token("").is_err());
        assert_eq!(store.refresh_token().unwrap(), None);
    }
}


#[cfg(all(not(feature = "os-keyring"), unix))]
impl SessionStore for KeyringStore {
    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        crate::file_store::configured()?.put_refresh_token(token)
    }
    fn refresh_token(&self) -> Result<Option<String>, AuthError> {
        crate::file_store::configured()?.refresh_token()
    }
    fn delete(&self) -> Result<(), AuthError> {
        crate::file_store::configured()?.delete()
    }
}

#[cfg(all(not(feature = "os-keyring"), not(unix)))]
impl SessionStore for KeyringStore {
    fn put_refresh_token(&self, _: &str) -> Result<(), AuthError> { Err(AuthError::NoKeychain) }
    fn refresh_token(&self) -> Result<Option<String>, AuthError> { Err(AuthError::NoKeychain) }
    fn delete(&self) -> Result<(), AuthError> { Err(AuthError::NoKeychain) }
}
