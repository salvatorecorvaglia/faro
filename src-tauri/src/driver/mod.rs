//! The database abstraction. Everything above this module is engine-agnostic.

mod clickhouse;
pub mod dialect;
#[cfg(feature = "duckdb-engine")]
mod duckdb;
mod fetch;
mod mongo;
mod mssql;
mod mysql;
mod postgres;
mod sqlite;
mod tx;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::model::{
    BrowseOptions, ConnectionConfig, Engine, ExecResult, GuardedStatement, QueryOutcome, ResultSet,
    SchemaInfo, TableColumns, TableDetail, TableInfo, TableRef,
};

pub use dialect::Dialect;

/// One live connection to one database.
///
/// Implementations own their own pool. Every feature in the app — browsing,
/// querying, export, backup — goes through this trait, so adding an engine
/// never requires touching UI or command code.
#[async_trait]
pub trait Driver: Send + Sync {
    /// Cheap round-trip used to validate credentials and detect dropped links.
    async fn ping(&self) -> Result<()>;

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>>;

    /// Tables and views in `schema`. `None` for engines without schemas.
    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>>;

    /// Columns, primary key, foreign keys and indexes for one table.
    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail>;

    /// The engine's own `CREATE TABLE` text, when it keeps one.
    ///
    /// Preferred by backup over Faro's reconstructed DDL, because it is exactly
    /// faithful: it keeps `AUTOINCREMENT`, inline `UNIQUE`, `CHECK` constraints,
    /// generated columns and collations that a column-by-column rebuild cannot
    /// express. `None` means the engine does not expose it and the generic path
    /// is used instead.
    async fn table_ddl(&self, _table: &TableRef) -> Result<Option<String>> {
        Ok(None)
    }

    /// Every table in `schema` with its column names, for editor autocomplete.
    ///
    /// The default walks `describe_table`, which is correct but costs several
    /// round trips per table — fine for SQLite, painful on a database with
    /// hundreds of tables. Engines that can answer in one catalog query should
    /// override; Postgres does.
    async fn schema_snapshot(&self, schema: Option<&str>) -> Result<Vec<TableColumns>> {
        let tables = self.list_tables(schema).await?;
        let mut out = Vec::with_capacity(tables.len());
        for t in tables {
            let table = TableRef {
                schema: t.schema.clone(),
                name: t.name.clone(),
            };
            // Skip tables that fail to describe (dropped mid-walk, or no
            // permission) rather than losing autocomplete for the whole schema.
            if let Ok(detail) = self.describe_table(&table).await {
                out.push(TableColumns {
                    schema: t.schema,
                    name: t.name,
                    columns: detail.columns.into_iter().map(|c| c.name).collect(),
                });
            }
        }
        Ok(out)
    }

    /// Run a statement that returns rows.
    ///
    /// Implementations must honour `cancel` so a runaway query can be aborted
    /// from the UI rather than blocking a pool slot until it finishes.
    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet>;

    /// Run a statement that returns no rows.
    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult>;

    /// Read a page of one table, with sorting and filtering applied by the
    /// engine rather than over the fetched rows.
    ///
    /// The default composes SQL, which is right for every SQL engine. MongoDB
    /// overrides it: there is no `SELECT` to compose, and translating a document
    /// query into SQL only to translate it back would be inventing a dialect
    /// nobody speaks.
    async fn browse(
        &self,
        table: &TableRef,
        options: &BrowseOptions,
        cancel: CancellationToken,
    ) -> Result<ResultSet> {
        let detail = self.describe_table(table).await?;
        let known: Vec<&str> = detail.columns.iter().map(|c| c.name.as_str()).collect();
        let limit = options.limit.unwrap_or(1000);
        let sql = crate::sql::build_browse_sql(table, options, &known, self.dialect(), limit);
        self.query(&sql, limit, cancel).await
    }

    /// Apply statements atomically, verifying each affects the rows it should.
    ///
    /// Everything commits or nothing does. A statement whose `expect` does not
    /// match the rows it actually touched aborts the batch: too few means the
    /// row moved or vanished under the user, too many means the key was not
    /// unique. Half-applying either would leave the data in a state nobody
    /// asked for.
    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64>;

