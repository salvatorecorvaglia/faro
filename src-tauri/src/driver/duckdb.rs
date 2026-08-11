use async_trait::async_trait;
use duckdb::types::{TimeUnit, ValueRef};
use duckdb::Connection;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    ColumnDetail, ColumnInfo, ConnectionConfig, ExecResult, ForeignKey, GuardedStatement,
    IndexInfo, ResultSet, SchemaInfo, TableColumns, TableDetail, TableInfo, TableKind, TableRef,
    Value,
};

pub struct DuckDbDialect;

impl Dialect for DuckDbDialect {
    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_double(ident)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        dialect::paginate_limit_offset(sql, limit, offset)
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        // DuckDB accepts SQLite-style blob literals.
        dialect::hex_bytes_x(bytes)
    }

    /// DuckDB has real schemas, but a file-backed database has one that matters
    /// (`main`). Faro presents it as schema-less so generated SQL stays
    /// unqualified, matching how it treats SQLite.
    fn supports_schemas(&self) -> bool {
        false
    }
}

/// DuckDB, wrapped to fit the async `Driver` trait.
///
/// The `duckdb` crate is synchronous, so every call is moved onto a blocking
/// thread. Without that, a long query would stall the whole Tokio runtime and
/// take the UI's IPC with it.
///
/// A single connection under a mutex, deliberately: DuckDB is embedded and
/// serializes writes anyway, and one connection keeps session state — temp
/// tables, `SET`, open transactions — coherent, which is the same reasoning as
/// the SQLite driver.
pub struct DuckDbDriver {
    conn: Arc<Mutex<Connection>>,
    dialect: DuckDbDialect,
}

