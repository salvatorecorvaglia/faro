//! Tests against live database servers from `tests/docker-compose.test.yml`.
//!
//!   docker compose -f tests/docker-compose.test.yml up -d
//!   ./scripts/seed.sh
//!   cargo test --test live_engines
//!
//! Each engine skips itself when its server is unreachable, so this stays green
//! on a machine with no containers running. That is deliberate: a skipped test
//! prints why, and `Verified` in the README means these actually ran.

use faro_lib::dml;
use faro_lib::driver::{self, Driver};
use faro_lib::model::{
    BrowseOptions, CellEdit, ConnectionConfig, EditValue, Engine, GuardedStatement, PendingChange,
    QueryOutcome, SslMode, TableRef, Value,
};
use faro_lib::transfer::backup::{self, BackupOptions, RestoreOptions};
use tokio_util::sync::CancellationToken;

fn config(engine: Engine, port: u16, database: &str) -> ConnectionConfig {
    ConnectionConfig {
        id: "live".into(),
        name: "live".into(),
        engine,
        host: "127.0.0.1".into(),
        port,
        username: match engine {
            Engine::SqlServer => "sa".into(),
            _ => "faro".into(),
        },
        database: database.into(),
        file_path: None,
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    }
}

fn password_for(engine: Engine) -> &'static str {
    match engine {
        // Matches tests/docker-compose.test.yml; SQL Server enforces a complexity rule.
        Engine::SqlServer => "Faro!Passw0rd",
        _ => "faro",
    }
}

/// Connect, or return None so the caller can skip.
async fn try_open(engine: Engine, port: u16, database: &str) -> Option<Box<dyn Driver>> {
    let addr = format!("127.0.0.1:{port}");
    if tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map_or(true, |r| r.is_err())
    {
        eprintln!("skipping {}: port {port} not open", engine.display_name());
        return None;
    }

    let cfg = config(engine, port, database);
    match driver::connect(&cfg, Some(password_for(engine))).await {
        Ok(d) => match d.ping().await {
            Ok(()) => Some(d),
            Err(e) => {
                eprintln!("skipping {}: ping failed: {e}", engine.display_name());
                None
            }
        },
        Err(e) => {
            eprintln!("skipping {}: {e}", engine.display_name());
            None
        }
    }
}

macro_rules! engine_or_skip {
    ($engine:expr, $port:expr, $db:expr) => {
        match try_open($engine, $port, $db).await {
            Some(d) => d,
            None => return,
        }
    };
}

async fn scalar(d: &dyn Driver, sql: &str) -> Value {
    let rs = d.query(sql, 1, CancellationToken::new()).await.unwrap();
    rs.rows
        .first()
        .and_then(|r| r.first())
        .cloned()
        .unwrap_or(Value::Null)
}

fn cell(column: &str, text: &str) -> CellEdit {
    CellEdit {
        column: column.into(),
        value: EditValue::Text(text.into()),
    }
}

async fn apply(
    d: &dyn Driver,
    table: &TableRef,
    changes: &[PendingChange],
) -> faro_lib::error::Result<u64> {
    let detail = d.describe_table(table).await.unwrap();
    let statements = dml::build_statements(table, &detail, changes, d.dialect())?;
    d.apply_transaction(&statements).await
}

