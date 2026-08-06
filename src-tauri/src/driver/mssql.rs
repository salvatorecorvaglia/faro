use async_trait::async_trait;
use bb8::Pool;
use bb8_tiberius::ConnectionManager;
use std::time::Instant;
use tiberius::ColumnData;
use tokio_util::sync::CancellationToken;

use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    ColumnDetail, ColumnInfo, ConnectionConfig, ExecResult, ForeignKey, GuardedStatement,
    IndexInfo, ResultSet, SchemaInfo, SslMode, TableColumns, TableDetail, TableInfo, TableKind,
    TableRef, Value,
};

pub struct SqlServerDialect;

impl Dialect for SqlServerDialect {
    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_bracket(ident)
    }

    /// SQL Server has no `LIMIT`, and it will not accept the wrapping trick the
    /// other engines use.
    ///
    /// T-SQL rejects `ORDER BY` inside a derived table unless that same `SELECT`
    /// also has `TOP` or `OFFSET`. Wrapping arbitrary user SQL therefore breaks
    /// any query that sorts — and Faro cannot inject `TOP` into the inner text
    /// without parsing it.
    ///
    /// So with no offset the SQL is left completely alone; the row cap is
    /// applied by stopping the result stream instead, which works for every
    /// statement no matter how it is written. With an offset, `OFFSET ... FETCH`
    /// is *appended* rather than wrapped, which is the natural T-SQL form.
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        let trimmed = sql.trim().trim_end_matches(';');
        if offset == 0 {
            return trimmed.to_string();
        }

        // OFFSET/FETCH requires an ORDER BY. Only Faro-generated browse SQL
        // reaches this path, and its ORDER BY (if any) is at the top level, so
        // a simple check is exact here rather than merely a guess.
        let order = if has_top_level_order_by(trimmed) {
            String::new()
        } else {
            // Satisfies the parser without imposing an order.
            " ORDER BY (SELECT NULL)".to_string()
        };

        format!("{trimmed}{order} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY")
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        // SQL Server binary literals are 0x-prefixed hex, not X'..'.
        let mut s = String::with_capacity(bytes.len() * 2 + 2);
        s.push_str("0x");
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

/// SQL Server, via tiberius.
///
/// The asymmetric driver: tiberius is not sqlx, so it brings no pool and no
/// shared query API. `bb8` supplies the pooling and the tokio-util compat layer
/// over the socket. Everything below keeps the same shape as the other drivers
/// so nothing above the `Driver` trait can tell the difference.
pub struct SqlServerDriver {
    /// User SQL. Single connection, so session state is coherent.
    pool: Pool<ConnectionManager>,
    /// Catalog reads only, so schema browsing never queues behind a long query.
    meta: Pool<ConnectionManager>,
    dialect: SqlServerDialect,
    schema: String,
}

impl SqlServerDriver {
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let mut tds = tiberius::Config::new();
        tds.host(&config.host);
        tds.port(if config.port == 0 { 1433 } else { config.port });
        tds.authentication(tiberius::AuthMethod::sql_server(
            &config.username,
            password.unwrap_or_default(),
        ));
        if !config.database.is_empty() {
            tds.database(&config.database);
        }
        match config.ssl_mode {
            // SQL Server images ship a self-signed certificate, so requiring a
            // verified chain would refuse almost every local instance. "Require"
            // still encrypts; verification is what is relaxed.
            SslMode::Disable | SslMode::Prefer => tds.trust_cert(),
            SslMode::Require => tds.trust_cert(),
        }

        // Probe once directly before building the pool.
        //
        // bb8 retries a failing connection until its timeout expires and then
        // reports "Timed out in bb8", which buries the reason. A wrong password
        // or a database that does not exist should say so — SQL Server tells us
        // exactly that, and this is the only place the message survives.
        probe(&tds).await?;

        let build = |cfg: tiberius::Config| async move {
            Pool::builder()
                .max_size(1)
                .connection_timeout(std::time::Duration::from_secs(15))
                .build(ConnectionManager::new(cfg))
                .await
                .map_err(|e| FaroError::Connection(e.to_string()))
        };

        let pool = build(tds.clone()).await?;
        let meta = build(tds).await?;

