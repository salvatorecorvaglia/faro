//! MongoDB.
//!
//! MongoDB is not a SQL database, but most of Faro's surface still applies: a
//! collection browses like a table, a document renders like a row, and export,
//! history, saved queries and the palette all work unchanged. Only two things
//! genuinely differ, and both are handled here rather than by duplicating the
//! app:
//!
//! - The query language. Faro accepts a `mongosh`-style call —
//!   `db.collection.find({...})` — parsed below.
//! - Browsing. `Driver::browse` is overridden so a page is a `find`, not a
//!   composed `SELECT`.
//!
//! Documents are schemaless, so `describe_table` samples documents and reports
//! the union of their top-level fields. That is a real inference, not a schema,
//! and it is labelled as such.

use async_trait::async_trait;
use bson::{Bson, Document};
use futures::TryStreamExt;
use mongodb::options::ClientOptions;
use mongodb::Client;
use std::future::IntoFuture;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

use super::dialect::Dialect;
use super::Driver;
use crate::error::{FaroError, Result};
use crate::model::{
    BrowseOptions, ColumnDetail, ColumnInfo, ConnectionConfig, ExecResult, FilterOp,
    GuardedStatement, ResultSet, SchemaInfo, TableColumns, TableDetail, TableInfo, TableKind,
    TableRef, Value,
};

/// How many documents to sample when inferring a collection's fields.
const SAMPLE_SIZE: i64 = 100;

pub struct MongoDialect;

impl Dialect for MongoDialect {
    /// MongoDB has no identifier quoting; names are strings in a document.
    /// Returned unchanged so any accidental use is visible rather than silently
    /// producing SQL-looking text.
    fn quote_ident(&self, ident: &str) -> String {
        ident.to_string()
    }

    /// Never used: `browse` is overridden and there is no SQL to paginate.
    fn paginate(&self, sql: &str, _limit: u64, _offset: u64) -> String {
        sql.to_string()
    }

    fn quote_bytes(&self, bytes: &[u8]) -> String {
        // Extended JSON's binary form, which is what a Mongo query would use.
        format!(
            "{{\"$binary\":{{\"base64\":\"{}\",\"subType\":\"00\"}}}}",
            base64_encode(bytes)
        )
    }

    /// Multi-document transactions exist but need a replica set; a standalone
    /// server rejects them. Faro does not assume one.
    fn supports_transactions(&self) -> bool {
        false
    }

    /// Databases are the namespace, and `db.collection` is the natural form.
    fn supports_schemas(&self) -> bool {
        true
    }

    /// The whole point of this flag: the editor, formatter and backup all need
    /// to know the query language is not SQL.
    fn is_sql(&self) -> bool {
        false
    }
}

/// Minimal base64, to avoid a dependency for one rarely-hit code path.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub struct MongoDriver {
    client: Client,
    database: String,
    dialect: MongoDialect,
}

impl MongoDriver {
    pub async fn connect(config: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let port = if config.port == 0 { 27017 } else { config.port };

        // Built as a URI so the driver handles escaping and option parsing, and
        // so a user could later paste a full connection string.
        let credentials = if config.username.is_empty() {
            String::new()
        } else {
            format!(
                "{}:{}@",
                urlencode(&config.username),
                urlencode(password.unwrap_or_default())
            )
        };
        let uri = format!("mongodb://{credentials}{}:{port}/", config.host);

        let mut options = ClientOptions::parse(&uri)
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;
        options.connect_timeout = Some(std::time::Duration::from_secs(15));
        options.server_selection_timeout = Some(std::time::Duration::from_secs(15));
        options.app_name = Some("Faro".to_string());

        let client =
            Client::with_options(options).map_err(|e| FaroError::Connection(e.to_string()))?;

        let driver = Self {
            client,
            database: if config.database.is_empty() {
                "test".to_string()
            } else {
                config.database.clone()
            },
            dialect: MongoDialect,
        };

        // Prove the credentials now rather than on the first query: the Mongo
        // client connects lazily.
        driver.ping().await?;
        Ok(driver)
    }

