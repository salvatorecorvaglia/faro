//! Live connections and in-flight queries.
//!
//! Holds the open `Driver` per connection id, and a cancellation token per
//! running query so the UI can abort one mid-flight instead of waiting for a
//! runaway statement to finish.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::driver::Driver;
use crate::error::{FaroError, Result};

/// An open connection and the properties of it that outlive any one command.
///
/// `read_only` and `name` live here rather than being re-read from the `Store`
/// on every call: the store is a mutex-guarded SQLite file, and a write guard
/// has to consult the flag before it can refuse anything.
struct Open {
    driver: Arc<dyn Driver>,
    read_only: bool,
    /// The connection's display name, for error messages that name it.
    name: String,
}

#[derive(Default)]
pub struct Registry {
    drivers: RwLock<HashMap<String, Open>>,
    /// Guarded by a plain `Mutex`, not the async one.
    ///
    /// Every operation on this map is a lookup or an insert with no `await`
    /// inside, so a blocking lock costs nothing — and it is what lets
    /// [`QueryGuard`]'s `Drop` deregister a finished query, which an async lock
    /// could not do from `Drop` at all.
    running: Mutex<HashMap<String, CancellationToken>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(
        &self,
        connection_id: String,
        driver: Arc<dyn Driver>,
        read_only: bool,
        name: String,
    ) {
        let entry = Open {
            driver,
            read_only,
            name,
        };
        // Replacing an entry drops the old pool; close it first so its sockets
        // are released rather than left to the Drop impl at an unknown time.
        //
        // The guard is bound and dropped before the await. Written as
        // `if let Some(old) = self.drivers.write().await.insert(..)`, the
        // temporary guard lives to the end of the `if let` block under edition
        // 2021 scoping, so the write lock was held across `close()` — which
        // waits for pool shutdown, blocking every other command meanwhile.
        let replaced = {
            let mut drivers = self.drivers.write().await;
            drivers.insert(connection_id, entry)
        };
        if let Some(old) = replaced {
            old.driver.close().await;
        }
    }

    pub async fn get(&self, connection_id: &str) -> Result<Arc<dyn Driver>> {
        self.drivers
            .read()
            .await
            .get(connection_id)
            .map(|o| o.driver.clone())
            .ok_or_else(|| FaroError::NotConnected(connection_id.to_string()))
    }

    /// The driver for a connection that is allowed to be written to.
    ///
    /// Every command that modifies data goes through this rather than `get`, so
    /// "open read-only" is enforced in one place instead of being re-checked —
    /// and eventually forgotten — at each call site. Engines that can enforce it
    /// themselves also do, but this is what makes the promise hold everywhere.
    pub async fn get_writable(&self, connection_id: &str) -> Result<Arc<dyn Driver>> {
        let drivers = self.drivers.read().await;
        let open = drivers
            .get(connection_id)
            .ok_or_else(|| FaroError::NotConnected(connection_id.to_string()))?;

        if open.read_only {
            return Err(FaroError::read_only(&open.name, None));
        }
        Ok(open.driver.clone())
    }

    pub async fn is_read_only(&self, connection_id: &str) -> bool {
        self.drivers
            .read()
            .await
            .get(connection_id)
            .is_some_and(|o| o.read_only)
    }

    pub async fn is_connected(&self, connection_id: &str) -> bool {
        self.drivers.read().await.contains_key(connection_id)
    }

    pub async fn connected_ids(&self) -> Vec<String> {
        self.drivers.read().await.keys().cloned().collect()
    }

    /// Close and forget a connection, cancelling anything still running on it.
    pub async fn remove(&self, connection_id: &str) {
        // Cancel first: a query holding a pool slot would otherwise delay close.
        self.cancel_for_connection(connection_id);
        // Guard dropped before the await; see the note in `insert`.
        let removed = {
            let mut drivers = self.drivers.write().await;
            drivers.remove(connection_id)
        };
        if let Some(open) = removed {
            open.driver.close().await;
        }
    }

    /// Close every open connection. Called on app exit.
    pub async fn close_all(&self) {
        let ids: Vec<String> = self.drivers.read().await.keys().cloned().collect();
        for id in ids {
            self.remove(&id).await;
        }
    }