        Ok(Self {
            pool,
            meta,
            dialect: SqlServerDialect,
            // Faro browses the default schema. `dbo` is SQL Server's, and using
            // it explicitly keeps catalog queries scoped.
            schema: "dbo".to_string(),
        })
    }

    /// Run a query on `pool` and collect it into a `ResultSet`.
    ///
    /// Consumes the stream row by row and stops one past `limit`. That extra row
    /// is what proves truncation, and stopping there is what keeps the row cap
    /// cheap — `into_first_result()` would materialize every row of a large
    /// table before the cap could be applied.
    async fn fetch(pool: &Pool<ConnectionManager>, sql: &str, limit: u64) -> Result<ResultSet> {
        use futures::TryStreamExt;

        let started = Instant::now();
        let mut client = pool
            .get()
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        let mut stream = client.simple_query(sql).await.map_err(map_err)?;

        let mut columns: Vec<ColumnInfo> = Vec::new();
        let mut data: Vec<Vec<Value>> = Vec::new();
        let mut seen = 0u64;

        while let Some(item) = stream.try_next().await.map_err(map_err)? {
            // The stream also carries metadata and done tokens; only rows matter.
            let tiberius::QueryItem::Row(row) = item else {
                continue;
            };

            if columns.is_empty() {
                columns = row
                    .columns()
                    .iter()
                    .map(|c| ColumnInfo {
                        name: c.name().to_string(),
                        type_name: format!("{:?}", c.column_type()).to_lowercase(),
                    })
                    .collect();
            }

            seen += 1;
            if seen > limit {
                break;
            }
            data.push(row.cells().map(|(_, cell)| decode_value(cell)).collect());
        }

        Ok(ResultSet {
            columns,
            rows: data,
            truncated: seen > limit,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Run catalog SQL and hand back the rows as text, which is all the
    /// information_schema queries below need.
    async fn meta_rows(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        Ok(Self::fetch(&self.meta, sql, u64::MAX).await?.rows)
    }
}

/// Whether `sql` ends with an `ORDER BY` that belongs to its outermost query.
///
/// Checks the tail rather than searching the whole string: an `ORDER BY` inside
/// a subquery is followed by that subquery's closing paren, so it never lands
/// last. Good enough because only Faro-generated SQL reaches the offset path.
fn has_top_level_order_by(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    match lower.rfind("order by") {
        // Nothing but the sort key may follow, or the ORDER BY was nested.
        Some(index) => !lower[index..].contains(')'),
        None => false,
    }
}

/// Open one connection and throw it away, purely to surface a real error.
async fn probe(config: &tiberius::Config) -> Result<()> {
    use tokio_util::compat::TokioAsyncWriteCompatExt;

    let tcp = tokio::net::TcpStream::connect(config.get_addr())
        .await
        .map_err(|e| FaroError::Connection(format!("could not reach the server: {e}")))?;
    tcp.set_nodelay(true).ok();

    let client = tiberius::Client::connect(config.clone(), tcp.compat_write())
        .await
        .map_err(|e| match map_err(e) {
            // A login failure is a connection problem from the user's side, not
            // a query error, so it is reported as one.
            FaroError::Database { message, .. } => FaroError::Connection(message),
            other => other,
        })?;

    let _ = client.close().await;
    Ok(())
}

fn map_err(e: tiberius::error::Error) -> FaroError {
    match &e {
        tiberius::error::Error::Server(token) => FaroError::Database {
            message: token.message().to_string(),
            code: Some(token.code().to_string()),
        },
        _ => FaroError::Database {
            message: e.to_string(),
            code: None,
        },
    }
}

/// Read a cell as text, for catalog results where the type is known to be
/// character data.
fn as_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Text(s)) => s.clone(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Decimal(d)) => d.clone(),
        _ => String::new(),
    }
}

fn as_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Int(i)) => *i,
        Some(Value::Decimal(d)) => d.parse().unwrap_or(0),
        _ => 0,
    }
}

