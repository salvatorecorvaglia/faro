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
        tokio_util::sync::CancellationToken::new(),
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
        tokio_util::sync::CancellationToken::new(),
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

#[tokio::test]
async fn a_cancelled_backup_leaves_no_file_that_looks_finished() {
    // The header and schema are written before the data, so a backup stopped
    // partway used to leave a plausible-looking dump missing its rows.
    let mut f = fixture_or_skip!("backup_cancelled");
    let dump = f.temp("cancelled.sql");
    let d = open(&f).await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = backup::write_backup(
        &*d,
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
        cancel,
        |_| {},
    )
    .await
    .unwrap_err();

    assert!(
        matches!(err, faro_lib::error::FaroError::Cancelled),
        "{err}"
    );
    assert!(!dump.exists(), "a partial backup was left at the target");
    let partial = dump.with_file_name(format!(
        ".{}.partial",
        dump.file_name().unwrap().to_string_lossy()
    ));
    assert!(!partial.exists(), "the temporary file was left behind");
}

#[tokio::test]
async fn a_backup_refuses_a_column_it_cannot_read_rather_than_dumping_its_type_name() {
    // GEOMETRY decodes to `Value::Unsupported("GEOMETRY")`. Dumped as a
    // literal that became the string 'GEOMETRY' in every row — a backup that
    // restores cleanly and is silently not the table. (This test used
    // INTERVAL until that type learned to decode.)
    let mut f = fixture_or_skip!("backup_unsupported");
    let dump = f.temp("unsupported.sql");
    let d = open(&f).await;
    d.execute(
        "CREATE TABLE spans (id INTEGER PRIMARY KEY, span GEOMETRY);
         INSERT INTO spans VALUES (1, 'POINT(1 2)'::GEOMETRY)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let err = backup::write_backup(
        &*d,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "spans".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("\"spans\""), "{message}");
    assert!(message.contains("\"span\""), "{message}");
}

// -- Streaming import -------------------------------------------------------

async fn stream(
    d: &dyn Driver,
    batches: Vec<Vec<GuardedStatement>>,
    finish: bool,
) -> faro_lib::error::Result<u64> {
    use faro_lib::model::ApplyChunk;
    let (tx, rx) = tokio::sync::mpsc::channel::<ApplyChunk>(2);
    let producer = tokio::spawn(async move {
        for batch in batches {
            if tx.send(ApplyChunk::Batch(batch)).await.is_err() {
                return;
            }
        }
        if finish {
            let _ = tx.send(ApplyChunk::Done).await;
        }
    });
    let applied = d.apply_streaming(rx).await;
    producer.await.unwrap();
    applied
}

fn insert(id: i64) -> GuardedStatement {
    GuardedStatement {
        sql: format!("INSERT INTO streamed VALUES ({id})"),
        expect: None,
    }
}

#[tokio::test]
async fn a_streamed_import_commits_every_batch_once_done() {
    let f = fixture_or_skip!("stream_commit");
    let d = open(&f).await;
    d.execute(
        "CREATE TABLE streamed (id INTEGER)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    // More batches than the channel holds, so the applier must keep draining
    // while the producer is still sending.
    let batches = (0..10)
        .map(|b| (0..50).map(|i| insert(b * 50 + i)).collect())
        .collect();
    stream(&*d, batches, true).await.unwrap();

    assert_eq!(
        scalar(&*d, "SELECT COUNT(*) FROM streamed").await,
        Value::Int(500)
    );
}

#[tokio::test]
async fn a_streamed_import_that_never_finishes_rolls_back() {
    // The reader stopping partway — a malformed row, a cancel — drops the
    // sender without `Done`. Committing what had arrived would load half a file.
    let f = fixture_or_skip!("stream_rollback");
    let d = open(&f).await;
    d.execute(
        "CREATE TABLE streamed (id INTEGER)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let err = stream(&*d, vec![vec![insert(1), insert(2)]], false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("nothing was applied"), "{err}");

    assert_eq!(
        scalar(&*d, "SELECT COUNT(*) FROM streamed").await,
        Value::Int(0)
    );
    // And the connection is usable again: no transaction was left open.
    d.execute("INSERT INTO streamed VALUES (3)", CancellationToken::new())
        .await
        .unwrap();
}

// -- Nested types and cancellation -----------------------------------------

#[tokio::test]
async fn nested_enum_and_interval_values_decode_instead_of_showing_their_type() {
    // These all came back as `Unsupported`: the grid showed only "LIST" or
    // "STRUCT", and the column could be neither exported nor backed up.
    let f = fixture_or_skip!("nested");
    let d = open(&f).await;
    d.execute(
        "CREATE TYPE mood AS ENUM ('calm', 'busy');
         CREATE TABLE nested (
           id INTEGER PRIMARY KEY, tags VARCHAR[], point STRUCT(x INTEGER, y INTEGER),
           attrs MAP(VARCHAR, INTEGER), m mood, span INTERVAL
         );
         INSERT INTO nested VALUES (
           1, ['a', 'b'], {'x': 1, 'y': 2}, MAP {'k': 7}, 'busy',
           INTERVAL '1 year 2 months 3 days 04:05:06.5'
         )",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let rs = d
        .query(
            "SELECT tags, point, attrs, m, span FROM nested",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let row = &rs.rows[0];
    assert_eq!(row[0], Value::Json(serde_json::json!(["a", "b"])));
    assert_eq!(row[1], Value::Json(serde_json::json!({"x": 1, "y": 2})));
    assert_eq!(row[2], Value::Json(serde_json::json!({"k": 7})));
    assert_eq!(row[3], Value::Text("busy".into()));
    assert_eq!(
        row[4],
        Value::Text("1 year 2 months 3 days 04:05:06.5".into())
    );
}

#[tokio::test]
async fn a_backup_of_nested_and_interval_columns_restores_to_the_same_values() {
    // Once these decode, a dump writes them as text; the restore must read
    // that text back as the same values, not just load without error.
    let mut f = fixture_or_skip!("nested_backup");
    let dump = f.temp("nested.sql");
    let target = f.temp("nested_restored.duckdb");
    let src = open(&f).await;
    src.execute(
        "CREATE TABLE nested (
           id INTEGER PRIMARY KEY, tags VARCHAR[], point STRUCT(x INTEGER, y INTEGER),
           attrs MAP(VARCHAR, INTEGER), span INTERVAL
         );
         INSERT INTO nested VALUES
           (1, ['a', 'b'], {'x': 1, 'y': 2}, MAP {'k': 7},
            INTERVAL '1 year 2 months 3 days 04:05:06.5'),
           (2, [], {'x': -1, 'y': 0}, MAP {}, INTERVAL 0 SECOND)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    backup::write_backup(
        &*src,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "nested".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

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
    let restored = backup::restore(
        &*dst,
        &std::fs::read_to_string(&dump).unwrap(),
        &RestoreOptions {
            stop_on_error: true,
        },
        CancellationToken::new(),
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(restored.failed, 0, "{:#?}", restored.errors);

    // Compared in the database, as values of their real types.
    assert_eq!(nested_rows(&*dst).await, nested_rows(&*src).await);
}

/// The `nested` table's rows, each nested value rendered by DuckDB itself.
async fn nested_rows(d: &dyn Driver) -> Vec<Vec<Value>> {
    d.query(
        "SELECT id, tags::VARCHAR, point::VARCHAR, attrs::VARCHAR, span::VARCHAR \
         FROM nested ORDER BY id",
        10,
        CancellationToken::new(),
    )
    .await
    .unwrap()
    .rows
}

#[tokio::test]
async fn a_timestamptz_keeps_its_offset_and_restores_to_the_same_instant() {
    // TIMESTAMPTZ values arrived like plain timestamps — `2024-01-01 08:00:00`,
    // with nothing to say it was UTC — so a dump restored them as that wall
    // time in the session's zone, moving every value by the difference.
    let mut f = fixture_or_skip!("tstz");
    let dump = f.temp("tstz.sql");
    let target = f.temp("tstz_restored.duckdb");
    let src = open(&f).await;
    src.execute(
        "SET TimeZone = 'Europe/Rome';
         CREATE TABLE events (id INTEGER PRIMARY KEY, happened TIMESTAMPTZ, plain TIMESTAMP);
         INSERT INTO events VALUES (1, '2024-01-01 10:00:00+02', '2024-01-01 10:00:00')",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let rs = src
        .query(
            "SELECT happened, plain FROM events",
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(rs.columns[0].type_name, "TIMESTAMPTZ");
    assert_eq!(
        rs.rows[0][0],
        Value::Timestamp("2024-01-01T08:00:00+00:00".into())
    );
    // A plain TIMESTAMP is a wall time with no zone, and stays exactly that.
    assert_eq!(
        rs.rows[0][1],
        Value::Timestamp("2024-01-01 10:00:00".into())
    );

    backup::write_backup(
        &*src,
        &dump,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "events".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    duckdb::Connection::open(&target).unwrap();
    let mut config = ConnectionConfig {
        id: "t2".into(),
        name: "t2".into(),
        engine: Engine::DuckDb,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: None,
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    config.file_path = Some(target.to_string_lossy().into_owned());
    let dst = driver::connect(&config, None).await.unwrap();
    // Restored under a different session zone: an instant must not move.
    dst.execute(
        "SET TimeZone = 'America/New_York'",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    backup::restore(
        &*dst,
        &std::fs::read_to_string(&dump).unwrap(),
        &RestoreOptions {
            stop_on_error: true,
        },
        CancellationToken::new(),
        |_, _| {},
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(&*dst, "SELECT epoch(happened) FROM events").await,
        scalar(&*src, "SELECT epoch(happened) FROM events").await,
        "the instant moved across the restore"
    );
}

#[tokio::test]
async fn unsigned_hugeints_decode_like_their_signed_sibling() {
    let f = fixture_or_skip!("uhugeint");
    let d = open(&f).await;
    assert_eq!(scalar(&*d, "SELECT 42::UHUGEINT").await, Value::Int(42));
    // Past i64, kept as exact digits rather than wrapped or rounded.
    assert_eq!(
        scalar(
            &*d,
            "SELECT 340282366920938463463374607431768211455::UHUGEINT"
        )
        .await,
        Value::Decimal("340282366920938463463374607431768211455".into())
    );
}

#[tokio::test]
async fn cancelling_a_long_query_stops_it_and_frees_the_connection() {
    // Cancel used to do nothing on DuckDB: the statement ran to completion on
    // its blocking thread while the UI waited, holding the connection's lock.
    let f = fixture_or_skip!("duck_cancel");
    let d: std::sync::Arc<dyn Driver> = std::sync::Arc::from(open(&f).await);
    let cancel = CancellationToken::new();

    let running = {
        let d = d.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            // Minutes of work, far longer than the test allows.
            d.query(
                "SELECT count(*) FROM range(100000000) a, range(100000) b",
                1,
                cancel,
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    cancel.cancel();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("the cancelled query kept running")
        .unwrap();
    assert!(
        matches!(outcome, Err(faro_lib::error::FaroError::Cancelled)),
        "{outcome:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "stopping took too long"
    );

    // And the connection works again straight away.
    assert_eq!(scalar(&*d, "SELECT 42").await, Value::Int(42));
}
