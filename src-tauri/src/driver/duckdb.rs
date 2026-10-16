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

    /// Nested columns are dumped in DuckDB's own text form.
    ///
    /// Read normally they become JSON, and JSON text does not cast back to
    /// every nested type — `'{"k":7}'` is not a MAP to DuckDB, so a restore of
    /// a MAP column failed. DuckDB's rendering of its own values (`{k=7}`)
    /// always casts back to the declared type, so the backup reads that.
    fn dump_column(&self, column: &crate::model::ColumnDetail) -> String {
        let name = self.quote_ident(&column.name);
        if is_nested_type(&column.type_name) {
            format!("CAST({name} AS VARCHAR) AS {name}")
        } else {
            name
        }
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
    /// Interrupts whatever statement the connection is running.
    ///
    /// Held outside the mutex on purpose: the running statement holds the
    /// lock, so anything that had to take it to stop the statement would wait
    /// for the statement to finish first.
    interrupt: Arc<duckdb::InterruptHandle>,
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
            interrupt: conn.interrupt_handle(),
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

    /// [`with_conn`](Self::with_conn), stopping the statement when `cancel`
    /// fires.
    ///
    /// Cancel used to do nothing here: the statement ran on a blocking thread
    /// that cannot be dropped like a future, so pressing Stop changed nothing
    /// until the query finished on its own — and it held the connection's only
    /// lock all the while. DuckDB can interrupt a running statement from
    /// another thread, which is what this does.
    async fn with_conn_cancellable<T, F>(&self, cancel: CancellationToken, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        let token = cancel.clone();
        let work = self.with_conn(move |c| {
            // Checked again once the lock is held. A cancel that arrived while
            // this waited behind an earlier statement had nothing running to
            // interrupt yet, and would otherwise be ignored.
            if token.is_cancelled() {
                return Err(FaroError::Cancelled);
            }
            f(c)
        });
        tokio::pin!(work);
        tokio::select! {
            biased;
            res = &mut work => res,
            _ = cancel.cancelled() => {
                self.interrupt.interrupt();
                // Wait for the interrupted statement to return, so the lock is
                // released — and the connection usable — before reporting.
                let _ = work.await;
                Err(FaroError::Cancelled)
            }
        }
    }

    /// Fetch a result set, capping at `limit` with one extra row to detect
    /// truncation.
    fn fetch(conn: &Connection, sql: &str, limit: u64) -> Result<ResultSet> {
        let started = Instant::now();
        let mut stmt = conn.prepare(sql).map_err(map_err)?;

        let mut rows = stmt.query([]).map_err(map_err)?;

        // Column names are only available once the statement has run.
        let mut columns: Vec<ColumnInfo> = Vec::new();
        // Which columns are TIMESTAMPTZ. Their values arrive exactly like a
        // plain TIMESTAMP's — microseconds since the epoch — so only the
        // column's declared type tells them apart.
        let mut zoned: Vec<bool> = Vec::new();
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
                zoned = (0..columns.len())
                    .map(|i| {
                        matches!(
                            row.as_ref().column_type(i),
                            duckdb::arrow::datatypes::DataType::Timestamp(_, Some(_))
                        )
                    })
                    .collect();
                for (column, &tz) in columns.iter_mut().zip(&zoned) {
                    if tz {
                        column.type_name = "TIMESTAMPTZ".into();
                    }
                }
            }

            seen += 1;
            if seen > limit {
                // The extra row proves truncation; it is not returned.
                break;
            }

            let width = columns.len();
            data.push(
                (0..width)
                    .map(|i| match row.get_ref(i) {
                        // An instant, so it says so: RFC 3339 with its UTC
                        // offset. Shown as a bare `2024-01-01 08:00:00` it lost
                        // the fact that it was UTC — and a dump of it restored
                        // as that wall time in whatever zone the session used,
                        // shifting every value by the difference.
                        Ok(ValueRef::Timestamp(unit, v)) if zoned[i] => {
                            Value::Timestamp(format_timestamp_utc(unit, v))
                        }
                        Ok(v) => decode_value(v),
                        Err(_) => Value::Null,
                    })
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

/// Whether a DuckDB type name is a nested type: a list or array (`INTEGER[]`,
/// `INTEGER[2]`), a STRUCT, a MAP or a UNION.
fn is_nested_type(type_name: &str) -> bool {
    let t = type_name.trim().to_ascii_uppercase();
    t.ends_with(']')
        || ["STRUCT(", "MAP(", "UNION("]
            .iter()
            .any(|p| t.starts_with(p))
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

            let mut pairs = Vec::new();
            let mut rows = stmt.query([]).map_err(map_err)?;
            while let Some(row) = rows.next().map_err(map_err)? {
                pairs.push((
                    row.get::<_, String>(0).map_err(map_err)?,
                    row.get::<_, String>(1).map_err(map_err)?,
                ));
            }
            Ok(super::catalog::group_columns(pairs, None))
        })
        .await
    }

    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
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
        self.with_conn_cancellable(cancel, move |c| Self::fetch(c, &sql, limit))
            .await
    }

    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        let owned = sql.to_string();
        self.with_conn_cancellable(cancel, move |c| {
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
                        let guard =
                            super::tx::guard_error(index, Some(owned.len()), expected, affected);
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

    /// Apply an import's statements as they arrive, in one transaction.
    ///
    /// The default buffered every statement before running any, which for a
    /// large file meant holding all of it in memory as SQL text — the thing the
    /// streaming import exists to avoid.
    ///
    /// The whole import runs in one blocking task holding the connection's
    /// mutex, receiving batches with `blocking_recv`. Taking the lock per batch
    /// instead would let any other DuckDB call — a browse, a schema refresh —
    /// slip in between batches and run *inside* the import's open transaction.
    async fn apply_streaming(
        &self,
        mut chunks: tokio::sync::mpsc::Receiver<crate::model::ApplyChunk>,
    ) -> Result<u64> {
        use crate::model::ApplyChunk;

        self.with_conn(move |c| {
            c.execute_batch("BEGIN TRANSACTION").map_err(map_err)?;
            let mut total = 0u64;
            let mut index = 0usize;
            let mut done = false;

            while let Some(chunk) = chunks.blocking_recv() {
                let batch = match chunk {
                    ApplyChunk::Batch(batch) => batch,
                    ApplyChunk::Done => {
                        done = true;
                        break;
                    }
                };
                for stmt in &batch {
                    let affected = match c.execute(&stmt.sql, []) {
                        Ok(n) => n as u64,
                        Err(e) => return Err(rollback(c, map_err(e))),
                    };
                    if let Some(expected) = stmt.expect {
                        if affected != expected {
                            let guard = super::tx::guard_error(index, None, expected, affected);
                            return Err(rollback(c, guard));
                        }
                    }
                    total += affected;
                    index += 1;
                }
            }

            // Only an explicit `Done` earns a commit; a sender dropped without
            // one means the reader failed partway through the file.
            if !done {
                return Err(rollback(
                    c,
                    FaroError::Other(
                        "the source could not be read to the end, so nothing was applied".into(),
                    ),
                ));
            }
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
        ValueRef::UHugeInt(_) => "UHUGEINT",
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
        // Named so a refusal says what the column is rather than "UNKNOWN".
        ValueRef::Geometry(..) => "GEOMETRY",
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
        // Fell through to `Unsupported("UNKNOWN")` while its signed sibling was
        // decoded — a UHUGEINT column showed no values at all.
        ValueRef::UHugeInt(v) => match i64::try_from(v) {
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
        ValueRef::Date32(days) => Value::Date(format_date(days)),
        ValueRef::Timestamp(unit, v) => Value::Timestamp(format_timestamp(unit, v)),
        ValueRef::Time64(unit, v) => Value::Time(format_time(unit, v)),
        // In DuckDB's own spelling, which it reads back as an INTERVAL — so a
        // dump of the column restores to the same values.
        ValueRef::Interval {
            months,
            days,
            nanos,
        } => Value::Text(format_interval(months, days, nanos)),
        // Nested values. These used to come back as `Unsupported` — the column
        // showed only its type name, and could be neither exported nor backed
        // up. An enum is its label; the rest keep their structure as JSON,
        // which the grid shows collapsed and the inspector in full.
        composite @ (ValueRef::Enum(..)
        | ValueRef::List(..)
        | ValueRef::Array(..)
        | ValueRef::Struct(..)
        | ValueRef::Map(..)
        | ValueRef::Union(..)) => match composite.to_owned() {
            duckdb::types::Value::Enum(label) => Value::Text(label),
            owned => Value::Json(duck_json(&owned)),
        },
        other => Value::Unsupported(type_name_of(other)),
    }
}

/// A day count since the epoch, as an ISO-8601 date.
fn format_date(days: i32) -> String {
    chrono::DateTime::from_timestamp(days as i64 * 86_400, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_else(|| days.to_string())
}

/// An interval as DuckDB itself prints it — `1 year 2 months 3 days
/// 04:05:06.5` — which is also a form DuckDB accepts back as an INTERVAL.
fn format_interval(months: i32, days: i32, nanos: i64) -> String {
    let unit = |n: i64, one: &str| format!("{n} {one}{}", if n.abs() == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    let (years, months) = (months / 12, months % 12);
    if years != 0 {
        parts.push(unit(years.into(), "year"));
    }
    if months != 0 {
        parts.push(unit(months.into(), "month"));
    }
    if days != 0 {
        parts.push(unit(days.into(), "day"));
    }
    if nanos != 0 || parts.is_empty() {
        let sign = if nanos < 0 { "-" } else { "" };
        let total = nanos.unsigned_abs();
        let micros = (total / 1_000) % 1_000_000;
        let seconds = total / 1_000_000_000;
        let mut time = format!(
            "{sign}{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        );
        if micros != 0 {
            time.push_str(format!(".{micros:06}").trim_end_matches('0'));
        }
        parts.push(time);
    }
    parts.join(" ")
}

/// A nested DuckDB value as JSON, recursively.
///
/// Numbers too wide for JSON — HUGEINT, a decimal — become strings so no digit
/// is lost, and temporal values use the same text as their scalar columns. Map
/// keys become object keys; a non-text key is written as its JSON text.
fn duck_json(value: &duckdb::types::Value) -> serde_json::Value {
    use duckdb::types::Value as D;
    use serde_json::Value as J;
    let float = |f: f64| serde_json::Number::from_f64(f).map_or(J::Null, J::Number);
    match value {
        D::Null => J::Null,
        D::Boolean(b) => J::Bool(*b),
        D::TinyInt(i) => (*i).into(),
        D::SmallInt(i) => (*i).into(),
        D::Int(i) => (*i).into(),
        D::BigInt(i) => (*i).into(),
        D::UTinyInt(i) => (*i).into(),
        D::USmallInt(i) => (*i).into(),
        D::UInt(i) => (*i).into(),
        D::UBigInt(i) => (*i).into(),
        D::HugeInt(i) => J::String(i.to_string()),
        D::UHugeInt(i) => J::String(i.to_string()),
        D::Float(f) => float(f64::from(*f)),
        D::Double(f) => float(*f),
        D::Decimal(d) => J::String(d.to_string()),
        D::Timestamp(unit, v) => J::String(format_timestamp(*unit, *v)),
        D::Text(s) | D::Enum(s) => J::String(s.clone()),
        D::Blob(b) => J::String(b.iter().map(|x| format!("{x:02x}")).collect()),
        D::Date32(days) => J::String(format_date(*days)),
        D::Time64(unit, v) => J::String(format_time(*unit, *v)),
        D::Interval {
            months,
            days,
            nanos,
        } => J::String(format_interval(*months, *days, *nanos)),
        D::List(items) | D::Array(items) => J::Array(items.iter().map(duck_json).collect()),
        D::Struct(fields) => J::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), duck_json(v)))
                .collect(),
        ),
        D::Map(entries) => J::Object(
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        D::Text(s) => s.clone(),
                        other => duck_json(other).to_string(),
                    };
                    (key, duck_json(v))
                })
                .collect(),
        ),
        D::Union(inner) => duck_json(inner),
        other => J::String(format!("{other:?}")),
    }
}

