use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    ApplyChunk, ColumnDetail, ColumnInfo, ConnectionConfig, ExecResult, GuardedStatement,
    IndexInfo, ResultSet, SchemaInfo, TableDetail, TableInfo, TableKind, TableRef, Value,
};
use async_trait::async_trait;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn db_error(error: rusqlite::Error) -> FaroError {
    let code = match &error {
        rusqlite::Error::SqliteFailure(code, _) => Some(code.extended_code.to_string()),
        _ => None,
    };
    FaroError::Database {
        message: error.to_string(),
        code,
    }
}

fn rollback_error(connection: &Connection, cause: FaroError) -> FaroError {
    if connection.is_autocommit() {
        return cause;
    }
    match connection.execute_batch("ROLLBACK") {
        Ok(()) => cause,
        Err(error) => FaroError::Other(format!("{cause}\n\nRollback failed ({error}). This session may still have an open transaction; disconnect and reconnect before retrying.")),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OperationPhase {
    Queued,
    Executing,
    Committing,
    Finished,
    RollingBack,
}
type PhaseState = Arc<Mutex<OperationPhase>>;

#[derive(Clone)]
struct Session {
    connection: Arc<Mutex<Option<Connection>>>,
    gate: Arc<tokio::sync::Semaphore>,
    interrupt: Arc<rusqlite::InterruptHandle>,
}
impl Session {
    fn new(connection: Connection) -> Self {
        let interrupt = Arc::new(connection.get_interrupt_handle());
        Self {
            connection: Arc::new(Mutex::new(Some(connection))),
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            interrupt,
        }
    }
    async fn run<T: Send + 'static>(
        &self,
        cancel: CancellationToken,
        f: impl FnOnce(&mut Connection, &CancellationToken, &PhaseState) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = tokio::select! { biased; _ = cancel.cancelled() => return Err(FaroError::Cancelled), permit = self.gate.clone().acquire_owned() => permit.map_err(|error| FaroError::Other(error.to_string()))? };
        let session = self.clone();
        let token = cancel.clone();
        let phase = Arc::new(Mutex::new(OperationPhase::Queued));
        let task_phase = phase.clone();
        let mut task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut guard = session
                .connection
                .lock()
                .map_err(|_| FaroError::Other("SQLite session is no longer usable".into()))?;
            let connection = guard
                .as_mut()
                .ok_or_else(|| FaroError::Connection("SQLite connection is closed".into()))?;
            if token.is_cancelled() {
                return Err(FaroError::Cancelled);
            }
            *task_phase.lock().unwrap_or_else(|error| error.into_inner()) =
                OperationPhase::Executing;
            let mut result = f(connection, &token, &task_phase);
            let mut state = task_phase.lock().unwrap_or_else(|error| error.into_inner());
            if result.is_err() && token.is_cancelled() && *state == OperationPhase::Executing {
                result = Err(FaroError::Cancelled);
            }
            *state = OperationPhase::Finished;
            result
        });
        let joined = tokio::select! { result = &mut task => result, _ = cancel.cancelled() => {
            loop {
                { let state = phase.lock().unwrap_or_else(|error| error.into_inner()); if *state == OperationPhase::Executing {self.interrupt.interrupt();} }
                tokio::select! { result = &mut task => break result, _ = tokio::time::sleep(Duration::from_millis(10)) => {} }
            }
        }};
        joined.map_err(|error| FaroError::Other(format!("SQLite worker failed: {error}")))?
    }
    async fn query(&self, sql: String, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        self.run(cancel, move |connection, token, _| {
            let started = Instant::now();
            let mut statement = connection.prepare(&sql).map_err(db_error)?;
            let mut columns: Vec<ColumnInfo> = statement
                .columns()
                .map(|column| ColumnInfo {
                    name: column.name().into(),
                    type_name: column.decl_type().unwrap_or("").into(),
                })
                .collect();
            let mut budget = crate::budget::RowBudget::default();
            budget.columns(&columns)?;
            let mut cursor = statement.query([]).map_err(db_error)?;
            let mut rows = Vec::new();
            let mut truncated = false;
            while let Some(row) = cursor.next().map_err(db_error)? {
                if token.is_cancelled() {
                    return Err(FaroError::Cancelled);
                }
                if rows.is_empty() {
                    for (index, column) in columns.iter_mut().enumerate() {
                        if column.type_name.is_empty() {
                            column.type_name = match row.get_ref(index) {
                                Ok(ValueRef::Null) => "NULL",
                                Ok(ValueRef::Integer(_)) => "INTEGER",
                                Ok(ValueRef::Real(_)) => "REAL",
                                Ok(ValueRef::Text(_)) => "TEXT",
                                Ok(ValueRef::Blob(_)) => "BLOB",
                                Err(_) => "UNKNOWN",
                            }
                            .into();
                        }
                    }
                    budget.columns(&columns)?;
                }
                if rows.len() as u64 >= limit {
                    truncated = true;
                    break;
                }
                let values = (0..columns.len())
                    .map(|index| match row.get_ref(index) {
                        Ok(ValueRef::Null) => Value::Null,
                        Ok(ValueRef::Integer(value))
                            if columns[index].type_name.eq_ignore_ascii_case("BOOLEAN") =>
                        {
                            Value::Bool(value != 0)
                        }
                        Ok(ValueRef::Integer(value)) => Value::Int(value),
                        Ok(ValueRef::Real(value)) => Value::Float(value),
                        Ok(ValueRef::Text(value)) => match std::str::from_utf8(value) {
                            Ok(value) => Value::Text(value.to_owned()),
                            Err(_) => Value::Unsupported("TEXT (invalid UTF-8)".into()),
                        },
                        Ok(ValueRef::Blob(value)) => Value::Bytes(value.to_vec()),
                        Err(_) => Value::Null,
                    })
                    .collect::<Vec<_>>();
                budget.push(&values)?;
                rows.push(values);
            }
            Ok(ResultSet {
                columns,
                rows,
                truncated,
                elapsed_ms: started.elapsed().as_millis() as u64,
            })
        })
        .await
    }
}

pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn snapshot_eq(&self, column: &str, value: &Value, _type_name: &str) -> String {
        match value {
            Value::Text(text) => format!(
                "CAST({column} AS BLOB) = {}",
                self.quote_bytes(text.as_bytes())
            ),
            _ => format!("{column} = {}", self.literal(value)),
        }
    }

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
    pool: Session,
    meta: Session,
    managed: Session,
    dialect: SqliteDialect,
}
impl SqliteDriver {
    pub async fn connect(config: &ConnectionConfig) -> Result<Self> {
        let mut path = config
            .file_path
            .clone()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| FaroError::Connection("no database file selected".into()))?;
        if path != ":memory:" && !std::path::Path::new(&path).exists() {
            return Err(FaroError::Connection(format!("no such file: {path}")));
        }
        let memory = path == ":memory:";
        if memory {
            path = format!(
                "file:faro-{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4()
            );
        }
        let readonly = config.read_only;
        tokio::task::spawn_blocking(move || {
            let open = || -> Result<Session> {
                let flags = if readonly && !memory {
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                } else {
                    OpenFlags::SQLITE_OPEN_READ_WRITE
                };
                let flags = flags
                    | OpenFlags::SQLITE_OPEN_URI
                    | if memory {
                        OpenFlags::SQLITE_OPEN_CREATE
                    } else {
                        OpenFlags::empty()
                    };
                let connection = Connection::open_with_flags(&path, flags).map_err(db_error)?;
                connection
                    .busy_timeout(Duration::from_secs(5))
                    .map_err(db_error)?;
                connection
                    .pragma_update(None, "foreign_keys", true)
                    .map_err(db_error)?;
                if readonly {
                    connection
                        .pragma_update(None, "query_only", true)
                        .map_err(db_error)?;
                }
                Ok(Session::new(connection))
            };
            Ok(Self {
                pool: open()?,
                meta: open()?,
                managed: open()?,
                dialect: SqliteDialect,
            })
        })
        .await
        .map_err(|error| FaroError::Connection(error.to_string()))?
    }
    async fn metadata(&self, sql: String) -> Result<Vec<Vec<Value>>> {
        Ok(self
            .meta
            .query(sql, u64::MAX, CancellationToken::new())
            .await?
            .rows)
    }
}
fn text(row: &[Value], index: usize) -> String {
    match row.get(index) {
        Some(Value::Text(value)) => value.clone(),
        _ => String::new(),
    }
}
fn integer(row: &[Value], index: usize) -> i64 {
    match row.get(index) {
        Some(Value::Int(value)) => *value,
        _ => 0,
    }
}
fn optional_text(row: &[Value], index: usize) -> Option<String> {
    match row.get(index) {
        Some(Value::Text(value)) => Some(value.clone()),
        _ => None,
    }
}
#[async_trait]
impl Driver for SqliteDriver {
    async fn rollback(&self) -> Result<()> {
        self.pool
            .run(CancellationToken::new(), |connection, _, _| {
                // SQLite can roll back the whole transaction when interrupted.
                if !connection.is_autocommit() {
                    connection.execute_batch("ROLLBACK").map_err(db_error)?;
                }
                Ok(())
            })
            .await
    }

