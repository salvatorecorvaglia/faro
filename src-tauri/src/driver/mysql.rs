use async_trait::async_trait;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow, MySqlSslMode};
use sqlx::{AssertSqlSafe, Row, TypeInfo, ValueRef};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    ColumnDetail, ConnectionConfig, ExecResult, ForeignKey, GuardedStatement, IndexInfo, ResultSet,
    SchemaInfo, SslMode, TableColumns, TableDetail, TableInfo, TableKind, TableRef, Value,
};

pub struct MySqlDialect;

impl Dialect for MySqlDialect {
    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_backtick(ident)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        dialect::paginate_limit_offset(sql, limit, offset)
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        dialect::hex_bytes_x(bytes)
    }

    /// MySQL and MariaDB honour backslash escapes inside string literals unless
    /// `NO_BACKSLASH_ESCAPES` is set, which it is not by default. Faro does not
    /// set it either, because doing so would change how the *user's* own SQL
    /// behaves; escaping correctly here is the fix that stays out of their way.
    fn quote_string(&self, s: &str) -> String {
        crate::model::quote_sql_string_backslash(s)
    }

    /// MySQL has no `TEXT` target for `CAST`; `CHAR` is the text cast.
    fn text_cast_type(&self) -> &'static str {
        "CHAR"
    }

    /// `SHOW CREATE TABLE` lists secondary indexes inline as `KEY name (col)`.
    fn native_ddl_includes_indexes(&self) -> bool {
        true
    }

    /// MySQL conflates "database" and "schema": a connection's schema *is* its
    /// database. Faro presents the current database as the one schema, so
    /// generated SQL stays unqualified and cannot accidentally reach across
    /// databases.
    fn supports_schemas(&self) -> bool {
        false
    }

    /// MySQL commits implicitly on every DDL statement, so a dump containing
    /// `CREATE TABLE` cannot be restored atomically no matter what transaction
    /// is open around it.
    fn ddl_is_transactional(&self) -> bool {
        false
    }
}

pub struct MySqlDriver {
    /// User SQL. Single connection, so session state is coherent.
    pool: MySqlPool,
    /// Catalog reads only, so schema browsing never queues behind a long query.
    meta: MySqlPool,
    dialect: MySqlDialect,
    /// The connected database, used to scope catalog queries.
    database: String,
}

impl MySqlDriver {
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let mut opts = MySqlConnectOptions::new()
            .host(&config.host)
            .port(if config.port == 0 { 3306 } else { config.port })
            .username(&config.username)
            .ssl_mode(match config.ssl_mode {
                SslMode::Disable => MySqlSslMode::Disabled,
                SslMode::Prefer => MySqlSslMode::Preferred,
                // `Required` encrypts without validating; the Verify modes are
                // the ones that actually authenticate the server.
                SslMode::Require => MySqlSslMode::Required,
                SslMode::VerifyCa => MySqlSslMode::VerifyCa,
                SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
            });

        if !config.database.is_empty() {
            opts = opts.database(&config.database);
        }
        if let Some(pw) = password {
            opts = opts.password(pw);
        }

        // See the Postgres driver for why user SQL gets one connection and
        // catalog reads get their own.
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(15))
            .connect_with(opts.clone())
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        let meta = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(15))
            .connect_with(opts)
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        if config.read_only {
            // Belt and braces alongside Faro's own guard: this makes the server
            // reject writes that a leading-keyword check cannot spot. It applies
            // to the session, so it also covers the implicit transaction around
            // each autocommit statement.
            //
            // Only the user-SQL pool needs it; `meta` runs Faro's own catalog
            // reads, which are reads by construction.
            sqlx::query("SET SESSION TRANSACTION READ ONLY")
                .execute(&pool)
                .await
                .map_err(|e| FaroError::Connection(e.to_string()))?;
        }

        // Resolve the database from the server rather than trusting the config:
        // it may have been omitted, and every catalog query needs it.
        let database: String = sqlx::query("SELECT DATABASE()")
            .fetch_one(&meta)
            .await
            .ok()
            .and_then(|r| r.get::<Option<String>, _>(0))
            .unwrap_or_else(|| config.database.clone());

        Ok(Self {
            pool,
            meta,
            dialect: MySqlDialect,
            database,
        })
    }
}

