use std::sync::Arc;
use tauri::State;

use super::AppState;
use crate::driver;
use crate::error::{FaroError, Result};
use crate::model::{ConnectionConfig, Engine};
use crate::secrets;

/// A saved connection plus whether it currently has an open pool.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    #[serde(flatten)]
    pub config: ConnectionConfig,
    pub connected: bool,
    /// Whether a password is held for this connection, so the edit dialog can
    /// show "saved" instead of an empty box. The password itself never crosses
    /// this boundary.
    pub has_password: bool,
}

#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> Result<Vec<ConnectionConfig>> {
    state.store.list_connections()
}

#[tauri::command]
pub async fn list_connection_status(state: State<'_, AppState>) -> Result<Vec<ConnectionStatus>> {
    let configs = state.store.list_connections()?;

    // The keychain lookups go to a blocking thread, all at once.
    //
    // `secrets::get_password` is a synchronous OS call — a DBus round trip to
    // the Secret Service on Linux — and this ran one per saved connection
    // directly on the async runtime, on every sidebar refresh. That stalled a
    // runtime worker for as long as the whole loop took, with nothing else on
    // that thread able to make progress meanwhile.
    let keys: Vec<String> = configs.iter().map(|c| c.secret_key()).collect();
    let stored: Vec<bool> = tokio::task::spawn_blocking(move || {
        keys.iter()
            .map(|k| secrets::get_password(k).is_some())
            .collect()
    })
    .await
    .map_err(|e| FaroError::Other(format!("could not read stored passwords: {e}")))?;

    let mut out = Vec::with_capacity(configs.len());
    for (index, config) in configs.into_iter().enumerate() {
        let connected = state.registry.is_connected(&config.id).await;
        out.push(ConnectionStatus {
            config,
            connected,
            has_password: stored.get(index).copied().unwrap_or(false),
        });
    }
    Ok(out)
}

/// Create or update a saved connection.
///
/// `password` is `None` when the user left the field untouched while editing,
/// which must not wipe an already-stored password. An explicit empty string
/// clears it.
#[tauri::command]
pub fn save_connection(
    state: State<'_, AppState>,
    mut config: ConnectionConfig,
    password: Option<String>,
) -> Result<ConnectionConfig> {
    if config.id.is_empty() {
        config.id = uuid::Uuid::new_v4().to_string();
    }
    if config.name.trim().is_empty() {
        config.name = default_name(&config);
    }
    if config.port == 0 && !config.engine.is_file_based() {
        config.port = config.engine.default_port();
    }

    state.store.upsert_connection(&config)?;

    // The row goes in first, so a keychain failure leaves a saved connection
    // whose password did not make it — the recoverable half of the split, since
    // the user can simply re-enter it. But it has to be said out loud: bubbling
    // the raw keychain error up made it look like nothing had been saved, and
    // staying silent meant the next connect failed with a bare authentication
    // error that pointed nowhere near the real cause.
    let stored = match password {
        Some(pw) if pw.is_empty() => secrets::delete_password(&config.secret_key()),
        Some(pw) => secrets::set_password(&config.secret_key(), &pw),
        None => Ok(()),
    };

    if let Err(e) = stored {
        return Err(FaroError::Keychain(format!(
            "\"{}\" was saved, but its password could not be stored: {e}\n\n\
             Edit the connection and re-enter the password to try again.",
            config.name
        )));
    }

    Ok(config)
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> Result<()> {
    state.registry.remove(&id).await;

    // Best effort on the secret, then delete the row regardless. Failing the
    // whole delete because the keychain was unreachable left the connection
    // visibly undeleted, which reads as the button not working; a stale
    // keychain entry for an id nothing references any more is harmless by
    // comparison, and `secret_key` is derived from the id so it can never be
    // handed to a different connection.
    if let Ok(Some(config)) = state.store.get_connection(&id) {
        let _ = secrets::delete_password(&config.secret_key());
    }

    state.store.delete_connection(&id)
}

/// Open a pool for a saved connection.
#[tauri::command]
pub async fn connect(state: State<'_, AppState>, id: String) -> Result<()> {
    let config = state
        .store
        .get_connection(&id)?
        .ok_or_else(|| FaroError::NotConnected(id.clone()))?;

    if !config.engine.is_supported() {
        return Err(FaroError::UnsupportedEngine(
            config.engine.display_name().into(),
        ));
    }

    let password = secrets::get_password(&config.secret_key());
    let driver = driver::connect(&config, password.as_deref()).await?;

    // Prove the credentials work now rather than surfacing a failure on the
    // user's first query: sqlx pools connect lazily.
    driver.ping().await?;

    state
        .registry
        .insert(id, Arc::from(driver), config.read_only, config.name.clone())
        .await;
    Ok(())
}

/// Try a configuration without saving it — the "Test connection" button.
///
/// Takes no `AppState`: the connection is opened, pinged and closed without
/// touching the registry, so nothing is left behind whether or not it worked.
#[tauri::command]
pub async fn test_connection(config: ConnectionConfig, password: Option<String>) -> Result<()> {
    if !config.engine.is_supported() {
        return Err(FaroError::UnsupportedEngine(
            config.engine.display_name().into(),
        ));
    }

    // An edit of an existing connection may send no password because the user
    // did not retype it; fall back to the stored one so Test reflects reality.
    let password = match password {
        Some(pw) => Some(pw),
        None if !config.id.is_empty() => secrets::get_password(&config.secret_key()),
        None => None,
    };

    let driver = driver::connect(&config, password.as_deref()).await?;
    let result = driver.ping().await;
    driver.close().await;
    result
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>, id: String) -> Result<()> {
    state.registry.remove(&id).await;
    Ok(())
}

#[tauri::command]
pub async fn connected_ids(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(state.registry.connected_ids().await)
}

/// Reported to the UI so it can warn that passwords will not persist.
#[tauri::command]
pub fn keychain_available() -> bool {
    secrets::keychain_available()
}

/// Engine metadata for the connect dialog: which engines exist, their default
/// ports, and which are implemented so far.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub id: Engine,
    pub name: &'static str,
    pub default_port: u16,
    pub file_based: bool,
    pub supported: bool,
    /// False for MongoDB: the editor and formatter need to know the query
    /// language is not SQL before a connection is even opened.
    pub is_sql: bool,
}