    /// Run a statement without knowing in advance whether it returns rows.
    ///
    /// The default classifies by leading keyword, which covers the common cases;
    /// engines with better introspection can override.
    async fn run(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<QueryOutcome> {
        if returns_rows(sql) {
            self.query(sql, limit, cancel).await.map(QueryOutcome::Rows)
        } else {
            self.execute(sql, cancel).await.map(QueryOutcome::Affected)
        }
    }

    fn dialect(&self) -> &dyn Dialect;

    /// Close the pool. Called when the user disconnects or quits.
    async fn close(&self);
}

/// The statement's leading keyword, upper-cased, ignoring leading whitespace and
/// opening parentheses.
fn leading_keyword(sql: &str) -> String {
    sql.trim_start()
        .trim_start_matches(|c: char| c == '(' || c.is_whitespace())
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// Guess whether a statement produces a result set from its leading keyword.
///
/// `WITH` is included because CTEs usually feed a SELECT, and `INSERT ...
/// RETURNING` is deliberately *not* matched here — the Postgres driver detects
/// RETURNING itself, where it can be sure.
pub fn returns_rows(sql: &str) -> bool {
    matches!(
        leading_keyword(sql).as_str(),
        "SELECT"
            | "WITH"
            | "SHOW"
            | "EXPLAIN"
            | "DESCRIBE"
            | "DESC"
            | "PRAGMA"
            | "VALUES"
            | "TABLE"
    )
}

/// Whether `sql` may legally be used as a derived table — `SELECT * FROM (<sql>)`.
///
/// Only matters for the drivers that cap a result by rewriting the statement.
/// `SHOW`, `EXPLAIN`, `DESCRIBE` and `PRAGMA` all return rows but none of them is
/// a query expression, so wrapping one is a syntax error on every engine. Their
/// results are inherently small — a table list, a query plan — so leaving them
/// uncapped costs nothing.
///
/// A leading `(` counts: a parenthesised `SELECT`, or a `UNION` of them, nests
/// fine.
pub fn is_wrappable(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    trimmed.starts_with('(')
        || matches!(
            leading_keyword(trimmed).as_str(),
            "SELECT" | "WITH" | "VALUES" | "TABLE"
        )
}

/// Open a connection for `config`, using `password` from the keychain.
///
/// Several engines share a driver. CockroachDB and Redshift speak the
/// PostgreSQL wire protocol, and MariaDB speaks MySQL's, so they reuse those
/// drivers rather than duplicating them — they remain distinct `Engine`
/// variants so their default ports and displayed names stay truthful.
pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Box<dyn Driver>> {
    match config.engine {
        Engine::Postgres | Engine::CockroachDb | Engine::Redshift => Ok(Box::new(
            postgres::PostgresDriver::connect(config, password).await?,
        )),
        Engine::MySql | Engine::MariaDb => Ok(Box::new(
            mysql::MySqlDriver::connect(config, password).await?,
        )),
        Engine::SqlServer => Ok(Box::new(
            mssql::SqlServerDriver::connect(config, password).await?,
        )),
        Engine::ClickHouse => Ok(Box::new(
            clickhouse::ClickHouseDriver::connect(config, password).await?,
        )),
        Engine::MongoDb => Ok(Box::new(
            mongo::MongoDriver::connect(config, password).await?,
        )),
        Engine::Sqlite => Ok(Box::new(sqlite::SqliteDriver::connect(config).await?)),
        #[cfg(feature = "duckdb-engine")]
        Engine::DuckDb => Ok(Box::new(duckdb::DuckDbDriver::connect(config).await?)),
        // Compiled out by `--no-default-features`, where DuckDB is genuinely
        // unavailable rather than merely unimplemented.
        #[cfg(not(feature = "duckdb-engine"))]
        Engine::DuckDb => Err(crate::error::FaroError::UnsupportedEngine(
            "DuckDB (this build was compiled without it)".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_row_returning_statements() {
        assert!(returns_rows("SELECT 1"));
        assert!(returns_rows("  \n select * from t"));
        assert!(returns_rows("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(returns_rows("(SELECT 1) UNION (SELECT 2)"));
        assert!(returns_rows("EXPLAIN SELECT 1"));
        assert!(returns_rows("PRAGMA table_info(t)"));
    }

    #[test]
    fn classifies_non_row_statements() {
        assert!(!returns_rows("INSERT INTO t VALUES (1)"));
        assert!(!returns_rows("UPDATE t SET a = 1"));
        assert!(!returns_rows("CREATE TABLE t (a int)"));
        assert!(!returns_rows(""));
    }

    #[test]
    fn query_expressions_are_wrappable() {
        assert!(is_wrappable("SELECT 1"));
        assert!(is_wrappable("  \n select * from t"));
        assert!(is_wrappable("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(is_wrappable("(SELECT 1) UNION (SELECT 2)"));
        assert!(is_wrappable("VALUES (1), (2)"));
    }

    #[test]
    fn row_returning_commands_are_not_wrappable() {
        // These all return rows, but none is a query expression: wrapping one in
        // `SELECT * FROM (...)` is a syntax error on every engine, which is what
        // used to make them impossible to run.
        for sql in [
            "SHOW TABLES",
            "show databases",
            "PRAGMA table_info(t)",
            "EXPLAIN SELECT 1",
            "EXPLAIN ANALYZE SELECT 1",
            "DESCRIBE users",
            "DESC users",
        ] {
            assert!(
                returns_rows(sql),
                "{sql} should still be treated as row-returning"
            );
            assert!(!is_wrappable(sql), "{sql} must not be wrapped");
        }
    }

    #[test]
    fn statements_that_return_nothing_are_not_wrappable() {
        // Not strictly required — they never reach `query` — but wrapping one
        // would be nonsense, so the predicate should say so.
        assert!(!is_wrappable("INSERT INTO t VALUES (1)"));
        assert!(!is_wrappable("CREATE TABLE t (a int)"));
        assert!(!is_wrappable(""));
    }
}
