//! Reading a result set with a row cap, for the sqlx-backed drivers.
//!
//! The cap is applied by **stopping the row stream**, not by rewriting the
//! statement.
//!
//! Wrapping the user's SQL as `SELECT * FROM (<sql>) AS faro_q LIMIT n` looks
//! tidier and keeps the unwanted rows on the server, but it only works for
//! statements that are legal in a `FROM` clause. Two very ordinary cases are
//! not:
//!
//! - `SHOW`, `EXPLAIN`, `DESCRIBE` and `PRAGMA` cannot appear as a derived
//!   table on any engine, so wrapping turned every one of them into a syntax
//!   error the user had no way to explain.
//! - MySQL rejects a derived table whose columns are not uniquely named, so
//!   `SELECT * FROM a JOIN b` failed whenever both tables had an `id`.
//!
//! Stopping the stream works for every statement no matter how it is written,
//! which is the same conclusion the SQL Server and DuckDB drivers had already
//! reached independently. Fetching one row past the limit is what proves the
//! result was truncated, without paying for a `COUNT(*)`.
//!
//! A macro rather than a generic function because sqlx exposes no trait tying a
//! pool to its row type, so the bound cannot be written — the same reason
//! `tx.rs` is a macro.

use futures::TryStreamExt;
use tokio_util::sync::CancellationToken;

use crate::error::{FaroError, Result};
use crate::model::{ColumnInfo, ResultSet, Value};

/// Generate a capped `fetch` for one sqlx pool type.
macro_rules! impl_fetch_capped {
    ($name:ident, $pool:ty, $decode:path) => {
        /// Run `sql` and collect at most `limit` rows.
        ///
        /// The statement is passed through untouched, so anything the engine
        /// accepts as a statement works here.
        pub async fn $name(
            pool: &$pool,
            sql: &str,
            limit: u64,
            cancel: CancellationToken,
        ) -> Result<ResultSet> {
            use sqlx::{Column, Row, TypeInfo};

            let started = std::time::Instant::now();

            // AssertSqlSafe: running user-authored SQL is the entire purpose of a
            // SQL client. The statement is the user's own, executed against their
            // own credentials, so sqlx's injection guard does not apply. Generated
            // SQL (browse filters, DML) is built with quoted identifiers and
            // escaped literals in the layers above.
            let mut stream = sqlx::query(sqlx::AssertSqlSafe(sql.to_string())).fetch(pool);

            let mut columns: Vec<ColumnInfo> = Vec::new();
            let mut rows: Vec<Vec<Value>> = Vec::new();
            let mut seen = 0u64;

            loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(FaroError::Cancelled),
                    row = stream.try_next() => row?,
                };
                let Some(row) = next else { break };

                // Column metadata comes off the first row; sqlx does not expose
                // it before the statement has produced one.
                if columns.is_empty() {
                    columns = row
                        .columns()
                        .iter()
                        .map(|c| ColumnInfo {
                            name: c.name().to_string(),
                            type_name: c.type_info().name().to_string(),
                        })
                        .collect();
                }

                seen += 1;
                if seen > limit {
                    // The extra row proves truncation; it is not returned.
                    break;
                }

                rows.push((0..columns.len()).map(|i| $decode(&row, i)).collect());
            }

            // A query that matched nothing still has columns worth showing.
            // sqlx only surfaces column metadata through the row stream, so
            // with zero rows the grid was left with no headers at all — it
            // rendered blank, which reads as "the query is broken" rather than
            // "no rows matched". Describing the statement recovers them.
            //
            // Best effort: preparing runs the statement through the engine's
            // parser, and engine-specific syntax it rejects will fail here. An
            // empty column list is still better than an error.
            if columns.is_empty() {
                use sqlx::{Executor, SqlSafeStr, Statement};
                let owned = sqlx::AssertSqlSafe(sql.to_string()).into_sql_str();
                if let Ok(prepared) = pool.prepare(owned).await {
                    columns = prepared
                        .columns()
                        .iter()
                        .map(|c| ColumnInfo {
                            name: c.name().to_string(),
                            type_name: c.type_info().name().to_string(),
                        })
                        .collect();
                }
            }

            Ok(ResultSet {
                columns,
                rows,
                truncated: seen > limit,
                elapsed_ms: started.elapsed().as_millis() as u64,
            })
        }
    };
}

impl_fetch_capped!(
    fetch_capped_pg,
    sqlx::PgPool,
    crate::driver::postgres::decode_value
);
impl_fetch_capped!(
    fetch_capped_mysql,
    sqlx::MySqlPool,
    crate::driver::mysql::decode_value
);
impl_fetch_capped!(
    fetch_capped_sqlite,
    sqlx::SqlitePool,
    crate::driver::sqlite::decode_value
);
