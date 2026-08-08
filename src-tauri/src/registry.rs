//! Live connections and in-flight queries.
//!
//! Holds the open `Driver` per connection id, and a cancellation token per
//! running query so the UI can abort one mid-flight instead of waiting for a
//! runaway statement to finish.

use std::collections::HashMap;
use std::sync::Arc;
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
    running: RwLock<HashMap<String, CancellationToken>>,
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
        if let Some(old) = self.drivers.write().await.insert(connection_id, entry) {
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
        self.cancel_for_connection(connection_id).await;
        if let Some(open) = self.drivers.write().await.remove(connection_id) {
            open.driver.close().await;
        }
    }

    pub async fn close_all(&self) {
        let ids: Vec<String> = self.drivers.read().await.keys().cloned().collect();
        for id in ids {
            self.remove(&id).await;
        }
    }

    /// Register a query as running. The key embeds the connection id so
    /// disconnecting can cancel everything on that connection.
    pub async fn begin_query(&self, connection_id: &str, query_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.running
            .write()
            .await
            .insert(running_key(connection_id, query_id), token.clone());
        token
    }

    pub async fn end_query(&self, connection_id: &str, query_id: &str) {
        self.running
            .write()
            .await
            .remove(&running_key(connection_id, query_id));
    }

    /// Returns false when the query already finished — a benign race when the
    /// user hits Cancel just as results arrive.
    pub async fn cancel_query(&self, connection_id: &str, query_id: &str) -> bool {
        match self
            .running
            .write()
            .await
            .remove(&running_key(connection_id, query_id))
        {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    async fn cancel_for_connection(&self, connection_id: &str) {
        let prefix = format!("{connection_id}\u{1}");
        let mut running = self.running.write().await;
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
        let token = reg.begin_query("c1", "q1").await;
        assert!(!token.is_cancelled());

        assert!(reg.cancel_query("c1", "q1").await);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_a_finished_query_is_false_not_panic() {
        let reg = Registry::new();
        reg.begin_query("c1", "q1").await;
        reg.end_query("c1", "q1").await;
        assert!(!reg.cancel_query("c1", "q1").await);
    }

    #[tokio::test]
    async fn queries_are_scoped_per_connection() {
        // Same query id on two connections must not cancel each other.
        let reg = Registry::new();
        let a = reg.begin_query("c1", "shared").await;
        let b = reg.begin_query("c2", "shared").await;

        reg.cancel_query("c1", "shared").await;

        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "cancel leaked across connections");
    }
}
