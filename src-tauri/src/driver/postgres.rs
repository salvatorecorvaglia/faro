use async_trait::async_trait;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow, PgSslMode};
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

pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_double(ident)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        dialect::paginate_limit_offset(sql, limit, offset)
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        // Postgres bytea hex format: '\x0a1b'::bytea
        let mut s = String::with_capacity(bytes.len() * 2 + 12);
        s.push_str("'\\x");
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s.push_str("'::bytea");
        s
    }
}

pub struct PostgresDriver {
    /// User SQL. Single connection, so session state is coherent.
    pool: PgPool,
    /// Catalog reads only, so schema browsing never queues behind a long query.
    meta: PgPool,
    dialect: PostgresDialect,
}

impl PostgresDriver {
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let mut opts = PgConnectOptions::new()
            .host(&config.host)
            .port(if config.port == 0 { 5432 } else { config.port })
            .username(&config.username)
            .ssl_mode(match config.ssl_mode {
                SslMode::Disable => PgSslMode::Disable,
                SslMode::Prefer => PgSslMode::Prefer,
                // `Require` encrypts without validating; the Verify modes are
                // the ones that actually authenticate the server.
                SslMode::Require => PgSslMode::Require,
                SslMode::VerifyCa => PgSslMode::VerifyCa,
                SslMode::VerifyFull => PgSslMode::VerifyFull,
            });

        if !config.database.is_empty() {
            opts = opts.database(&config.database);
        }
        if let Some(pw) = password {
            opts = opts.password(pw);
        }
        if config.read_only {
            // Enforced by the server as well as by Faro. The Faro-side guard
            // classifies statements by leading keyword and so cannot catch a
            // write hidden inside a function call; this can.
            opts = opts.options([("default_transaction_read_only", "on")]);
        }

        // One connection for user SQL, so the user gets one coherent session.
        //
        // Temp tables, `SET search_path`, and open transactions are all
        // per-connection. A larger pool would scatter statements across
        // sessions, so `CREATE TEMP TABLE t` then `INSERT INTO t` could fail
        // with "relation does not exist" for no reason the user can see.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            // Fail fast on a wrong host rather than making the user wait out
            // the OS-level TCP timeout.
            .acquire_timeout(std::time::Duration::from_secs(15))
            .connect_with(opts.clone())
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        // A second connection reserved for catalog reads.
        //
        // Schema browsing and autocomplete must stay responsive while a long
        // query runs; sharing the single session connection would make the
        // schema tree hang behind the user's own `SELECT`. Catalog queries
        // carry no session state, so splitting them off costs nothing.
        //
        // One consequence worth knowing: temp tables created on the query
        // connection are invisible to this one, so they do not appear in the
        // schema tree.
        let meta = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(15))
            .connect_with(opts)
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        Ok(Self {
            pool,
            meta,
            dialect: PostgresDialect,
        })
    }
}