impl DuckDbDriver {
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let path = config
            .file_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| FaroError::Connection("no database file selected".into()))?
            .to_string();

        // Refuse to create a new file by accident: opening a mistyped path
        // should be an error, not a silently empty database.
        if path != ":memory:" && !std::path::Path::new(&path).exists() {
            return Err(FaroError::Connection(format!("no such file: {path}")));
        }

        let read_only = config.read_only;
        let conn = tokio::task::spawn_blocking(move || {
            if path == ":memory:" {
                // An in-memory database starts empty, so opening it read-only
                // would make it permanently useless rather than protected.
                Connection::open_in_memory()
            } else if read_only {
                // Enforced by DuckDB itself, so a write Faro's own guard missed
                // still cannot land.
                let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
                Connection::open_with_flags(&path, config)
            } else {
                Connection::open(&path)
            }
        })
        .await
        .map_err(|e| FaroError::Connection(e.to_string()))?
        .map_err(|e| FaroError::Connection(e.to_string()))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            dialect: DuckDbDialect,
        })
    }

    /// Run a closure against the connection on a blocking thread.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().map_err(|_| {
                FaroError::Other("the DuckDB connection is no longer usable".into())
            })?;
            f(&guard)
        })
        .await
        .map_err(|e| FaroError::Other(format!("DuckDB task failed: {e}")))?
    }

    /// Fetch a result set, capping at `limit` with one extra row to detect
    /// truncation.
    fn fetch(conn: &Connection, sql: &str, limit: u64) -> Result<ResultSet> {
        let started = Instant::now();
        let mut stmt = conn.prepare(sql).map_err(map_err)?;

        let mut rows = stmt.query([]).map_err(map_err)?;

        // Column names are only available once the statement has run.
        let mut columns: Vec<ColumnInfo> = Vec::new();
        let mut data: Vec<Vec<Value>> = Vec::new();
        let mut seen = 0u64;

        while let Some(row) = rows.next().map_err(map_err)? {
            if columns.is_empty() {
                // Types come from the first row's values: DuckDB reports them
                // per value, not as statement metadata.
                columns = row
                    .as_ref()
                    .column_names()
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| ColumnInfo {
                        name,
                        type_name: row
                            .get_ref(i)
                            .map(type_name_of)
                            .unwrap_or_else(|_| "UNKNOWN".into()),
                    })
                    .collect();
            }

            seen += 1;
            if seen > limit {
                // The extra row proves truncation; it is not returned.
                break;
            }

            let width = columns.len();
            data.push(
                (0..width)
                    .map(|i| row.get_ref(i).map(decode_value).unwrap_or(Value::Null))
                    .collect(),
            );
        }

        // A statement returning no rows still has columns worth reporting.
        if columns.is_empty() {
            columns = stmt
                .column_names()
                .into_iter()
                .map(|name| ColumnInfo {
                    name,
                    type_name: String::new(),
                })
                .collect();
        }

        Ok(ResultSet {
            columns,
            rows: data,
            truncated: seen > limit,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// Roll the transaction back, preserving `cause` as the error the user sees.
///
/// A failing rollback is reported alongside the original cause rather than
/// discarded with `let _ =`: it means the connection still has an open
/// transaction, and since DuckDB runs everything through one mutex-guarded
/// connection, every later statement inherits it. The SQL Server driver
/// already did this; the two implementations had drifted apart.
fn rollback(c: &duckdb::Connection, cause: FaroError) -> FaroError {
    match c.execute_batch("ROLLBACK") {
        Ok(()) => cause,
        Err(e) => FaroError::Other(format!(
            "{cause}\n\nThe rollback then failed too ({e}). \
             This connection may still hold an open transaction — \
             disconnect and reconnect before making further changes."
        )),
    }
}

fn map_err(e: duckdb::Error) -> FaroError {
    FaroError::Database {
        message: e.to_string(),
        code: None,
    }
}

#[async_trait]
impl Driver for DuckDbDriver {
    async fn ping(&self) -> Result<()> {
        self.with_conn(|c| {
            c.execute_batch("SELECT 1").map_err(map_err)?;
            Ok(())
        })
        .await
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        // One namespace, matching how the dialect qualifies names.
        Ok(vec![SchemaInfo {
            name: "main".into(),
            is_system: false,
        }])
    }

    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<TableInfo>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT table_name, table_type
                     FROM information_schema.tables
                     WHERE table_schema = 'main'
                     ORDER BY table_name",
                )
                .map_err(map_err)?;

            let mut out = Vec::new();
            let mut rows = stmt.query([]).map_err(map_err)?;
            while let Some(row) = rows.next().map_err(map_err)? {
                let name: String = row.get(0).map_err(map_err)?;
                let kind: String = row.get(1).map_err(map_err)?;
                out.push(TableInfo {
                    schema: None,
                    name,
                    kind: if kind.eq_ignore_ascii_case("VIEW") {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                    // DuckDB keeps no cheap row estimate in the catalog.
                    estimated_rows: None,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let name = table.name.clone();
        let table_ref = table.clone();

        self.with_conn(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT column_name, data_type, is_nullable, column_default, ordinal_position
                     FROM information_schema.columns
                     WHERE table_schema = 'main' AND table_name = ?
                     ORDER BY ordinal_position",
                )
                .map_err(map_err)?;

            let mut columns = Vec::new();
            let mut rows = stmt.query([&name]).map_err(map_err)?;
            while let Some(row) = rows.next().map_err(map_err)? {
                columns.push(ColumnDetail {
                    name: row.get::<_, String>(0).map_err(map_err)?,
                    type_name: row.get::<_, String>(1).map_err(map_err)?,
                    nullable: row
                        .get::<_, String>(2)
                        .map(|s| s.eq_ignore_ascii_case("YES"))
                        .unwrap_or(true),
                    default: row.get::<_, Option<String>>(3).map_err(map_err)?,
                    is_primary_key: false,
                    ordinal: row.get::<_, i64>(4).map_err(map_err)? as i32,
                });
            }

            if columns.is_empty() {
                return Err(FaroError::Database {
                    message: format!("table \"{name}\" not found"),
                    code: None,
                });
            }

            // duckdb_constraints is the only place DuckDB exposes keys. The
            // column list arrives as a VARCHAR[] rendered like [a, b].
            let mut pk_stmt = c
                .prepare(
                    "SELECT constraint_type, constraint_column_names::VARCHAR
                     FROM duckdb_constraints()
                     WHERE table_name = ? AND schema_name = 'main'",
                )
                .map_err(map_err)?;

            let mut primary_key: Vec<String> = Vec::new();
            let mut unique_sets: Vec<Vec<String>> = Vec::new();
            let mut rows = pk_stmt.query([&name]).map_err(map_err)?;
            while let Some(row) = rows.next().map_err(map_err)? {
                let kind: String = row.get(0).map_err(map_err)?;
                let cols = parse_column_list(&row.get::<_, String>(1).map_err(map_err)?);
                match kind.to_ascii_uppercase().as_str() {
                    "PRIMARY KEY" => primary_key = cols,
                    "UNIQUE" => unique_sets.push(cols),
                    _ => {}
                }
            }

            for column in &mut columns {
                column.is_primary_key = primary_key.contains(&column.name);
            }

            let indexes: Vec<IndexInfo> = unique_sets
                .into_iter()
                .enumerate()
                .map(|(i, cols)| IndexInfo {
                    name: format!("{name}_unique_{i}"),
                    columns: cols,
                    is_unique: true,
                    is_primary: false,
                    // Backed by a UNIQUE constraint in the table definition, so
                    // backup must not emit a CREATE INDEX for it.
                    is_constraint: true,
                })
                .collect();

            Ok(TableDetail {
                table: table_ref,
                kind: TableKind::Table,
                columns,
                primary_key,
                // DuckDB's catalog does not expose foreign key column mappings
                // in a form Faro can read back reliably, so none are reported
                // rather than reporting them wrongly.
                foreign_keys: Vec::<ForeignKey>::new(),
                indexes,
            })
        })
        .await
    }

    async fn schema_snapshot(&self, _schema: Option<&str>) -> Result<Vec<TableColumns>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT table_name, column_name
                     FROM information_schema.columns
                     WHERE table_schema = 'main'
                     ORDER BY table_name, ordinal_position",
                )
                .map_err(map_err)?;

            let mut out: Vec<TableColumns> = Vec::new();
            let mut rows = stmt.query([]).map_err(map_err)?;
            while let Some(row) = rows.next().map_err(map_err)? {
                let table: String = row.get(0).map_err(map_err)?;
                let column: String = row.get(1).map_err(map_err)?;
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
        })
        .await
    }

    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        // `fetch` already stops one row past the limit, so the wrapping is only
        // there to keep the surplus rows on the engine's side. Statements that
        // cannot be a derived table — `PRAGMA`, `DESCRIBE`, `SHOW`, `EXPLAIN` —
        // are run as written; the row cap still applies as they are decoded.
        let sql = if super::is_wrappable(sql) {
            self.dialect
                .paginate(sql, crate::driver::probe_limit(limit), 0)
        } else {
            sql.to_string()
        };
        self.with_conn(move |c| Self::fetch(c, &sql, limit)).await
    }

    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        let owned = sql.to_string();
        self.with_conn(move |c| {
            let started = Instant::now();
            // `execute` reports affected rows; a script it rejects as
            // multi-statement falls back to execute_batch, which reports none.
            //
            // Only `MultipleStatement` retries. The blanket `Err(_)` here also
            // caught `ExecuteReturnedResults`, which DuckDB raises *after* the
            // statement has run — so an `INSERT … RETURNING` was executed, then
            // executed a second time by `execute_batch`, inserting twice. Any
            // other error is the user's to see, not something to re-run.
            let affected = match c.execute(&owned, []) {
                Ok(n) => n as u64,
                Err(duckdb::Error::MultipleStatement) => {
                    c.execute_batch(&owned).map_err(map_err)?;
                    0
                }
                // The statement already ran and produced rows; `run` routes
                // row-returning SQL to `query`, so reaching here means the
                // caller wanted only a count. Reporting zero is honest, and
                // re-running it would not be.
                Err(duckdb::Error::ExecuteReturnedResults) => 0,
                Err(e) => return Err(map_err(e)),
            };
            Ok(ExecResult {
                rows_affected: affected,
                elapsed_ms: started.elapsed().as_millis() as u64,
            })
        })
        .await
    }

    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        let owned: Vec<GuardedStatement> = statements.to_vec();

        self.with_conn(move |c| {
            if owned.is_empty() {
                return Ok(0);
            }

            c.execute_batch("BEGIN TRANSACTION").map_err(map_err)?;
            let mut total = 0u64;

            for (index, stmt) in owned.iter().enumerate() {
                let affected = match c.execute(&stmt.sql, []) {
                    Ok(n) => n as u64,
                    Err(e) => return Err(rollback(c, map_err(e))),
                };

                if let Some(expected) = stmt.expect {
                    if affected != expected {
                        // Same safety net as the sqlx drivers: a statement that
                        // touched the wrong number of rows aborts the batch
                        // rather than half-applying it.
                        let guard = super::tx::guard_error(index, owned.len(), expected, affected);
                        return Err(rollback(c, guard));
                    }
                }
                total += affected;
            }

            // A failing COMMIT leaves the transaction open, so it gets the same
            // treatment as any other failure rather than returning bare.
            if let Err(e) = c.execute_batch("COMMIT") {
                return Err(rollback(c, map_err(e)));
            }
            Ok(total)
        })
        .await
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // The connection closes when the Arc drops; DuckDB has no explicit
        // close that can be called through a shared reference.
    }
}

