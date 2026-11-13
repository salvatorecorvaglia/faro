use async_trait::async_trait;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::catalog::{cell_int, cell_text, group_columns};
use super::dialect::{self, Dialect};
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    is_plain_number, ColumnDetail, ColumnInfo, ConnectionConfig, ExecResult, GuardedStatement,
    IndexInfo, ResultSet, SchemaInfo, SslMode, TableColumns, TableDetail, TableInfo, TableKind,
    TableRef, Value,
};

pub struct ClickHouseDialect;

impl Dialect for ClickHouseDialect {
    /// ClickHouse applies C-style backslash escapes inside string literals and
    /// accepts backticks for identifiers, but `"` stays an identifier quote.
    fn lex_rules(&self) -> crate::sql::LexRules {
        crate::sql::LexRules::CLICKHOUSE
    }

    fn quote_ident(&self, ident: &str) -> String {
        dialect::quote_backtick(ident)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        dialect::paginate_limit_offset(sql, limit, offset)
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        // ClickHouse strings are byte strings; unhex is the portable way to
        // write arbitrary bytes as a literal.
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!("unhex('{hex}')")
    }

    /// ClickHouse applies C-style backslash escapes inside string literals, so
    /// doubling the quote alone leaves a trailing backslash able to escape the
    /// closing delimiter.
    fn quote_string(&self, s: &str) -> String {
        crate::model::quote_sql_string_backslash(s)
    }

    /// `String` is the native name; the `TEXT` alias exists but is not the one
    /// ClickHouse's own errors and docs use.
    fn text_cast_type(&self) -> &'static str {
        "String"
    }

    /// ClickHouse's `LIKE` takes no `ESCAPE` clause — it is a syntax error —
    /// but it already treats backslash as the escape character, which is the
    /// one Faro escapes patterns with, so dropping the clause loses nothing.
    fn supports_like_escape(&self) -> bool {
        false
    }

    /// ClickHouse has no transactions worth the name. Callers check this before
    /// assuming a batch can be rolled back.
    fn supports_transactions(&self) -> bool {
        false
    }

    /// ClickHouse databases are schemas, and `db.table` is its normal form.
    fn supports_schemas(&self) -> bool {
        true
    }
}

/// ClickHouse, over its HTTP interface.
///
/// No connection pool: HTTP requests are independent, and reqwest keeps its own
/// keep-alive pool underneath. That also means there is no session to preserve,
/// so the split between a query connection and a metadata connection that the
/// other drivers need does not apply here — concurrent requests cannot starve
/// each other.
pub struct ClickHouseDriver {
    http: reqwest::Client,
    url: String,
    user: String,
    password: String,
    database: String,
    /// Mirrors the connection setting; sent to ClickHouse on every request.
    read_only: bool,
    dialect: ClickHouseDialect,
}

