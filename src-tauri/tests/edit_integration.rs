//! Editing against a real database.
//!
//! These are the tests that matter most in Phase 4: they prove that the
//! transaction actually rolls back, that the row-count guard actually fires,
//! and that a failed batch leaves the data exactly as it was.
//!
//! Each test works on its own copy of the fixture, so a test that writes
//! cannot affect another. Run `./scripts/seed.sh` first.

use faro_lib::dml;
use faro_lib::driver::{self, Driver};
use faro_lib::error::FaroError;
use faro_lib::model::{
    CellEdit, ConnectionConfig, EditValue, Engine, GuardedStatement, PendingChange, SslMode,
    TableRef, Value,
};
use faro_lib::registry::Registry;
use tokio_util::sync::CancellationToken;

/// Copy the fixture so each test owns its data.
struct Fixture {
    path: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("tests/fixtures/faro_test.db");
        if !source.exists() {
            return None;
        }
        let path = std::env::temp_dir().join(format!("faro_edit_{tag}_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::copy(&source, &path).ok()?;
        Some(Self { path })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn open(fixture: &Fixture) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(fixture.path.to_string_lossy().into_owned()),
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
                eprintln!("skipping: run ./scripts/seed.sh to create the fixture");
                return;
            }
        }
    };
}

fn cell(column: &str, text: &str) -> CellEdit {
    CellEdit {
        column: column.into(),
        value: EditValue::Text(text.into()),
    }
}

async fn scalar(d: &dyn Driver, sql: &str) -> Value {
    let rs = d.query(sql, 1, CancellationToken::new()).await.unwrap();
    rs.rows
        .first()
        .and_then(|r| r.first())
        .cloned()
        .unwrap_or(Value::Null)
}

async fn count(d: &dyn Driver, table: &str) -> i64 {
    match scalar(d, &format!("SELECT COUNT(*) FROM {table}")).await {
        Value::Int(n) => n,
        other => panic!("expected a count, got {other:?}"),
    }
}

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

// -- The happy path --------------------------------------------------------

#[tokio::test]
async fn an_update_persists() {
    let f = fixture_or_skip!("update");
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
async fn an_insert_and_delete_round_trip() {
    let f = fixture_or_skip!("insert_delete");
    let d = open(&f).await;
    let before = count(&*d, "authors").await;

    apply(
        &*d,
        "authors",
        &[PendingChange::Insert {
            cells: vec![
                CellEdit {
                    column: "id".into(),
                    value: EditValue::Default,
                },
                cell("name", "Temp Author"),
            ],
        }],
    )
    .await
    .unwrap();
    assert_eq!(count(&*d, "authors").await, before + 1);

    let new_id = match scalar(&*d, "SELECT id FROM authors WHERE name = 'Temp Author'").await {
        Value::Int(n) => n,
        other => panic!("expected an id, got {other:?}"),
    };

    apply(
        &*d,
        "authors",
        &[PendingChange::Delete {
            key: vec![cell("id", &new_id.to_string())],
        }],
    )
    .await
    .unwrap();
    assert_eq!(count(&*d, "authors").await, before);
}

#[tokio::test]
async fn writing_null_clears_the_column() {
    let f = fixture_or_skip!("null");
    let d = open(&f).await;

    apply(
        &*d,
        "authors",
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![CellEdit {
                column: "bio".into(),
                value: EditValue::Null,
            }],
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(&*d, "SELECT bio FROM authors WHERE id = 1").await,
        Value::Null
    );
}

#[tokio::test]
async fn an_empty_string_is_stored_as_an_empty_string_not_null() {
    let f = fixture_or_skip!("empty");
    let d = open(&f).await;

    apply(
        &*d,
        "authors",
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("bio", "")],
        }],
    )
    .await
    .unwrap();

    // The NULL-versus-empty distinction the UI forces must survive the write.
    assert_eq!(
        scalar(&*d, "SELECT bio FROM authors WHERE id = 1").await,
        Value::Text(String::new())
    );
}