/// Render the column list from `duckdb_constraints()`, which arrives as
/// `[a, b]`, into individual names.
fn parse_column_list(raw: &str) -> Vec<String> {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// DuckDB's own name for a value's type, shown in the grid header.
fn type_name_of(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "NULL",
        ValueRef::Boolean(_) => "BOOLEAN",
        ValueRef::TinyInt(_) => "TINYINT",
        ValueRef::SmallInt(_) => "SMALLINT",
        ValueRef::Int(_) => "INTEGER",
        ValueRef::BigInt(_) => "BIGINT",
        ValueRef::HugeInt(_) => "HUGEINT",
        ValueRef::UTinyInt(_) => "UTINYINT",
        ValueRef::USmallInt(_) => "USMALLINT",
        ValueRef::UInt(_) => "UINTEGER",
        ValueRef::UBigInt(_) => "UBIGINT",
        ValueRef::Float(_) => "FLOAT",
        ValueRef::Double(_) => "DOUBLE",
        ValueRef::Decimal(_) => "DECIMAL",
        ValueRef::Timestamp(..) => "TIMESTAMP",
        ValueRef::Text(_) => "VARCHAR",
        ValueRef::Blob(_) => "BLOB",
        ValueRef::Date32(_) => "DATE",
        ValueRef::Time64(..) => "TIME",
        ValueRef::Interval { .. } => "INTERVAL",
        ValueRef::List(..) => "LIST",
        ValueRef::Enum(..) => "ENUM",
        ValueRef::Struct(..) => "STRUCT",
        ValueRef::Array(..) => "ARRAY",
        ValueRef::Map(..) => "MAP",
        ValueRef::Union(..) => "UNION",
        _ => "UNKNOWN",
    }
    .to_string()
}

