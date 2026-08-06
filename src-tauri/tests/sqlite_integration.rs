//! End-to-end tests against the seeded SQLite fixture.
//!
//! Run `./scripts/seed.sh` first to build `tests/fixtures/faro_test.db`. These
//! are skipped automatically when the fixture is absent so `cargo test` stays
//! green on a clean checkout.

use faro_lib::driver::{self, Driver};
use faro_lib::model::{ConnectionConfig, Engine, SslMode, TableKind, TableRef, Value};
use tokio_util::sync::CancellationToken;

fn fixture_path() -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("tests/fixtures/faro_test.db");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

async fn open() -> Option<Box<dyn Driver>> {
    let path = fixture_path()?;
    let config = ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(path),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    Some(driver::connect(&config, None).await.expect("connect failed"))
}

/// Skip rather than fail when the fixture has not been generated.
macro_rules! driver_or_skip {
    () => {
        match open().await {
            Some(d) => d,
            None => {
                eprintln!("skipping: run ./scripts/seed.sh to create the fixture");
                return;
            }
        }
    };
}

#[tokio::test]
async fn pings_and_lists_tables() {
    let d = driver_or_skip!();
    d.ping().await.unwrap();

    let tables = d.list_tables(None).await.unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();

    assert!(names.contains(&"authors"));
    assert!(names.contains(&"books"));
    // Internal sqlite_* tables must not leak into the tree.
    assert!(!names.iter().any(|n| n.starts_with("sqlite_")));
}

#[tokio::test]
async fn distinguishes_views_from_tables() {
    let d = driver_or_skip!();
    let tables = d.list_tables(None).await.unwrap();

    let view = tables.iter().find(|t| t.name == "books_with_authors").unwrap();
    assert_eq!(view.kind, TableKind::View);

    let table = tables.iter().find(|t| t.name == "books").unwrap();
    assert_eq!(table.kind, TableKind::Table);
}

#[tokio::test]
async fn describes_columns_and_primary_key() {
    let d = driver_or_skip!();
    let detail = d
        .describe_table(&TableRef { schema: None, name: "books".into() })
        .await
        .unwrap();

    assert_eq!(detail.primary_key, vec!["id"]);
    assert!(detail.is_editable());

    let title = detail.columns.iter().find(|c| c.name == "title").unwrap();
    assert!(!title.nullable);
    let isbn = detail.columns.iter().find(|c| c.name == "isbn").unwrap();
    assert!(isbn.nullable);
}

#[tokio::test]
async fn reads_composite_primary_key_in_order() {
    let d = driver_or_skip!();
    let detail = d
        .describe_table(&TableRef { schema: None, name: "book_stores".into() })
        .await
        .unwrap();

    // Order matters: it determines the WHERE clause of generated DML.
    assert_eq!(detail.primary_key, vec!["book_id", "store_id"]);
}

#[tokio::test]
async fn table_without_primary_key_is_not_editable() {
    let d = driver_or_skip!();
    let detail = d
        .describe_table(&TableRef { schema: None, name: "access_log".into() })
        .await
        .unwrap();

    assert!(detail.primary_key.is_empty());
    assert!(
        !detail.is_editable(),
        "a PK-less table must stay read-only or an UPDATE could hit many rows"
    );
}

#[tokio::test]
async fn reads_foreign_keys() {
    let d = driver_or_skip!();
    let detail = d
        .describe_table(&TableRef { schema: None, name: "books".into() })
        .await
        .unwrap();

    let fk = detail
        .foreign_keys
        .iter()
        .find(|f| f.referenced_table.name == "authors")
        .expect("missing FK to authors");
    assert_eq!(fk.columns, vec!["author_id"]);
}

#[tokio::test]
async fn opening_a_database_does_not_rewrite_its_journal_mode() {
    let path = match fixture_path() {
        Some(p) => p,
        None => return,
    };

    // Read the mode via a connection Faro does not own...
    let before = journal_mode(&path).await;

    // ...open it through the driver, exercising a query on each pool...
    {
        let d = driver_or_skip!();
        d.query("SELECT 1", 1, CancellationToken::new()).await.unwrap();
        d.list_tables(None).await.unwrap();
        d.close().await;
    }

    // ...and confirm the user's file came out the way it went in. Converting
    // to WAL would persistently alter their database and strew -wal/-shm files
    // beside it, purely as a side effect of looking at the data.
    assert_eq!(
        before,
        journal_mode(&path).await,
        "Faro changed the database's journal mode"
    );
}