/// Convert a DuckDB timestamp to ISO-8601.
fn format_timestamp(unit: TimeUnit, value: i64) -> String {
    let (secs, nanos) = split_unit(unit, value);
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|d| d.naive_utc().to_string())
        .unwrap_or_else(|| value.to_string())
}

/// A TIMESTAMPTZ instant as RFC 3339 in UTC, offset included.
fn format_timestamp_utc(unit: TimeUnit, value: i64) -> String {
    let (secs, nanos) = split_unit(unit, value);
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|d| d.to_rfc3339())
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
    fn nested_types_are_recognised_by_their_declared_name() {
        for t in [
            "VARCHAR[]",
            "INTEGER[2]",
            "STRUCT(x INTEGER, y INTEGER)",
            "MAP(VARCHAR, INTEGER)",
            "UNION(t VARCHAR)",
        ] {
            assert!(is_nested_type(t), "{t}");
        }
        for t in ["VARCHAR", "INTEGER", "INTERVAL", "DECIMAL(10,2)", "mood"] {
            assert!(!is_nested_type(t), "{t}");
        }
    }

    #[test]
    fn intervals_print_as_duckdb_prints_them() {
        assert_eq!(
            format_interval(14, 3, 14_706_500_000_000),
            "1 year 2 months 3 days 04:05:06.5"
        );
        assert_eq!(format_interval(1, 1, 0), "1 month 1 day");
        assert_eq!(format_interval(0, 0, 0), "00:00:00");
        assert_eq!(format_interval(0, 0, -1_000_000_000), "-00:00:01");
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
