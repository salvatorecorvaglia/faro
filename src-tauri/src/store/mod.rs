//! Local app state: saved connections, saved queries and history.
//! A small SQLite file in the platform config directory.
//!
//! Passwords are never stored here — see `secrets`.
//!
//! One `Store`, one connection, one mutex — but three unrelated tables, so
//! the queries for each live in their own file. Those files are children of
//! this module, which is what lets them reach `lock()` and the row-mapping
//! helpers here without any of it having to become crate-public.

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{FaroError, Result};
use crate::model::{Engine, SavedQuery, SslMode};

pub struct Store {
    conn: Mutex<Connection>,
}

/// Where the app-state database lives, per platform convention.
pub fn default_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| FaroError::Store("could not locate a config directory".into()))?
        .join("faro");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("faro.db"))
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            conn: Mutex::new(Connection::open_in_memory()?),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Wrap an existing connection without migrating, so tests can stand up an
    /// older schema and then migrate it.
    #[cfg(test)]
    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// Take the connection lock, recovering from poisoning rather than
    /// panicking.
    ///
    /// A poisoned mutex means some earlier caller panicked while holding it.
    /// The `Connection` itself is still usable — rusqlite does not leave a
    /// half-written statement behind — so the only thing poisoning would
    /// achieve here is turning one panic into a permanently unusable settings
    /// database. Worse, with `panic = "abort"` in release the `expect()` this
    /// replaces would take the whole app down rather than surfacing an error.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Versioned migrations via `user_version`, so later phases can add tables
    /// without breaking an existing install.
    fn migrate(&self) -> Result<()> {
        let conn = self.lock();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);

        if version < 1 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS connections (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL,
                    engine      TEXT NOT NULL,
                    host        TEXT NOT NULL DEFAULT '',
                    port        INTEGER NOT NULL DEFAULT 0,
                    username    TEXT NOT NULL DEFAULT '',
                    database    TEXT NOT NULL DEFAULT '',
                    file_path   TEXT,
                    ssl_mode    TEXT NOT NULL DEFAULT 'prefer',
                    color       TEXT,
                    read_only   INTEGER NOT NULL DEFAULT 0,
                    sort_order  INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );
                PRAGMA user_version = 1;
                "#,
            )?;
        }

        if version < 2 {
            // No foreign key from either table to `connections`: deleting a
            // connection must not delete the queries and history that
            // reference it. A dangling id is harmless; losing the user's work
            // is not.
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS saved_queries (
                    id            TEXT PRIMARY KEY,
                    name          TEXT NOT NULL,
                    folder        TEXT,
                    sql           TEXT NOT NULL,
                    connection_id TEXT,
                    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS saved_queries_folder
                    ON saved_queries(folder, name);

                CREATE TABLE IF NOT EXISTS query_history (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    sql             TEXT NOT NULL,
                    connection_id   TEXT,
                    connection_name TEXT,
                    executed_at     TEXT NOT NULL DEFAULT (datetime('now')),
                    duration_ms     INTEGER NOT NULL DEFAULT 0,
                    row_count       INTEGER NOT NULL DEFAULT 0,
                    error           TEXT,
                    succeeded       INTEGER NOT NULL DEFAULT 1
                );
                CREATE INDEX IF NOT EXISTS query_history_time
                    ON query_history(id DESC);

                PRAGMA user_version = 2;
                "#,
            )?;
        }
        Ok(())
    }
}

mod connections;
mod library;

