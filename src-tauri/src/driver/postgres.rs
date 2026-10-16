use async_trait::async_trait;
use sqlx::postgres::{PgConnectOptions, PgPool, PgRow, PgSslMode};
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

    /// Arrays are dumped as Postgres's own array text, `'{a,b}'`.
    ///
    /// Read as values they became `ARRAY['…', …]` literals, which Postgres
    /// types as `text[]` — and a `uuid[]`, `boolean[]` or `numeric[]` column
    /// refuses `text[]`, so the restore failed. An untyped `'{…}'` literal is
    /// coerced to whatever array type the column declares.
    fn dump_column(&self, column: &crate::model::ColumnDetail) -> String {
        let name = self.quote_ident(&column.name);
        if column.type_name.trim_end().ends_with("[]") {
            format!("{name}::text AS {name}")
        } else {
            name
        }
    }

    /// Faro's reconstructed Postgres DDL omits foreign keys; the backup adds
    /// them once every table and its rows are in place, so neither table order
    /// nor a circular reference can break the restore.
    fn native_ddl_includes_foreign_keys(&self) -> bool {
        false
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

/// What `table_ddl` needs to know about one column.
struct DdlColumn {
    name: String,
    type_name: String,
    not_null: bool,
    default: Option<String>,
    /// `a` (ALWAYS), `d` (BY DEFAULT) or empty: `attidentity`.
    identity: String,
    /// `s` for a stored generated column, else empty: `attgenerated`.
    generated: String,
    /// The sequence a `nextval(...)` default draws from, schema-qualified.
    sequence: Option<String>,
    /// Whether that sequence is owned by this column, as `serial` makes it.
    owns_sequence: bool,
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

        // See `driver::pool::dual_pool` for why this is two single-connection
        // pools rather than one larger one. Worth knowing here specifically:
        // temp tables created on the query connection are invisible on the
        // catalog one, so they do not appear in the schema tree.
        let (pool, meta) = super::pool::dual_pool::<sqlx::Postgres>(
            opts,
            Some(std::time::Duration::from_secs(15)),
            None,
        )
        .await?;

        Ok(Self {
            pool,
            meta,
            dialect: PostgresDialect,
        })
    }
}