#[async_trait]
impl Driver for PostgresDriver {
    async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = sqlx::query(
            r#"
            SELECT nspname AS name
            FROM pg_namespace
            WHERE nspname NOT LIKE 'pg_toast%' AND nspname NOT LIKE 'pg_temp%'
            ORDER BY nspname
            "#,
        )
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let name: String = r.get("name");
                let is_system = name == "information_schema" || name.starts_with("pg_");
                SchemaInfo { name, is_system }
            })
            .collect())
    }

    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let schema = schema.unwrap_or("public");

        // reltuples is the planner's estimate. An exact COUNT(*) per table would
        // make the schema tree unusable on a large database.
        let rows = sqlx::query(
            r#"
            SELECT c.relname AS name,
                   c.relkind::text AS kind,
                   CASE WHEN c.reltuples < 0 THEN NULL
                        ELSE c.reltuples::bigint END AS est_rows
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
            ORDER BY c.relname
            "#,
        )
        .bind(schema)
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let kind: String = r.get("kind");
                TableInfo {
                    schema: Some(schema.to_string()),
                    name: r.get("name"),
                    kind: match kind.as_str() {
                        "v" => TableKind::View,
                        "m" => TableKind::MaterializedView,
                        _ => TableKind::Table,
                    },
                    estimated_rows: r.get("est_rows"),
                }
            })
            .collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let schema = table.schema.as_deref().unwrap_or("public");

        let col_rows = sqlx::query(
            r#"
            SELECT a.attname AS name,
                   format_type(a.atttypid, a.atttypmod) AS type_name,
                   NOT a.attnotnull AS nullable,
                   pg_get_expr(d.adbin, d.adrelid) AS default_expr,
                   a.attnum AS ordinal
            FROM pg_attribute a
            JOIN pg_class c ON c.oid = a.attrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
            WHERE n.nspname = $1 AND c.relname = $2
              AND a.attnum > 0 AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(schema)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        if col_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }

        // Index metadata carries the PK, uniqueness and column order in one query.
        let idx_rows = sqlx::query(
            r#"
            SELECT i.relname AS name,
                   ix.indisunique AS is_unique,
                   ix.indisprimary AS is_primary,
                   -- Backed by a PRIMARY KEY, UNIQUE or EXCLUDE constraint,
                   -- which creates the index itself.
                   EXISTS (
                     SELECT 1 FROM pg_constraint con
                     WHERE con.conindid = ix.indexrelid
                   ) AS is_constraint,
                   ARRAY(
                     SELECT a.attname
                     FROM unnest(ix.indkey) WITH ORDINALITY AS k(attnum, ord)
                     JOIN pg_attribute a
                       ON a.attrelid = ix.indrelid AND a.attnum = k.attnum
                     ORDER BY k.ord
                   ) AS cols
            FROM pg_index ix
            JOIN pg_class i ON i.oid = ix.indexrelid
            JOIN pg_class c ON c.oid = ix.indrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = $1 AND c.relname = $2
            ORDER BY ix.indisprimary DESC, i.relname
            "#,
        )
        .bind(schema)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        let indexes: Vec<IndexInfo> = idx_rows
            .into_iter()
            .map(|r| IndexInfo {
                name: r.get("name"),
                columns: r.get::<Vec<String>, _>("cols"),
                is_unique: r.get("is_unique"),
                is_primary: r.get("is_primary"),
                is_constraint: r.get("is_constraint"),
            })
            .collect();

        let primary_key: Vec<String> = indexes
            .iter()
            .find(|i| i.is_primary)
            .map(|i| i.columns.clone())
            .unwrap_or_default();

        let fk_rows = sqlx::query(
            r#"
            SELECT con.conname AS name,
                   ARRAY(
                     SELECT a.attname FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                     JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                     ORDER BY k.ord
                   ) AS cols,
                   fn.nspname AS ref_schema,
                   fc.relname AS ref_table,
                   ARRAY(
                     SELECT a.attname FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
                     JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum
                     ORDER BY k.ord
                   ) AS ref_cols
            FROM pg_constraint con
            JOIN pg_class c ON c.oid = con.conrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            JOIN pg_class fc ON fc.oid = con.confrelid
            JOIN pg_namespace fn ON fn.oid = fc.relnamespace
            WHERE n.nspname = $1 AND c.relname = $2 AND con.contype = 'f'
            ORDER BY con.conname
            "#,
        )
        .bind(schema)
        .bind(&table.name)
        .fetch_all(&self.meta)
        .await?;

        let kind_row = sqlx::query(
            r#"SELECT c.relkind::text AS kind FROM pg_class c
               JOIN pg_namespace n ON n.oid = c.relnamespace
               WHERE n.nspname = $1 AND c.relname = $2"#,
        )
        .bind(schema)
        .bind(&table.name)
        .fetch_one(&self.meta)
        .await?;
        let kind: String = kind_row.get("kind");

        Ok(TableDetail {
            table: table.clone(),
            kind: match kind.as_str() {
                "v" => TableKind::View,
                "m" => TableKind::MaterializedView,
                _ => TableKind::Table,
            },
            columns: col_rows
                .into_iter()
                .map(|r| {
                    let name: String = r.get("name");
                    ColumnDetail {
                        is_primary_key: primary_key.contains(&name),
                        name,
                        type_name: r.get("type_name"),
                        nullable: r.get("nullable"),
                        default: r.get("default_expr"),
                        ordinal: r.get::<i16, _>("ordinal") as i32,
                    }
                })
                .collect(),
            primary_key,
            foreign_keys: fk_rows
                .into_iter()
                .map(|r| ForeignKey {
                    name: r.get("name"),
                    columns: r.get::<Vec<String>, _>("cols"),
                    referenced_table: TableRef {
                        schema: Some(r.get("ref_schema")),
                        name: r.get("ref_table"),
                    },
                    referenced_columns: r.get::<Vec<String>, _>("ref_cols"),
                })
                .collect(),
            indexes,
        })
    }

    /// One catalog query for the whole schema.
    ///
    /// The default walk would issue four queries per table; on a schema with
    /// 500 tables that is 2000 round trips before autocomplete works.
    async fn schema_snapshot(&self, schema: Option<&str>) -> Result<Vec<TableColumns>> {
        let schema = schema.unwrap_or("public");

        let rows = sqlx::query(
            r#"
            SELECT c.relname AS name,
                   ARRAY(
                     SELECT a.attname
                     FROM pg_attribute a
                     WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
                     ORDER BY a.attnum
                   ) AS cols
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
            ORDER BY c.relname
            "#,
        )
        .bind(schema)
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TableColumns {
                schema: Some(schema.to_string()),
                name: r.get("name"),
                columns: r.get::<Vec<String>, _>("cols"),
            })
            .collect())
    }

    /// Run a statement and cap the rows as they arrive.
    ///
    /// The SQL is passed through untouched — see `driver::fetch` for why it is
    /// not wrapped in a paginating subquery.
    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        super::fetch::fetch_capped_pg(&self.pool, sql, limit, cancel).await
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
        super::tx::apply_guarded_pg(&self.pool, statements).await
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // Both pools, or the metadata connection lingers after "Disconnect".
        self.pool.close().await;
        self.meta.close().await;
    }
}