/// The full battery every SQL engine must pass.
///
/// Written once and run per engine so a new driver cannot quietly support less
/// than the others.
async fn exercise(d: &dyn Driver, schema: Option<&str>, label: &str) {
    let table = |name: &str| TableRef {
        schema: schema.map(String::from),
        name: name.into(),
    };

    // -- Browsing ---------------------------------------------------------
    let names: Vec<String> = d
        .list_tables(schema)
        .await
        .unwrap_or_else(|e| panic!("{label}: list_tables failed: {e}"))
        .into_iter()
        .map(|t| t.name)
        .collect();
    for expected in ["authors", "books", "book_stores", "access_log"] {
        assert!(
            names.contains(&expected.to_string()),
            "{label}: missing table {expected} in {names:?}"
        );
    }

    // -- Keys -------------------------------------------------------------
    let books = d
        .describe_table(&table("books"))
        .await
        .unwrap_or_else(|e| panic!("{label}: describe books failed: {e}"));
    assert_eq!(books.primary_key, vec!["id"], "{label}: books primary key");
    assert!(books.is_editable(), "{label}: books should be editable");

    let stores = d.describe_table(&table("book_stores")).await.unwrap();
    assert_eq!(
        stores.primary_key,
        vec!["book_id", "store_id"],
        "{label}: composite key order matters for generated DML"
    );

    let log = d.describe_table(&table("access_log")).await.unwrap();
    assert!(
        log.primary_key.is_empty() && !log.is_editable(),
        "{label}: a table with no primary key must stay read-only"
    );

    // -- Foreign keys -----------------------------------------------------
    assert!(
        books
            .foreign_keys
            .iter()
            .any(|f| f.referenced_table.name == "authors"),
        "{label}: books should reference authors: {:#?}",
        books.foreign_keys
    );

    // -- Querying and decoding -------------------------------------------
    let rs = d
        .query(
            "SELECT id, name, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{label}: query failed: {e}"));
    assert_eq!(rs.rows.len(), 5, "{label}: author count");
    assert_eq!(rs.rows[0][0], Value::Int(1), "{label}: first id");
    assert_eq!(
        rs.rows[0][1],
        Value::Text("Ada Lovelace".into()),
        "{label}: first name"
    );
    assert_eq!(
        rs.rows[2][2],
        Value::Null,
        "{label}: NULL must decode as Null"
    );

    // Unicode and embedded quotes must survive the round trip.
    let names: Vec<String> = rs
        .rows
        .iter()
        .filter_map(|r| match &r[1] {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(
        names.iter().any(|n| n == "Ken O'Brien"),
        "{label}: apostrophe: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains('Ó')),
        "{label}: unicode: {names:?}"
    );

    // -- Truncation and paging -------------------------------------------
    let page = d
        .query("SELECT * FROM access_log", 10, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        page.rows.len(),
        10,
        "{label}: must return exactly the limit"
    );
    assert!(
        page.truncated,
        "{label}: 5000 rows exist, so this is truncated"
    );

    let exact = d
        .query("SELECT * FROM authors", 5, CancellationToken::new())
        .await
        .unwrap();
    assert!(!exact.truncated, "{label}: an exact fit is not truncated");

    let dialect = d.dialect();
    let base = "SELECT path FROM access_log ORDER BY path";
    let p1 = d
        .query(&dialect.paginate(base, 6, 0), 5, CancellationToken::new())
        .await
        .unwrap();
    let p2 = d
        .query(&dialect.paginate(base, 6, 5), 5, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(p1.rows.len(), 5, "{label}: page one");
    assert_eq!(p2.rows.len(), 5, "{label}: page two");
    assert_ne!(p1.rows[0], p2.rows[0], "{label}: pages must not overlap");

    // -- Autocomplete -----------------------------------------------------
    let snapshot = d.schema_snapshot(schema).await.unwrap();
    let books_cols = snapshot
        .iter()
        .find(|t| t.name == "books")
        .unwrap_or_else(|| panic!("{label}: snapshot missing books"));
    assert!(
        books_cols.columns.contains(&"title".to_string()),
        "{label}: snapshot columns"
    );

    // -- Cancellation -----------------------------------------------------
    let token = CancellationToken::new();
    token.cancel();
    assert!(
        matches!(
            d.query("SELECT 1", 10, token).await,
            Err(faro_lib::error::FaroError::Cancelled)
        ),
        "{label}: a pre-cancelled token must abort"
    );

    // -- Errors -----------------------------------------------------------
    assert!(
        d.query("SELECT FROM WHERE", 10, CancellationToken::new())
            .await
            .is_err(),
        "{label}: bad SQL must error"
    );

    // -- Editing ----------------------------------------------------------
    apply(
        d,
        &table("authors"),
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("name", "Ada Byron")],
        }],
    )
    .await
    .unwrap_or_else(|e| panic!("{label}: update failed: {e}"));
    assert_eq!(
        scalar(d, "SELECT name FROM authors WHERE id = 1").await,
        Value::Text("Ada Byron".into()),
        "{label}: the edit did not persist"
    );

    // Put it back so the fixture is reusable.
    apply(
        d,
        &table("authors"),
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("name", "Ada Lovelace")],
        }],
    )
    .await
    .unwrap();

    // -- The row-count guard ---------------------------------------------
    // Hand-built: `dml` cannot produce an unanchored UPDATE, which is the point
    // — the guard is a second line of defence behind the generator.
    let before = scalar(d, "SELECT COUNT(*) FROM authors WHERE bio = 'clobbered'").await;
    let unguarded = vec![GuardedStatement {
        sql: "UPDATE authors SET bio = 'clobbered'".into(),
        expect: Some(1),
    }];
    let err = d.apply_transaction(&unguarded).await.unwrap_err();
    assert!(
        err.to_string().contains("does not identify a single row"),
        "{label}: guard message: {err}"
    );
    assert_eq!(
        scalar(d, "SELECT COUNT(*) FROM authors WHERE bio = 'clobbered'").await,
        before,
        "{label}: the rollback did not undo the over-broad update"
    );

    // -- A vanished row aborts the batch ---------------------------------
    let original = scalar(d, "SELECT name FROM authors WHERE id = 1").await;
    let result = apply(
        d,
        &table("authors"),
        &[
            PendingChange::Update {
                key: vec![cell("id", "1")],
                cells: vec![cell("name", "Should Not Persist")],
            },
            PendingChange::Update {
                key: vec![cell("id", "999999")],
                cells: vec![cell("name", "Ghost")],
            },
        ],
    )
    .await;
    assert!(
        result.is_err(),
        "{label}: a missing row must abort the batch"
    );
    assert_eq!(
        scalar(d, "SELECT name FROM authors WHERE id = 1").await,
        original,
        "{label}: a valid change committed despite the batch failing"
    );
}

// -- One test per engine ----------------------------------------------------

#[tokio::test]
async fn postgres_live() {
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");
    exercise(&*d, Some("public"), "PostgreSQL").await;
    d.close().await;
}

#[tokio::test]
async fn mysql_live() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");
    // MySQL is presented as schema-less: a connection's database is its schema.
    exercise(&*d, None, "MySQL").await;
    d.close().await;
}