/// Decode one cell into the engine-agnostic `Value`.
fn decode_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Boolean(b) => Value::Bool(b),
        ValueRef::TinyInt(v) => Value::Int(v as i64),
        ValueRef::SmallInt(v) => Value::Int(v as i64),
        ValueRef::Int(v) => Value::Int(v as i64),
        ValueRef::BigInt(v) => Value::Int(v),
        ValueRef::UTinyInt(v) => Value::Int(v as i64),
        ValueRef::USmallInt(v) => Value::Int(v as i64),
        ValueRef::UInt(v) => Value::Int(v as i64),
        // These can exceed i64. Kept as exact decimal text rather than wrapped
        // into a negative number or rounded through a float.
        ValueRef::UBigInt(v) => match i64::try_from(v) {
            Ok(n) => Value::Int(n),
            Err(_) => Value::Decimal(v.to_string()),
        },
        ValueRef::HugeInt(v) => match i64::try_from(v) {
            Ok(n) => Value::Int(n),
            Err(_) => Value::Decimal(v.to_string()),
        },
        ValueRef::Float(v) => Value::Float(v as f64),
        ValueRef::Double(v) => Value::Float(v),
        ValueRef::Decimal(d) => Value::Decimal(d.to_string()),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => Value::Text(s.to_string()),
            // Not valid UTF-8, so it is bytes rather than text.
            Err(_) => Value::Bytes(bytes.to_vec()),
        },
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
        ValueRef::Date32(days) => Value::Date(
            chrono::DateTime::from_timestamp(days as i64 * 86_400, 0)
                .map(|d| d.date_naive().to_string())
                .unwrap_or_else(|| days.to_string()),
        ),
        ValueRef::Timestamp(unit, v) => Value::Timestamp(format_timestamp(unit, v)),
        ValueRef::Time64(unit, v) => Value::Time(format_time(unit, v)),
        // Composite and interval types have no faithful scalar form; their type
        // name is reported so the user sees something truthful.
        other => Value::Unsupported(type_name_of(other)),
    }
}