impl ClickHouseDriver {
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let port = if config.port == 0 {
            match config.ssl_mode {
                SslMode::Disable => 8123,
                _ => 8443,
            }
        } else {
            config.port
        };
        let driver = Self::build(config, password, scheme_for(config.ssl_mode), port)?;
        // Proves the host, the scheme and the credentials in one round trip,
        // rather than surfacing any of them on the user's first query.
        driver.ping().await?;
        Ok(driver)
    }

    fn build(
        config: &ConnectionConfig,
        password: Option<&str>,
        scheme: &str,
        port: u16,
    ) -> Result<Self> {
        let builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(15));

        let http = builder
            .build()
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        Ok(Self {
            http,
            url: format!("{scheme}://{}:{port}/", config.host),
            user: config.username.clone(),
            password: password.unwrap_or_default().to_string(),
            database: if config.database.is_empty() {
                "default".to_string()
            } else {
                config.database.clone()
            },
            read_only: config.read_only,
            dialect: ClickHouseDialect,
        })
    }

    /// POST a statement and return the raw response body.
    async fn request(&self, sql: &str) -> Result<reqwest::Response> {
        // ClickHouse's own read-only mode, alongside Faro's guard. Level 2 rather
        // than 1: level 1 also forbids the settings sent on every request below,
        // which would make a read-only connection unable to run anything at all.
        let readonly = if self.read_only { "2" } else { "0" };

        let response = self
            .http
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .query(&[
                ("database", self.database.as_str()),
                ("readonly", readonly),
                // Read queries should stop server-side when their HTTP stream is
                // abandoned. This setting does not promise cancellation of writes.
                ("cancel_http_readonly_queries_on_client_close", "1"),
                // Both keep exact values out of JSON's float representation:
                // a Decimal or an Int64 near its limit would otherwise be
                // rounded on the way through.
                ("output_format_json_quote_64bit_integers", "1"),
                ("output_format_json_quote_decimals", "1"),
            ])
            .body(sql.to_string())
            .send()
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;

        Ok(response)
    }

    async fn post(&self, sql: &str) -> Result<String> {
        let response = self.request(sql).await?;
        let status = response.status();
        let body = read_response_limited(response, 1024 * 1024).await?;
        if !status.is_success() {
            return Err(parse_error(&body));
        }
        Ok(body)
    }

    /// Decode complete lines as chunks arrive. Never retain the whole body.
    async fn fetch(&self, sql: &str, limit: u64) -> Result<ResultSet> {
        let started = Instant::now();
        let mut response = self
            .request(&format!(
                "{sql}\nFORMAT JSONCompactEachRowWithNamesAndTypes"
            ))
            .await?;
        if !response.status().is_success() {
            return Err(parse_error(
                &read_response_limited(response, 1024 * 1024).await?,
            ));
        }
        let mut decoder = RowDecoder::new(limit);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?
        {
            if decoder.feed(&chunk)? {
                break;
            }
        }
        decoder.finish(started.elapsed().as_millis() as u64)
    }

    /// Catalog helper: run SQL and return rows, unlimited.
    async fn meta_rows(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        Ok(self.fetch(sql, u64::MAX).await?.rows)
    }

    fn literal(&self, text: &str) -> String {
        self.dialect.literal(&Value::Text(text.to_string()))
    }
}

async fn read_response_limited(mut response: reqwest::Response, max: usize) -> Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| FaroError::Connection(e.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(crate::budget::exceeded("The HTTP response"));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|e| FaroError::Other(e.to_string()))
}

