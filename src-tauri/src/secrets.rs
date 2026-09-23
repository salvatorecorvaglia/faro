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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

/// How long to wait before probing again after the keychain came back absent.
///
/// Long enough that a headless box is not doing a DBus round trip per password
/// read, short enough that a user who unlocks their keyring does not have to
/// restart Faro to benefit.
const REPROBE_AFTER: Duration = Duration::from_secs(5);

/// Whether a real OS keychain is reachable. When false, passwords are kept only
/// for the current session and the user should be told.
///
/// Only the *positive* answer is cached permanently. A keychain that is there
/// stays there, but an absent one can appear later — a Linux login keyring
/// unlocked after Faro started is the ordinary case — and memoizing that "no"
/// for the whole process meant every password stayed in the session-only
/// fallback and was silently lost on quit, with the UI still reporting the
/// state it saw at launch.
pub fn keychain_available() -> bool {
    static AVAILABLE: AtomicBool = AtomicBool::new(false);
    if AVAILABLE.load(Ordering::Relaxed) {
        return true;
    }

    // Throttle the negative case so repeated password reads (the connection
    // list refreshes one per saved connection) do not each pay for a probe.
    static LAST_PROBE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST_PROBE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if last.is_some_and(|at| at.elapsed() < REPROBE_AFTER) {
        return false;
    }
    *last = Some(Instant::now());

    // store_status() reports the one-time backend initialization without
    // writing anything, so probing costs nothing and leaves no stray entry.
    let ok = keyring::Entry::store_status().is_ok();
    if ok {
        AVAILABLE.store(true, Ordering::Relaxed);
    }
    ok
}

pub fn set_password(key: &str, password: &str) -> Result<()> {
    if keychain_available() {
        entry(key)?.set_password(password)?;
    } else {
        memory_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), password.to_string());
    }
    Ok(())
}

/// Fetch a password. `None` means none is stored — not an error, since many
/// connections legitimately have no password (trust auth, socket auth, SQLite).
///
/// Both stores are consulted, keychain first. Since [`keychain_available`] can
/// go from false to true within a session, a password written to the in-memory
/// fallback earlier would otherwise become unreadable the moment the keychain
/// appeared — the value is still perfectly good, it is just in the other store.
pub fn get_password(key: &str) -> Option<String> {
    if keychain_available() {
        if let Some(found) = entry(key).ok().and_then(|e| e.get_password().ok()) {
            return Some(found);
        }
    }

    memory_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .cloned()
}

/// Remove a password from wherever it ended up.
///
/// Clears both stores rather than only the currently-available one: a value
/// written before the keychain appeared lives in the fallback, and leaving it
/// there would resurrect a password the user just deleted.
pub fn delete_password(key: &str) -> Result<()> {
    // A missing entry is the desired end state, so absence is not an error.
    if keychain_available() {
        if let Ok(e) = entry(key) {
            let _ = e.delete_credential();
        }
    }

    memory_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_password() {
        let key = format!("faro:test-{}", uuid::Uuid::new_v4());
        // Generated rather than a literal: the test only cares that whatever
        // goes in comes back out, and a fresh value keeps concurrent runs from
        // colliding on a shared keychain.
        let secret = uuid::Uuid::new_v4().to_string();
        set_password(&key, &secret).unwrap();
        assert_eq!(get_password(&key).as_deref(), Some(secret.as_str()));
        delete_password(&key).unwrap();
        assert_eq!(get_password(&key), None);
    }

    #[test]
    fn missing_key_is_none_not_error() {
        let key = format!("faro:absent-{}", uuid::Uuid::new_v4());
        assert_eq!(get_password(&key), None);
    }

    #[test]
    fn a_password_in_the_session_fallback_is_still_found() {
        // `keychain_available` can go false -> true within a session, so a
        // password written while it was false lives in the fallback. Looking
        // only in the keychain afterwards made it vanish, and the connection
        // then failed to authenticate for no reason the user could see.
        let key = format!("faro:fallback-{}", uuid::Uuid::new_v4());
        memory_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), "from-fallback".to_string());

        assert_eq!(get_password(&key).as_deref(), Some("from-fallback"));

        // And deleting must clear it wherever it lives.
        delete_password(&key).unwrap();
        assert_eq!(get_password(&key), None);
    }

    #[test]
    fn deleting_absent_key_is_ok() {
        // Deleting a connection that never had a password must not fail.
        let key = format!("faro:absent-{}", uuid::Uuid::new_v4());
        assert!(delete_password(&key).is_ok());
    }
}
