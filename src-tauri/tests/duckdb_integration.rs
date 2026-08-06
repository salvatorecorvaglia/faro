//! End-to-end tests against a real DuckDB database.
//!
//! No container and no CLI needed: the fixture is built here from
//! `scripts/seed/duckdb.sql` using the same crate the driver uses, so these run
//! on a clean checkout.

#![cfg(feature = "duckdb-engine")]

use faro_lib::dml;
use faro_lib::driver::{self, Driver};
use faro_lib::model::{
    CellEdit, ConnectionConfig, EditValue, Engine, GuardedStatement, PendingChange, SslMode,
    TableRef, Value,
};
use faro_lib::transfer::backup::{self, BackupOptions, RestoreOptions};
use tokio_util::sync::CancellationToken;

struct Fixture {
    path: std::path::PathBuf,
    extra: Vec<std::path::PathBuf>,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("scripts/seed/duckdb.sql");
        let script = std::fs::read_to_string(&seed).ok()?;

        let path =
            std::env::temp_dir().join(format!("faro_duck_{tag}_{}.duckdb", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // Built with the crate rather than a CLI, so no external tool is needed.
        let conn = duckdb::Connection::open(&path).ok()?;
        conn.execute_batch(&script)
            .unwrap_or_else(|e| panic!("seeding DuckDB failed: {e}"));
        drop(conn);

        Some(Self {
            path,
            extra: vec![],
        })
    }

    fn temp(&mut self, name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("faro_duck_{}_{name}", std::process::id()));
        self.extra.push(p.clone());
        p
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for p in &self.extra {
            let _ = std::fs::remove_file(p);
        }
    }
}

async fn open(f: &Fixture) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "t".into(),
        name: "t".into(),
        engine: Engine::DuckDb,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(f.path.to_string_lossy().into_owned()),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    driver::connect(&config, None)
        .await
        .expect("connect failed")
}