/// Decode one cell into the engine-agnostic `Value`.
///
/// Postgres type names are matched by their pg_type name rather than OID so the
/// arms stay readable. Anything unmatched falls back to its text rendering, then
/// to `Unsupported` carrying the type name — never a silent NULL.
pub(super) fn decode_value(row: &PgRow, idx: usize) -> Value {
    let raw = match row.try_get_raw(idx) {
        Ok(r) => r,
        Err(_) => return Value::Null,
    };
    if raw.is_null() {
        return Value::Null;
    }

    let type_name = raw.type_info().name().to_string();

    match type_name.as_str() {
        "BOOL" => row.try_get::<bool, _>(idx).map(Value::Bool),
        "INT2" => row.try_get::<i16, _>(idx).map(|v| Value::Int(v as i64)),
        "INT4" => row.try_get::<i32, _>(idx).map(|v| Value::Int(v as i64)),
        "INT8" => row.try_get::<i64, _>(idx).map(Value::Int),
        "FLOAT4" => row.try_get::<f32, _>(idx).map(|v| Value::Float(v as f64)),
        "FLOAT8" => row.try_get::<f64, _>(idx).map(Value::Float),
        // Decoded through `BigDecimal`, which is arbitrary-precision.
        //
        // `rust_decimal` — the obvious choice, and what this used to use — has
        // a 96-bit mantissa, about 28 significant digits, while Postgres
        // `numeric` allows up to 131,072. Anything longer came back silently
        // rounded. Asking for a `String` instead does not help: sqlx sends
        // numeric in the binary format and refuses to decode it as text, so
        // that attempt always failed and fell through to the rounding path.
        //
        // `to_plain_string` rather than `to_string`: `Display` switches to
        // exponential notation past a digit threshold, and a cell showing
        // `1.2e+20` where the column holds an exact integer is a lie about the
        // stored value.
        //
        // `NaN` and the infinities are legal `numeric` values that `BigDecimal`
        // cannot hold; they fall through to `Unsupported` and show as the type
        // name rather than as a wrong number.
        "NUMERIC" => row
            .try_get::<bigdecimal::BigDecimal, _>(idx)
            .map(|v| Value::Decimal(v.to_plain_string())),
        "UUID" => row
            .try_get::<uuid::Uuid, _>(idx)
            .map(|v| Value::Uuid(v.to_string())),
        "JSON" | "JSONB" => row.try_get::<serde_json::Value, _>(idx).map(Value::Json),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Date(v.to_string())),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Time(v.to_string())),
        "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Timestamp(v.to_string())),
        "TIMESTAMPTZ" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(idx)
            .map(|v| Value::Timestamp(v.to_rfc3339())),
        "BYTEA" => row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CHAR" => {
            row.try_get::<String, _>(idx).map(Value::Text)
        }
        "TEXT[]" | "VARCHAR[]" => row
            .try_get::<Vec<String>, _>(idx)
            .map(|v| Value::Array(v.into_iter().map(Value::Text).collect())),
        "INT4[]" => row
            .try_get::<Vec<i32>, _>(idx)
            .map(|v| Value::Array(v.into_iter().map(|i| Value::Int(i as i64)).collect())),
        "INT8[]" => row
            .try_get::<Vec<i64>, _>(idx)
            .map(|v| Value::Array(v.into_iter().map(Value::Int).collect())),
        // Enums, ranges, network types, geometry and every extension type land
        // here. Postgres can render all of them as text, so try that first.
        _ => row.try_get::<String, _>(idx).map(Value::Text),
    }
    .unwrap_or(Value::Unsupported(type_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytea_uses_postgres_hex_syntax() {
        // MySQL's X'..' form is a syntax error in Postgres; this must be '\x..'.
        assert_eq!(
            PostgresDialect.quote_bytes(&[0x0a, 0x1b]),
            r"'\x0a1b'::bytea"
        );
    }

    #[test]
    fn qualify_uses_schema_when_present() {
        assert_eq!(
            PostgresDialect.qualify(Some("public"), "users"),
            r#""public"."users""#
        );
        assert_eq!(PostgresDialect.qualify(None, "users"), r#""users""#);
    }

    #[test]
    fn mixed_case_identifiers_are_quoted() {
        // Unquoted, Postgres would fold "MyTable" to "mytable" and fail.
        assert_eq!(PostgresDialect.quote_ident("MyTable"), r#""MyTable""#);
    }
}