struct RowDecoder {
    pending: Vec<u8>,
    names: Option<Vec<String>>,
    columns: Option<Vec<ColumnInfo>>,
    rows: Vec<Vec<Value>>,
    limit: u64,
    truncated: bool,
    budget: crate::budget::RowBudget,
}
impl RowDecoder {
    fn new(limit: u64) -> Self {
        Self {
            pending: Vec::new(),
            names: None,
            columns: None,
            rows: Vec::new(),
            limit,
            truncated: false,
            budget: Default::default(),
        }
    }
    fn line(&mut self, line: &[u8]) -> Result<bool> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(false);
        }
        if self.names.is_none() {
            self.names = Some(serde_json::from_slice(line)?);
            return Ok(false);
        }
        if self.columns.is_none() {
            let types: Vec<String> = serde_json::from_slice(line)?;
            let names = self.names.as_ref().unwrap();
            if names.len() != types.len() {
                return Err(FaroError::Other(
                    "ClickHouse returned mismatched column metadata".into(),
                ));
            }
            self.columns = Some(
                names
                    .iter()
                    .zip(types)
                    .map(|(name, type_name)| ColumnInfo {
                        name: name.clone(),
                        type_name,
                    })
                    .collect(),
            );
            self.budget.columns(self.columns.as_ref().unwrap())?;
            return Ok(false);
        }
        if self.rows.len() as u64 >= self.limit {
            self.truncated = true;
            return Ok(true);
        }
        let cells: Vec<serde_json::Value> = serde_json::from_slice(line)?;
        let row = self
            .columns
            .as_ref()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, column)| decode_value(cells.get(index), &column.type_name))
            .collect::<Vec<_>>();
        self.budget.push(&row)?;
        self.rows.push(row);
        Ok(false)
    }
    fn feed(&mut self, bytes: &[u8]) -> Result<bool> {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.columns.is_some()
                && self.rows.len() as u64 >= self.limit
                && part.iter().any(|byte| !byte.is_ascii_whitespace())
            {
                self.truncated = true;
                return Ok(true);
            }
            if self.pending.len().saturating_add(part.len()) > crate::budget::RESULT_BYTES {
                return Err(crate::budget::exceeded("A ClickHouse row"));
            }
            self.pending.extend_from_slice(part);
            if part.last() == Some(&b'\n') {
                let line = std::mem::take(&mut self.pending);
                if self.line(&line)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn finish(mut self, elapsed_ms: u64) -> Result<ResultSet> {
        if !self.pending.is_empty() && !self.truncated {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }
        if self.columns.is_none() {
            return Err(FaroError::Other(
                "ClickHouse ended the response before returning column metadata".into(),
            ));
        }
        Ok(ResultSet {
            columns: self.columns.unwrap_or_default(),
            rows: self.rows,
            truncated: self.truncated,
            elapsed_ms,
        })
    }
}

// ClickHouse cannot upgrade HTTP to HTTPS. A failed TLS handshake must not
// downgrade a later query or its Basic credentials to plaintext.
fn scheme_for(mode: SslMode) -> &'static str {
    match mode {
        SslMode::Disable => "http",
        _ => "https",
    }
}

/// Turn ClickHouse's plain-text error into a structured one.
///
/// Its messages look like `Code: 62. DB::Exception: ... (SYNTAX_ERROR)`, which is
/// genuinely informative, so the text is kept and only the code is lifted out.
fn parse_error(body: &str) -> FaroError {
    let trimmed = body.trim();
    let code = trimmed
        .strip_prefix("Code: ")
        .and_then(|rest| rest.split('.').next())
        .map(|c| c.trim().to_string());

    FaroError::Database {
        message: trimmed.to_string(),
        code,
    }
}

#[async_trait]
impl Driver for ClickHouseDriver {
    async fn managed_session(&self) -> Result<Box<dyn Driver>> {
        // HTTP requests have no persistent editor transaction to share.
        Ok(Box::new(Self {
            http: self.http.clone(),
            url: self.url.clone(),
            user: self.user.clone(),
            password: self.password.clone(),
            database: self.database.clone(),
            read_only: self.read_only,
            dialect: ClickHouseDialect,
        }))
    }