/// Convert a DuckDB timestamp to ISO-8601.
fn format_timestamp(unit: TimeUnit, value: i64) -> String {
    let (secs, nanos) = split_unit(unit, value);
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|d| d.naive_utc().to_string())
        .unwrap_or_else(|| value.to_string())
}

fn format_time(unit: TimeUnit, value: i64) -> String {
    let (secs, nanos) = split_unit(unit, value);
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|d| d.time().to_string())
        .unwrap_or_else(|| value.to_string())
}

/// Split a unit-scaled instant into whole seconds and leftover nanoseconds.
fn split_unit(unit: TimeUnit, value: i64) -> (i64, u32) {
    let per_second: i64 = match unit {
        TimeUnit::Second => 1,
        TimeUnit::Millisecond => 1_000,
        TimeUnit::Microsecond => 1_000_000,
        TimeUnit::Nanosecond => 1_000_000_000,
    };
    let secs = value.div_euclid(per_second);
    let rem = value.rem_euclid(per_second);
    let nanos = (rem * (1_000_000_000 / per_second)) as u32;
    (secs, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_use_double_quotes_and_escape_them() {
        assert_eq!(DuckDbDialect.quote_ident("my table"), r#""my table""#);
        assert_eq!(DuckDbDialect.quote_ident(r#"we"ird"#), r#""we""ird""#);
    }

    #[test]
    fn duckdb_is_presented_as_schema_less() {
        assert!(!DuckDbDialect.supports_schemas());
        assert_eq!(DuckDbDialect.qualify(Some("main"), "t"), r#""t""#);
    }

    #[test]
    fn parses_the_constraint_column_list() {
        assert_eq!(parse_column_list("[id]"), vec!["id"]);
        assert_eq!(
            parse_column_list("[book_id, store_id]"),
            vec!["book_id", "store_id"]
        );
        assert_eq!(parse_column_list("[]"), Vec::<String>::new());
        // Quoted forms appear on some builds.
        assert_eq!(parse_column_list("['a', 'b']"), vec!["a", "b"]);
    }

    #[test]
    fn time_units_split_into_seconds_and_nanoseconds() {
        assert_eq!(split_unit(TimeUnit::Second, 5), (5, 0));
        assert_eq!(split_unit(TimeUnit::Millisecond, 1_500), (1, 500_000_000));
        assert_eq!(split_unit(TimeUnit::Microsecond, 2_000_001), (2, 1_000));
        assert_eq!(split_unit(TimeUnit::Nanosecond, 1_000_000_001), (1, 1));
    }

    #[test]
    fn negative_instants_do_not_produce_a_negative_nanosecond_part() {
        // Euclidean division keeps the remainder non-negative, which is what
        // chrono requires; a plain `%` would yield an invalid timestamp.
        let (secs, nanos) = split_unit(TimeUnit::Millisecond, -1_500);
        assert_eq!(secs, -2);
        assert_eq!(nanos, 500_000_000);
    }
}