    async fn snapshot_session(&self) -> Result<Option<Box<dyn Driver>>> {
        let session = Self {
            pool: self.managed.clone(),
            meta: self.managed.clone(),
            managed: self.managed.clone(),
            dialect: SqliteDialect,
        };
        session
            .execute("BEGIN DEFERRED", CancellationToken::new())
            .await?;
        Ok(Some(Box::new(session)))
    }
    async fn managed_session(&self) -> Result<Box<dyn Driver>> {
        Ok(Box::new(Self {
            pool: self.managed.clone(),
            meta: self.meta.clone(),
            managed: self.managed.clone(),
            dialect: SqliteDialect,
        }))
    }
    async fn ping(&self) -> Result<()> {
        self.query("SELECT 1", 1, CancellationToken::new()).await?;
        Ok(())
    }
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        Ok(vec![SchemaInfo {
            name: "main".into(),
            is_system: false,
        }])
    }
    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<TableInfo>> {
        Ok(self.metadata("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name".into()).await?.iter().map(|row| TableInfo {schema: None, name: text(row, 0), kind: if text(row, 1) == "view" {TableKind::View} else {TableKind::Table}, estimated_rows: None}).collect())
    }
    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let quoted = self.dialect.quote_ident(&table.name);
        let column_rows = self
            .metadata(format!("PRAGMA table_info({quoted})"))
            .await?;
        if column_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }
        let mut keys = column_rows
            .iter()
            .filter(|row| integer(row, 5) > 0)
            .map(|row| (integer(row, 5), text(row, 1)))
            .collect::<Vec<_>>();
        keys.sort_by_key(|key| key.0);
        let columns = column_rows
            .iter()
            .map(|row| ColumnDetail {
                name: text(row, 1),
                type_name: text(row, 2),
                nullable: integer(row, 3) == 0,
                default: optional_text(row, 4),
                is_primary_key: integer(row, 5) > 0,
                ordinal: integer(row, 0) as i32,
            })
            .collect();
        let foreign_rows = self
            .metadata(format!("PRAGMA foreign_key_list({quoted})"))
            .await?;
        let foreign_keys = super::fk::group_foreign_keys(
            foreign_rows
                .iter()
                .map(|row| super::fk::FkColumnRow {
                    group: integer(row, 0) as i32,
                    column: text(row, 3),
                    referenced_schema: None,
                    referenced_table: text(row, 2),
                    referenced_column: optional_text(row, 4),
                })
                .collect(),
            |id| format!("fk_{}_{}", table.name, id),
        );
        let mut indexes = Vec::new();
        for row in self
            .metadata(format!("PRAGMA index_list({quoted})"))
            .await?
        {
            let name = text(&row, 1);
            let origin = text(&row, 3);
            let info = self
                .metadata(format!(
                    "PRAGMA index_info({})",
                    self.dialect.quote_ident(&name)
                ))
                .await?;
            indexes.push(IndexInfo {
                name,
                columns: info
                    .iter()
                    .filter_map(|row| optional_text(row, 2))
                    .collect(),
                is_unique: integer(&row, 2) == 1,
                is_primary: origin == "pk",
                is_constraint: origin != "c",
            });
        }
        let kind = self
            .metadata(format!(
                "SELECT type FROM sqlite_master WHERE name = {}",
                self.dialect.quote_string(&table.name)
            ))
            .await?;
        Ok(TableDetail {
            table: table.clone(),
            kind: if kind.first().is_some_and(|row| text(row, 0) == "view") {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns,
            primary_key: keys.into_iter().map(|key| key.1).collect(),
            foreign_keys,
            indexes,
        })
    }
    async fn table_ddl(&self, table: &TableRef) -> Result<Option<String>> {
        let rows = self
            .metadata(format!(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = {}",
                self.dialect.quote_string(&table.name)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|row| optional_text(row, 0))
            .map(|sql| format!("{};", sql.trim().trim_end_matches(';'))))
    }
    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        self.pool
            .query(sql.into(), crate::driver::clamp_limit(Some(limit)), cancel)
            .await
    }
    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        let sql = sql.to_owned();
        self.pool
            .run(cancel, move |connection, _, _| {
                let started = Instant::now();
                let before = connection.total_changes();
                connection.execute_batch(&sql).map_err(db_error)?;
                Ok(ExecResult {
                    rows_affected: if connection.total_changes() == before {
                        0
                    } else {
                        connection.changes()
                    },
                    elapsed_ms: started.elapsed().as_millis() as u64,
                })
            })
            .await
    }
    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        self.apply_transaction_cancelled(statements, CancellationToken::new())
            .await
    }
    async fn apply_streaming_cancelled(
        &self,
        mut chunks: tokio::sync::mpsc::Receiver<ApplyChunk>,
        cancel: CancellationToken,
    ) -> Result<u64> {
        let runtime = tokio::runtime::Handle::current();
        self.managed.run(cancel, move |connection, token, phase| {
            connection.execute_batch("BEGIN").map_err(db_error)?;
            let outcome = (|| {
                let mut total = 0; let mut index = 0;
                loop {
                    let chunk = runtime.block_on(async {tokio::select! { biased; _ = token.cancelled() => None, chunk = chunks.recv() => chunk }});
                    if token.is_cancelled() {return Err(FaroError::Cancelled);}
                    match chunk {
                        Some(ApplyChunk::Done) => return Ok(total),
                        Some(ApplyChunk::Batch(batch)) => for statement in batch {
                            if token.is_cancelled() {return Err(FaroError::Cancelled);}
                            let affected = connection.execute(&statement.sql, []).map_err(|error| if token.is_cancelled() {FaroError::Cancelled} else {db_error(error)})? as u64;
                            if let Some(expected) = statement.expect {if affected != expected {return Err(super::tx::guard_error(index, None, expected, affected));}}
                            total += affected; index += 1;
                        },
                        None => return Err(FaroError::Other("the source could not be read to the end, so nothing was applied".into())),
                    }
                }
            })();
            let may_commit = {
                let mut state = phase.lock().unwrap_or_else(|error| error.into_inner());
                let commit = outcome.is_ok() && !token.is_cancelled();
                *state = if commit {OperationPhase::Committing} else {OperationPhase::RollingBack};
                commit
            };
            if may_commit {
                match connection.execute_batch("COMMIT") {
                    Ok(()) => outcome,
                    Err(error) => Err(rollback_error(connection, db_error(error)))
                }
            } else {
                Err(rollback_error(connection, outcome.err().unwrap_or(FaroError::Cancelled)))
            }
        }).await
    }
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }
    async fn close(&self) {
        for session in [&self.pool, &self.meta, &self.managed] {
            let _ = session
                .run(CancellationToken::new(), |_, _, _| Ok(()))
                .await;
            let connection = session.connection.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut guard) = connection.lock() {
                    guard.take();
                }
            })
            .await;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queued_cancellation_does_not_interrupt_the_session_owner() {
        let session = Session::new(Connection::open_in_memory().unwrap());
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let owner = session.clone();
        let work = tokio::spawn(async move {
            owner
                .run(CancellationToken::new(), move |connection, _, _| {
                    let _ = entered.send(());
                    wait.recv().unwrap();
                    connection
                        .query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                        .map_err(db_error)
                })
                .await
        });
        ready.await.unwrap();
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            session.query("SELECT 99".into(), 1, token).await,
            Err(FaroError::Cancelled)
        ));
        release.send(()).unwrap();
        assert_eq!(work.await.unwrap().unwrap(), 42);
    }

    #[tokio::test]
    async fn invalid_utf8_is_not_silently_changed_and_expression_types_are_reported() {
        let session = Session::new(Connection::open_in_memory().unwrap());
        let result = session
            .query(
                "SELECT CAST(X'80' AS TEXT), 42".into(),
                10,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(&result.rows[0][0], Value::Unsupported(_)));
        assert_eq!(result.columns[1].type_name, "INTEGER");
        assert_eq!(result.rows[0][1], Value::Int(42));
    }

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