macro_rules! fixture_or_skip {
    ($tag:expr) => {
        match Fixture::new($tag) {
            Some(f) => f,
            None => {
                eprintln!("skipping: could not build the DuckDB fixture");
                return;
            }
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

// -- Connecting and browsing ------------------------------------------------

#[tokio::test]
async fn connects_and_lists_tables() {
    let f = fixture_or_skip!("list");
    let d = open(&f).await;
    d.ping().await.unwrap();

    let names: Vec<String> = d
        .list_tables(None)
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
}

#[tokio::test]
async fn refuses_to_open_a_file_that_does_not_exist() {
    // Opening a mistyped path must be an error, not a silently empty database.
    let config = ConnectionConfig {
        id: "t".into(),
        name: "t".into(),
        engine: Engine::DuckDb,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some("/tmp/faro_definitely_absent.duckdb".into()),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    assert!(driver::connect(&config, None).await.is_err());
}

#[tokio::test]
async fn describes_columns_and_primary_keys() {
    let f = fixture_or_skip!("describe");
    let d = open(&f).await;

    let books = d
        .describe_table(&TableRef {
            schema: None,
            name: "books".into(),
        })
        .await
        .unwrap();
    assert_eq!(books.primary_key, vec!["id"]);
    assert!(books.is_editable());
    assert!(books
        .columns
        .iter()
        .any(|c| c.name == "title" && !c.nullable));

    // Composite key order matters: generated DML depends on it.
    let stores = d
        .describe_table(&TableRef {
            schema: None,
            name: "book_stores".into(),
        })
        .await
        .unwrap();
    assert_eq!(stores.primary_key, vec!["book_id", "store_id"]);
}

#[tokio::test]
async fn a_table_without_a_primary_key_is_not_editable() {
    let f = fixture_or_skip!("nopk");
    let d = open(&f).await;

    let log = d
        .describe_table(&TableRef {
            schema: None,
            name: "access_log".into(),
        })
        .await
        .unwrap();
    assert!(log.primary_key.is_empty());
    assert!(!log.is_editable());
}

#[tokio::test]
async fn schema_snapshot_feeds_autocomplete() {
    let f = fixture_or_skip!("snapshot");
    let d = open(&f).await;

    let snapshot = d.schema_snapshot(None).await.unwrap();
    let books = snapshot.iter().find(|t| t.name == "books").unwrap();
    assert!(books.columns.contains(&"title".to_string()));
}

// -- Querying and decoding -------------------------------------------------

#[tokio::test]
async fn decodes_values_and_preserves_null() {
    let f = fixture_or_skip!("decode");
    let d = open(&f).await;

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
    assert_eq!(rs.rows[0][0], Value::Int(1));
    assert_eq!(rs.rows[0][1], Value::Text("Ada Lovelace".into()));
    // A NULL bio must decode as Null, not as an empty string.
    assert_eq!(rs.rows[2][2], Value::Null);
}

#[tokio::test]
async fn preserves_unicode_and_embedded_quotes() {
    let f = fixture_or_skip!("unicode");
    let d = open(&f).await;

    let rs = d
        .query(
            "SELECT name FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let names: Vec<String> = rs
        .rows
        .iter()
        .filter_map(|r| match &r[0] {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    assert!(names.iter().any(|n| n == "Ken O'Brien"), "{names:?}");
    assert!(names.iter().any(|n| n.contains('Ó')), "{names:?}");
}

#[tokio::test]
async fn integers_beyond_i64_stay_exact() {
    let f = fixture_or_skip!("bignum");
    let d = open(&f).await;

    // UBIGINT at its maximum: wrapping it into an i64 would report -1.
    let ubig = scalar(&*d, "SELECT a_ubigint FROM type_gallery WHERE id = 1").await;
    assert_eq!(ubig, Value::Decimal("18446744073709551615".into()));

    // HUGEINT at i128's maximum, far past any float's precision.
    let huge = scalar(&*d, "SELECT a_hugeint FROM type_gallery WHERE id = 1").await;
    assert_eq!(
        huge,
        Value::Decimal("170141183460469231731687303715884105727".into())
    );
}

#[tokio::test]
async fn decimals_keep_full_precision() {
    let f = fixture_or_skip!("decimal");
    let d = open(&f).await;

    // Unlike SQLite, DuckDB has a real DECIMAL, so the digits must survive.
    match scalar(&*d, "SELECT a_decimal FROM type_gallery WHERE id = 1").await {
        Value::Decimal(s) => assert!(
            s.starts_with("12345678901234567890.09876543"),
            "precision lost: {s}"
        ),
        other => panic!("expected a decimal, got {other:?}"),
    }
}

#[tokio::test]
async fn decodes_blobs_booleans_and_dates() {
    let f = fixture_or_skip!("types");
    let d = open(&f).await;

    assert_eq!(
        scalar(&*d, "SELECT a_blob FROM type_gallery WHERE id = 1").await,
        Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
    );
    assert_eq!(
        scalar(&*d, "SELECT a_bool FROM type_gallery WHERE id = 1").await,
        Value::Bool(true)
    );

    match scalar(&*d, "SELECT a_date FROM type_gallery WHERE id = 1").await {
        Value::Date(s) => assert_eq!(s, "2026-08-05"),
        other => panic!("expected a date, got {other:?}"),
    }
}

#[tokio::test]
async fn reports_truncation_and_pages_correctly() {
    let f = fixture_or_skip!("paging");
    let d = open(&f).await;

    let page = d
        .query("SELECT * FROM access_log", 10, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 10, "must return exactly the limit");
    assert!(page.truncated, "5000 rows exist, so this page is truncated");

    // An exact fit must not be reported as truncated.
    let exact = d
        .query("SELECT * FROM authors", 5, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(exact.rows.len(), 5);
    assert!(!exact.truncated);

    // Pages must not overlap.
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
    assert_ne!(p1.rows[0], p2.rows[0]);
}

#[tokio::test]
async fn a_pre_cancelled_token_aborts_before_running() {
    let f = fixture_or_skip!("cancel");
    let d = open(&f).await;

    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        d.query("SELECT 1", 10, token).await,
        Err(faro_lib::error::FaroError::Cancelled)
    ));
}

#[tokio::test]
async fn a_syntax_error_surfaces_the_engine_message() {
    let f = fixture_or_skip!("syntax");
    let d = open(&f).await;

    let err = d
        .query("SELECT FROM WHERE", 10, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("syntax"),
        "unhelpful error: {err}"
    );
}

// -- Editing ----------------------------------------------------------------

async fn apply(
    d: &dyn Driver,
    table: &str,
    changes: &[PendingChange],
) -> faro_lib::error::Result<u64> {
    let table_ref = TableRef {
        schema: None,
        name: table.into(),
    };
    let detail = d.describe_table(&table_ref).await.unwrap();
    let statements = dml::build_statements(&table_ref, &detail, changes, d.dialect())?;
    d.apply_transaction(&statements).await
}

#[tokio::test]
async fn an_edit_persists() {
    let f = fixture_or_skip!("edit");
    let d = open(&f).await;

    apply(
        &*d,
        "authors",
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("name", "Ada Byron")],
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(&*d, "SELECT name FROM authors WHERE id = 1").await,
        Value::Text("Ada Byron".into())
    );
}

#[tokio::test]
async fn a_failed_batch_rolls_back_entirely() {
    let f = fixture_or_skip!("rollback");
    let d = open(&f).await;
    let original = scalar(&*d, "SELECT name FROM authors WHERE id = 1").await;

    let result = apply(
        &*d,
        "authors",
        &[
            PendingChange::Update {
                key: vec![cell("id", "1")],
                cells: vec![cell("name", "Should Not Persist")],
            },
            // No such row, so the guard trips and the batch must roll back.
            PendingChange::Update {
                key: vec![cell("id", "999999")],
                cells: vec![cell("name", "Ghost")],
            },
        ],
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
        scalar(&*d, "SELECT name FROM authors WHERE id = 1").await,
        original,
        "the valid change committed despite the batch failing"
    );
}

#[tokio::test]
async fn the_row_count_guard_fires_on_an_over_broad_statement() {
    let f = fixture_or_skip!("guard");
    let d = open(&f).await;

    // Hand-built: `dml` cannot produce this, which is the point — the guard is
    // a second line of defence behind the generator.
    let unguarded = vec![GuardedStatement {
        sql: "UPDATE authors SET bio = 'clobbered'".into(),
        expect: Some(1),
    }];

    let err = d.apply_transaction(&unguarded).await.unwrap_err();
    assert!(
        err.to_string().contains("does not identify a single row"),
        "{err}"
    );

    assert_eq!(
        scalar(&*d, "SELECT COUNT(*) FROM authors WHERE bio = 'clobbered'").await,
        Value::Int(0),
        "the rollback did not undo the over-broad update"
    );
}

// -- Backup -----------------------------------------------------------------

#[tokio::test]
async fn a_duckdb_database_backs_up_and_restores() {
    let mut f = fixture_or_skip!("backup");
    let dump = f.temp("dump.sql");
    let target = f.temp("restored.duckdb");

    let src = open(&f).await;
    let result = backup::write_backup(
        &*src,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "authors".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(result.rows, 5);

    // Restore into a fresh database.
    duckdb::Connection::open(&target).unwrap();
    let config = ConnectionConfig {
        id: "t2".into(),
        name: "t2".into(),
        engine: Engine::DuckDb,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(target.to_string_lossy().into_owned()),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    let dst = driver::connect(&config, None).await.unwrap();

    let script = std::fs::read_to_string(&dump).unwrap();
    let restored = backup::restore(
        &*dst,
        &script,
        &RestoreOptions {
            stop_on_error: true,
        },
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(restored.failed, 0, "{:#?}", restored.errors);

    assert_eq!(
        scalar(&*dst, "SELECT COUNT(*) FROM authors").await,
        Value::Int(5)
    );
    // The awkward values must have survived.
    assert_eq!(
        scalar(&*dst, "SELECT name FROM authors WHERE id = 4").await,
        Value::Text("Ken O'Brien".into())
    );
}