#[async_trait]
impl Driver for SqlServerDriver {
    async fn ping(&self) -> Result<()> {
        Self::fetch(&self.pool, "SELECT 1", 1).await.map(|_| ())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = self
            .meta_rows(
                "SELECT name FROM sys.schemas
                 WHERE name NOT IN ('sys', 'INFORMATION_SCHEMA')
                 ORDER BY name",
            )
            .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let name = as_text(r.first());
                SchemaInfo {
                    // db_* schemas are SQL Server's built-in database roles.
                    is_system: name.starts_with("db_") || name == "guest",
                    name,
                }
            })
            .collect())
    }

    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let schema = schema.unwrap_or(&self.schema).to_string();
        let quoted = crate::model::Value::Text(schema.clone());
        let literal = self.dialect.literal(&quoted);

        // Row counts come from sys.dm_db_partition_stats, which is an estimate
        // maintained by the engine — a COUNT(*) per table would stall the tree.
        let rows = self
            .meta_rows(&format!(
                "SELECT t.name, 'TABLE' AS kind,
                        ISNULL(SUM(p.row_count), 0) AS est_rows
                 FROM sys.tables t
                 JOIN sys.schemas s ON s.schema_id = t.schema_id
                 LEFT JOIN sys.dm_db_partition_stats p
                        ON p.object_id = t.object_id AND p.index_id IN (0, 1)
                 WHERE s.name = {literal}
                 GROUP BY t.name
                 UNION ALL
                 SELECT v.name, 'VIEW', 0
                 FROM sys.views v
                 JOIN sys.schemas s ON s.schema_id = v.schema_id
                 WHERE s.name = {literal}
                 ORDER BY 1"
            ))
            .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let kind = as_text(r.get(1));
                TableInfo {
                    schema: Some(schema.clone()),
                    name: as_text(r.first()),
                    kind: if kind == "VIEW" {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                    estimated_rows: Some(as_int(r.get(2))),
                }
            })
            .collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let schema = table.schema.clone().unwrap_or_else(|| self.schema.clone());
        let s_lit = self.dialect.literal(&Value::Text(schema.clone()));
        let t_lit = self.dialect.literal(&Value::Text(table.name.clone()));

        let col_rows = self
            .meta_rows(&format!(
                "SELECT c.COLUMN_NAME,
                        c.DATA_TYPE
                          + CASE WHEN c.CHARACTER_MAXIMUM_LENGTH IS NOT NULL
                                 THEN '(' + CASE WHEN c.CHARACTER_MAXIMUM_LENGTH = -1
                                                 THEN 'max'
                                                 ELSE CAST(c.CHARACTER_MAXIMUM_LENGTH AS VARCHAR) END + ')'
                                 ELSE '' END AS type_name,
                        c.IS_NULLABLE,
                        c.COLUMN_DEFAULT,
                        c.ORDINAL_POSITION
                 FROM INFORMATION_SCHEMA.COLUMNS c
                 WHERE c.TABLE_SCHEMA = {s_lit} AND c.TABLE_NAME = {t_lit}
                 ORDER BY c.ORDINAL_POSITION"
            ))
            .await?;

        if col_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }

        // Primary key columns in key order.
        let pk_rows = self
            .meta_rows(&format!(
                "SELECT k.COLUMN_NAME
                 FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
                 JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k
                      ON k.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
                     AND k.TABLE_SCHEMA = tc.TABLE_SCHEMA
                 WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY'
                   AND tc.TABLE_SCHEMA = {s_lit} AND tc.TABLE_NAME = {t_lit}
                 ORDER BY k.ORDINAL_POSITION"
            ))
            .await?;
        let primary_key: Vec<String> = pk_rows.iter().map(|r| as_text(r.first())).collect();

        let fk_rows = self
            .meta_rows(&format!(
                "SELECT fk.name,
                        pc.name AS col,
                        rs.name AS ref_schema,
                        rt.name AS ref_table,
                        rc.name AS ref_col
                 FROM sys.foreign_keys fk
                 JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
                 JOIN sys.tables t ON t.object_id = fk.parent_object_id
                 JOIN sys.schemas s ON s.schema_id = t.schema_id
                 JOIN sys.columns pc ON pc.object_id = t.object_id
                                    AND pc.column_id = fkc.parent_column_id
                 JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id
                 JOIN sys.schemas rs ON rs.schema_id = rt.schema_id
                 JOIN sys.columns rc ON rc.object_id = rt.object_id
                                    AND rc.column_id = fkc.referenced_column_id
                 WHERE s.name = {s_lit} AND t.name = {t_lit}
                 ORDER BY fk.name, fkc.constraint_column_id"
            ))
            .await?;

        // One row per column of a composite key, grouped by constraint name.
        let mut foreign_keys: Vec<ForeignKey> = Vec::new();
        for r in &fk_rows {
            let name = as_text(r.first());
            let column = as_text(r.get(1));
            let ref_schema = as_text(r.get(2));
            let ref_table = as_text(r.get(3));
            let ref_column = as_text(r.get(4));

            match foreign_keys.iter_mut().find(|f| f.name == name) {
                Some(fk) => {
                    fk.columns.push(column);
                    fk.referenced_columns.push(ref_column);
                }
                None => foreign_keys.push(ForeignKey {
                    name,
                    columns: vec![column],
                    referenced_table: TableRef {
                        schema: Some(ref_schema),
                        name: ref_table,
                    },
                    referenced_columns: vec![ref_column],
                }),
            }
        }

        let idx_rows = self
            .meta_rows(&format!(
                "SELECT i.name,
                        CAST(i.is_unique AS INT),
                        CAST(i.is_primary_key AS INT),
                        CAST(CASE WHEN i.is_unique_constraint = 1 OR i.is_primary_key = 1
                                  THEN 1 ELSE 0 END AS INT) AS is_constraint,
                        c.name AS col
                 FROM sys.indexes i
                 JOIN sys.tables t ON t.object_id = i.object_id
                 JOIN sys.schemas s ON s.schema_id = t.schema_id
                 JOIN sys.index_columns ic ON ic.object_id = i.object_id
                                          AND ic.index_id = i.index_id
                 JOIN sys.columns c ON c.object_id = t.object_id
                                   AND c.column_id = ic.column_id
                 WHERE s.name = {s_lit} AND t.name = {t_lit} AND i.name IS NOT NULL
                 ORDER BY i.name, ic.key_ordinal"
            ))
            .await?;

        let mut indexes: Vec<IndexInfo> = Vec::new();
        for r in &idx_rows {
            let name = as_text(r.first());
            let column = as_text(r.get(4));
            match indexes.iter_mut().find(|i| i.name == name) {
                Some(index) => index.columns.push(column),
                None => indexes.push(IndexInfo {
                    name,
                    columns: vec![column],
                    is_unique: as_int(r.get(1)) == 1,
                    is_primary: as_int(r.get(2)) == 1,
                    is_constraint: as_int(r.get(3)) == 1,
                }),
            }
        }

        let is_view = self
            .meta_rows(&format!(
                "SELECT COUNT(*) FROM sys.views v
                 JOIN sys.schemas s ON s.schema_id = v.schema_id
                 WHERE s.name = {s_lit} AND v.name = {t_lit}"
            ))
            .await?
            .first()
            .map(|r| as_int(r.first()) > 0)
            .unwrap_or(false);

        Ok(TableDetail {
            table: table.clone(),
            kind: if is_view {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns: col_rows
                .iter()
                .map(|r| {
                    let name = as_text(r.first());
                    ColumnDetail {
                        is_primary_key: primary_key.contains(&name),
                        name,
                        type_name: as_text(r.get(1)),
                        nullable: as_text(r.get(2)).eq_ignore_ascii_case("YES"),
                        default: match r.get(3) {
                            Some(Value::Null) | None => None,
                            other => Some(as_text(other)),
                        },
                        ordinal: as_int(r.get(4)) as i32,
                    }
                })
                .collect(),
            primary_key,
            foreign_keys,
            indexes,
        })
    }

    async fn schema_snapshot(&self, schema: Option<&str>) -> Result<Vec<TableColumns>> {
        let schema = schema.unwrap_or(&self.schema).to_string();
        let s_lit = self.dialect.literal(&Value::Text(schema.clone()));

        // One catalog query rather than four per table.
        let rows = self
            .meta_rows(&format!(
                "SELECT TABLE_NAME, COLUMN_NAME
                 FROM INFORMATION_SCHEMA.COLUMNS
                 WHERE TABLE_SCHEMA = {s_lit}
                 ORDER BY TABLE_NAME, ORDINAL_POSITION"
            ))
            .await?;

        let mut out: Vec<TableColumns> = Vec::new();
        for r in &rows {
            let table = as_text(r.first());
            let column = as_text(r.get(1));
            match out.last_mut().filter(|t| t.name == table) {
                Some(entry) => entry.columns.push(column),
                None => out.push(TableColumns {
                    schema: Some(schema.clone()),
                    name: table,
                    columns: vec![column],
                }),
            }
        }
        Ok(out)
    }

    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        let paged = self.dialect.paginate(sql, limit + 1, 0);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(FaroError::Cancelled),
            res = Self::fetch(&self.pool, &paged, limit) => res,
        }
    }

    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        let started = Instant::now();
        let owned = sql.to_string();

        let run = async {
            let mut client = self
                .pool
                .get()
                .await
                .map_err(|e| FaroError::Connection(e.to_string()))?;
            let result = client.execute(&owned, &[]).await.map_err(map_err)?;
            Ok::<u64, FaroError>(result.rows_affected().iter().sum())
        };

        let rows_affected = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FaroError::Cancelled),
            res = run => res?,
        };

        Ok(ExecResult {
            rows_affected,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        if statements.is_empty() {
            return Ok(0);
        }

        let mut client = self
            .pool
            .get()
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        client
            .simple_query("BEGIN TRANSACTION")
            .await
            .map_err(map_err)?
            .into_results()
            .await
            .map_err(map_err)?;

        let mut total = 0u64;
        for (index, stmt) in statements.iter().enumerate() {
            let affected: u64 = match client.execute(&stmt.sql, &[]).await {
                Ok(r) => r.rows_affected().iter().sum(),
                Err(e) => {
                    let _ = client.simple_query("ROLLBACK").await;
                    return Err(map_err(e));
                }
            };

            if let Some(expected) = stmt.expect {
                if affected != expected {
                    // The same safety net as the other drivers: a statement that
                    // touched the wrong number of rows aborts the whole batch.
                    let _ = client.simple_query("ROLLBACK").await;
                    return Err(super::tx::guard_error(
                        index,
                        statements.len(),
                        expected,
                        affected,
                    ));
                }
            }
            total += affected;
        }

        client
            .simple_query("COMMIT")
            .await
            .map_err(map_err)?
            .into_results()
            .await
            .map_err(map_err)?;

        Ok(total)
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // bb8 closes its connections when the pool is dropped; there is no
        // explicit shutdown reachable through a shared reference.
    }
}

