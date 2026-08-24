use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow};
use sqlx::{AssertSqlSafe, Row, TypeInfo, ValueRef};
use std::str::FromStr;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    ColumnDetail, ConnectionConfig, ExecResult, GuardedStatement, IndexInfo, ResultSet, SchemaInfo,
    TableDetail, TableInfo, TableKind, TableRef, Value,
};

pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_double(ident)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        dialect::paginate_limit_offset(sql, limit, offset)
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        dialect::hex_bytes_x(bytes)
    }

    /// SQLite has ATTACH, but a single-file connection has one namespace. Faro
    /// presents it as schema-less so generated SQL stays unqualified.
    fn supports_schemas(&self) -> bool {
        false
    }
}

pub struct SqliteDriver {
    /// User SQL. Single connection, so session state is coherent.
    pool: SqlitePool,
    /// Catalog reads only, so schema browsing never queues behind a long query.
    meta: SqlitePool,
    dialect: SqliteDialect,
}

impl SqliteDriver {
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let path = config
            .file_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| FaroError::Connection("no database file selected".into()))?;

        // Refuse to create a new file by accident: opening a mistyped path
        // should be an error, not a silently empty database.
        if path != ":memory:" && !std::path::Path::new(path).exists() {
            return Err(FaroError::Connection(format!("no such file: {path}")));
        }

        let opts = SqliteConnectOptions::from_str(path)
            .map_err(|e| FaroError::Connection(e.to_string()))?
            .create_if_missing(false)
            .read_only(config.read_only)
            // Journal mode is deliberately left alone.
            //
            // Setting it — even to the value the file already has — takes an
            // exclusive lock and persistently rewrites the user's database,
            // leaving -wal and -shm files behind. Faro is here to inspect data,
            // not to reconfigure someone's database as a side effect of opening
            // it, and forcing the change fails outright on a read-only file or
            // in a read-only directory.
            //
            // Wait out brief locks instead, which is all the concurrency
            // support a client actually needs.
            .busy_timeout(std::time::Duration::from_secs(5))
            .foreign_keys(true);

        // See `driver::pool::dual_pool` for the general reasoning (session
        // state needs one connection; catalog reads get their own so the
        // schema tree stays responsive). No `acquire_timeout` here — unlike a
        // networked engine, a local file has no host to hang on. WAL permits
        // concurrent readers, so splitting catalog reads off costs nothing.
        let (pool, meta) = super::pool::dual_pool::<sqlx::Sqlite>(opts, None).await?;

        Ok(Self {
            pool,
            meta,
            dialect: SqliteDialect,
        })
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        // One implicit namespace; returning it keeps the tree shape uniform
        // with the server engines instead of special-casing the UI.
        Ok(vec![SchemaInfo {
            name: "main".into(),
            is_system: false,
        }])
    }

    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let rows = sqlx::query(
            r#"
            SELECT name, type
            FROM sqlite_master
            WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
            ORDER BY name
            "#,
        )
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let kind: String = r.get("type");
                TableInfo {
                    schema: None,
                    name: r.get("name"),
                    kind: if kind == "view" {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                    // SQLite keeps no row estimate, and COUNT(*) per table would
                    // stall the tree on a large file.
                    estimated_rows: None,
                }
            })
            .collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let quoted = self.dialect.quote_ident(&table.name);

        // PRAGMA does not accept bind parameters, so the table name is
        // interpolated — quote_ident escapes embedded quotes, which is what
        // makes that safe. Same reasoning for every AssertSqlSafe below.
        let col_rows = sqlx::query(AssertSqlSafe(format!("PRAGMA table_info({quoted})")))
            .fetch_all(&self.meta)
            .await?;

        if col_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }

        let mut primary_key = Vec::new();
        let mut columns = Vec::new();
        for r in &col_rows {
            let name: String = r.get("name");
            // pk is the 1-based position within the PK, 0 when not part of it.
            let pk_pos: i32 = r.get("pk");
            if pk_pos > 0 {
                primary_key.push((pk_pos, name.clone()));
            }
            columns.push(ColumnDetail {
                name,
                type_name: r.get::<Option<String>, _>("type").unwrap_or_default(),
                nullable: r.get::<i32, _>("notnull") == 0,
                default: r.get::<Option<String>, _>("dflt_value"),
                is_primary_key: pk_pos > 0,
                ordinal: r.get::<i32, _>("cid"),
            });
        }
        primary_key.sort_by_key(|(pos, _)| *pos);
        let primary_key: Vec<String> = primary_key.into_iter().map(|(_, n)| n).collect();

        let fk_rows = sqlx::query(AssertSqlSafe(format!("PRAGMA foreign_key_list({quoted})")))
            .fetch_all(&self.meta)
            .await?;

        // SQLite reports one row per column of a composite FK, sharing an id
        // — grouped by that id below, not by position: SQLite numbers foreign
        // keys in reverse order of declaration, so the ids arrive descending
        // and are not indices into the output list.
        let fks = super::fk::group_foreign_keys(
            fk_rows
                .iter()
                .map(|r| super::fk::FkColumnRow {
                    group: r.get::<i32, _>("id"),
                    column: r.get("from"),
                    referenced_schema: None,
                    referenced_table: r.get("table"),
                    referenced_column: r.get("to"),
                })
                .collect(),
            |id| format!("fk_{}_{}", table.name, id),
        );

        let idx_rows = sqlx::query(AssertSqlSafe(format!("PRAGMA index_list({quoted})")))
            .fetch_all(&self.meta)
            .await?;

        let mut indexes = Vec::new();
        for r in &idx_rows {
            let name: String = r.get("name");
            let unique: i32 = r.get("unique");
            let origin: String = r.get::<Option<String>, _>("origin").unwrap_or_default();
            let info = sqlx::query(AssertSqlSafe(format!(
                "PRAGMA index_info({})",
                self.dialect.quote_ident(&name)
            )))
            .fetch_all(&self.meta)
            .await?;
            indexes.push(IndexInfo {
                name,
                columns: info
                    .iter()
                    .filter_map(|c| c.get::<Option<String>, _>("name"))
                    .collect(),
                is_unique: unique == 1,
                is_primary: origin == "pk",
                // PRAGMA index_list reports origin: 'c' for CREATE INDEX,
                // 'u' for a UNIQUE constraint, 'pk' for the primary key. Only
                // 'c' indexes can be recreated with CREATE INDEX.
                is_constraint: origin != "c",
            });
        }

        let kind_row = sqlx::query("SELECT type FROM sqlite_master WHERE name = ?")
            .bind(&table.name)
            .fetch_optional(&self.meta)
            .await?;
        let kind = kind_row
            .map(|r| r.get::<String, _>("type"))
            .unwrap_or_else(|| "table".into());

        Ok(TableDetail {
            table: table.clone(),
            kind: if kind == "view" {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns,
            primary_key,
            foreign_keys: fks,
            indexes,
        })
    }

    /// SQLite keeps the original `CREATE TABLE` text in `sqlite_master`, which
    /// is far more faithful than anything reconstructed from `table_info` —
    /// `AUTOINCREMENT`, inline `UNIQUE` and `CHECK` all survive.
    async fn table_ddl(&self, table: &TableRef) -> Result<Option<String>> {
        let row = sqlx::query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(&table.name)
            .fetch_optional(&self.meta)
            .await?;

        Ok(row
            .and_then(|r| r.get::<Option<String>, _>("sql"))
            .map(|sql| {
                // sqlite_master omits the trailing semicolon.
                let trimmed = sql.trim().trim_end_matches(';').to_string();
                format!("{trimmed};")
            }))
    }

    /// Run a statement and cap the rows as they arrive.
    ///
    /// Passing the SQL through untouched is what lets a user run `PRAGMA
    /// table_info(t)` — no engine accepts a `PRAGMA` as a derived table, so the
    /// old wrapping made every one of them a syntax error. See `driver::fetch`.
    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        super::fetch::fetch_capped_sqlite(&self.pool, sql, limit, cancel).await
    }

    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        let started = Instant::now();
        let owned = sql.to_string();
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FaroError::Cancelled),
            res = sqlx::query(AssertSqlSafe(owned)).execute(&self.pool) => res?,
        };
        Ok(ExecResult {
            rows_affected: res.rows_affected(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        super::tx::apply_guarded_sqlite(&self.pool, statements).await
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // Both pools, or the metadata connection keeps the file handle open.
        self.pool.close().await;
        self.meta.close().await;
    }
}

/// Decode one cell.
///
/// SQLite is dynamically typed: the *value's* storage class matters more than
/// the column's declared type, so this switches on what actually came back.
pub(super) fn decode_value(row: &SqliteRow, idx: usize) -> Value {
    let raw = match row.try_get_raw(idx) {
        Ok(r) => r,
        Err(_) => return Value::Null,
    };
    if raw.is_null() {
        return Value::Null;
    }

    let type_name = raw.type_info().name().to_uppercase();

    match type_name.as_str() {
        "INTEGER" | "INT" | "BIGINT" => row.try_get::<i64, _>(idx).map(Value::Int),
        "REAL" | "DOUBLE" | "FLOAT" => row.try_get::<f64, _>(idx).map(Value::Float),
        "BLOB" => row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes),
        "BOOLEAN" => row.try_get::<bool, _>(idx).map(Value::Bool),
        // TEXT and anything with a declared-but-unenforced type. SQLite will
        // coerce most things to text; if it cannot, fall through to Int/Float
        // before giving up, since a column declared NUMERIC can hold either.
        _ => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .or_else(|_| row.try_get::<i64, _>(idx).map(Value::Int))
            .or_else(|_| row.try_get::<f64, _>(idx).map(Value::Float)),
    }
    .unwrap_or(Value::Unsupported(type_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_is_schema_less() {
        // Qualifying with a schema would produce "main"."t", which is valid but
        // noisy; Faro emits bare table names instead.
        assert!(!SqliteDialect.supports_schemas());
        assert_eq!(SqliteDialect.qualify(Some("main"), "users"), r#""users""#);
    }

    #[test]
    fn blob_literals_use_x_prefix() {
        assert_eq!(SqliteDialect.quote_bytes(&[0xff, 0x00]), "X'ff00'");
    }

    #[test]
    fn quoting_neutralizes_a_pragma_injection_attempt() {
        // PRAGMA takes no bind params, so this escaping is what keeps
        // describe_table safe against a hostile table name.
        let evil = r#"t") ; DROP TABLE users; --"#;
        let quoted = SqliteDialect.quote_ident(evil);
        assert_eq!(quoted, r#""t"") ; DROP TABLE users; --""#);
        // The injected quote is doubled, so the identifier stays one token.
        assert!(quoted.starts_with('"') && quoted.ends_with('"'));
    }
}