#[async_trait]
impl Driver for MySqlDriver {
    async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        // The connected database is the only namespace Faro browses. Listing
        // every database on the server would invite writing across them by
        // accident.
        Ok(vec![SchemaInfo {
            name: self.database.clone(),
            is_system: false,
        }])
    }

    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let rows = sqlx::query(
            r#"
            SELECT table_name AS name, table_type AS kind,
                   CAST(table_rows AS SIGNED) AS est_rows
            FROM information_schema.tables
            WHERE table_schema = ?
            ORDER BY table_name
            "#,
        )
        .bind(&self.database)
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let kind: String = r.get("kind");
                TableInfo {
                    schema: None,
                    name: r.get("name"),
                    kind: if kind.eq_ignore_ascii_case("VIEW") {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                    // information_schema.table_rows is an estimate for InnoDB,
                    // which is what makes it cheap enough to read here.
                    //
                    // CAST(... AS SIGNED) in the query above: information_schema
                    // declares this UNSIGNED, and its width differs between
                    // MySQL and MariaDB, so casting is what keeps one Rust type
                    // working on both.
                    estimated_rows: r.get::<Option<i64>, _>("est_rows"),
                }
            })
            .collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let col_rows = sqlx::query(
            r#"
            SELECT column_name AS name,
                   column_type AS type_name,
                   is_nullable AS nullable,
                   column_default AS default_expr,
                   CAST(ordinal_position AS SIGNED) AS ordinal,
                   column_key AS col_key
            FROM information_schema.columns
            WHERE table_schema = ? AND table_name = ?
            ORDER BY ordinal_position
            "#,
        )
        .bind(&self.database)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        if col_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }

        // Primary key columns in key order, which is what generated DML needs.
        let pk_rows = sqlx::query(
            r#"
            SELECT column_name AS name
            FROM information_schema.key_column_usage
            WHERE table_schema = ? AND table_name = ? AND constraint_name = 'PRIMARY'
            ORDER BY ordinal_position
            "#,
        )
        .bind(&self.database)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;
        let primary_key: Vec<String> = pk_rows.iter().map(|r| r.get("name")).collect();

        let fk_rows = sqlx::query(
            r#"
            SELECT k.constraint_name AS name,
                   k.column_name AS column_name,
                   k.referenced_table_name AS ref_table,
                   k.referenced_column_name AS ref_column
            FROM information_schema.key_column_usage k
            WHERE k.table_schema = ? AND k.table_name = ?
              AND k.referenced_table_name IS NOT NULL
            ORDER BY k.constraint_name, k.ordinal_position
            "#,
        )
        .bind(&self.database)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        // One row per column of a composite key, grouped by constraint name.
        let mut foreign_keys: Vec<ForeignKey> = Vec::new();
        for r in &fk_rows {
            let name: String = r.get("name");
            let column: String = r.get("column_name");
            let ref_table: String = r.get("ref_table");
            let ref_column: String = r.get("ref_column");

            match foreign_keys.iter_mut().find(|f| f.name == name) {
                Some(fk) => {
                    fk.columns.push(column);
                    fk.referenced_columns.push(ref_column);
                }
                None => foreign_keys.push(ForeignKey {
                    name,
                    columns: vec![column],
                    referenced_table: TableRef {
                        schema: None,
                        name: ref_table,
                    },
                    referenced_columns: vec![ref_column],
                }),
            }
        }

        let idx_rows = sqlx::query(
            r#"
            SELECT index_name AS name,
                   CAST(non_unique AS SIGNED) AS non_unique,
                   column_name AS column_name
            FROM information_schema.statistics
            WHERE table_schema = ? AND table_name = ?
            ORDER BY index_name, seq_in_index
            "#,
        )
        .bind(&self.database)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        let mut indexes: Vec<IndexInfo> = Vec::new();
        for r in &idx_rows {
            let name: String = r.get("name");
            let column: Option<String> = r.get("column_name");
            let non_unique: i64 = r.get("non_unique");

            match indexes.iter_mut().find(|i| i.name == name) {
                Some(index) => {
                    if let Some(c) = column {
                        index.columns.push(c);
                    }
                }
                None => {
                    let is_primary = name == "PRIMARY";
                    indexes.push(IndexInfo {
                        name: name.clone(),
                        columns: column.into_iter().collect(),
                        is_unique: non_unique == 0,
                        is_primary,
                        // MySQL creates an index for every UNIQUE constraint and
                        // for the primary key; information_schema does not
                        // distinguish those from a plain CREATE INDEX, so a
                        // unique index is treated as constraint-backed. Emitting
                        // a duplicate is the worse error.
                        is_constraint: is_primary || non_unique == 0,
                    });
                }
            }
        }

        // Aliased deliberately: MySQL returns information_schema columns in
        // upper case, and sqlx looks them up case-sensitively, so an unaliased
        // `table_type` is not found by that name.
        let kind_row = sqlx::query(
            "SELECT table_type AS kind FROM information_schema.tables
             WHERE table_schema = ? AND table_name = ?",
        )
        .bind(&self.database)
        .bind(&table.name)
        .fetch_optional(&self.meta)
        .await?;
        let is_view = kind_row
            .map(|r| r.get::<String, _>("kind").eq_ignore_ascii_case("VIEW"))
            .unwrap_or(false);

        Ok(TableDetail {
            table: table.clone(),
            kind: if is_view {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns: col_rows
                .into_iter()
                .map(|r| {
                    let name: String = r.get("name");
                    ColumnDetail {
                        is_primary_key: primary_key.contains(&name),
                        name,
                        type_name: r.get("type_name"),
                        nullable: r.get::<String, _>("nullable").eq_ignore_ascii_case("YES"),
                        default: r.get("default_expr"),
                        ordinal: r.get::<i64, _>("ordinal") as i32,
                    }
                })
                .collect(),
            primary_key,
            foreign_keys,
            indexes,
        })
    }

    /// MySQL exposes the original DDL through `SHOW CREATE TABLE`, which is far
    /// more faithful than a rebuild — `AUTO_INCREMENT`, charsets, collations and
    /// `ON UPDATE` clauses all survive.
    async fn table_ddl(&self, table: &TableRef) -> Result<Option<String>> {
        let quoted = self.dialect.quote_ident(&table.name);
        let row = sqlx::query(AssertSqlSafe(format!("SHOW CREATE TABLE {quoted}")))
            .fetch_optional(&self.meta)
            .await?;

        Ok(row.and_then(|r| {
            // The second column holds the statement; its name differs between
            // tables ("Create Table") and views ("Create View").
            r.try_get::<String, _>(1).ok().map(|sql| {
                let trimmed = sql.trim().trim_end_matches(';').to_string();
                format!("{trimmed};")
            })
        }))
    }

    async fn schema_snapshot(&self, _schema: Option<&str>) -> Result<Vec<TableColumns>> {
        // One catalog query for every table's columns, rather than four per
        // table through the default walk.
        let rows = sqlx::query(
            r#"
            SELECT table_name AS name, column_name AS column_name
            FROM information_schema.columns
            WHERE table_schema = ?
            ORDER BY table_name, ordinal_position
            "#,
        )
        .bind(&self.database)
        .fetch_all(&self.meta)
        .await?;

        let mut out: Vec<TableColumns> = Vec::new();
        for r in &rows {
            let table: String = r.get("name");
            let column: String = r.get("column_name");
            match out.last_mut().filter(|t| t.name == table) {
                Some(entry) => entry.columns.push(column),
                None => out.push(TableColumns {
                    schema: None,
                    name: table,
                    columns: vec![column],
                }),
            }
        }
        Ok(out)
    }

    /// Run a statement and cap the rows as they arrive.
    ///
    /// Passing the SQL through untouched matters especially here: MySQL refuses a
    /// derived table whose columns are not uniquely named, so the old wrapping
    /// broke `SELECT * FROM a JOIN b` whenever both tables had an `id`. See
    /// `driver::fetch`.
    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        super::fetch::fetch_capped_mysql(&self.pool, sql, limit, cancel).await
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
        super::tx::apply_guarded_mysql(&self.pool, statements).await
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        self.pool.close().await;
        self.meta.close().await;
    }
}

