use serde::{Deserialize, Serialize};

/// Every engine Faro can talk to.
///
/// The PG-wire family (Cockroach, Redshift, Supabase, Neon) and MariaDB are
/// distinct variants rather than aliases: they share a driver but differ in
/// catalog queries and defaults, and the user deserves to see the real name of
/// the thing they connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    MySql,
    MariaDb,
    Sqlite,
    SqlServer,
    DuckDb,
    ClickHouse,
    CockroachDb,
    Redshift,
    MongoDb,
}

impl Engine {
    pub fn display_name(self) -> &'static str {
        match self {
            Engine::Postgres => "PostgreSQL",
            Engine::MySql => "MySQL",
            Engine::MariaDb => "MariaDB",
            Engine::Sqlite => "SQLite",
            Engine::SqlServer => "SQL Server",
            Engine::DuckDb => "DuckDB",
            Engine::ClickHouse => "ClickHouse",
            Engine::CockroachDb => "CockroachDB",
            Engine::Redshift => "Redshift",
            Engine::MongoDb => "MongoDB",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Engine::Postgres | Engine::Redshift => 5432,
            Engine::CockroachDb => 26257,
            Engine::MySql | Engine::MariaDb => 3306,
            Engine::SqlServer => 1433,
            Engine::ClickHouse => 8123,
            Engine::MongoDb => 27017,
            // File-backed engines have no port.
            Engine::Sqlite | Engine::DuckDb => 0,
        }
    }

    /// File-backed engines take a path instead of host/port/credentials, which
    /// changes the whole shape of the connect dialog.
    pub fn is_file_based(self) -> bool {
        matches!(self, Engine::Sqlite | Engine::DuckDb)
    }

    /// Whether the query language is SQL.
    ///
    /// Known before a connection exists, so the UI can pick an editor mode and
    /// hide SQL-only actions from the moment an engine is selected.
    pub fn is_sql(self) -> bool {
        self != Engine::MongoDb
    }

    /// Whether this engine is implemented as of the current phase.
    ///
    /// The PG-wire family and MariaDB are supported because they reuse the
    /// Postgres and MySQL drivers respectively.
    pub fn is_supported(self) -> bool {
        // DuckDB is behind a compile-time feature, so the answer has to match
        // how this build was compiled rather than a fixed list.
        if self == Engine::DuckDb {
            return cfg!(feature = "duckdb-engine");
        }
        matches!(
            self,
            Engine::Postgres
                | Engine::Sqlite
                | Engine::MySql
                | Engine::MariaDb
                | Engine::SqlServer
                | Engine::ClickHouse
                | Engine::MongoDb
                | Engine::CockroachDb
                | Engine::Redshift
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
}

/// A saved connection, minus its password.
///
/// The password never appears in this struct: it lives in the OS keychain and
/// is fetched only at the moment a pool is opened. That keeps it out of the
/// app-state database, out of IPC payloads, and out of any log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub engine: Engine,

    /// Host for server engines; empty for file-based ones.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub database: String,

    /// Filesystem path for SQLite/DuckDB.
    #[serde(default)]
    pub file_path: Option<String>,

    #[serde(default)]
    pub ssl_mode: SslMode,

    /// UI accent, so production connections can be visually flagged.
    #[serde(default)]
    pub color: Option<String>,

    #[serde(default)]
    pub read_only: bool,
}

impl ConnectionConfig {
    /// Keychain entry key. Stable across renames because it uses the id.
    pub fn secret_key(&self) -> String {
        format!("faro:{}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_engines_have_no_port() {
        assert_eq!(Engine::Sqlite.default_port(), 0);
        assert!(Engine::Sqlite.is_file_based());
        assert!(!Engine::Postgres.is_file_based());
    }

    #[test]
    fn pg_wire_family_keeps_distinct_ports() {
        // Cockroach shares Postgres' driver but not its port.
        assert_eq!(Engine::Postgres.default_port(), 5432);
        assert_eq!(Engine::Redshift.default_port(), 5432);
        assert_eq!(Engine::CockroachDb.default_port(), 26257);
    }

    #[test]
    fn secret_key_follows_id_not_name() {
        let c = ConnectionConfig {
            id: "abc".into(),
            name: "Renamed".into(),
            engine: Engine::Postgres,
            host: "localhost".into(),
            port: 5432,
            username: "postgres".into(),
            database: "postgres".into(),
            file_path: None,
            ssl_mode: SslMode::Prefer,
            color: None,
            read_only: false,
        };
        assert_eq!(c.secret_key(), "faro:abc");
    }
}