    async fn ping(&self) -> Result<()> {
        self.post("SELECT 1").await.map(|_| ())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = self
            .meta_rows("SELECT name FROM system.databases ORDER BY name")
            .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let name = cell_text(r.first());
                SchemaInfo {
                    // ClickHouse's own bookkeeping databases, which are rarely
                    // what someone opened the app to look at.
                    is_system: matches!(
                        name.as_str(),
                        "system" | "INFORMATION_SCHEMA" | "information_schema"
                    ),
                    name,
                }
            })
            .collect())
    }

    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let db = schema.unwrap_or(&self.database).to_string();
        let rows = self
            .meta_rows(&format!(
                "SELECT name, engine, total_rows FROM system.tables
                 WHERE database = {} ORDER BY name",
                self.literal(&db)
            ))
            .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let engine = cell_text(r.get(1));
                TableInfo {
                    schema: Some(db.clone()),
                    name: cell_text(r.first()),
                    // ClickHouse spells views as engine names rather than a
                    // separate object type.
                    kind: if engine.contains("View") {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                    // Exact for MergeTree tables and cheap to read, unlike the
                    // estimates the other engines expose.
                    estimated_rows: match r.get(2) {
                        Some(Value::Null) | None => None,
                        other => Some(cell_int(other)),
                    },
                }
            })
            .collect())
    }

    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let db = table
            .schema
            .clone()
            .unwrap_or_else(|| self.database.clone());
        let d_lit = self.literal(&db);
        let t_lit = self.literal(&table.name);

        let col_rows = self
            .meta_rows(&format!(
                "SELECT name, type, default_expression, position
                 FROM system.columns
                 WHERE database = {d_lit} AND table = {t_lit}
                 ORDER BY position"
            ))
            .await?;

        if col_rows.is_empty() {
            return Err(FaroError::Database {
                message: format!("table \"{}\" not found", table.name),
                code: None,
            });
        }

        let meta = self
            .meta_rows(&format!(
                "SELECT engine, sorting_key, primary_key FROM system.tables
                 WHERE database = {d_lit} AND name = {t_lit}"
            ))
            .await?;
        let engine = meta
            .first()
            .map(|r| cell_text(r.first()))
            .unwrap_or_default();
        let sorting_key = meta
            .first()
            .map(|r| cell_text(r.get(1)))
            .unwrap_or_default();

        // The sorting key is surfaced as an index so the user can see it, but
        // never as a primary key.
        let indexes = if sorting_key.is_empty() {
            Vec::new()
        } else {
            vec![IndexInfo {
                name: format!("{} sorting key", table.name),
                columns: sorting_key
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                is_unique: false,
                is_primary: false,
                // Part of the table's ENGINE clause, not a CREATE INDEX.
                is_constraint: true,
            }]
        };

        Ok(TableDetail {
            table: table.clone(),
            kind: if engine.contains("View") {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns: col_rows
                .iter()
                .map(|r| {
                    let type_name = cell_text(r.get(1));
                    let default = cell_text(r.get(2));
                    ColumnDetail {
                        name: cell_text(r.first()),
                        // Nullability is part of the type in ClickHouse
                        // (`Nullable(String)`), not a separate flag.
                        nullable: type_name.starts_with("Nullable("),
                        type_name,
                        default: (!default.is_empty()).then_some(default),
                        is_primary_key: false,
                        ordinal: cell_int(r.get(3)) as i32,
                    }
                })
                .collect(),
            // Deliberately empty, which makes the grid read-only.
            //
            // A MergeTree primary key is a sparse sorting index, not a unique
            // constraint — several rows can share one. There is therefore no
            // WHERE clause that provably matches exactly one row, and ClickHouse
            // mutations (`ALTER TABLE ... UPDATE`) are asynchronous and report no
            // row count. Offering row editing here would be offering something
            // Faro cannot make safe.
            primary_key: Vec::new(),
            // ClickHouse does not enforce foreign keys.
            foreign_keys: Vec::new(),
            indexes,
        })
    }

    async fn table_ddl(&self, table: &TableRef) -> Result<Option<String>> {
        let db = table
            .schema
            .clone()
            .unwrap_or_else(|| self.database.clone());
        let rows = self
            .meta_rows(&format!(
                "SELECT create_table_query FROM system.tables
                 WHERE database = {} AND name = {}",
                self.literal(&db),
                self.literal(&table.name)
            ))
            .await?;

        // ClickHouse keeps the full CREATE TABLE, including the ENGINE and
        // ORDER BY clauses that a column-by-column rebuild cannot express.
        Ok(rows.first().map(|r| {
            let sql = cell_text(r.first());
            format!("{};", sql.trim().trim_end_matches(';'))
        }))
    }

    async fn schema_snapshot(&self, schema: Option<&str>) -> Result<Vec<TableColumns>> {
        let db = schema.unwrap_or(&self.database).to_string();
        let rows = self
            .meta_rows(&format!(
                "SELECT table, name FROM system.columns
                 WHERE database = {} ORDER BY table, position",
                self.literal(&db)
            ))
            .await?;

        Ok(group_columns(
            rows.iter()
                .map(|r| (cell_text(r.first()), cell_text(r.get(1)))),
            Some(db),
        ))
    }

    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        // `fetch` already stops one row past the limit, so the wrapping only
        // keeps the surplus rows on the server. Statements that cannot be a
        // derived table — `SHOW`, `DESCRIBE`, `EXPLAIN` — are run as written.
        let paged = if super::is_wrappable(sql) {
            self.dialect
                .paginate(sql, crate::driver::probe_limit(limit), 0)
        } else {
            sql.trim().trim_end_matches(';').to_string()
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(FaroError::Cancelled),
            res = self.fetch(&paged, limit) => res,
        }
    }

    async fn execute(&self, sql: &str, cancel: CancellationToken) -> Result<ExecResult> {
        let started = Instant::now();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FaroError::Cancelled),
            res = self.post(sql) => { res?; }
        }
        Ok(ExecResult {
            // ClickHouse's HTTP interface reports no affected-row count for
            // writes, and inventing one would be worse than admitting zero.
            rows_affected: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Apply statements one after another. **Not atomic.**
    ///
    /// ClickHouse has no usable transactions, so a failure partway through
    /// leaves the earlier statements applied. That cannot be hidden, so the
    /// error says so explicitly and names how many went through — the caller and
    /// the user both need to know the data is in a partial state.
    ///
    /// In practice only import reaches this: the grid stays read-only because
    /// `describe_table` reports no primary key.
    async fn apply_transaction(&self, statements: &[GuardedStatement]) -> Result<u64> {
        for (index, stmt) in statements.iter().enumerate() {
            if let Err(e) = self.post(&stmt.sql).await {
                if index == 0 {
                    return Err(e);
                }
                return Err(FaroError::Other(format!(
                    "{e}\n\nClickHouse has no transactions, so this could not be rolled back: \
                     {index} of {} statements were already applied and remain in place.",
                    statements.len()
                )));
            }
        }
        Ok(0)
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // Nothing to close: reqwest's pool is dropped with the client.
    }
}

/// Decode one JSON cell, guided by ClickHouse's declared type.
///
/// Numbers arrive as JSON strings when they are 64-bit integers or decimals —
/// that is what the quoting settings on every request buy — so those are kept as
/// text rather than parsed into a float.
fn decode_value(cell: Option<&serde_json::Value>, type_name: &str) -> Value {
    use serde_json::Value as J;

    let Some(cell) = cell else {
        return Value::Null;
    };
    // `Nullable(T)` wraps the real type; unwrap it before matching.
    let inner = type_name
        .strip_prefix("Nullable(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(type_name);

    match cell {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => {
            if inner.starts_with("Float") {
                n.as_f64().map(Value::Float).unwrap_or(Value::Null)
            } else if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                // Wider than i64, so keep the digits rather than rounding.
                Value::Decimal(n.to_string())
            }
        }
        J::String(s) => decode_string(s, inner),
        // Arrays, tuples, maps and nested types come back as JSON structures.
        other => Value::Json(other.clone()),
    }
}

/// Interpret a JSON string according to the column's ClickHouse type.
///
/// `Value::Decimal` is rendered into generated SQL verbatim, unescaped, on the
/// premise that a decimal is always just digits — see `Value::to_sql_literal`.
/// That premise holds for a value ClickHouse actually produced, but this text
/// comes straight from the HTTP response body: a compromised server could
/// return anything here. Checked before trusting it as a bare literal;
/// anything else falls back to `Value::Text`, which is quoted and escaped.
fn decode_string(s: &str, inner: &str) -> Value {
    if inner.starts_with("Decimal") {
        return if is_plain_number(s) {
            Value::Decimal(s.to_string())
        } else {
            Value::Text(s.to_string())
        };
    }
    if inner.starts_with("Int") || inner.starts_with("UInt") {
        // Quoted because it is 64-bit or wider. Parse when it fits, keep the
        // exact digits when it does not — UInt64's maximum would otherwise wrap
        // to -1.
        return match s.parse::<i64>() {
            Ok(n) => Value::Int(n),
            Err(_) if is_plain_number(s) => Value::Decimal(s.to_string()),
            Err(_) => Value::Text(s.to_string()),
        };
    }
    if inner.starts_with("Date32") || inner == "Date" {
        return Value::Date(s.to_string());
    }
    if inner.starts_with("DateTime") {
        return Value::Timestamp(s.to_string());
    }
    if inner == "UUID" {
        return Value::Uuid(s.to_string());
    }
    if inner.starts_with("Bool") {
        // Some versions render Bool as text.
        return match s {
            "true" | "1" => Value::Bool(true),
            "false" | "0" => Value::Bool(false),
            _ => Value::Text(s.to_string()),
        };
    }
    Value::Text(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_rows_survive_chunk_boundaries_inside_utf8() {
        let bytes = "[\"name\"]\n[\"String\"]\n[\"café\"]\n".as_bytes();
        let mut decoder = RowDecoder::new(10);
        for byte in bytes {
            assert!(!decoder.feed(&[*byte]).unwrap());
        }
        let result = decoder.finish(1).unwrap();
        assert_eq!(result.rows, vec![vec![Value::Text("café".into())]]);
        assert!(!result.truncated);
    }

    #[test]
    fn capped_stream_does_not_decode_or_buffer_the_remaining_body() {
        let mut decoder = RowDecoder::new(1);
        assert!(!decoder.feed(b"[\"id\"]\n[\"Int32\"]\n[1]\n").unwrap());
        assert!(decoder
            .feed(b"the rest does not need to be valid JSON")
            .unwrap());
        let result = decoder.finish(0).unwrap();
        assert!(result.truncated);
        assert_eq!(result.rows, vec![vec![Value::Int(1)]]);
    }

    #[test]
    fn incomplete_stream_metadata_is_an_error() {
        let mut decoder = RowDecoder::new(10);
        decoder.feed(b"[\"id\"]\n").unwrap();
        assert!(decoder.finish(0).is_err());
    }

    #[test]
    fn only_explicit_disable_uses_plain_http() {
        assert_eq!(scheme_for(SslMode::Disable), "http");
        for mode in [
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            assert_eq!(scheme_for(mode), "https");
        }
    }

    #[test]
    fn identifiers_use_backticks_and_escape_them() {
        assert_eq!(ClickHouseDialect.quote_ident("my table"), "`my table`");
        assert_eq!(ClickHouseDialect.quote_ident("we`ird"), "`we``ird`");
    }

    #[test]
    fn like_takes_no_escape_clause() {
        // `LIKE '...' ESCAPE '\\'` is a hard syntax error here, so every text
        // filter would fail if this ever went back to the default.
        assert!(!ClickHouseDialect.supports_like_escape());
    }

    #[test]
    fn transactions_are_reported_as_unsupported() {
        // Callers must be able to tell that a batch here cannot be rolled back.
        assert!(!ClickHouseDialect.supports_transactions());
    }

    #[test]
    fn databases_are_schemas() {
        assert!(ClickHouseDialect.supports_schemas());
        assert_eq!(
            ClickHouseDialect.qualify(Some("app"), "events"),
            "`app`.`events`"
        );
    }

    #[test]
    fn byte_literals_use_unhex() {
        // X'..' and 0x.. are both syntax errors here.
        assert_eq!(
            ClickHouseDialect.quote_bytes(&[0xde, 0xad]),
            "unhex('dead')"
        );
    }

    #[test]
    fn errors_keep_the_message_and_lift_the_code() {
        let raw = "Code: 62. DB::Exception: Syntax error: failed at position 18. (SYNTAX_ERROR)";
        match parse_error(raw) {
            FaroError::Database { message, code } => {
                assert_eq!(code.as_deref(), Some("62"));
                assert!(message.contains("Syntax error"), "{message}");
            }
            other => panic!("expected a database error, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecognized_error_still_carries_its_text() {
        match parse_error("something went wrong") {
            FaroError::Database { message, code } => {
                assert_eq!(code, None);
                assert_eq!(message, "something went wrong");
            }
            other => panic!("expected a database error, got {other:?}"),
        }
    }

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn quoted_wide_integers_stay_exact() {
        // UInt64's maximum does not fit an i64; parsing it would wrap to -1.
        let v = decode_value(Some(&json(r#""18446744073709551615""#)), "UInt64");
        assert_eq!(v, Value::Decimal("18446744073709551615".into()));

        // One that does fit comes back as an integer.
        let v = decode_value(Some(&json(r#""42""#)), "Int64");
        assert_eq!(v, Value::Int(42));
    }

    #[test]
    fn decimals_keep_every_digit() {
        let v = decode_value(
            Some(&json(r#""12345678901234567890.0987654321""#)),
            "Decimal(38, 10)",
        );
        assert_eq!(v, Value::Decimal("12345678901234567890.0987654321".into()));
    }

    #[test]
    fn a_non_numeric_decimal_string_falls_back_to_text_instead_of_an_unescaped_literal() {
        // Value::Decimal is spliced into generated SQL unescaped, on the
        // premise that it is always bare digits. This string came straight
        // off the wire — a compromised or MITM'd server could send anything
        // — so anything that is not actually numeric must not be trusted as
        // one, or it becomes a live SQL injection in exports/backups.
        let v = decode_value(Some(&json(r#""1'); DROP TABLE t; --""#)), "Decimal(38, 10)");
        assert_eq!(v, Value::Text("1'); DROP TABLE t; --".into()));

        // Same for the wider-than-i64 integer fallback path.
        let v = decode_value(Some(&json(r#""18446744073709551615; --""#)), "UInt64");
        assert_eq!(v, Value::Text("18446744073709551615; --".into()));
    }

    #[test]
    fn only_bare_signed_decimals_count_as_numbers() {
        for ok in [
            "0",
            "42",
            "-42",
            "1.5",
            "-1.5",
            "12345678901234567890.0987654321",
        ] {
            assert!(is_plain_number(ok), "{ok} should be accepted");
        }
        for bad in ["", "-", ".", "1.2.3", "1e10", "nan", "1 OR 1=1", "1'; --"] {
            assert!(!is_plain_number(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn nullable_types_are_unwrapped_before_matching() {
        // Without unwrapping, "Nullable(Decimal(38, 10))" would fall through to
        // text and lose its decimal identity.
        let v = decode_value(Some(&json(r#""1.50""#)), "Nullable(Decimal(38, 2))");
        assert_eq!(v, Value::Decimal("1.50".into()));

        let v = decode_value(Some(&json("null")), "Nullable(String)");
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn temporal_and_uuid_types_are_recognized() {
        assert_eq!(
            decode_value(Some(&json(r#""2026-08-05""#)), "Date"),
            Value::Date("2026-08-05".into())
        );
        assert_eq!(
            decode_value(Some(&json(r#""2026-08-05 13:45:00""#)), "DateTime"),
            Value::Timestamp("2026-08-05 13:45:00".into())
        );
        assert!(matches!(
            decode_value(
                Some(&json(r#""0192e8f0-0000-0000-0000-000000000000""#)),
                "UUID"
            ),
            Value::Uuid(_)
        ));
    }

    #[test]
    fn floats_stay_floats_and_small_ints_stay_ints() {
        assert_eq!(
            decode_value(Some(&json("1.5")), "Float64"),
            Value::Float(1.5)
        );
        assert_eq!(decode_value(Some(&json("7")), "UInt8"), Value::Int(7));
    }

    #[test]
    fn composite_types_are_carried_as_json() {
        assert!(matches!(
            decode_value(Some(&json("[1,2,3]")), "Array(UInt8)"),
            Value::Json(_)
        ));
    }

    #[test]
    fn a_missing_cell_is_null_rather_than_a_panic() {
        assert_eq!(decode_value(None, "String"), Value::Null);
    }
}