async fn journal_mode(path: &str) -> String {
    use sqlx::Row;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{path}"))
        .await
        .expect("probe connect failed");
    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("pragma failed");
    let mode: String = row.get(0);
    pool.close().await;
    mode
}

#[tokio::test]
async fn schema_snapshot_carries_every_table_with_its_columns() {
    let d = driver_or_skip!();
    let snapshot = d.schema_snapshot(None).await.unwrap();

    let books = snapshot.iter().find(|t| t.name == "books").unwrap();
    assert!(books.columns.contains(&"title".to_string()));
    assert!(books.columns.contains(&"author_id".to_string()));

    // Views matter for autocomplete too — they are queryable like tables.
    assert!(snapshot.iter().any(|t| t.name == "books_with_authors"));
    assert!(!snapshot.iter().any(|t| t.name.starts_with("sqlite_")));
}

#[tokio::test]
async fn catalog_reads_are_not_blocked_by_a_running_query() {
    let d = driver_or_skip!();

    // The metadata connection exists so the schema tree and autocomplete stay
    // responsive while user SQL runs. Interleaving the two here would deadlock
    // if both shared the single session connection.
    let query = d.query(
        "SELECT COUNT(*) FROM access_log a, access_log b",
        1,
        CancellationToken::new(),
    );
    let catalog = d.schema_snapshot(None);

    let (_, snapshot) = tokio::join!(query, catalog);
    assert!(!snapshot.unwrap().is_empty(), "catalog read starved behind the query");
}