/// Decode one cell.
///
/// MySQL reports types by name (`INT`, `VARCHAR`, `DATETIME`). Unsigned
/// integers are handled separately because a `BIGINT UNSIGNED` near its maximum
/// does not fit an `i64`.
pub(super) fn decode_value(row: &MySqlRow, idx: usize) -> Value {
    let raw = match row.try_get_raw(idx) {
        Ok(r) => r,
        Err(_) => return Value::Null,
    };
    if raw.is_null() {
        return Value::Null;
    }

    let type_name = raw.type_info().name().to_uppercase();

    match type_name.as_str() {
        "BOOLEAN" | "BOOL" => row.try_get::<bool, _>(idx).map(Value::Bool),
        // MySQL has no real boolean; TINYINT(1) is the convention, but a plain
        // TINYINT holding 0-255 is a number, so it stays an integer here.
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => {
            row.try_get::<i64, _>(idx).map(Value::Int)
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED" => {
            row.try_get::<u32, _>(idx).map(|v| Value::Int(v as i64))
        }
        "BIGINT UNSIGNED" => row
            .try_get::<u64, _>(idx)
            // Past i64::MAX it is kept as text rather than silently wrapping
            // into a negative number.
            .map(|v| match i64::try_from(v) {
                Ok(n) => Value::Int(n),
                Err(_) => Value::Decimal(v.to_string()),
            }),
        "FLOAT" => row.try_get::<f32, _>(idx).map(|v| Value::Float(v as f64)),
        "DOUBLE" => row.try_get::<f64, _>(idx).map(Value::Float),
        // Read as text; see the matching comment in the Postgres driver.
        // `rust_decimal` tops out around 28 significant digits, while MySQL
        // DECIMAL allows 65, so decoding through it silently rounds.
        "DECIMAL" | "NEWDECIMAL" => {
            row.try_get::<String, _>(idx)
                .map(Value::Decimal)
                .or_else(|_| {
                    row.try_get::<rust_decimal::Decimal, _>(idx)
                        .map(|v| Value::Decimal(v.to_string()))
                })
        }
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Date(v.to_string())),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Time(v.to_string())),
        "DATETIME" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Timestamp(v.to_string())),
        "TIMESTAMP" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(idx)
            .map(|v| Value::Timestamp(v.to_rfc3339())),
        "JSON" => row.try_get::<serde_json::Value, _>(idx).map(Value::Json),
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" => {
            row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes)
        }
        // TEXT, VARCHAR, CHAR, ENUM, SET and anything unrecognized. MySQL
        // renders most types as text, so this is a safe landing place.
        _ => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .or_else(|_| row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes)),
    }
    .unwrap_or(Value::Unsupported(type_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_use_backticks() {
        assert_eq!(MySqlDialect.quote_ident("my table"), "`my table`");
    }

    #[test]
    fn an_embedded_backtick_is_doubled() {
        // The classic injection seam for backtick quoting.
        assert_eq!(MySqlDialect.quote_ident("we`ird"), "`we``ird`");
    }

    #[test]
    fn mysql_is_presented_as_schema_less() {
        // A connection's database *is* its schema, so qualifying would let
        // generated SQL reach across databases.
        assert!(!MySqlDialect.supports_schemas());
        assert_eq!(MySqlDialect.qualify(Some("app"), "users"), "`users`");
    }

    #[test]
    fn blob_literals_use_the_x_prefix() {
        assert_eq!(MySqlDialect.quote_bytes(&[0xff, 0x00]), "X'ff00'");
    }

    #[test]
    fn pagination_wraps_the_original_query() {
        let out = MySqlDialect.paginate("SELECT * FROM t LIMIT 5", 10, 0);
        assert_eq!(
            out,
            "SELECT * FROM (SELECT * FROM t LIMIT 5) AS faro_q LIMIT 10"
        );
    }
}
