//! Password storage, backed by the OS keychain.
//!
//! Faro never writes a password to its own database or its config files. On
//! macOS this is the Keychain, on Windows the Credential Manager, on Linux the
//! Secret Service (GNOME Keyring / KWallet).
//!
//! Linux boxes without a running Secret Service are a real case (headless, some
//! minimal WMs). Rather than failing opaquely, a keychain miss falls back to an
//! in-memory map that lives only for the session, and `keychain_available()`
//! lets the UI say so plainly.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::error::Result;

const SERVICE: &str = "dev.faro.app";

/// Session-only fallback for systems with no working keychain.
fn memory_store() -> &'static Mutex<HashMap<String, String>> {
    static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry(key: &str) -> std::result::Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(SERVICE, key)
}

/// Whether a real OS keychain is reachable. When false, passwords are kept only
/// for the current session and the user should be told.
pub fn keychain_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    // store_status() reports the one-time backend initialization without
    // writing anything, so probing costs nothing and leaves no stray entry.
    *AVAILABLE.get_or_init(|| keyring::Entry::store_status().is_ok())
}

pub fn set_password(key: &str, password: &str) -> Result<()> {
    if keychain_available() {
        entry(key)?.set_password(password)?;
    } else {
        memory_store()
            .lock()
            .expect("secrets mutex poisoned")
            .insert(key.to_string(), password.to_string());
    }
    Ok(())
}

/// Fetch a password. `None` means none is stored — not an error, since many
/// connections legitimately have no password (trust auth, socket auth, SQLite).
pub fn get_password(key: &str) -> Option<String> {
    if keychain_available() {
        entry(key).ok()?.get_password().ok()
    } else {
        memory_store().lock().ok()?.get(key).cloned()
    }
}

pub fn delete_password(key: &str) -> Result<()> {
    if keychain_available() {
        // A missing entry is the desired end state, so absence is not an error.
        if let Ok(e) = entry(key) {
            let _ = e.delete_credential();
        }
    } else {
        memory_store().lock().expect("secrets mutex poisoned").remove(key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_password() {
        let key = format!("faro:test-{}", uuid::Uuid::new_v4());
        set_password(&key, "hunter2").unwrap();
        assert_eq!(get_password(&key).as_deref(), Some("hunter2"));
        delete_password(&key).unwrap();
        assert_eq!(get_password(&key), None);
    }

    #[test]
    fn missing_key_is_none_not_error() {
        let key = format!("faro:absent-{}", uuid::Uuid::new_v4());
        assert_eq!(get_password(&key), None);
    }

    #[test]
    fn deleting_absent_key_is_ok() {
        // Deleting a connection that never had a password must not fail.
        let key = format!("faro:absent-{}", uuid::Uuid::new_v4());
        assert!(delete_password(&key).is_ok());
    }
}