#[tokio::test]
async fn a_composite_key_addresses_exactly_one_row() {
    let f = fixture_or_skip!("composite");
    let d = open(&f).await;

    apply(
        &*d,
        "book_stores",
        &[PendingChange::Update {
            key: vec![cell("book_id", "1"), cell("store_id", "1")],
            cells: vec![cell("quantity", "99")],
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(
            &*d,
            "SELECT quantity FROM book_stores WHERE book_id = 1 AND store_id = 1"
        )
        .await,
        Value::Int(99)
    );
    // The sibling row must be untouched.
    assert_eq!(
        scalar(
            &*d,
            "SELECT quantity FROM book_stores WHERE book_id = 1 AND store_id = 2"
        )
        .await,
        Value::Int(0)
    );
}

#[tokio::test]
async fn quotes_and_unicode_survive_a_write() {
    let f = fixture_or_skip!("unicode");
    let d = open(&f).await;
    let tricky = "O'Brien — 日本語 🎉 \"quoted\"";

    apply(
        &*d,
        "authors",
        &[PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("name", tricky)],
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(&*d, "SELECT name FROM authors WHERE id = 1").await,
        Value::Text(tricky.into())
    );
}

// -- The safety net --------------------------------------------------------

#[tokio::test]
async fn a_batch_rolls_back_entirely_when_one_change_fails() {
    let f = fixture_or_skip!("rollback");
    let d = open(&f).await;
    let original = scalar(&*d, "SELECT name FROM authors WHERE id = 1").await;

    // The first change is valid; the second targets a row that does not exist.
    let result = apply(
        &*d,
        "authors",
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

    assert!(result.is_err(), "a missing row must abort the batch");
    assert_eq!(
        scalar(&*d, "SELECT name FROM authors WHERE id = 1").await,
        original,
        "the valid change was committed despite the batch failing"
    );
}

#[tokio::test]
async fn updating_a_vanished_row_reports_it_clearly() {
    let f = fixture_or_skip!("vanished");
    let d = open(&f).await;

    let err = apply(
        &*d,
        "authors",
        &[PendingChange::Update {
            key: vec![cell("id", "999999")],
            cells: vec![cell("name", "Ghost")],
        }],
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("no longer matches"),
        "unhelpful message: {msg}"
    );
    assert!(msg.contains("Nothing has been saved"), "{msg}");
}

#[tokio::test]
async fn the_guard_fires_when_a_statement_would_hit_many_rows() {
    let f = fixture_or_skip!("many");
    let d = open(&f).await;
    let before = count(&*d, "authors").await;

    // Hand-built to simulate a key that fails to identify one row — the case
    // the guard exists to catch. `dml` cannot produce this, which is the point:
    // the guard is a second line of defence behind the generator.
    let unguarded = vec![GuardedStatement {
        sql: "UPDATE authors SET bio = 'clobbered'".into(),
        expect: Some(1),
    }];

    let err = d.apply_transaction(&unguarded).await.unwrap_err();
    assert!(
        err.to_string().contains("does not identify a single row"),
        "{err}"
    );

    // Every row must be untouched.
    assert_eq!(count(&*d, "authors").await, before);
    assert_eq!(
        count(&*d, "authors WHERE bio = 'clobbered'").await,
        0,
        "the rollback did not undo the over-broad update"
    );
}

#[tokio::test]
async fn a_table_without_a_primary_key_is_refused() {
    let f = fixture_or_skip!("nopk");
    let d = open(&f).await;
    let before = count(&*d, "access_log").await;

    let err = apply(
        &*d,
        "access_log",
        &[PendingChange::Update {
            key: vec![cell("path", "/page/1")],
            cells: vec![cell("status", "500")],
        }],
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("no primary key"), "{err}");
    assert_eq!(count(&*d, "access_log").await, before);
}

#[tokio::test]
async fn a_constraint_violation_rolls_back_the_whole_batch() {
    let f = fixture_or_skip!("constraint");
    let d = open(&f).await;
    let before = count(&*d, "authors").await;

    // `email` is UNIQUE; the second insert collides with the first.
    let result = apply(
        &*d,
        "authors",
        &[
            PendingChange::Insert {
                cells: vec![cell("name", "One"), cell("email", "dup@example.com")],
            },
            PendingChange::Insert {
                cells: vec![cell("name", "Two"), cell("email", "dup@example.com")],
            },
        ],
    )
    .await;

    assert!(result.is_err(), "a unique violation must fail the batch");
    assert_eq!(
        count(&*d, "authors").await,
        before,
        "the first insert survived a failed batch"
    );
}

#[tokio::test]
async fn a_type_error_is_reported_by_the_database_not_silently_coerced() {
    let f = fixture_or_skip!("typed");
    let d = open(&f).await;

    // Text in an integer column is emitted quoted so the engine decides. SQLite
    // is permissive and stores it; the point is that Faro does not silently
    // rewrite the value to something the user did not type.
    apply(
        &*d,
        "book_stores",
        &[PendingChange::Update {
            key: vec![cell("book_id", "1"), cell("store_id", "1")],
            cells: vec![cell("quantity", "not a number")],
        }],
    )
    .await
    .unwrap();

    let stored = scalar(
        &*d,
        "SELECT quantity FROM book_stores WHERE book_id = 1 AND store_id = 1",
    )
    .await;
    assert_eq!(
        stored,
        Value::Text("not a number".into()),
        "the value should be stored as typed, not coerced to 0"
    );
}

#[tokio::test]
async fn deletes_run_before_inserts_so_a_key_can_be_reused() {
    let f = fixture_or_skip!("reuse");
    let d = open(&f).await;

    // Replacing a row with a new one under the same primary key in a single
    // batch only works if the delete is ordered first.
    apply(
        &*d,
        "book_stores",
        &[
            PendingChange::Insert {
                cells: vec![
                    cell("book_id", "1"),
                    cell("store_id", "1"),
                    cell("quantity", "7"),
                ],
            },
            PendingChange::Delete {
                key: vec![cell("book_id", "1"), cell("store_id", "1")],
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        scalar(
            &*d,
            "SELECT quantity FROM book_stores WHERE book_id = 1 AND store_id = 1"
        )
        .await,
        Value::Int(7)
    );
}

#[tokio::test]
async fn an_empty_change_set_is_a_no_op() {
    let f = fixture_or_skip!("noop");
    let d = open(&f).await;
    let before = count(&*d, "authors").await;

    assert_eq!(apply(&*d, "authors", &[]).await.unwrap(), 0);
    assert_eq!(count(&*d, "authors").await, before);
}

// -- Read-only connections ---------------------------------------------------

/// Open the fixture with `read_only` set, as the connection dialog's checkbox does.
async fn open_read_only(f: &Fixture) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "test".into(),
        name: "Production".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(f.path.to_string_lossy().into_owned()),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: true,
    };
    driver::connect(&config, None)
        .await
        .expect("connect failed")
}

#[tokio::test]
async fn the_registry_refuses_to_hand_out_a_read_only_connection_for_writing() {
    let f = fixture_or_skip!("ro_registry");
    let d = open_read_only(&f).await;

    // This is the guard every write command goes through, so it is the single
    // point where "Open read-only" either holds or does not.
    let registry = Registry::new();
    registry
        .insert(
            "c1".into(),
            std::sync::Arc::from(d),
            true,
            "Production".into(),
        )
        .await;

    assert!(
        registry.get("c1").await.is_ok(),
        "reads must still be allowed"
    );

    // `Arc<dyn Driver>` is not Debug, so unwrap the Result by hand.
    let Err(err) = registry.get_writable("c1").await else {
        panic!("a read-only connection must not be handed out for writing");
    };
    assert!(matches!(err, FaroError::ReadOnly(_)), "got {err:?}");
    // The message has to name the connection and say how to change it, or the
    // user is left guessing why an edit did nothing.
    let text = err.to_string();
    assert!(text.contains("Production"), "{text}");
    assert!(text.contains("Open read-only"), "{text}");
}

#[tokio::test]
async fn a_writable_connection_is_handed_out_normally() {
    let f = fixture_or_skip!("ro_writable");
    let d = open(&f).await;

    let registry = Registry::new();
    registry
        .insert("c1".into(), std::sync::Arc::from(d), false, "Local".into())
        .await;

    assert!(registry.get_writable("c1").await.is_ok());
    assert!(!registry.is_read_only("c1").await);
}

#[tokio::test]
async fn the_engine_refuses_writes_on_a_read_only_connection_too() {
    let f = fixture_or_skip!("ro_engine");
    let d = open_read_only(&f).await;

    // Defence in depth: even if a write slipped past Faro's own guard, SQLite
    // itself was opened read-only and rejects it. Reads keep working.
    let before = count(&*d, "authors").await;
    assert!(before > 0, "the fixture should have authors to read");

    let err = d
        .execute("DELETE FROM authors WHERE id = 1", CancellationToken::new())
        .await
        .expect_err("the engine should reject a write on a read-only connection");
    assert!(
        err.to_string().to_lowercase().contains("readonly")
            || err.to_string().to_lowercase().contains("read-only"),
        "expected a read-only error, got: {err}"
    );

    assert_eq!(count(&*d, "authors").await, before, "the row was deleted");
}