#[tauri::command]
pub fn list_engines() -> Vec<EngineInfo> {
    [
        Engine::Postgres,
        Engine::MySql,
        Engine::MariaDb,
        Engine::Sqlite,
        Engine::SqlServer,
        Engine::DuckDb,
        Engine::ClickHouse,
        Engine::CockroachDb,
        Engine::Redshift,
        Engine::MongoDb,
    ]
    .into_iter()
    .map(|e| EngineInfo {
        id: e,
        name: e.display_name(),
        default_port: e.default_port(),
        file_based: e.is_file_based(),
        supported: e.is_supported(),
        is_sql: e.is_sql(),
    })
    .collect()
}

/// A readable fallback name so an unnamed connection is still identifiable.
fn default_name(config: &ConnectionConfig) -> String {
    if config.engine.is_file_based() {
        config
            .file_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| config.engine.display_name().to_string())
    } else if config.database.is_empty() {
        config.host.clone()
    } else {
        format!("{}/{}", config.host, config.database)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SslMode;

    fn base(engine: Engine) -> ConnectionConfig {
        ConnectionConfig {
            id: String::new(),
            name: String::new(),
            engine,
            host: "db.example.com".into(),
            port: 0,
            username: "u".into(),
            database: "app".into(),
            file_path: None,
            ssl_mode: SslMode::Prefer,
            color: None,
            read_only: false,
        }
    }

    #[test]
    fn names_server_connections_by_host_and_database() {
        assert_eq!(default_name(&base(Engine::Postgres)), "db.example.com/app");
    }

    #[test]
    fn names_file_connections_by_file_name() {
        let mut c = base(Engine::Sqlite);
        c.file_path = Some("/Users/me/data/inventory.db".into());
        assert_eq!(default_name(&c), "inventory.db");
    }

    #[test]
    fn falls_back_to_host_when_database_is_blank() {
        let mut c = base(Engine::Postgres);
        c.database = String::new();
        assert_eq!(default_name(&c), "db.example.com");
    }
}
