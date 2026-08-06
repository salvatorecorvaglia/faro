//! Local app state: saved connections, and (from Phase 3) saved queries and
//! history. A small SQLite file in the platform config directory.
//!
//! Passwords are never stored here — see `secrets`.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{FaroError, Result};
use crate::model::{ConnectionConfig, Engine, HistoryEntry, NewHistoryEntry, SavedQuery, SslMode};

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

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store mutex poisoned")
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

    pub fn list_connections(&self) -> Result<Vec<ConnectionConfig>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, engine, host, port, username, database, file_path,
                    ssl_mode, color, read_only
             FROM connections ORDER BY sort_order, name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConnectionConfig {
                id: r.get("id")?,
                name: r.get("name")?,
                engine: parse_engine(&r.get::<_, String>("engine")?),
                host: r.get("host")?,
                port: r.get::<_, i64>("port")? as u16,
                username: r.get("username")?,
                database: r.get("database")?,
                file_path: r.get("file_path")?,
                ssl_mode: parse_ssl(&r.get::<_, String>("ssl_mode")?),
                color: r.get("color")?,
                read_only: r.get::<_, i64>("read_only")? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_connection(&self, id: &str) -> Result<Option<ConnectionConfig>> {
        // Reuses the list mapping rather than duplicating the column list;
        // the connection count is tiny by nature.
        Ok(self.list_connections()?.into_iter().find(|c| c.id == id))
    }

    /// Insert or update. The id is generated by the caller so the keychain entry
    /// and the row can be created together.
    pub fn upsert_connection(&self, c: &ConnectionConfig) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO connections
                (id, name, engine, host, port, username, database, file_path,
                 ssl_mode, color, read_only)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                engine = excluded.engine,
                host = excluded.host,
                port = excluded.port,
                username = excluded.username,
                database = excluded.database,
                file_path = excluded.file_path,
                ssl_mode = excluded.ssl_mode,
                color = excluded.color,
                read_only = excluded.read_only
            "#,
            params![
                c.id,
                c.name,
                engine_str(c.engine),
                c.host,
                c.port as i64,
                c.username,
                c.database,
                c.file_path,
                ssl_str(c.ssl_mode),
                c.color,
                c.read_only as i64,
            ],
        )?;
        Ok(())
    }

    pub fn delete_connection(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM connections WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn connection_exists(&self, id: &str) -> Result<bool> {
        let conn = self.lock();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM connections WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    // -- Saved queries -----------------------------------------------------

    /// All saved queries: foldered ones first grouped by folder, then loose
    /// ones, each alphabetical. Matches how file trees group, and keeps the
    /// sidebar order stable regardless of when things were created.
    ///
    /// `folder IS NULL` sorts 0 before 1, which is what puts foldered rows
    /// ahead of unfoldered ones.
    pub fn list_saved_queries(&self) -> Result<Vec<SavedQuery>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, folder, sql, connection_id, created_at, updated_at
             FROM saved_queries
             ORDER BY folder IS NULL, folder COLLATE NOCASE, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], map_saved_query)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_saved_query(&self, id: &str) -> Result<Option<SavedQuery>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, folder, sql, connection_id, created_at, updated_at
             FROM saved_queries WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id], map_saved_query).optional()?)
    }

    /// Insert or update. `updated_at` moves on every write; `created_at` is
    /// preserved so the original save time survives an edit.
    pub fn upsert_saved_query(&self, q: &SavedQuery) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO saved_queries (id, name, folder, sql, connection_id)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                folder = excluded.folder,
                sql = excluded.sql,
                connection_id = excluded.connection_id,
                updated_at = datetime('now')
            "#,
            params![q.id, q.name, q.folder, q.sql, q.connection_id],
        )?;
        Ok(())
    }

    pub fn delete_saved_query(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM saved_queries WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- History -----------------------------------------------------------

    /// Record one execution. Returns the new row id.
    pub fn add_history(&self, entry: &NewHistoryEntry) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            r#"
            INSERT INTO query_history
                (sql, connection_id, connection_name, duration_ms, row_count, error, succeeded)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                entry.sql,
                entry.connection_id,
                entry.connection_name,
                entry.duration_ms,
                entry.row_count,
                entry.error,
                entry.error.is_none() as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Most recent first, optionally narrowed by a substring of the SQL.
    ///
    /// `search` matches the SQL text only. Matching timestamps or durations
    /// would surprise more than it would help.
    pub fn list_history(&self, search: Option<&str>, limit: i64) -> Result<Vec<HistoryEntry>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, sql, connection_id, connection_name, executed_at,
                   duration_ms, row_count, error, succeeded
            FROM query_history
            WHERE (?1 IS NULL OR sql LIKE '%' || ?1 || '%')
            ORDER BY id DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![search, limit], |r| {
            Ok(HistoryEntry {
                id: r.get("id")?,
                sql: r.get("sql")?,
                connection_id: r.get("connection_id")?,
                connection_name: r.get("connection_name")?,
                executed_at: r.get("executed_at")?,
                duration_ms: r.get("duration_ms")?,
                row_count: r.get("row_count")?,
                error: r.get("error")?,
                succeeded: r.get::<_, i64>("succeeded")? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.lock().execute("DELETE FROM query_history", [])?;
        Ok(())
    }

    pub fn delete_history_entry(&self, id: i64) -> Result<()> {
        self.lock()
            .execute("DELETE FROM query_history WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Drop the oldest rows beyond `keep`.
    ///
    /// History is written on every run, so without a cap it grows without
    /// bound for the lifetime of the install.
    pub fn prune_history(&self, keep: i64) -> Result<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM query_history WHERE id NOT IN (
                 SELECT id FROM query_history ORDER BY id DESC LIMIT ?1
             )",
            params![keep],
        )?)
    }
}

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

/// Unknown strings fall back to Postgres rather than failing the whole list:
/// one unreadable row should not hide every other saved connection.
fn parse_engine(s: &str) -> Engine {
    match s {
        "mysql" => Engine::MySql,
        "mariadb" => Engine::MariaDb,
        "sqlite" => Engine::Sqlite,
        "sqlserver" => Engine::SqlServer,
        "duckdb" => Engine::DuckDb,
        "clickhouse" => Engine::ClickHouse,
        "cockroachdb" => Engine::CockroachDb,
        "redshift" => Engine::Redshift,
        "mongodb" => Engine::MongoDb,
        _ => Engine::Postgres,
    }
}

fn ssl_str(m: SslMode) -> &'static str {
    match m {
        SslMode::Disable => "disable",
        SslMode::Prefer => "prefer",
        SslMode::Require => "require",
    }
}

fn parse_ssl(s: &str) -> SslMode {
    match s {
        "disable" => SslMode::Disable,
        "require" => SslMode::Require,
        _ => SslMode::Prefer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