    fn db(&self, name: Option<&str>) -> mongodb::Database {
        self.client.database(name.unwrap_or(&self.database))
    }

    /// Sample documents from a collection.
    async fn sample(&self, table: &TableRef, limit: i64) -> Result<Vec<Document>> {
        let collection = self
            .db(table.schema.as_deref())
            .collection::<Document>(&table.name);

        let cursor = collection
            .find(Document::new())
            .limit(limit)
            .await
            .map_err(map_err)?;
        cursor.try_collect().await.map_err(map_err)
    }

    /// Turn documents into a `ResultSet` whose columns are the union of their
    /// top-level fields.
    ///
    /// `_id` is forced first because it is the one field every document has and
    /// the one a reader looks for. Remaining fields keep first-seen order, so a
    /// collection with consistent documents renders in its natural shape.
    fn to_result_set(documents: &[Document], elapsed_ms: u64, truncated: bool) -> ResultSet {
        let mut names: Vec<String> = Vec::new();
        if documents.iter().any(|d| d.contains_key("_id")) {
            names.push("_id".to_string());
        }
        for doc in documents {
            for key in doc.keys() {
                if !names.iter().any(|n| n == key) {
                    names.push(key.clone());
                }
            }
        }

        let columns: Vec<ColumnInfo> = names
            .iter()
            .map(|name| ColumnInfo {
                name: name.clone(),
                // The BSON type of the first document that has this field.
                // Documents need not agree, which is why it is a hint.
                type_name: documents
                    .iter()
                    .find_map(|d| d.get(name).map(bson_type_name))
                    .unwrap_or("missing")
                    .to_string(),
            })
            .collect();

        let rows = documents
            .iter()
            .map(|doc| {
                names
                    .iter()
                    .map(|name| doc.get(name).map(decode_bson).unwrap_or(Value::Null))
                    .collect()
            })
            .collect();

        ResultSet {
            columns,
            rows,
            truncated,
            elapsed_ms,
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn map_err(e: mongodb::error::Error) -> FaroError {
    FaroError::Database {
        message: e.to_string(),
        code: None,
    }
}

/// A parsed `db.collection.operation(args...)` call.
#[derive(Debug, PartialEq)]
pub struct MongoQuery {
    pub collection: String,
    pub operation: String,
    pub args: Vec<serde_json::Value>,
}

/// Parse a `mongosh`-style call.
///
/// Deliberately a small, strict parser rather than a JavaScript engine: Faro
/// needs to read queries, not run arbitrary scripts, and a real JS runtime would
/// be a large dependency and a much larger attack surface for something typed
/// into an editor.
///
/// Accepts `db.<collection>.<operation>(<json>, <json>, ...)`, where each
/// argument is JSON or MongoDB Extended JSON.
pub fn parse_query(input: &str) -> Result<MongoQuery> {
    let trimmed = input.trim().trim_end_matches(';').trim();

    let rest = trimmed.strip_prefix("db.").ok_or_else(|| {
        FaroError::Sql(
            "expected a query of the form db.collection.find({ ... }) — MongoDB does not use SQL"
                .into(),
        )
    })?;

    let open = rest
        .find('(')
        .ok_or_else(|| FaroError::Sql("missing '(' — try db.collection.find({})".into()))?;
    if !rest.trim_end().ends_with(')') {
        return Err(FaroError::Sql("missing the closing ')'".into()));
    }

    let path = &rest[..open];
    let (collection, operation) = path.rsplit_once('.').ok_or_else(|| {
        FaroError::Sql(format!(
            "expected db.collection.operation(...), but found db.{path}(...)"
        ))
    })?;

    if collection.is_empty() {
        return Err(FaroError::Sql("no collection named in the query".into()));
    }

    let inner = rest[open + 1..rest.trim_end().len() - 1].trim();
    let args = split_json_args(inner)?;

    Ok(MongoQuery {
        collection: collection.trim().to_string(),
        operation: operation.trim().to_string(),
        args,
    })
}

/// Split an argument list on top-level commas, then parse each as JSON.
///
/// Tracks brace, bracket and string nesting so a comma inside a document or a
/// string is not mistaken for an argument separator.
fn split_json_args(inner: &str) -> Result<Vec<serde_json::Value>> {
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for c in inner.chars() {
        if in_string {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                current.push(c);
            }
            '{' | '[' => {
                depth += 1;
                current.push(c);
            }
            '}' | ']' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }

    parts
        .iter()
        .map(|p| {
            serde_json::from_str(p.trim()).map_err(|e| {
                FaroError::Sql(format!("could not read the argument `{}`: {e}", p.trim()))
            })
        })
        .collect()
}

/// Convert a JSON argument into a BSON document, honouring Extended JSON.
fn to_document(value: &serde_json::Value) -> Result<Document> {
    if value.is_null() {
        return Ok(Document::new());
    }
    // Bson::try_from understands Extended JSON, so {"$oid": "..."} becomes a
    // real ObjectId rather than a nested document that would never match.
    match Bson::try_from(value.clone()).map_err(|e| FaroError::Sql(e.to_string()))? {
        Bson::Document(d) => Ok(d),
        other => Err(FaroError::Sql(format!(
            "expected a document like {{ ... }}, found {}",
            bson_type_name(&other)
        ))),
    }
}

#[async_trait]
impl Driver for MongoDriver {
    async fn ping(&self) -> Result<()> {
        self.db(None)
            .run_command(bson::doc! { "ping": 1 })
            .await
            .map_err(|e| FaroError::Connection(e.to_string()))?;
        Ok(())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>> {
        let names = self.client.list_database_names().await.map_err(map_err)?;

        Ok(names
            .into_iter()
            .map(|name| SchemaInfo {
                // MongoDB's own bookkeeping databases.
                is_system: matches!(name.as_str(), "admin" | "config" | "local"),
                name,
            })
            .collect())
    }

    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let db = self.db(schema);
        let names = db.list_collection_names().await.map_err(map_err)?;
        let database = schema.unwrap_or(&self.database).to_string();

        let mut out = Vec::with_capacity(names.len());
        for name in names {
            // estimated_document_count reads collection metadata rather than
            // scanning, which is what makes it cheap enough for a tree.
            let estimated = db
                .collection::<Document>(&name)
                .estimated_document_count()
                .await
                .ok()
                .map(|n| n as i64);

            out.push(TableInfo {
                schema: Some(database.clone()),
                name,
                kind: TableKind::Table,
                estimated_rows: estimated,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Infer a collection's shape by sampling documents.
    ///
    /// MongoDB collections have no declared schema, so this is genuinely an
    /// inference over `SAMPLE_SIZE` documents: a field absent from every sampled
    /// document will not appear. The type name records which BSON types were
    /// actually seen, so a field holding both strings and numbers says so
    /// instead of picking one and being wrong half the time.
    async fn describe_table(&self, table: &TableRef) -> Result<TableDetail> {
        let documents = self.sample(table, SAMPLE_SIZE).await?;

        if documents.is_empty() {
            // An empty collection is legitimate, not an error — it just has no
            // fields to report yet.
            return Ok(TableDetail {
                table: table.clone(),
                kind: TableKind::Table,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            });
        }

        // Field order: `_id` first, then first-seen.
        let mut names: Vec<String> = vec!["_id".to_string()];
        for doc in &documents {
            for key in doc.keys() {
                if !names.iter().any(|n| n == key) {
                    names.push(key.clone());
                }
            }
        }

        let total = documents.len();
        let columns = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let mut seen_types: Vec<&str> = Vec::new();
                let mut present = 0usize;
                for doc in &documents {
                    if let Some(v) = doc.get(name) {
                        present += 1;
                        let t = bson_type_name(v);
                        if !seen_types.contains(&t) {
                            seen_types.push(t);
                        }
                    }
                }

                ColumnDetail {
                    name: name.clone(),
                    // Several types means the field is genuinely polymorphic;
                    // showing all of them is more honest than choosing one.
                    type_name: if seen_types.is_empty() {
                        "missing".to_string()
                    } else {
                        seen_types.join(" | ")
                    },
                    // Absent from some sampled documents counts as nullable —
                    // reading such a field yields no value.
                    nullable: present < total || seen_types.contains(&"null"),
                    default: None,
                    is_primary_key: false,
                    ordinal: i as i32,
                }
            })
            .collect();

        // Indexes are real in MongoDB and worth showing.
        let indexes = match self
            .db(table.schema.as_deref())
            .collection::<Document>(&table.name)
            .list_indexes()
            .await
        {
            Ok(cursor) => {
                let models: Vec<_> = cursor.try_collect().await.unwrap_or_default();
                models
                    .iter()
                    .map(|m| {
                        let name = m
                            .options
                            .as_ref()
                            .and_then(|o| o.name.clone())
                            .unwrap_or_else(|| "index".to_string());
                        let unique = m.options.as_ref().and_then(|o| o.unique).unwrap_or(false);
                        crate::model::IndexInfo {
                            columns: m.keys.keys().map(String::from).collect(),
                            // The _id index is created implicitly and cannot be
                            // recreated by hand.
                            is_primary: name == "_id_",
                            is_constraint: name == "_id_",
                            is_unique: unique || name == "_id_",
                            name,
                        }
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        };

        Ok(TableDetail {
            table: table.clone(),
            kind: TableKind::Table,
            columns,
            // Deliberately empty, which makes the grid read-only.
            //
            // `_id` genuinely is unique, so editing is possible in principle —
            // but the edit path generates SQL `UPDATE` statements, which mean
            // nothing here. Reporting no key keeps the grid honest until a
            // document-shaped edit path exists, rather than offering a button
            // that would send SQL to a document store.
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
            indexes,
        })
    }

    async fn schema_snapshot(&self, schema: Option<&str>) -> Result<Vec<TableColumns>> {
        let tables = self.list_tables(schema).await?;
        let mut out = Vec::with_capacity(tables.len());

        for t in tables {
            let table = TableRef {
                schema: t.schema.clone(),
                name: t.name.clone(),
            };
            // A smaller sample than describe_table: this feeds autocomplete,
            // where breadth across collections matters more than completeness
            // within one.
            let documents = self.sample(&table, 20).await.unwrap_or_default();

            let mut columns: Vec<String> = Vec::new();
            for doc in &documents {
                for key in doc.keys() {
                    if !columns.iter().any(|c| c == key) {
                        columns.push(key.clone());
                    }
                }
            }
            out.push(TableColumns {
                schema: t.schema,
                name: t.name,
                columns,
            });
        }
        Ok(out)
    }

    async fn query(&self, sql: &str, limit: u64, cancel: CancellationToken) -> Result<ResultSet> {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }

        let parsed = parse_query(sql)?;
        let started = Instant::now();
        let collection = self.db(None).collection::<Document>(&parsed.collection);

        // One extra document reveals truncation without a count.
        let fetch = limit.saturating_add(1) as i64;

        let documents: Vec<Document> = match parsed.operation.as_str() {
            "find" | "findOne" => {
                let filter = parsed
                    .args
                    .first()
                    .map(to_document)
                    .transpose()?
                    .unwrap_or_default();

                let mut find = collection.find(filter);
                if let Some(projection) = parsed.args.get(1) {
                    find = find.projection(to_document(projection)?);
                }
                find = find.limit(if parsed.operation == "findOne" {
                    1
                } else {
                    fetch
                });

                let cursor = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(FaroError::Cancelled),
                    res = find.into_future() => res.map_err(map_err)?,
                };
                cursor.try_collect().await.map_err(map_err)?
            }
            "aggregate" => {
                let stages: Vec<Document> = match parsed.args.first() {
                    Some(serde_json::Value::Array(items)) => {
                        items.iter().map(to_document).collect::<Result<_>>()?
                    }
                    Some(other) => vec![to_document(other)?],
                    None => Vec::new(),
                };
                let cursor = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(FaroError::Cancelled),
                    res = collection.aggregate(stages).into_future() => res.map_err(map_err)?,
                };
                let all: Vec<Document> = cursor.try_collect().await.map_err(map_err)?;
                all.into_iter().take(fetch as usize).collect()
            }
            "countDocuments" | "count" => {
                let filter = parsed
                    .args
                    .first()
                    .map(to_document)
                    .transpose()?
                    .unwrap_or_default();
                let count = collection.count_documents(filter).await.map_err(map_err)?;
                vec![bson::doc! { "count": count as i64 }]
            }
            "distinct" => {
                let field = match parsed.args.first() {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    _ => {
                        return Err(FaroError::Sql(
                            "distinct needs a field name, e.g. db.c.distinct(\"status\")".into(),
                        ))
                    }
                };
                let filter = parsed
                    .args
                    .get(1)
                    .map(to_document)
                    .transpose()?
                    .unwrap_or_default();
                let values = collection.distinct(&field, filter).await.map_err(map_err)?;
                values
                    .into_iter()
                    .map(|v| bson::doc! { field.clone(): v })
                    .collect()
            }
            other => {
                return Err(FaroError::Sql(format!(
                    "`{other}` is not supported. Faro reads MongoDB with find, findOne, \
                     aggregate, countDocuments and distinct."
                )))
            }
        };

        let truncated = documents.len() as u64 > limit;
        let shown: Vec<Document> = documents.into_iter().take(limit as usize).collect();

        Ok(Self::to_result_set(
            &shown,
            started.elapsed().as_millis() as u64,
            truncated,
        ))
    }

    async fn execute(&self, sql: &str, _cancel: CancellationToken) -> Result<ExecResult> {
        // Reads are what Faro supports here. Silently accepting a write and
        // reporting zero affected documents would be worse than declining.
        Err(FaroError::Sql(format!(
            "Faro's MongoDB support is read-only. `{}` would modify data; use find, \
             aggregate, countDocuments or distinct instead.",
            sql.trim().chars().take(40).collect::<String>()
        )))
    }

    /// Browse a collection as a `find`, honouring sort and filters natively.
    async fn browse(
        &self,
        table: &TableRef,
        options: &BrowseOptions,
        cancel: CancellationToken,
    ) -> Result<ResultSet> {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        let started = Instant::now();
        let limit = options.limit.unwrap_or(1000);

        // Grid filters become a query document. Their semantics map cleanly:
        // MongoDB has native regex, comparison and existence operators.
        let mut filter = Document::new();
        for f in &options.filters {
            let condition = match f.op {
                FilterOp::Equals => Bson::String(f.value.clone()),
                FilterOp::NotEquals => Bson::Document(bson::doc! { "$ne": &f.value }),
                FilterOp::GreaterThan => Bson::Document(bson::doc! { "$gt": &f.value }),
                FilterOp::LessThan => Bson::Document(bson::doc! { "$lt": &f.value }),
                // Regex-escaped so a filter for "a.b" does not match "axb".
                FilterOp::Contains => Bson::Document(bson::doc! {
                    "$regex": regex_escape(&f.value), "$options": "i"
                }),
                FilterOp::StartsWith => Bson::Document(bson::doc! {
                    "$regex": format!("^{}", regex_escape(&f.value)), "$options": "i"
                }),
                // `$type: "null"` would match only an explicit null; a missing
                // field is the other half of what a user means by "is null".
                FilterOp::IsNull => Bson::Document(bson::doc! { "$eq": Bson::Null }),
                FilterOp::IsNotNull => Bson::Document(bson::doc! { "$ne": Bson::Null }),
            };
            filter.insert(f.column.clone(), condition);
        }

        let collection = self
            .db(table.schema.as_deref())
            .collection::<Document>(&table.name);

        let mut find = collection
            .find(filter)
            .skip(options.offset)
            .limit(limit.saturating_add(1) as i64);

        if let Some(column) = options.sort_column.as_deref() {
            find = find.sort(bson::doc! { column: if options.sort_desc { -1 } else { 1 } });
        }

        let cursor = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(FaroError::Cancelled),
            res = find.into_future() => res.map_err(map_err)?,
        };
        let documents: Vec<Document> = cursor.try_collect().await.map_err(map_err)?;

        let truncated = documents.len() as u64 > limit;
        let shown: Vec<Document> = documents.into_iter().take(limit as usize).collect();

        Ok(Self::to_result_set(
            &shown,
            started.elapsed().as_millis() as u64,
            truncated,
        ))
    }

    async fn apply_transaction(&self, _statements: &[GuardedStatement]) -> Result<u64> {
        Err(FaroError::Sql(
            "Faro's MongoDB support is read-only, so there is nothing to apply.".into(),
        ))
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    async fn close(&self) {
        // The Mongo client shuts its pool down when dropped.
    }
}

/// Escape regex metacharacters so a substring filter matches literally.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// BSON's name for a value's type, shown in the grid header.
fn bson_type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::JavaScriptCode(_) | Bson::JavaScriptCodeWithScope(_) => "javascript",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::Timestamp(_) => "timestamp",
        Bson::Binary(_) => "binary",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Symbol(_) => "symbol",
        Bson::Decimal128(_) => "decimal128",
        Bson::Undefined => "undefined",
        Bson::MaxKey => "maxKey",
        Bson::MinKey => "minKey",
        Bson::DbPointer(_) => "dbPointer",
    }
}

/// Decode BSON into the engine-agnostic `Value`.
pub fn decode_bson(value: &Bson) -> Value {
    match value {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => Value::Int(*i as i64),
        Bson::Int64(i) => Value::Int(*i),
        Bson::Double(d) => Value::Float(*d),
        // Decimal128 is an exact type; rendering it as text keeps it that way
        // rather than pushing it through f64.
        Bson::Decimal128(d) => Value::Decimal(d.to_string()),
        Bson::String(s) => Value::Text(s.clone()),
        // An ObjectId is the closest thing a document has to a primary key, so
        // it is shown as its hex form rather than as a nested object.
        Bson::ObjectId(id) => Value::Text(id.to_hex()),
        Bson::DateTime(dt) => Value::Timestamp(
            dt.try_to_rfc3339_string()
                .unwrap_or_else(|_| dt.to_string()),
        ),
        Bson::Binary(b) => Value::Bytes(b.bytes.clone()),
        // Nested documents and arrays keep their structure, which the grid shows
        // collapsed and the JSON view and cell inspector show in full.
        Bson::Document(d) => Value::Json(bson_to_json(&Bson::Document(d.clone()))),
        Bson::Array(a) => Value::Json(bson_to_json(&Bson::Array(a.clone()))),
        other => Value::Unsupported(bson_type_name(other).to_string()),
    }
}

/// Render BSON as JSON for display.
///
/// Uses the relaxed Extended JSON form, which is what a human reading a document
/// expects to see — `{"$oid": "..."}` rather than a byte array.
fn bson_to_json(value: &Bson) -> serde_json::Value {
    value.clone().into_relaxed_extjson()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mongo_reports_that_it_is_not_sql() {
        // This flag is what makes the editor, formatter and backup behave.
        assert!(!MongoDialect.is_sql());
        assert!(!MongoDialect.supports_transactions());
    }

    #[test]
    fn parses_a_simple_find() {
        let q = parse_query("db.authors.find({})").unwrap();
        assert_eq!(q.collection, "authors");
        assert_eq!(q.operation, "find");
        assert_eq!(q.args.len(), 1);
    }

    #[test]
    fn parses_a_find_with_a_filter_and_projection() {
        let q = parse_query(r#"db.books.find({"price": {"$gt": 20}}, {"title": 1})"#).unwrap();
        assert_eq!(q.collection, "books");
        assert_eq!(q.args.len(), 2);
        // The comma inside the filter document must not split the arguments.
        assert!(q.args[0].get("price").is_some());
        assert!(q.args[1].get("title").is_some());
    }

    #[test]
    fn a_comma_inside_a_string_does_not_split_arguments() {
        let q = parse_query(r#"db.c.find({"name": "Smith, John"})"#).unwrap();
        assert_eq!(q.args.len(), 1);
        assert_eq!(q.args[0]["name"], "Smith, John");
    }

    #[test]
    fn parses_an_aggregate_pipeline() {
        let q = parse_query(r#"db.books.aggregate([{"$match": {}}, {"$limit": 5}])"#).unwrap();
        assert_eq!(q.operation, "aggregate");
        assert_eq!(q.args.len(), 1);
        assert_eq!(q.args[0].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parses_no_arguments() {
        let q = parse_query("db.authors.find()").unwrap();
        assert!(q.args.is_empty());
    }

    #[test]
    fn tolerates_a_trailing_semicolon_and_whitespace() {
        let q = parse_query("  db.authors.find({}) ;  ").unwrap();
        assert_eq!(q.collection, "authors");
    }

    #[test]
    fn sql_is_rejected_with_a_message_that_explains_why() {
        // Someone will type SQL here; the error should teach rather than confuse.
        let err = parse_query("SELECT * FROM authors").unwrap_err();
        assert!(err.to_string().contains("db.collection.find"), "{err}");
        assert!(err.to_string().contains("does not use SQL"), "{err}");
    }

    #[test]
    fn malformed_calls_are_reported_clearly() {
        assert!(parse_query("db.authors.find({").is_err());
        assert!(parse_query("db.find({})").is_err());
        assert!(parse_query("db..find({})").is_err());
        assert!(parse_query("db.authors.find({bad json})").is_err());
    }

    #[test]
    fn extended_json_becomes_real_bson() {
        // Without Extended JSON handling this would become a nested document
        // that could never match an _id.
        let value: serde_json::Value =
            serde_json::from_str(r#"{"_id": {"$oid": "0192e8f00000000000000000"}}"#).unwrap();
        let doc = to_document(&value).unwrap();
        assert!(matches!(doc.get("_id"), Some(Bson::ObjectId(_))));
    }

    #[test]
    fn a_non_document_argument_is_refused() {
        let value: serde_json::Value = serde_json::from_str("42").unwrap();
        assert!(to_document(&value).is_err());
    }

    #[test]
    fn regex_metacharacters_are_escaped_for_substring_filters() {
        // Unescaped, "a.b" would match "axb" and surprise the user.
        assert_eq!(regex_escape("a.b"), r"a\.b");
        assert_eq!(regex_escape("x(1)"), r"x\(1\)");
        assert_eq!(regex_escape("plain"), "plain");
    }

    #[test]
    fn documents_become_rows_with_id_first() {
        let docs = vec![
            bson::doc! { "name": "Ada", "_id": 1 },
            bson::doc! { "_id": 2, "name": "Grace", "extra": true },
        ];
        let rs = MongoDriver::to_result_set(&docs, 1, false);

        // _id leads regardless of its position in the document.
        assert_eq!(rs.columns[0].name, "_id");
        // The union of fields, so a field only some documents have still shows.
        let names: Vec<&str> = rs.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"extra"));
        assert_eq!(rs.rows.len(), 2);
        // A document missing a field reads as NULL rather than shifting columns.
        let extra_index = names.iter().position(|n| *n == "extra").unwrap();
        assert_eq!(rs.rows[0][extra_index], Value::Null);
    }

    #[test]
    fn decimal128_is_carried_as_exact_text() {
        let d: bson::Decimal128 = "12345678901234567890.0987654321".parse().unwrap();
        assert!(matches!(
            decode_bson(&Bson::Decimal128(d)),
            Value::Decimal(_)
        ));
    }

    #[test]
    fn object_ids_render_as_hex() {
        let id = bson::oid::ObjectId::new();
        assert_eq!(decode_bson(&Bson::ObjectId(id)), Value::Text(id.to_hex()));
    }

    #[test]
    fn nested_documents_keep_their_structure() {
        let nested = bson::doc! { "a": { "b": 1 } };
        assert!(matches!(
            decode_bson(&Bson::Document(nested)),
            Value::Json(_)
        ));
    }

    #[test]
    fn base64_matches_the_reference_encoding() {
        // Padding is the easy thing to get wrong.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn credentials_are_percent_encoded_for_the_uri() {
        // An unescaped '@' or ':' in a password would corrupt the URI.
        assert_eq!(urlencode("p@ss:w/rd"), "p%40ss%3Aw%2Frd");
        assert_eq!(urlencode("plain-1_2.3~"), "plain-1_2.3~");
    }
}