    /// Register a query as running, returning a guard that deregisters it.
    ///
    /// A guard rather than a matching `end_query` call. The call was an ordinary
    /// statement near the end of each command, and any `?` between the two
    /// skipped it — `export_table`'s `JoinError` path did exactly that — leaving
    /// a token in the map that nothing would ever remove. A later `disconnect`
    /// would then "cancel" a query that had finished long ago.
    ///
    /// The key embeds the connection id so disconnecting can cancel everything
    /// running on that connection.
    pub fn begin_query(&self, connection_id: &str, query_id: &str) -> QueryGuard<'_> {
        let token = CancellationToken::new();
        let key = running_key(connection_id, query_id);
        self.lock_running().insert(key.clone(), token.clone());
        QueryGuard {
            registry: self,
            key,
            token,
        }
    }

    /// Take the lock, recovering from poisoning rather than panicking.
    ///
    /// A panic elsewhere must not make every later query unrunnable; the map is
    /// only ever left in a consistent state.
    fn lock_running(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns false when the query already finished — a benign race when the
    /// user hits Cancel just as results arrive.
    pub fn cancel_query(&self, connection_id: &str, query_id: &str) -> bool {
        match self
            .lock_running()
            .remove(&running_key(connection_id, query_id))
        {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    fn cancel_for_connection(&self, connection_id: &str) {
        let prefix = format!("{connection_id}\u{1}");
        let mut running = self.lock_running();
        let keys: Vec<String> = running
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for k in keys {
            if let Some(token) = running.remove(&k) {
                token.cancel();
            }
        }
    }
}

/// Keeps a running query registered for exactly as long as it is running.
///
/// Dropping it deregisters the query, so no early return can leak the
/// registration — see [`Registry::begin_query`].
pub struct QueryGuard<'a> {
    registry: &'a Registry,
    key: String,
    token: CancellationToken,
}

impl QueryGuard<'_> {
    /// The token to hand the driver, so a runaway statement can be aborted.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for QueryGuard<'_> {
    fn drop(&mut self) {
        self.registry.lock_running().remove(&self.key);
    }
}

/// Composite key. Uses U+0001 as the separator because it cannot occur in a
/// UUID, so a crafted query id can never collide with another connection's.
fn running_key(connection_id: &str, query_id: &str) -> String {
    format!("{connection_id}\u{1}{query_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_unknown_connection_is_not_connected_error() {
        let reg = Registry::new();
        assert!(matches!(
            reg.get("nope").await,
            Err(FaroError::NotConnected(_))
        ));
        assert!(!reg.is_connected("nope").await);
    }

    #[tokio::test]
    async fn cancel_signals_the_token() {
        let reg = Registry::new();
        let query = reg.begin_query("c1", "q1");
        let token = query.token();
        assert!(!token.is_cancelled());

        assert!(reg.cancel_query("c1", "q1"));
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_a_finished_query_is_false_not_panic() {
        let reg = Registry::new();
        drop(reg.begin_query("c1", "q1"));
        assert!(!reg.cancel_query("c1", "q1"));
    }

    #[tokio::test]
    async fn a_query_is_deregistered_even_when_its_command_returns_early() {
        // The leak this guard exists to prevent: `export_table` removed its
        // registration with an ordinary call placed *after* a `?`, so a failure
        // on that path left the token behind for the life of the process.
        let reg = Registry::new();

        fn fails_early(reg: &Registry) -> Result<()> {
            let _query = reg.begin_query("c1", "q1");
            Err(FaroError::Other("something went wrong".into()))
        }

        assert!(fails_early(&reg).is_err());
        assert!(
            !reg.cancel_query("c1", "q1"),
            "the registration outlived the command that made it"
        );
    }

    #[tokio::test]
    async fn queries_are_scoped_per_connection() {
        // Same query id on two connections must not cancel each other.
        let reg = Registry::new();
        // The guards must stay alive: dropping one deregisters its query.
        let qa = reg.begin_query("c1", "shared");
        let qb = reg.begin_query("c2", "shared");
        let a = qa.token();
        let b = qb.token();

        reg.cancel_query("c1", "shared");

        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "cancel leaked across connections");
    }
}