impl PostgresDriver {
    /// The table's oid, if it is a plain or partitioned table.
    async fn table_oid(&self, schema: &str, name: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT c.oid::int8 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p')",
        )
        .bind(schema)
        .bind(name)
        .fetch_optional(&self.meta)
        .await?)
    }

    /// The columns of table `oid`, with their identity, generated and
    /// sequence details.
    async fn ddl_columns(&self, oid: i64) -> Result<Vec<DdlColumn>> {
        let rows = sqlx::query(
            r#"
            SELECT a.attname AS name,
                   format_type(a.atttypid, a.atttypmod) AS type_name,
                   a.attnotnull AS not_null,
                   pg_get_expr(d.adbin, d.adrelid) AS default_expr,
                   a.attidentity::text AS identity,
                   a.attgenerated::text AS generated,
                   s.seq AS sequence,
                   s.owned AS owns_sequence
            FROM pg_attribute a
            LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
            -- The sequence a nextval() default draws from, from the default's
            -- own dependency on it, and whether the column owns it.
            LEFT JOIN LATERAL (
                SELECT format('%I.%I', sn.nspname, sc.relname) AS seq,
                       EXISTS (
                           SELECT 1 FROM pg_depend o
                           WHERE o.classid = 'pg_class'::regclass AND o.objid = sc.oid
                             AND o.refobjid = a.attrelid AND o.refobjsubid = a.attnum
                             AND o.deptype = 'a'
                       ) AS owned
                FROM pg_depend dep
                JOIN pg_class sc ON sc.oid = dep.refobjid AND sc.relkind = 'S'
                JOIN pg_namespace sn ON sn.oid = sc.relnamespace
                WHERE dep.classid = 'pg_attrdef'::regclass AND dep.objid = d.oid
                LIMIT 1
            ) s ON true
            WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(oid)
        .fetch_all(&self.meta)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| DdlColumn {
                name: r.get("name"),
                type_name: r.get("type_name"),
                not_null: r.get("not_null"),
                default: r.get("default_expr"),
                identity: r.get("identity"),
                generated: r.get("generated"),
                sequence: r.get("sequence"),
                owns_sequence: r.get::<Option<bool>, _>("owns_sequence").unwrap_or(false),
            })
            .collect())
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

    /// A `CREATE TABLE` faithful enough to restore into an empty database.
    ///
    /// Postgres keeps no DDL text, so the generic rebuild was used — and it
    /// kept a serial column's `DEFAULT nextval('books_id_seq')` while never
    /// creating that sequence, so a dump restored into a fresh database failed
    /// on its first table. It also dropped every UNIQUE and CHECK constraint
    /// and every identity column, and named enum types it never created.
    ///
    /// Rebuilt here from the catalog instead: enum types first (kept if they
    /// already exist), then each column's sequence, then the table with its
    /// identity columns and every PRIMARY KEY, UNIQUE, CHECK and EXCLUDE
    /// constraint in Postgres's own words (`pg_get_constraintdef`). Foreign
    /// keys are left to the backup's later pass, after every table exists.
    async fn table_ddl(&self, table: &TableRef) -> Result<Option<String>> {
        let schema = table.schema.as_deref().unwrap_or("public");
        let Some(oid) = self.table_oid(schema, &table.name).await? else {
            return Ok(None);
        };
        let qualified = self.dialect.qualify(Some(schema), &table.name);
        let columns = self.ddl_columns(oid).await?;
        let mut out: Vec<String> = Vec::new();

        // Enum types the columns use, created only when absent.
        let enums = sqlx::query(
            r#"
            SELECT format_type(t.oid, NULL) AS name,
                   ARRAY(SELECT e.enumlabel FROM pg_enum e
                         WHERE e.enumtypid = t.oid ORDER BY e.enumsortorder) AS labels
            FROM pg_type t
            WHERE t.typtype = 'e'
              AND t.oid IN (SELECT a.atttypid FROM pg_attribute a
                            WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped)
            ORDER BY 1
            "#,
        )
        .bind(oid)
        .fetch_all(&self.meta)
        .await?;
        for e in &enums {
            let name: String = e.get("name");
            let labels: Vec<String> = e.get("labels");
            let labels: Vec<String> = labels
                .iter()
                .map(|l| self.dialect.quote_string(l))
                .collect();
            out.push(format!(
                "DO $faro$ BEGIN CREATE TYPE {name} AS ENUM ({}); \
                 EXCEPTION WHEN duplicate_object THEN NULL; END $faro$;",
                labels.join(", ")
            ));
        }

        // Each serial column's sequence, before the default that names it.
        for c in columns.iter().filter(|c| c.identity.is_empty()) {
            if let Some(sequence) = &c.sequence {
                out.push(format!("CREATE SEQUENCE IF NOT EXISTS {sequence};"));
            }
        }

        let mut lines: Vec<String> = columns
            .iter()
            .map(|c| {
                let mut line = format!("  {} {}", self.dialect.quote_ident(&c.name), c.type_name);
                if !c.generated.is_empty() {
                    if let Some(expr) = &c.default {
                        line.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
                    }
                } else if !c.identity.is_empty() {
                    // Always created BY DEFAULT so the dump's own key values
                    // can be inserted; an ALWAYS column is switched back once
                    // they are in (see `post_data_sql`).
                    line.push_str(" GENERATED BY DEFAULT AS IDENTITY");
                } else if let Some(expr) = &c.default {
                    line.push_str(&format!(" DEFAULT {expr}"));
                }
                if c.not_null {
                    line.push_str(" NOT NULL");
                }
                line
            })
            .collect();

        let constraints = sqlx::query(
            r#"
            SELECT conname AS name, pg_get_constraintdef(oid) AS def
            FROM pg_constraint
            WHERE conrelid = $1 AND contype IN ('p', 'u', 'c', 'x')
            ORDER BY contype = 'p' DESC, conname
            "#,
        )
        .bind(oid)
        .fetch_all(&self.meta)
        .await?;
        for c in &constraints {
            let name: String = c.get("name");
            let def: String = c.get("def");
            lines.push(format!(
                "  CONSTRAINT {} {def}",
                self.dialect.quote_ident(&name)
            ));
        }
        out.push(format!(
            "CREATE TABLE {qualified} (\n{}\n);",
            lines.join(",\n")
        ));

        // Sequences belong to their columns, so dropping the table drops them.
        for c in columns
            .iter()
            .filter(|c| c.identity.is_empty() && c.owns_sequence)
        {
            if let Some(sequence) = &c.sequence {
                out.push(format!(
                    "ALTER SEQUENCE {sequence} OWNED BY {qualified}.{};",
                    self.dialect.quote_ident(&c.name)
                ));
            }
        }
        Ok(Some(out.join("\n")))
    }

    /// STORED generated columns: Postgres refuses an explicit value for one,
    /// so a dump that inserted it failed to restore.
    async fn generated_columns(&self, table: &TableRef) -> Result<Vec<String>> {
        let schema = table.schema.as_deref().unwrap_or("public");
        let Some(oid) = self.table_oid(schema, &table.name).await? else {
            return Ok(Vec::new());
        };
        Ok(self
            .ddl_columns(oid)
            .await?
            .into_iter()
            .filter(|c| !c.generated.is_empty())
            .map(|c| c.name)
            .collect())
    }

    /// Move each serial or identity sequence past the restored keys, and give
    /// an ALWAYS identity column back its ALWAYS.
    ///
    /// Without this a restored table's next insert drew `1` from its fresh
    /// sequence and collided with the restored row that already had it.
    async fn post_data_sql(&self, table: &TableRef) -> Result<Vec<String>> {
        let schema = table.schema.as_deref().unwrap_or("public");
        let Some(oid) = self.table_oid(schema, &table.name).await? else {
            return Ok(Vec::new());
        };
        let qualified = self.dialect.qualify(Some(schema), &table.name);
        let mut out = Vec::new();
        for c in self.ddl_columns(oid).await? {
            let column = self.dialect.quote_ident(&c.name);
            let sequence = if c.identity.is_empty() {
                c.sequence.as_deref().map(|s| self.dialect.quote_string(s))
            } else {
                Some(format!(
                    "pg_get_serial_sequence({}, {})",
                    self.dialect.quote_string(&qualified),
                    self.dialect.quote_string(&c.name)
                ))
            };
            if let Some(sequence) = sequence {
                out.push(format!(
                    "SELECT setval({sequence}, COALESCE(MAX({column}), 1), MAX({column}) IS NOT NULL) \
                     FROM {qualified};"
                ));
            }
            if c.identity == "a" {
                out.push(format!(
                    "ALTER TABLE {qualified} ALTER COLUMN {column} SET GENERATED ALWAYS;"
                ));
            }
        }
        Ok(out)
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
        let mut conn = super::pool::acquire(&self.pool, &cancel).await?;
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FaroError::Cancelled),
            res = sqlx::query(AssertSqlSafe(owned)).execute(&mut *conn) => res?,
        };
        Ok(ExecResult {
            rows_affected: res.rows_affected(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        super::tx::apply_guarded_pg(&self.pool, statements).await
    }

    async fn apply_streaming(
        &self,
        chunks: tokio::sync::mpsc::Receiver<crate::model::ApplyChunk>,
    ) -> Result<u64> {
        super::tx::apply_streaming_pg(&self.pool, chunks).await
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

    // User queries run over the simple protocol and arrive as text; see
    // `fetch_capped_pg`. The binary arms below remain for anything that still
    // reads with the extended protocol.
    if raw.format() == sqlx::postgres::PgValueFormat::Text {
        return match row.try_get_unchecked::<&str, _>(idx) {
            Ok(text) => decode_text(&type_name, text),
            Err(_) => Value::Unsupported(type_name),
        };
    }

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
            .map(|v| Value::Decimal(v.normalized().to_plain_string())),
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

/// Decode a value Postgres sent as text, guided by its type name.
///
/// Text is Postgres's own rendering — what psql shows — and is also valid
/// input for the same type, which is what makes a dump of it restore. Types
/// with a natural Faro form get it (numbers, booleans, exact numerics, UUIDs,
/// JSON, dates, bytea, one-dimensional arrays); every other type — enums,
/// intervals, network types, money, ranges, geometry, extension types — is
/// kept as that text rather than reported as unsupported.
pub(super) fn decode_text(type_name: &str, text: &str) -> Value {
    if let Some(element) = type_name.strip_suffix("[]") {
        // `box` separates array elements with `;`, not `,`; left as text.
        if element != "BOX" {
            if let Some(items) = parse_array(text) {
                return Value::Array(
                    items
                        .into_iter()
                        .map(|item| item.map_or(Value::Null, |t| decode_text(element, &t)))
                        .collect(),
                );
            }
        }
        return Value::Text(text.to_string());
    }

    let text_value = || Value::Text(text.to_string());
    match type_name {
        "BOOL" => match text {
            "t" => Value::Bool(true),
            "f" => Value::Bool(false),
            _ => text_value(),
        },
        "INT2" | "INT4" | "INT8" | "OID" => text.parse().map_or_else(|_| text_value(), Value::Int),
        // Rust reads Postgres's `NaN`, `Infinity` and `-Infinity` as floats.
        "FLOAT4" | "FLOAT8" => text.parse().map_or_else(|_| text_value(), Value::Float),
        // Kept exact, trailing zeros trimmed as before. `NaN` and the
        // infinities are not plain numbers, so they stay quoted text rather
        // than being spliced into SQL bare.
        "NUMERIC" => match text.parse::<bigdecimal::BigDecimal>() {
            Ok(d) if crate::model::is_plain_number(text) => {
                Value::Decimal(d.normalized().to_plain_string())
            }
            _ => text_value(),
        },
        "UUID" => Value::Uuid(text.to_string()),
        "JSON" | "JSONB" => serde_json::from_str(text).map_or_else(|_| text_value(), Value::Json),
        "DATE" => Value::Date(text.to_string()),
        "TIME" => Value::Time(text.to_string()),
        "TIMESTAMP" => Value::Timestamp(text.to_string()),
        // Rendered in the session's zone; normalised to RFC 3339 UTC, as the
        // other engines' zoned timestamps are.
        "TIMESTAMPTZ" => chrono::DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%#z")
            .map_or_else(
                |_| Value::Timestamp(text.to_string()),
                |t| Value::Timestamp(t.with_timezone(&chrono::Utc).to_rfc3339()),
            ),
        "BYTEA" => text
            .strip_prefix("\\x")
            .and_then(decode_hex)
            .map_or_else(text_value, Value::Bytes),
        _ => text_value(),
    }
}

/// `\x0a1b`'s digits as bytes.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The elements of a one-dimensional Postgres array literal — `{a,"b,c",NULL}`
/// — with `None` for SQL NULL. `None` overall for anything else, including a
/// multi-dimensional array, which the caller keeps as text.
fn parse_array(text: &str) -> Option<Vec<Option<String>>> {
    let body = text.strip_prefix('{')?.strip_suffix('}')?;
    let mut items = Vec::new();
    if body.is_empty() {
        return Some(items);
    }
    let mut chars = body.chars().peekable();
    loop {
        let item = if chars.peek() == Some(&'"') {
            chars.next();
            let mut value = String::new();
            loop {
                match chars.next()? {
                    '\\' => value.push(chars.next()?),
                    '"' => break,
                    c => value.push(c),
                }
            }
            Some(value)
        } else {
            let mut value = String::new();
            while let Some(&c) = chars.peek() {
                if c == ',' {
                    break;
                }
                // A nested `{` means more than one dimension.
                if c == '{' {
                    return None;
                }
                value.push(c);
                chars.next();
            }
            (!value.eq_ignore_ascii_case("NULL")).then_some(value)
        };
        items.push(item);
        match chars.next() {
            Some(',') => continue,
            None => return Some(items),
            Some(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_values_keep_a_natural_form_where_faro_has_one() {
        assert_eq!(decode_text("INT4", "42"), Value::Int(42));
        assert_eq!(decode_text("BOOL", "t"), Value::Bool(true));
        assert_eq!(decode_text("FLOAT8", "1.5"), Value::Float(1.5));
        assert_eq!(
            decode_text("NUMERIC", "12.3400"),
            Value::Decimal("12.34".into())
        );
        assert_eq!(
            decode_text("BYTEA", r"\xdead"),
            Value::Bytes(vec![0xde, 0xad])
        );
        assert_eq!(
            decode_text("JSONB", r#"{"a": 1}"#),
            Value::Json(serde_json::json!({"a": 1}))
        );
        assert_eq!(
            decode_text("TIMESTAMPTZ", "2024-01-01 10:00:00+02"),
            Value::Timestamp("2024-01-01T08:00:00+00:00".into())
        );
    }

    #[test]
    fn types_without_one_keep_postgres_own_text_instead_of_being_unsupported() {
        for (t, text) in [
            ("mood", "busy"),
            ("INTERVAL", "1 year 2 mons 3 days 04:05:06.5"),
            ("INET", "10.0.0.1"),
            ("MONEY", "$12.34"),
            ("INT4RANGE", "[1,5)"),
            ("geometry", "0101000000000000000000F03F0000000000000040"),
        ] {
            assert_eq!(decode_text(t, text), Value::Text(text.into()), "{t}");
        }
        // A NaN numeric is not a plain number, so it is never spliced bare.
        assert_eq!(decode_text("NUMERIC", "NaN"), Value::Text("NaN".into()));
    }

    #[test]
    fn one_dimensional_arrays_become_arrays_of_their_elements() {
        assert_eq!(
            decode_text("BOOL[]", "{t,f,NULL}"),
            Value::Array(vec![Value::Bool(true), Value::Bool(false), Value::Null])
        );
        assert_eq!(
            decode_text("TEXT[]", r#"{plain,"with, comma","quote \" here","NULL"}"#),
            Value::Array(vec![
                Value::Text("plain".into()),
                Value::Text("with, comma".into()),
                Value::Text("quote \" here".into()),
                // Quoted, so the string NULL rather than SQL NULL.
                Value::Text("NULL".into()),
            ])
        );
        assert_eq!(decode_text("INT4[]", "{}"), Value::Array(vec![]));
        // More than one dimension stays as Postgres renders it.
        assert_eq!(
            decode_text("INT4[]", "{{1,2},{3,4}}"),
            Value::Text("{{1,2},{3,4}}".into())
        );
    }

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

    #[test]
    fn numeric_precision_strips_trailing_zeros() {
        let dec: bigdecimal::BigDecimal = "12345678901234567890.098765432100".parse().unwrap();
        assert_eq!(
            dec.normalized().to_plain_string(),
            "12345678901234567890.0987654321"
        );
    }
}