#[tokio::test]
async fn missing_table_is_an_error_not_an_empty_result() {
    let d = driver_or_skip!();
    let result = d
        .describe_table(&TableRef { schema: None, name: "no_such_table".into() })
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn runs_a_query_and_decodes_values() {
    let d = driver_or_skip!();
    let rs = d
        .query(
            "SELECT id, name, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(rs.columns.len(), 3);
    assert_eq!(rs.rows.len(), 5);
    assert!(!rs.truncated);

    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("Ada Lovelace".into()));
    // A NULL bio must decode as Null, not as an empty string.
    assert_eq!(rs.rows[2][2], Value::Null);
}

#[tokio::test]
async fn preserves_unicode_and_embedded_quotes() {
    let d = driver_or_skip!();
    let rs = d
        .query(
            "SELECT name FROM authors WHERE name LIKE '%Brien%' OR name LIKE 'Ólafur%' ORDER BY name",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let names: Vec<String> = rs
        .rows
        .iter()
        .map(|r| match &r[0] {
            Value::Text(s) => s.clone(),
            other => panic!("expected text, got {other:?}"),
        })
        .collect();

    assert!(names.iter().any(|n| n == "Ken O'Brien"));
    assert!(names.iter().any(|n| n.contains("🎉") || n.starts_with("Ólafur")));
}

#[tokio::test]
async fn reports_truncation_when_more_rows_exist() {
    let d = driver_or_skip!();
    let rs = d
        .query("SELECT * FROM access_log", 10, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(rs.rows.len(), 10, "must return exactly the limit, not limit+1");
    assert!(rs.truncated, "5000 rows exist, so this page is truncated");
}

#[tokio::test]
async fn does_not_report_truncation_on_an_exact_fit() {
    let d = driver_or_skip!();
    // Exactly 5 authors, fetched with a limit of 5: the extra probe row finds
    // nothing, so this must not be flagged as truncated.
    let rs = d
        .query("SELECT * FROM authors", 5, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(rs.rows.len(), 5);
    assert!(!rs.truncated);
}

#[tokio::test]
async fn paginates_with_a_stable_offset() {
    let d = driver_or_skip!();
    let dialect = d.dialect();

    let page1 = d
        .query(
            &dialect.paginate("SELECT path FROM access_log ORDER BY path", 5, 0),
            5,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let page2 = d
        .query(
            &dialect.paginate("SELECT path FROM access_log ORDER BY path", 5, 5),
            5,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(page1.rows.len(), 5);
    assert_eq!(page2.rows.len(), 5);
    assert_ne!(page1.rows[0], page2.rows[0], "pages must not overlap");
}

#[tokio::test]
async fn preserves_high_precision_decimals_stored_as_text() {
    let d = driver_or_skip!();
    let rs = d
        .query(
            "SELECT a_decimal_txt FROM type_gallery WHERE a_decimal_txt IS NOT NULL",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // The value exceeds f64 precision. Faro must hand it back byte for byte
    // rather than routing it through a float on the way to the grid.
    match &rs.rows[0][0] {
        Value::Text(s) => assert_eq!(s, "12345678901234567890.0987654321"),
        other => panic!("expected the exact digits to survive, got {other:?}"),
    }
}

#[tokio::test]
async fn documents_sqlite_numeric_affinity_lossiness() {
    let d = driver_or_skip!();
    let rs = d
        .query(
            "SELECT a_numeric FROM type_gallery WHERE a_numeric IS NOT NULL",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Not a Faro bug: SQLite's NUMERIC affinity coerced this to a REAL at
    // INSERT time, so the precision was gone before any driver read it. Pinned
    // as a test so the behaviour is understood rather than rediscovered — and
    // so it is visible if a future change starts papering over it.
    assert!(
        matches!(rs.rows[0][0], Value::Float(_)),
        "expected SQLite to have coerced NUMERIC to REAL, got {:?}",
        rs.rows[0][0]
    );
}

#[tokio::test]
async fn decodes_blobs_as_bytes() {
    let d = driver_or_skip!();
    let rs = d
        .query(
            "SELECT a_blob FROM type_gallery WHERE a_blob IS NOT NULL",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(rs.rows[0][0], Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
}

#[tokio::test]
async fn session_state_survives_across_statements() {
    let d = driver_or_skip!();
    let token = CancellationToken::new();

    // Regression guard for connection pooling. With a pool larger than one,
    // these two statements can land on different sessions and the INSERT fails
    // with "no such table" — a baffling error for the user, since the table was
    // just created. The drivers pin a single connection to prevent it.
    d.execute("CREATE TEMP TABLE t_session (a int)", token.clone())
        .await
        .unwrap();
    d.execute("INSERT INTO t_session VALUES (1)", token.clone())
        .await
        .expect("temp table vanished — the pool is handing out multiple sessions");

    let rs = d
        .query("SELECT a FROM t_session", 10, token)
        .await
        .unwrap();
    assert_eq!(rs.rows.len(), 1);
    assert_eq!(rs.rows[0][0], Value::Int(1));
}

#[tokio::test]
async fn executes_dml_and_reports_affected_rows() {
    let d = driver_or_skip!();
    let token = CancellationToken::new();

    d.execute("CREATE TEMP TABLE t_dml (a int)", token.clone())
        .await
        .unwrap();
    let res = d
        .execute("INSERT INTO t_dml VALUES (1), (2), (3)", token.clone())
        .await
        .unwrap();

    assert_eq!(res.rows_affected, 3);
}

#[tokio::test]
async fn a_pre_cancelled_token_aborts_before_running() {
    let d = driver_or_skip!();
    let token = CancellationToken::new();
    token.cancel();

    let result = d.query("SELECT 1", 10, token).await;
    assert!(
        matches!(result, Err(faro_lib::error::FaroError::Cancelled)),
        "cancellation must short-circuit the query"
    );
}

#[tokio::test]
async fn run_dispatches_between_rows_and_affected() {
    let d = driver_or_skip!();
    let token = CancellationToken::new();

    let rows = d.run("SELECT 1 AS a", 10, token.clone()).await.unwrap();
    assert!(matches!(rows, faro_lib::model::QueryOutcome::Rows(_)));

    d.execute("CREATE TEMP TABLE t_run (a int)", token.clone())
        .await
        .unwrap();
    let affected = d
        .run("INSERT INTO t_run VALUES (1)", 10, token)
        .await
        .unwrap();
    assert!(matches!(affected, faro_lib::model::QueryOutcome::Affected(_)));
}

#[tokio::test]
async fn syntax_errors_surface_the_engine_message() {
    let d = driver_or_skip!();
    let err = d
        .query("SELECT FROM WHERE", 10, CancellationToken::new())
        .await
        .unwrap_err();

    // The user is better served by SQLite's own words than a generic wrapper.
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("syntax"), "unhelpful error: {err}");
}
