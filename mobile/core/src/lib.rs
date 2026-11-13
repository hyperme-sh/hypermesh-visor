//! Mobile binding spike. Native apps supply storage; this crate opens no listener.
use hypermesh_session::{AuthError, SessionStore};
use std::sync::Arc;

uniffi::setup_scaffolding!();

/// Fixed errors keep native store diagnostics and credentials out of FFI errors.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum StoreError {
    #[error("secure session storage is unavailable")]
    Unavailable,
    #[error("invalid session credential")]
    InvalidCredential,
}

/// Implement with the native app's secure store. Never fall back to a plain file.
/// Calls may arrive off the UI thread; implementations must be thread-safe.
#[uniffi::export(foreign)]
pub trait NativeSessionStore: Send + Sync {
    fn read_refresh_token(&self) -> Result<Option<String>, StoreError>;
    fn write_refresh_token(&self, token: String) -> Result<(), StoreError>;
    fn delete_refresh_token(&self) -> Result<(), StoreError>;
}

struct AppStore(Arc<dyn NativeSessionStore>);

impl SessionStore for AppStore {
    fn refresh_token(&self) -> Result<Option<String>, AuthError> {
        self.0
            .read_refresh_token()
            .map_err(|_| AuthError::NoKeychain)
    }

    fn put_refresh_token(&self, token: &str) -> Result<(), AuthError> {
        self.0
            .write_refresh_token(token.to_owned())
            .map_err(|_| AuthError::NoKeychain)
    }

    fn delete(&self) -> Result<(), AuthError> {
        self.0
            .delete_refresh_token()
            .map_err(|_| AuthError::NoKeychain)
    }
}

/// Storage presence is not device enrollment or authorization to call a service.
#[derive(uniffi::Object)]
pub struct MobileSession {
    store: AppStore,
}

fn valid_credential(token: &str) -> bool {
    !token.trim().is_empty() && token.len() <= 65_536 && !token.chars().any(char::is_control)
}

#[uniffi::export]
impl MobileSession {
    #[uniffi::constructor]
    pub fn new(store: Arc<dyn NativeSessionStore>) -> Self {
        Self {
            store: AppStore(store),
        }
    }

    pub fn has_stored_session(&self) -> Result<bool, StoreError> {
        match hypermesh_session::refresh_token(&self.store).map_err(|_| StoreError::Unavailable)? {
            None => Ok(false),
            Some(token) if valid_credential(&token) => Ok(true),
            Some(_) => Err(StoreError::InvalidCredential),
        }
    }

    /// Called only by the native auth owner after its authorized sign-in flow.
    pub fn store_refresh_token(&self, token: String) -> Result<(), StoreError> {
        if !valid_credential(&token) {
            return Err(StoreError::InvalidCredential);
        }
        self.store
            .put_refresh_token(&token)
            .map_err(|_| StoreError::Unavailable)
    }

    /// Local deletion only; the owning auth flow must separately revoke remotely.
    pub fn clear_local_session(&self) -> Result<(), StoreError> {
        self.store.delete().map_err(|_| StoreError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestStore {
        token: Mutex<Option<String>>,
        unavailable: bool,
    }

    impl NativeSessionStore for TestStore {
        fn read_refresh_token(&self) -> Result<Option<String>, StoreError> {
            if self.unavailable {
                return Err(StoreError::Unavailable);
            }
            Ok(self.token.lock().unwrap().clone())
        }
        fn write_refresh_token(&self, token: String) -> Result<(), StoreError> {
            if self.unavailable {
                return Err(StoreError::Unavailable);
            }
            *self.token.lock().unwrap() = Some(token);
            Ok(())
        }
        fn delete_refresh_token(&self) -> Result<(), StoreError> {
            if self.unavailable {
                return Err(StoreError::Unavailable);
            }
            *self.token.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn delegates_to_the_app_store_without_caching() {
        let store = Arc::new(TestStore::default());
        let session = MobileSession::new(store.clone());
        assert!(!session.has_stored_session().unwrap());
        session.store_refresh_token("fixture-one".into()).unwrap();
        assert_eq!(
            store.read_refresh_token().unwrap().as_deref(),
            Some("fixture-one")
        );
        session.store_refresh_token("fixture-two".into()).unwrap();
        assert_eq!(
            store.read_refresh_token().unwrap().as_deref(),
            Some("fixture-two")
        );
        store.delete_refresh_token().unwrap();
        assert!(!session.has_stored_session().unwrap());
        session.store_refresh_token("fixture-three".into()).unwrap();
        session.clear_local_session().unwrap();
        assert_eq!(store.read_refresh_token().unwrap(), None);
    }

    #[test]
    fn unavailable_store_fails_every_operation() {
        let session = MobileSession::new(Arc::new(TestStore {
            unavailable: true,
            ..Default::default()
        }));
        assert!(matches!(
            session.has_stored_session(),
            Err(StoreError::Unavailable)
        ));
        assert!(matches!(
            session.store_refresh_token("fixture".into()),
            Err(StoreError::Unavailable)
        ));
        assert!(matches!(
            session.clear_local_session(),
            Err(StoreError::Unavailable)
        ));
    }

    #[test]
    fn invalid_write_preserves_the_existing_entry() {
        let store = Arc::new(TestStore::default());
        let session = MobileSession::new(store.clone());
        session.store_refresh_token("fixture".into()).unwrap();
        for token in [
            "".into(),
            "  ".into(),
            "bad\nvalue".into(),
            "x".repeat(65_537),
        ] {
            assert!(matches!(
                session.store_refresh_token(token),
                Err(StoreError::InvalidCredential)
            ));
            assert_eq!(
                store.read_refresh_token().unwrap().as_deref(),
                Some("fixture")
            );
        }
    }

    #[test]
    fn malformed_native_entry_is_not_a_session() {
        let store = Arc::new(TestStore::default());
        *store.token.lock().unwrap() = Some(String::new());
        assert!(matches!(
            MobileSession::new(store).has_stored_session(),
            Err(StoreError::InvalidCredential)
        ));
    }
}