fn map_saved_query(r: &rusqlite::Row<'_>) -> rusqlite::Result<SavedQuery> {
    Ok(SavedQuery {
        id: r.get("id")?,
        name: r.get("name")?,
        folder: r.get("folder")?,
        sql: r.get("sql")?,
        connection_id: r.get("connection_id")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn engine_str(e: Engine) -> &'static str {
    match e {
        Engine::Postgres => "postgres",
        Engine::MySql => "mysql",
        Engine::MariaDb => "mariadb",
        Engine::Sqlite => "sqlite",
        Engine::SqlServer => "sqlserver",
        Engine::DuckDb => "duckdb",
        Engine::ClickHouse => "clickhouse",
        Engine::CockroachDb => "cockroachdb",
        Engine::Redshift => "redshift",
        Engine::MongoDb => "mongodb",
    }
}

/// `None` for an engine name this build does not know.
///
/// Previously unknown strings became `Postgres`, on the reasoning that one
/// unreadable row should not hide every other saved connection. The first half
/// of that is right and is still honoured — the caller skips the row and keeps
/// the rest — but the fallback was not: a connection saved as `mongodb` by a
/// newer build would come back as Postgres and then try to speak the Postgres
/// wire protocol to a MongoDB port.
fn parse_engine(s: &str) -> Option<Engine> {
    Some(match s {
        "postgres" => Engine::Postgres,
        "mysql" => Engine::MySql,
        "mariadb" => Engine::MariaDb,
        "sqlite" => Engine::Sqlite,
        "sqlserver" => Engine::SqlServer,
        "duckdb" => Engine::DuckDb,
        "clickhouse" => Engine::ClickHouse,
        "cockroachdb" => Engine::CockroachDb,
        "redshift" => Engine::Redshift,
        "mongodb" => Engine::MongoDb,
        _ => return None,
    })
}

fn ssl_str(m: SslMode) -> &'static str {
    match m {
        SslMode::Disable => "disable",
        SslMode::Prefer => "prefer",
        SslMode::Require => "require",
        SslMode::VerifyCa => "verify-ca",
        SslMode::VerifyFull => "verify-full",
    }
}

fn parse_ssl(s: &str) -> SslMode {
    match s {
        "disable" => SslMode::Disable,
        "require" => SslMode::Require,
        "verify-ca" => SslMode::VerifyCa,
        "verify-full" => SslMode::VerifyFull,
        _ => SslMode::Prefer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionConfig, NewHistoryEntry};

    fn sample(id: &str) -> ConnectionConfig {
        ConnectionConfig {
            id: id.into(),
            name: "Local".into(),
            engine: Engine::Postgres,
            host: "localhost".into(),
            port: 5432,
            username: "postgres".into(),
            database: "postgres".into(),
            file_path: None,
            ssl_mode: SslMode::Prefer,
            color: Some("#4f8".into()),
            read_only: false,
        }
    }

    #[test]
    fn round_trips_a_connection() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_connection(&sample("a")).unwrap();

        let all = store.list_connections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Local");
        assert_eq!(all[0].engine, Engine::Postgres);
        assert_eq!(all[0].port, 5432);
        assert_eq!(all[0].color.as_deref(), Some("#4f8"));
    }

    #[test]
    fn upsert_updates_rather_than_duplicating() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_connection(&sample("a")).unwrap();

        let mut edited = sample("a");
        edited.name = "Renamed".into();
        edited.port = 6543;
        store.upsert_connection(&edited).unwrap();

        let all = store.list_connections().unwrap();
        assert_eq!(all.len(), 1, "same id must not create a second row");
        assert_eq!(all[0].name, "Renamed");
        assert_eq!(all[0].port, 6543);
    }

    #[test]
    fn delete_removes_only_the_target() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_connection(&sample("a")).unwrap();
        store.upsert_connection(&sample("b")).unwrap();

        store.delete_connection("a").unwrap();

        let all = store.list_connections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "b");
        assert!(!store.connection_exists("a").unwrap());
    }

    #[test]
    fn engine_and_ssl_survive_a_round_trip() {
        let store = Store::open_in_memory().unwrap();
        for engine in [
            Engine::MySql,
            Engine::Sqlite,
            Engine::SqlServer,
            Engine::MongoDb,
        ] {
            let mut c = sample("x");
            c.engine = engine;
            c.ssl_mode = SslMode::Require;
            store.upsert_connection(&c).unwrap();
            let got = store.get_connection("x").unwrap().unwrap();
            assert_eq!(got.engine, engine);
            assert_eq!(got.ssl_mode, SslMode::Require);
        }
    }

    #[test]
    fn file_path_survives_for_sqlite() {
        let store = Store::open_in_memory().unwrap();
        let mut c = sample("s");
        c.engine = Engine::Sqlite;
        c.file_path = Some("/tmp/x.db".into());
        store.upsert_connection(&c).unwrap();
        assert_eq!(
            store
                .get_connection("s")
                .unwrap()
                .unwrap()
                .file_path
                .as_deref(),
            Some("/tmp/x.db")
        );
    }

    #[test]
    fn migrating_twice_is_safe() {
        // Simulates reopening an existing install.
        let store = Store::open_in_memory().unwrap();
        store.migrate().unwrap();
        assert!(store.list_connections().unwrap().is_empty());
        assert!(store.list_saved_queries().unwrap().is_empty());
        assert!(store.list_history(None, 10).unwrap().is_empty());
    }

    /// Exactly the schema migration v1 produced, so the upgrade path is
    /// exercised against what a real Phase 1/2 install actually has on disk.
    const V1_SCHEMA: &str = r#"
        CREATE TABLE connections (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            engine      TEXT NOT NULL,
            host        TEXT NOT NULL DEFAULT '',
            port        INTEGER NOT NULL DEFAULT 0,
            username    TEXT NOT NULL DEFAULT '',
            database    TEXT NOT NULL DEFAULT '',
            file_path   TEXT,
            ssl_mode    TEXT NOT NULL DEFAULT 'prefer',
            color       TEXT,
            read_only   INTEGER NOT NULL DEFAULT 0,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO connections (id, name, engine, host, port, username, database)
        VALUES ('keepme', 'Precious', 'postgres', 'localhost', 5432, 'me', 'app');
        PRAGMA user_version = 1;
    "#;

    #[test]
    fn upgrades_a_v1_database_without_losing_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        let store = Store::from_connection(conn);

        store.migrate().unwrap();

        // The user's existing connection must come through untouched...
        let all = store.list_connections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Precious");
        assert_eq!(all[0].port, 5432);

        // ...and the new tables must be usable.
        store.upsert_saved_query(&saved("q1", "New", None)).unwrap();
        store.add_history(&history("SELECT 1", None)).unwrap();
        assert_eq!(store.list_saved_queries().unwrap().len(), 1);
        assert_eq!(store.list_history(None, 10).unwrap().len(), 1);

        let version: i32 = store
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn migrating_a_v1_database_twice_is_safe() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        let store = Store::from_connection(conn);

        store.migrate().unwrap();
        store.add_history(&history("SELECT 1", None)).unwrap();
        // A second pass must not recreate the tables and discard the row.
        store.migrate().unwrap();

        assert_eq!(store.list_history(None, 10).unwrap().len(), 1);
    }

    fn saved(id: &str, name: &str, folder: Option<&str>) -> SavedQuery {
        SavedQuery {
            id: id.into(),
            name: name.into(),
            folder: folder.map(String::from),
            sql: "SELECT 1".into(),
            connection_id: Some("c1".into()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn round_trips_a_saved_query() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_query(&saved("q1", "Daily counts", Some("Reports")))
            .unwrap();

        let got = store.get_saved_query("q1").unwrap().unwrap();
        assert_eq!(got.name, "Daily counts");
        assert_eq!(got.folder.as_deref(), Some("Reports"));
        assert_eq!(got.sql, "SELECT 1");
        // Timestamps are filled in by the database, not the caller.
        assert!(!got.created_at.is_empty());
    }

    #[test]
    fn upsert_edits_rather_than_duplicating_and_keeps_created_at() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_query(&saved("q1", "Original", None))
            .unwrap();
        let first = store.get_saved_query("q1").unwrap().unwrap();

        let mut edited = saved("q1", "Renamed", None);
        edited.sql = "SELECT 2".into();
        store.upsert_saved_query(&edited).unwrap();

        let all = store.list_saved_queries().unwrap();
        assert_eq!(all.len(), 1, "same id must not create a second row");
        assert_eq!(all[0].name, "Renamed");
        assert_eq!(all[0].sql, "SELECT 2");
        assert_eq!(
            all[0].created_at, first.created_at,
            "created_at must survive an edit"
        );
    }

    #[test]
    fn saved_queries_group_folders_first_then_loose_queries() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_query(&saved("b", "zebra", None))
            .unwrap();
        store
            .upsert_saved_query(&saved("c", "alpha", Some("Reports")))
            .unwrap();
        store
            .upsert_saved_query(&saved("a", "apple", None))
            .unwrap();

        let names: Vec<String> = store
            .list_saved_queries()
            .unwrap()
            .into_iter()
            .map(|q| q.name)
            .collect();
        // Foldered first (as file trees group), then loose ones, each A–Z.
        assert_eq!(names, vec!["alpha", "apple", "zebra"]);
    }

    #[test]
    fn saved_queries_sort_case_insensitively_within_a_folder() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_query(&saved("a", "banana", Some("F")))
            .unwrap();
        store
            .upsert_saved_query(&saved("b", "Apple", Some("F")))
            .unwrap();

        let names: Vec<String> = store
            .list_saved_queries()
            .unwrap()
            .into_iter()
            .map(|q| q.name)
            .collect();
        // Without NOCASE, "Apple" and "banana" would order by byte value.
        assert_eq!(names, vec!["Apple", "banana"]);
    }

    #[test]
    fn deleting_a_connection_leaves_saved_queries_intact() {
        // Losing saved work as a side effect of removing a connection would be
        // a nasty surprise; a dangling connection_id is the lesser evil.
        let store = Store::open_in_memory().unwrap();
        store.upsert_connection(&sample("c1")).unwrap();
        store
            .upsert_saved_query(&saved("q1", "Keeper", None))
            .unwrap();

        store.delete_connection("c1").unwrap();

        let all = store.list_saved_queries().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].connection_id.as_deref(), Some("c1"));
    }

    fn history(sql: &str, error: Option<&str>) -> NewHistoryEntry {
        NewHistoryEntry {
            sql: sql.into(),
            connection_id: Some("c1".into()),
            connection_name: Some("Local".into()),
            duration_ms: 12,
            row_count: 3,
            error: error.map(String::from),
        }
    }

    #[test]
    fn records_a_successful_run() {
        let store = Store::open_in_memory().unwrap();
        store.add_history(&history("SELECT 1", None)).unwrap();

        let all = store.list_history(None, 10).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].succeeded);
        assert_eq!(all[0].error, None);
        assert_eq!(all[0].duration_ms, 12);
        assert_eq!(all[0].row_count, 3);
        assert!(!all[0].executed_at.is_empty());
    }

    #[test]
    fn records_failures_too() {
        // Failed queries are often exactly what someone wants to revisit.
        let store = Store::open_in_memory().unwrap();
        store
            .add_history(&history("SELECT bad", Some("no such column")))
            .unwrap();

        let all = store.list_history(None, 10).unwrap();
        assert!(!all[0].succeeded);
        assert_eq!(all[0].error.as_deref(), Some("no such column"));
    }

    #[test]
    fn history_is_newest_first() {
        let store = Store::open_in_memory().unwrap();
        for sql in ["first", "second", "third"] {
            store.add_history(&history(sql, None)).unwrap();
        }
        let all = store.list_history(None, 10).unwrap();
        assert_eq!(
            all.iter().map(|h| h.sql.as_str()).collect::<Vec<_>>(),
            vec!["third", "second", "first"]
        );
    }

    #[test]
    fn history_search_matches_a_substring_of_the_sql() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_history(&history("SELECT * FROM authors", None))
            .unwrap();
        store
            .add_history(&history("SELECT * FROM books", None))
            .unwrap();

        let found = store.list_history(Some("authors"), 10).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].sql.contains("authors"));

        // No search term returns everything, rather than nothing.
        assert_eq!(store.list_history(None, 10).unwrap().len(), 2);
    }

    #[test]
    fn history_search_treats_like_wildcards_as_literal_text() {
        // `%` and `_` are LIKE wildcards. Interpolated unescaped, a search for
        // "50%" matched every row containing "50" — and "_" matched any single
        // character — so the box quietly did something other than what it said.
        let store = Store::open_in_memory().unwrap();
        store
            .add_history(&history("SELECT '50%' AS pct", None))
            .unwrap();
        store
            .add_history(&history("SELECT 500 AS five_hundred", None))
            .unwrap();

        let found = store.list_history(Some("50%"), 10).unwrap();
        assert_eq!(found.len(), 1, "wildcard matched beyond the literal text");
        assert!(found[0].sql.contains("'50%'"), "{}", found[0].sql);

        // The same for `_`, which would otherwise match any single character.
        let found = store.list_history(Some("five_hundred"), 10).unwrap();
        assert_eq!(found.len(), 1);

        // And a lone backslash must not break the pattern it is escaped into.
        assert!(store.list_history(Some(r"\"), 10).is_ok());
    }

    #[test]
    fn history_keeps_the_connection_name_after_the_connection_is_gone() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_connection(&sample("c1")).unwrap();
        store.add_history(&history("SELECT 1", None)).unwrap();

        store.delete_connection("c1").unwrap();

        let all = store.list_history(None, 10).unwrap();
        assert_eq!(all.len(), 1, "history must outlive the connection");
        assert_eq!(
            all[0].connection_name.as_deref(),
            Some("Local"),
            "the denormalized name is what keeps this readable"
        );
    }

    #[test]
    fn prune_keeps_the_newest_and_drops_the_rest() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..10 {
            store.add_history(&history(&format!("q{i}"), None)).unwrap();
        }

        let removed = store.prune_history(4).unwrap();
        assert_eq!(removed, 6);

        let all = store.list_history(None, 100).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].sql, "q9", "the newest must survive");
        assert_eq!(all[3].sql, "q6");
    }

    #[test]
    fn pruning_below_the_cap_removes_nothing() {
        let store = Store::open_in_memory().unwrap();
        store.add_history(&history("only", None)).unwrap();
        assert_eq!(store.prune_history(100).unwrap(), 0);
        assert_eq!(store.list_history(None, 10).unwrap().len(), 1);
    }

    #[test]
    fn clear_and_delete_remove_history() {
        let store = Store::open_in_memory().unwrap();
        let id = store.add_history(&history("a", None)).unwrap();
        store.add_history(&history("b", None)).unwrap();

        store.delete_history_entry(id).unwrap();
        assert_eq!(store.list_history(None, 10).unwrap().len(), 1);

        store.clear_history().unwrap();
        assert!(store.list_history(None, 10).unwrap().is_empty());
    }

    #[test]
    fn limit_caps_the_history_page() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..5 {
            store.add_history(&history(&format!("q{i}"), None)).unwrap();
        }
        assert_eq!(store.list_history(None, 2).unwrap().len(), 2);
    }

    #[test]
    fn every_engine_round_trips_through_storage() {
        // `parse_engine` now returns None for anything it does not recognise,
        // and the loader drops such rows. If these two ever disagree, saved
        // connections would silently disappear from the sidebar.
        for engine in [
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
        ] {
            assert_eq!(
                parse_engine(engine_str(engine)),
                Some(engine),
                "{engine:?} does not survive a save/load cycle"
            );
        }
    }

    #[test]
    fn an_unknown_engine_is_rejected_rather_than_guessed() {
        // It used to become Postgres, which meant a connection saved by a newer
        // build would try the Postgres wire protocol against the wrong server.
        assert_eq!(parse_engine("some-future-engine"), None);
        assert_eq!(parse_engine(""), None);
    }

    #[test]
    fn every_ssl_mode_round_trips_through_storage() {
        for mode in [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            assert_eq!(parse_ssl(ssl_str(mode)), mode, "{mode:?} did not survive");
        }
    }
}