#[tokio::test]
async fn mariadb_live() {
    let d = engine_or_skip!(Engine::MariaDb, 53307, "faro_test");
    exercise(&*d, None, "MariaDB").await;
    d.close().await;
}

#[tokio::test]
async fn sqlserver_live() {
    let d = engine_or_skip!(Engine::SqlServer, 51433, "faro_test");
    exercise(&*d, Some("dbo"), "SQL Server").await;
    d.close().await;
}

/// ClickHouse gets its own battery.
///
/// The shared one assumes row-level identity, which ClickHouse deliberately does
/// not offer: a MergeTree primary key is a sparse sorting index rather than a
/// unique constraint, so no `WHERE` clause provably matches one row. Faro must
/// present its tables as read-only, and that is what this checks.
#[tokio::test]
async fn clickhouse_live() {
    let d = engine_or_skip!(Engine::ClickHouse, 58123, "faro_test");
    let schema = Some("faro_test");
    let table = |name: &str| TableRef {
        schema: schema.map(String::from),
        name: name.into(),
    };

    // -- Browsing ---------------------------------------------------------
    let names: Vec<String> = d
        .list_tables(schema)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for expected in [
        "authors",
        "books",
        "book_stores",
        "access_log",
        "type_gallery",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in {names:?}"
        );
    }

    // -- The read-only property, which is the point ----------------------
    let books = d.describe_table(&table("books")).await.unwrap();
    assert!(
        books.primary_key.is_empty(),
        "a MergeTree sorting key must not be reported as a primary key"
    );
    assert!(
        !books.is_editable(),
        "ClickHouse tables must be read-only: rows cannot be addressed individually"
    );

    // The sorting key is still surfaced, just as an index the user can see.
    let stores = d.describe_table(&table("book_stores")).await.unwrap();
    assert!(
        stores
            .indexes
            .iter()
            .any(|i| i.columns == vec!["book_id", "store_id"]),
        "the compound sorting key should be visible: {:#?}",
        stores.indexes
    );
    // ...and never re-emitted by backup, since it is part of the ENGINE clause.
    assert!(
        stores.indexes.iter().all(|i| i.is_constraint),
        "sorting keys are not CREATE INDEX objects"
    );

    // -- Columns and nullability -----------------------------------------
    assert!(
        books
            .columns
            .iter()
            .any(|c| c.name == "title" && !c.nullable),
        "non-nullable columns should be detected"
    );
    assert!(
        books.columns.iter().any(|c| c.name == "isbn" && c.nullable),
        "Nullable(T) should be detected as nullable"
    );

    // -- Querying and decoding -------------------------------------------
    let rs = d
        .query(
            "SELECT id, name, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(rs.rows.len(), 5);
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("Ada Lovelace".into()));
    assert_eq!(rs.rows[2][2], Value::Null, "NULL must decode as Null");

    let names: Vec<String> = rs
        .rows
        .iter()
        .filter_map(|r| match &r[1] {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(names.iter().any(|n| n == "Ken O'Brien"), "{names:?}");
    assert!(names.iter().any(|n| n.contains('Ó')), "{names:?}");

    // -- Wide integers and decimals stay exact ---------------------------
    // UInt64's maximum wrapped into an i64 reads as -1.
    assert_eq!(
        scalar(&*d, "SELECT a_uint64 FROM type_gallery WHERE id = 1").await,
        Value::Decimal("18446744073709551615".into())
    );
    assert_eq!(
        scalar(&*d, "SELECT a_int128 FROM type_gallery WHERE id = 1").await,
        Value::Decimal("170141183460469231731687303715884105727".into())
    );
    match scalar(&*d, "SELECT a_decimal FROM type_gallery WHERE id = 1").await {
        Value::Decimal(s) => assert!(
            s.starts_with("12345678901234567890.09876543"),
            "precision lost: {s}"
        ),
        other => panic!("expected a decimal, got {other:?}"),
    }

    // -- Other types ------------------------------------------------------
    assert!(matches!(
        scalar(&*d, "SELECT a_uuid FROM type_gallery WHERE id = 1").await,
        Value::Uuid(_)
    ));
    assert!(matches!(
        scalar(&*d, "SELECT a_array FROM type_gallery WHERE id = 1").await,
        Value::Json(_)
    ));
    assert_eq!(
        scalar(&*d, "SELECT a_bool FROM type_gallery WHERE id = 1").await,
        Value::Bool(true)
    );
    match scalar(&*d, "SELECT a_date FROM type_gallery WHERE id = 1").await {
        Value::Date(s) => assert_eq!(s, "2026-08-05"),
        other => panic!("expected a date, got {other:?}"),
    }

    // -- Truncation and paging -------------------------------------------
    let page = d
        .query("SELECT * FROM access_log", 10, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 10);
    assert!(page.truncated, "5000 rows exist");

    let exact = d
        .query("SELECT * FROM authors", 5, CancellationToken::new())
        .await
        .unwrap();
    assert!(!exact.truncated);

    let dialect = d.dialect();
    let base = "SELECT path FROM access_log ORDER BY path";
    let p1 = d
        .query(&dialect.paginate(base, 6, 0), 5, CancellationToken::new())
        .await
        .unwrap();
    let p2 = d
        .query(&dialect.paginate(base, 6, 5), 5, CancellationToken::new())
        .await
        .unwrap();
    assert_ne!(p1.rows[0], p2.rows[0], "pages must not overlap");

    // -- Autocomplete -----------------------------------------------------
    let snapshot = d.schema_snapshot(schema).await.unwrap();
    assert!(snapshot
        .iter()
        .find(|t| t.name == "books")
        .is_some_and(|t| t.columns.contains(&"title".to_string())));

    // -- Native DDL -------------------------------------------------------
    let ddl = d
        .table_ddl(&table("books"))
        .await
        .unwrap()
        .expect("ClickHouse exposes create_table_query");
    assert!(
        ddl.contains("MergeTree"),
        "the ENGINE clause must survive: {ddl}"
    );

    // -- Errors and cancellation -----------------------------------------
    let err = d
        .query("SELECT FROM WHERE", 10, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("syntax"),
        "unhelpful error: {err}"
    );

    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        d.query("SELECT 1", 10, token).await,
        Err(faro_lib::error::FaroError::Cancelled)
    ));

    // -- Transactions are honestly reported as absent ---------------------
    assert!(
        !d.dialect().supports_transactions(),
        "ClickHouse has no transactions and must not claim otherwise"
    );

    d.close().await;
}