/// Decode one cell into the engine-agnostic `Value`.
fn decode_value(cell: &ColumnData<'static>) -> Value {
    match cell {
        ColumnData::U8(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I16(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I32(v) => v.map(|x| Value::Int(x as i64)).unwrap_or(Value::Null),
        ColumnData::I64(v) => v.map(Value::Int).unwrap_or(Value::Null),
        ColumnData::F32(v) => v.map(|x| Value::Float(x as f64)).unwrap_or(Value::Null),
        ColumnData::F64(v) => v.map(Value::Float).unwrap_or(Value::Null),
        ColumnData::Bit(v) => v.map(Value::Bool).unwrap_or(Value::Null),
        ColumnData::String(v) => v
            .as_ref()
            .map(|s| Value::Text(s.to_string()))
            .unwrap_or(Value::Null),
        ColumnData::Guid(v) => v.map(|g| Value::Uuid(g.to_string())).unwrap_or(Value::Null),
        ColumnData::Binary(v) => v
            .as_ref()
            .map(|b| Value::Bytes(b.to_vec()))
            .unwrap_or(Value::Null),
        // Rendered rather than converted: DECIMAL/MONEY must not pass through
        // f64, which would round anything past 15 significant digits.
        ColumnData::Numeric(v) => v
            .map(|n| Value::Decimal(n.to_string()))
            .unwrap_or(Value::Null),
        ColumnData::Xml(v) => v
            .as_ref()
            .map(|x| Value::Text(x.to_string()))
            .unwrap_or(Value::Null),
        ColumnData::DateTime(_)
        | ColumnData::SmallDateTime(_)
        | ColumnData::DateTime2(_)
        | ColumnData::DateTimeOffset(_) => decode_temporal(cell, TemporalKind::Timestamp),
        ColumnData::Date(_) => decode_temporal(cell, TemporalKind::Date),
        ColumnData::Time(_) => decode_temporal(cell, TemporalKind::Time),
    }
}

enum TemporalKind {
    Date,
    Time,
    Timestamp,
}

/// Convert a temporal column through tiberius' chrono support.
///
/// tiberius' own date types are wire representations rather than calendar
/// values, so they are read out via `FromSql` into chrono types instead of being
/// destructured by hand.
fn decode_temporal(cell: &ColumnData<'static>, kind: TemporalKind) -> Value {
    use tiberius::time::chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use tiberius::FromSql;

    match kind {
        TemporalKind::Date => NaiveDate::from_sql(cell)
            .ok()
            .flatten()
            .map(|d| Value::Date(d.to_string()))
            .unwrap_or(Value::Null),
        TemporalKind::Time => NaiveTime::from_sql(cell)
            .ok()
            .flatten()
            .map(|t| Value::Time(t.to_string()))
            .unwrap_or(Value::Null),
        TemporalKind::Timestamp => NaiveDateTime::from_sql(cell)
            .ok()
            .flatten()
            .map(|t| Value::Timestamp(t.to_string()))
            .unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_use_brackets_and_escape_them() {
        assert_eq!(SqlServerDialect.quote_ident("my table"), "[my table]");
        // A literal ] is escaped by doubling; without this an identifier could
        // break out of its own brackets.
        assert_eq!(SqlServerDialect.quote_ident("we]ird"), "[we]]ird]");
    }

    #[test]
    fn pagination_without_an_offset_leaves_the_sql_untouched() {
        // Wrapping would break any ORDER BY the caller wrote, so the row cap is
        // applied by stopping the result stream instead.
        let out = SqlServerDialect.paginate("SELECT * FROM t ORDER BY a", 10, 0);
        assert_eq!(out, "SELECT * FROM t ORDER BY a");
    }

    #[test]
    fn pagination_with_an_offset_appends_offset_fetch() {
        let out = SqlServerDialect.paginate("SELECT * FROM t ORDER BY a", 10, 20);
        assert_eq!(
            out,
            "SELECT * FROM t ORDER BY a OFFSET 20 ROWS FETCH NEXT 10 ROWS ONLY"
        );
    }

    #[test]
    fn pagination_supplies_an_order_by_when_the_query_has_none() {
        // OFFSET/FETCH is a syntax error without ORDER BY.
        let out = SqlServerDialect.paginate("SELECT * FROM t", 10, 20);
        assert!(out.contains("ORDER BY (SELECT NULL)"), "{out}");
        assert!(
            out.ends_with("OFFSET 20 ROWS FETCH NEXT 10 ROWS ONLY"),
            "{out}"
        );
    }

    #[test]
    fn pagination_strips_a_trailing_semicolon() {
        let out = SqlServerDialect.paginate("SELECT 1;", 5, 0);
        assert_eq!(out, "SELECT 1");
    }

    #[test]
    fn a_nested_order_by_does_not_count_as_top_level() {
        // Skipping our own ORDER BY here would make OFFSET/FETCH a syntax error.
        assert!(!has_top_level_order_by(
            "SELECT * FROM (SELECT 1 ORDER BY a) q"
        ));
        assert!(has_top_level_order_by("SELECT * FROM t ORDER BY a"));
        assert!(!has_top_level_order_by("SELECT * FROM t"));
    }

    #[test]
    fn binary_literals_use_the_0x_prefix() {
        // X'..' is MySQL/SQLite syntax and a syntax error here.
        assert_eq!(SqlServerDialect.quote_bytes(&[0xde, 0xad]), "0xdead");
    }

    #[test]
    fn schema_qualification_uses_brackets() {
        assert_eq!(
            SqlServerDialect.qualify(Some("dbo"), "users"),
            "[dbo].[users]"
        );
    }
}