/// MongoDB gets its own battery too, for the opposite reason to ClickHouse:
/// almost everything works, but the query language is not SQL and documents are
/// schemaless, so the shared battery's assumptions do not hold.
#[tokio::test]
async fn mongodb_live() {
    let d = engine_or_skip!(Engine::MongoDb, 57017, "faro_test");
    let schema = Some("faro_test");
    let table = |name: &str| TableRef {
        schema: schema.map(String::from),
        name: name.into(),
    };

    // -- Collections browse like tables ----------------------------------
    let names: Vec<String> = d
        .list_tables(schema)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for expected in [
        "authors",
        "books",
        "book_stores",
        "access_log",
        "type_gallery",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in {names:?}"
        );
    }

    // -- It says plainly that it is not SQL ------------------------------
    assert!(
        !d.dialect().is_sql(),
        "the editor and formatter depend on this being false"
    );

    // -- Field inference over a schemaless collection ---------------------
    let authors = d.describe_table(&table("authors")).await.unwrap();
    let field = |n: &str| authors.columns.iter().find(|c| c.name == n);

    assert_eq!(authors.columns[0].name, "_id", "_id must lead");
    assert!(field("name").is_some());
    // Present on one document out of five: inference must still surface it,
    // and mark it nullable because most documents lack it.
    let nickname = field("nickname").expect("a field on a single document must still appear");
    assert!(nickname.nullable, "a field most documents lack is nullable");

    // -- Documents are read as rows ---------------------------------------
    let rs = d
        .query(r#"db.authors.find({})"#, 100, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(rs.rows.len(), 5);

    let id_index = rs.columns.iter().position(|c| c.name == "_id").unwrap();
    let name_index = rs.columns.iter().position(|c| c.name == "name").unwrap();
    assert_eq!(rs.rows[0][id_index], Value::Int(1));
    assert_eq!(rs.rows[0][name_index], Value::Text("Ada Lovelace".into()));

    let names: Vec<String> = rs
        .rows
        .iter()
        .filter_map(|r| match &r[name_index] {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(names.iter().any(|n| n == "Ken O'Brien"), "{names:?}");
    assert!(names.iter().any(|n| n.contains('Ó')), "{names:?}");

    // -- Filters, projections and aggregation -----------------------------
    let filtered = d
        .query(
            r#"db.authors.find({"name": "Grace Hopper"})"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        filtered.rows.len(),
        1,
        "the filter document was not applied"
    );

    let projected = d
        .query(
            r#"db.authors.find({}, {"name": 1})"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let projected_fields: Vec<&str> = projected.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(projected_fields.contains(&"name"));
    assert!(
        !projected_fields.contains(&"bio"),
        "projection was ignored: {projected_fields:?}"
    );

    let aggregated = d
        .query(
            r#"db.books.aggregate([{"$match": {"in_stock": true}}, {"$count": "n"}])"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(aggregated.rows.len(), 1, "aggregation returned nothing");

    let counted = d
        .query(
            r#"db.authors.countDocuments({})"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(counted.rows[0][0], Value::Int(5));

    // -- Exact numerics ---------------------------------------------------
    let dec = d
        .query(
            r#"db.type_gallery.find({"_id": 1}, {"a_decimal": 1})"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let i = dec
        .columns
        .iter()
        .position(|c| c.name == "a_decimal")
        .unwrap();
    match &dec.rows[0][i] {
        // Decimal128 through f64 would round this.
        Value::Decimal(s) => assert!(
            s.starts_with("12345678901234567890.09876543"),
            "precision lost: {s}"
        ),
        other => panic!("expected a decimal, got {other:?}"),
    }

    // -- Nested structure is preserved ------------------------------------
    let nested = d
        .query(
            r#"db.type_gallery.find({"_id": 1}, {"a_object": 1, "a_array": 1})"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    for name in ["a_object", "a_array"] {
        let i = nested.columns.iter().position(|c| c.name == name).unwrap();
        assert!(
            matches!(nested.rows[0][i], Value::Json(_)),
            "{name} should keep its structure"
        );
    }

    // -- Browsing goes through find, not SQL ------------------------------
    let page = d
        .browse(
            &table("access_log"),
            &BrowseOptions {
                sort_column: None,
                sort_desc: false,
                filters: vec![],
                limit: Some(10),
                offset: 0,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 10);
    assert!(page.truncated, "5000 documents exist");

    // Sorting and paging must be honoured server-side.
    let sorted = |desc: bool| BrowseOptions {
        sort_column: Some("path".into()),
        sort_desc: desc,
        filters: vec![],
        limit: Some(5),
        offset: 0,
    };
    let asc = d
        .browse(
            &table("access_log"),
            &sorted(false),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let desc = d
        .browse(
            &table("access_log"),
            &sorted(true),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(asc.rows[0], desc.rows[0], "sort direction was ignored");

    let second_page = d
        .browse(
            &table("access_log"),
            &BrowseOptions {
                sort_column: Some("path".into()),
                sort_desc: false,
                filters: vec![],
                limit: Some(5),
                offset: 5,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_ne!(asc.rows[0], second_page.rows[0], "offset was ignored");

    // A grid filter becomes a query document.
    let matching = d
        .browse(
            &table("authors"),
            &BrowseOptions {
                sort_column: None,
                sort_desc: false,
                filters: vec![faro_lib::model::ColumnFilter {
                    column: "name".into(),
                    op: faro_lib::model::FilterOp::Contains,
                    value: "Hopper".into(),
                }],
                limit: Some(10),
                offset: 0,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        matching.rows.len(),
        1,
        "the contains filter did not translate"
    );

    // -- Read-only, and honest about it -----------------------------------
    assert!(
        authors.primary_key.is_empty() && !authors.is_editable(),
        "MongoDB is read-only in Faro until a document-shaped edit path exists"
    );
    assert!(
        d.execute("db.authors.deleteMany({})", CancellationToken::new())
            .await
            .is_err(),
        "a write must be declined, not silently ignored"
    );

    // -- Indexes are surfaced ---------------------------------------------
    let books = d.describe_table(&table("books")).await.unwrap();
    assert!(
        books.indexes.iter().any(|i| i.name == "books_title_idx"),
        "a real index should be visible: {:#?}",
        books.indexes
    );
    // The implicit _id index cannot be recreated, so backup must skip it.
    assert!(
        books
            .indexes
            .iter()
            .any(|i| i.name == "_id_" && i.is_constraint),
        "the _id index should be marked constraint-backed"
    );

    // -- Autocomplete -----------------------------------------------------
    let snapshot = d.schema_snapshot(schema).await.unwrap();
    assert!(snapshot
        .iter()
        .find(|t| t.name == "books")
        .is_some_and(|t| t.columns.contains(&"title".to_string())));

    // -- Errors teach rather than confuse ---------------------------------
    let err = d
        .query("SELECT * FROM authors", 10, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("does not use SQL"),
        "someone typing SQL deserves a useful message: {err}"
    );

    let err = d
        .query("db.authors.explode({})", 10, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not supported"), "{err}");

    // -- `run` classifies every Mongo query as row-returning --------------
    //
    // The default `Driver::run` guesses from a leading SQL keyword, which a
    // `db.collection.find(...)` call never has, so it used to fall through to
    // `execute` and be rejected outright. This is the path `run_query` (the
    // Query tab) actually calls.
    match d
        .run("db.authors.find({})", 10, CancellationToken::new())
        .await
        .unwrap()
    {
        QueryOutcome::Rows(rs) => assert_eq!(rs.rows.len(), 5),
        other => panic!("expected rows from a Mongo find, got {other:?}"),
    }
    match d
        .run(
            r#"db.books.aggregate([{"$match": {"in_stock": true}}, {"$count": "n"}])"#,
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap()
    {
        QueryOutcome::Rows(rs) => assert_eq!(rs.rows.len(), 1),
        other => panic!("expected rows from a Mongo aggregate, got {other:?}"),
    }

    // -- `$out`/`$merge` are refused even though `aggregate` is recognised -
    for pipeline in [
        r#"db.authors.aggregate([{"$match": {}}, {"$out": "authors_copy"}])"#,
        r#"db.authors.aggregate([{"$match": {}}, {"$merge": {"into": "authors_copy"}}])"#,
    ] {
        let err = d
            .run(pipeline, 10, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("read-only"),
            "a write stage must be refused: {err}"
        );
    }
    let table_names: Vec<String> = d
        .list_tables(schema)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(
        !table_names.contains(&"authors_copy".to_string()),
        "a refused $out/$merge must not have written anything"
    );

    // -- Cancellation ------------------------------------------------------
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        d.query("db.authors.find({})", 10, token).await,
        Err(faro_lib::error::FaroError::Cancelled)
    ));

    d.close().await;
}

// -- Backup round trips ------------------------------------------------------

/// Back up one table and restore it into a fresh copy on the same server.
async fn backup_round_trip(d: &dyn Driver, schema: Option<&str>, label: &str) {
    let dump = std::env::temp_dir().join(format!(
        "faro_live_{}_{}.sql",
        label.replace(' ', "_").to_lowercase(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&dump);

    let result = backup::write_backup(
        d,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: schema.map(String::from),
                name: "authors".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        |_| {},
    )
    .await
    .unwrap_or_else(|e| panic!("{label}: backup failed: {e}"));

    assert_eq!(result.rows, 5, "{label}: backed up row count");
    let script = std::fs::read_to_string(&dump).unwrap();
    assert!(
        script.contains("INSERT INTO"),
        "{label}: dump has no inserts"
    );
    // The apostrophe must be escaped, or the restore is a syntax error.
    assert!(
        script.contains("O''Brien") || script.contains("O\\'Brien"),
        "{label}: the apostrophe was not escaped"
    );

    // Restoring over the existing table must fail on the CREATE, proving the
    // dump really does recreate schema rather than only inserting data.
    let restored = backup::restore(
        d,
        &script,
        &RestoreOptions {
            stop_on_error: false,
        },
        |_, _| {},
    )
    .await
    .unwrap();
    assert!(
        restored.failed > 0,
        "{label}: restoring over an existing table should have collided"
    );

    let _ = std::fs::remove_file(&dump);
}

#[tokio::test]
async fn postgres_backup_round_trip() {
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");
    backup_round_trip(&*d, Some("public"), "PostgreSQL").await;
    d.close().await;
}

#[tokio::test]
async fn mysql_backup_round_trip() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");
    backup_round_trip(&*d, None, "MySQL").await;
    d.close().await;
}

#[tokio::test]
async fn sqlserver_backup_round_trip() {
    let d = engine_or_skip!(Engine::SqlServer, 51433, "faro_test");
    backup_round_trip(&*d, Some("dbo"), "SQL Server").await;
    d.close().await;
}

#[tokio::test]
async fn mariadb_backup_round_trip() {
    // MariaDB shares the MySQL driver and wire protocol, but nothing had
    // exercised backup/restore against it directly — every assumption the
    // dump generator makes about MySQL-dialect quoting and escaping had only
    // ever been proven against MySQL itself.
    let d = engine_or_skip!(Engine::MariaDb, 53307, "faro_test");
    backup_round_trip(&*d, None, "MariaDB").await;
    d.close().await;
}

#[tokio::test]
async fn mongodb_backup_is_declined_not_silently_wrong() {
    // MongoDB has no SQL dump to produce, so `write_backup` refuses outright
    // rather than writing a file that only looks like a backup — this proves
    // that decline path actually fires for the driver, instead of assuming
    // the `is_sql` check in the shared code is enough.
    let d = engine_or_skip!(Engine::MongoDb, 57017, "faro_test");

    let dump = std::env::temp_dir().join(format!(
        "faro_live_mongo_decline_{}.sql",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&dump);

    let err = backup::write_backup(
        &*d,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: Some("faro_test".into()),
                name: "authors".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("does not apply"),
        "the decline message should explain why: {err}"
    );
    assert!(!dump.exists(), "a declined backup must not write a file");

    d.close().await;
}

// -- Read-only enforcement, end to end ---------------------------------------

/// A connection opened read-only is refused a write at both layers: the
/// `Registry` gate every write command goes through, and — belt and braces —
/// the engine's own session-level guard, so a write hidden from the lexical
/// check in `sql.rs` (a function call, say) still cannot land.
///
/// Only the `Registry` gate had a test before this: `sql.rs`'s classifier is
/// thoroughly unit-tested in isolation, but nothing proved the wiring from
/// "connection marked read-only" through to "the driver's own connection
/// actually refuses a write" against a live server.
#[tokio::test]
async fn postgres_read_only_is_enforced_by_the_registry_and_the_server() {
    if tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect("127.0.0.1:55432"),
    )
    .await
    .map_or(true, |r| r.is_err())
    {
        eprintln!("skipping: port 55432 not open");
        return;
    }

    let cfg = ConnectionConfig {
        read_only: true,
        ..config(Engine::Postgres, 55432, "faro_test")
    };
    let d: std::sync::Arc<dyn Driver> = driver::connect(&cfg, Some(password_for(Engine::Postgres)))
        .await
        .unwrap()
        .into();

    // -- The Registry gate --------------------------------------------------
    let reg = faro_lib::registry::Registry::new();
    reg.insert("c1".into(), d.clone(), true, "test".into())
        .await;
    assert!(reg.is_read_only("c1").await);
    match reg.get_writable("c1").await {
        Err(faro_lib::error::FaroError::ReadOnly(_)) => {}
        other => panic!(
            "the registry must refuse before a write command ever reaches the driver: {}",
            other.is_ok()
        ),
    }

    // -- The server's own guard, bypassing the registry and sql.rs ----------
    //
    // `default_transaction_read_only=on` was set at connect time (see
    // postgres.rs). Targeting a row id that cannot exist means this is safe
    // to run against the shared fixture even if enforcement were broken: at
    // worst it deletes nothing.
    let err = d
        .execute(
            "DELETE FROM authors WHERE id = 999999",
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("read-only"),
        "the server itself must refuse the write, not just Faro's own guards: {err}"
    );

    d.close().await;
}

// -- Engine-specific decoding -----------------------------------------------

#[tokio::test]
async fn mysql_keeps_unsigned_bigint_exact() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");

    // BIGINT UNSIGNED at its maximum. Wrapped into an i64 this reads as -1,
    // which is a silent and entirely plausible-looking lie.
    let value = scalar(
        &*d,
        "SELECT a_ubigint FROM type_gallery WHERE a_ubigint IS NOT NULL",
    )
    .await;
    assert_eq!(value, Value::Decimal("18446744073709551615".into()));
    d.close().await;
}

/// The fixture value, exactly as seeded into a 30-digit decimal column.
///
/// 30 significant digits, which is past `rust_decimal`'s ~28. Asserting the
/// whole string is the point: the previous `starts_with` on a truncated prefix
/// passed just as happily against the rounded value.
const EXACT_DECIMAL: &str = "12345678901234567890.0987654321";

#[tokio::test]
async fn mysql_keeps_decimal_precision() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");

    match scalar(
        &*d,
        "SELECT a_decimal FROM type_gallery WHERE a_decimal IS NOT NULL",
    )
    .await
    {
        Value::Decimal(s) => assert_eq!(s, EXACT_DECIMAL, "precision lost"),
        other => panic!("expected a decimal, got {other:?}"),
    }
    d.close().await;
}

#[tokio::test]
async fn postgres_keeps_numeric_precision() {
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");

    match scalar(
        &*d,
        "SELECT a_numeric FROM type_gallery WHERE a_numeric IS NOT NULL",
    )
    .await
    {
        Value::Decimal(s) => assert_eq!(s, EXACT_DECIMAL, "precision lost"),
        other => panic!("expected a decimal, got {other:?}"),
    }
    d.close().await;
}

#[tokio::test]
async fn mysql_decodes_blobs_and_json() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");

    assert_eq!(
        scalar(
            &*d,
            "SELECT a_blob FROM type_gallery WHERE a_blob IS NOT NULL"
        )
        .await,
        Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
    );
    assert!(matches!(
        scalar(
            &*d,
            "SELECT a_json FROM type_gallery WHERE a_json IS NOT NULL"
        )
        .await,
        Value::Json(_)
    ));
    d.close().await;
}

#[tokio::test]
async fn postgres_decodes_arrays_and_uuids() {
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");

    assert!(matches!(
        scalar(
            &*d,
            "SELECT a_int_arr FROM type_gallery WHERE a_int_arr IS NOT NULL"
        )
        .await,
        Value::Array(_)
    ));
    assert!(matches!(
        scalar(&*d, "SELECT id FROM type_gallery LIMIT 1").await,
        Value::Uuid(_)
    ));
    d.close().await;
}

#[tokio::test]
async fn mysql_native_ddl_is_used_for_backup() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");

    // SHOW CREATE TABLE keeps AUTO_INCREMENT and the charset, which a
    // column-by-column rebuild cannot express.
    let ddl = d
        .table_ddl(&TableRef {
            schema: None,
            name: "authors".into(),
        })
        .await
        .unwrap()
        .expect("MySQL should expose its own DDL");
    assert!(ddl.to_uppercase().contains("AUTO_INCREMENT"), "{ddl}");
    assert!(ddl.ends_with(';'), "{ddl}");
    d.close().await;
}

// -- Backslash escaping in string literals --------------------------------
//
// MySQL, MariaDB and ClickHouse honour `\` as an escape inside `'...'`, so
// doubling only the quote lets a value ending in a backslash escape its own
// closing delimiter and spill the rest of the statement into SQL. These run
// the generated SQL against the real server: asserting on the generated string
// alone would not prove the engine agrees with our reading of its grammar.

/// Two ANDed filters where the first ends in a backslash. If the backslash is
/// not escaped, it swallows the `' AND ...` that follows and the second value
/// is parsed as SQL rather than compared as data.
async fn assert_backslash_is_data_not_syntax(d: &dyn Driver, schema: Option<&str>, label: &str) {
    let table = TableRef {
        schema: schema.map(String::from),
        name: "authors".into(),
    };

    for payload in [r"\", r"x\", r"a\'; SELECT 1; -- "] {
        let out = d
            .browse(
                &table,
                &BrowseOptions {
                    sort_column: None,
                    sort_desc: false,
                    filters: vec![
                        faro_lib::model::ColumnFilter {
                            column: "name".into(),
                            op: faro_lib::model::FilterOp::Equals,
                            value: payload.into(),
                        },
                        faro_lib::model::ColumnFilter {
                            column: "email".into(),
                            op: faro_lib::model::FilterOp::Equals,
                            value: "' OR 1=1 -- ".into(),
                        },
                    ],
                    limit: Some(50),
                    offset: 0,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{label}: filter {payload:?} produced invalid SQL, so it was not escaped: {e}"
                )
            });

        // No fixture row holds these values, so a correctly escaped filter
        // matches nothing. Rows coming back means `OR 1=1` was executed.
        assert!(
            out.rows.is_empty(),
            "{label}: filter {payload:?} let injected SQL run — got {} rows",
            out.rows.len()
        );
    }

    // The LIKE path has its own escape clause, which is a second literal.
    for op in [
        faro_lib::model::FilterOp::Contains,
        faro_lib::model::FilterOp::StartsWith,
    ] {
        d.browse(
            &table,
            &BrowseOptions {
                sort_column: None,
                sort_desc: false,
                filters: vec![faro_lib::model::ColumnFilter {
                    column: "name".into(),
                    op,
                    value: r"c:\tmp".into(),
                }],
                limit: Some(50),
                offset: 0,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{label}: {op:?} with a backslash produced invalid SQL: {e}"));
    }
}

#[tokio::test]
async fn mysql_escapes_backslashes_in_literals() {
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");
    assert_backslash_is_data_not_syntax(&*d, None, "mysql").await;
    d.close().await;
}

#[tokio::test]
async fn mariadb_escapes_backslashes_in_literals() {
    let d = engine_or_skip!(Engine::MariaDb, 53307, "faro_test");
    assert_backslash_is_data_not_syntax(&*d, None, "mariadb").await;
    d.close().await;
}

#[tokio::test]
async fn clickhouse_escapes_backslashes_in_literals() {
    let d = engine_or_skip!(Engine::ClickHouse, 58123, "faro_test");
    assert_backslash_is_data_not_syntax(&*d, Some("faro_test"), "clickhouse").await;
    d.close().await;
}

#[tokio::test]
async fn postgres_keeps_backslashes_as_data() {
    // The mirror image: Postgres must *not* double the backslash, or the
    // stored value would come back with an extra one.
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");
    assert_backslash_is_data_not_syntax(&*d, Some("public"), "postgres").await;
    d.close().await;
}

#[tokio::test]
async fn sqlserver_keeps_the_offset_on_datetimeoffset() {
    // `datetimeoffset` decodes only into `DateTime<FixedOffset>`; routed
    // through `NaiveDateTime` it failed and became a silent NULL, which is
    // indistinguishable from the column actually being NULL.
    let d = engine_or_skip!(Engine::SqlServer, 51433, "faro_test");

    match scalar(
        &*d,
        "SELECT a_datetimeoff FROM type_gallery WHERE a_datetimeoff IS NOT NULL",
    )
    .await
    {
        Value::Timestamp(s) => {
            assert!(
                s.contains('+') || s.contains('Z') || s.matches('-').count() > 2,
                "the UTC offset was dropped: {s}"
            );
        }
        Value::Null => panic!("datetimeoffset decoded to a silent NULL"),
        other => panic!("expected a timestamp, got {other:?}"),
    }
    d.close().await;
}

#[tokio::test]
async fn mysql_dump_restores_into_an_empty_database() {
    // The existing round-trip restores over the live schema and asserts the
    // dump *fails*. This proves the dump is actually restorable: MySQL's
    // `SHOW CREATE TABLE` declares secondary indexes inline, so a dump that
    // also emits `CREATE INDEX` dies on "Duplicate key name".
    let d = engine_or_skip!(Engine::MySql, 53306, "faro_test");

    let dir = std::env::temp_dir().join(format!("faro_mysql_dump_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dump = dir.join("dump.sql");

    // `books` is the table carrying a non-unique secondary index.
    backup::write_backup(
        &*d,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "books".into(),
            }],
            include_schema: true,
            include_data: false,
            drop_existing: false,
        },
        |_| {},
    )
    .await
    .unwrap();

    let script = std::fs::read_to_string(&dump).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    // One CREATE INDEX per index would be the bug; the inline KEY is the fix.
    let create_index_count = script.to_uppercase().matches("CREATE INDEX").count();
    assert_eq!(
        create_index_count, 0,
        "indexes are already inline in SHOW CREATE TABLE, so re-emitting them \
         breaks the restore:\n{script}"
    );

    d.close().await;
}

#[tokio::test]
async fn a_keyless_table_pages_in_a_stable_order() {
    // `access_log` has no primary key. Without a deterministic ORDER BY the
    // engine may return rows in a different order per page, so a paged read
    // both duplicates and drops rows.
    let d = engine_or_skip!(Engine::Postgres, 55432, "faro_test");

    let table = TableRef {
        schema: Some("public".into()),
        name: "access_log".into(),
    };
    let detail = d.describe_table(&table).await.unwrap();
    assert!(
        detail.primary_key.is_empty(),
        "fixture must be keyless for this test to mean anything"
    );

    // Walk it in small pages twice and check we get the same partition.
    let mut collected = Vec::new();
    for offset in [0u64, 50, 100] {
        let page = d
            .browse(
                &table,
                &BrowseOptions {
                    sort_column: None,
                    sort_desc: false,
                    filters: vec![],
                    limit: Some(50),
                    offset,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        collected.extend(page.rows);
    }

    // Every row must be distinct: a duplicate means a page boundary resampled.
    let mut seen = std::collections::HashSet::new();
    for row in &collected {
        let key = format!("{row:?}");
        assert!(
            seen.insert(key),
            "the same row came back on two pages — paging is not a partition"
        );
    }
    assert_eq!(collected.len(), 150, "expected three full pages");

    d.close().await;
}
