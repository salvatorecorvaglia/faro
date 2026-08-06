//! Backing up a real database and restoring it into an empty one.
//!
//! This is the phase's exit criterion. Run `./scripts/seed.sh` first.

use faro_lib::driver::{self, Driver};
use faro_lib::model::{ConnectionConfig, Engine, SslMode, Value};
use faro_lib::transfer::backup::{self, BackupOptions, RestoreOptions};
use tokio_util::sync::CancellationToken;

struct Fixture {
    /// A copy of the seeded database, to back up from.
    source: std::path::PathBuf,
    /// An empty database, to restore into.
    target: std::path::PathBuf,
    dump: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("tests/fixtures/faro_test.db");
        if !seed.exists() {
            return None;
        }

        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let source = dir.join(format!("faro_bk_src_{tag}_{pid}.db"));
        let target = dir.join(format!("faro_bk_dst_{tag}_{pid}.db"));
        let dump = dir.join(format!("faro_bk_{tag}_{pid}.sql"));

        for p in [&source, &target, &dump] {
            let _ = std::fs::remove_file(p);
        }
        std::fs::copy(&seed, &source).ok()?;

        // An empty SQLite file is a zero-byte file; the driver refuses to
        // create one, so make it explicitly.
        std::fs::File::create(&target).ok()?;

        Some(Self {
            source,
            target,
            dump,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for p in [&self.source, &self.target, &self.dump] {
            let _ = std::fs::remove_file(p);
        }
    }
}

async fn open(path: &std::path::Path) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "t".into(),
        name: "t".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(path.to_string_lossy().into_owned()),
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

async fn count(d: &dyn Driver, table: &str) -> i64 {
    let rs = d
        .query(
            &format!("SELECT COUNT(*) FROM {table}"),
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    match &rs.rows[0][0] {
        Value::Int(n) => *n,
        other => panic!("expected a count, got {other:?}"),
    }
}

fn options() -> BackupOptions {
    BackupOptions {
        tables: vec![],
        include_schema: true,
        include_data: true,
        drop_existing: false,
    }
}

#[tokio::test]
async fn a_database_backs_up_and_restores_into_an_empty_one() {
    let f = fixture_or_skip!("roundtrip");
    let src = open(&f.source).await;

    // Record what the source holds.
    let expected: Vec<(String, i64)> = {
        let mut out = Vec::new();
        for t in [
            "authors",
            "books",
            "book_stores",
            "access_log",
            "type_gallery",
        ] {
            out.push((t.to_string(), count(&*src, t).await));
        }
        out
    };

    let result = backup::write_backup(&*src, &f.dump, &options(), |_| {})
        .await
        .unwrap();

    assert!(
        result.tables >= 5,
        "expected every table, got {}",
        result.tables
    );
    assert!(
        result.rows > 5000,
        "expected the 5000-row log table, got {}",
        result.rows
    );
    assert!(result.bytes > 0);
    src.close().await;

    // Restore into the empty database.
    let dst = open(&f.target).await;
    let script = std::fs::read_to_string(&f.dump).unwrap();
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

    assert_eq!(
        restored.failed, 0,
        "restore reported failures: {:#?}",
        restored.errors
    );

    // Every table must come back with the same number of rows.
    for (table, n) in expected {
        assert_eq!(
            count(&*dst, &table).await,
            n,
            "row count differs for {table}"
        );
    }
}

#[tokio::test]
async fn restored_values_match_including_awkward_ones() {
    let f = fixture_or_skip!("values");
    let src = open(&f.source).await;
    backup::write_backup(&*src, &f.dump, &options(), |_| {})
        .await
        .unwrap();
    src.close().await;

    let dst = open(&f.target).await;
    let script = std::fs::read_to_string(&f.dump).unwrap();
    backup::restore(
        &*dst,
        &script,
        &RestoreOptions {
            stop_on_error: true,
        },
        |_, _| {},
    )
    .await
    .unwrap();

    let rs = dst
        .query(
            "SELECT name, bio FROM authors ORDER BY id",
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

    assert!(
        names.iter().any(|n| n == "Ken O'Brien"),
        "apostrophe lost: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains('Ó')),
        "unicode lost: {names:?}"
    );
    // The NULL bio must still be NULL, not the string "NULL".
    assert!(
        rs.rows.iter().any(|r| r[1] == Value::Null),
        "NULL did not survive the round trip"
    );
}

#[tokio::test]
async fn blobs_survive_the_round_trip() {
    let f = fixture_or_skip!("blobs");
    let src = open(&f.source).await;
    backup::write_backup(&*src, &f.dump, &options(), |_| {})
        .await
        .unwrap();
    src.close().await;

    let dst = open(&f.target).await;
    let script = std::fs::read_to_string(&f.dump).unwrap();
    backup::restore(
        &*dst,
        &script,
        &RestoreOptions {
            stop_on_error: true,
        },
        |_, _| {},
    )
    .await
    .unwrap();

    let rs = dst
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
async fn the_restored_schema_carries_keys_and_indexes() {
    let f = fixture_or_skip!("schema");
    let src = open(&f.source).await;
    backup::write_backup(&*src, &f.dump, &options(), |_| {})
        .await
        .unwrap();
    src.close().await;

    let dst = open(&f.target).await;
    let script = std::fs::read_to_string(&f.dump).unwrap();
    backup::restore(
        &*dst,
        &script,
        &RestoreOptions {
            stop_on_error: true,
        },
        |_, _| {},
    )
    .await
    .unwrap();

    let books = dst
        .describe_table(&faro_lib::model::TableRef {
            schema: None,
            name: "books".into(),
        })
        .await
        .unwrap();
    assert_eq!(books.primary_key, vec!["id"]);
    assert!(
        books.is_editable(),
        "the restored table lost its primary key"
    );

    // The composite key must come back in order, or generated DML would be wrong.
    let stores = dst
        .describe_table(&faro_lib::model::TableRef {
            schema: None,
            name: "book_stores".into(),
        })
        .await
        .unwrap();
    assert_eq!(stores.primary_key, vec!["book_id", "store_id"]);

    // A non-primary index should have been recreated.
    assert!(
        books.indexes.iter().any(|i| !i.is_primary),
        "secondary indexes were not restored: {:#?}",
        books.indexes
    );
}

#[tokio::test]
async fn selecting_tables_limits_what_is_dumped() {
    let f = fixture_or_skip!("selective");
    let src = open(&f.source).await;

    let mut o = options();
    o.tables = vec![faro_lib::model::TableRef {
        schema: None,
        name: "authors".into(),
    }];
    let result = backup::write_backup(&*src, &f.dump, &o, |_| {})
        .await
        .unwrap();

    assert_eq!(result.tables, 1);
    assert_eq!(result.rows, 5);

    let script = std::fs::read_to_string(&f.dump).unwrap();
    assert!(script.contains("authors"));
    assert!(
        !script.contains("access_log"),
        "an unselected table leaked into the dump"
    );
}

#[tokio::test]
async fn a_schema_only_backup_has_no_inserts() {
    let f = fixture_or_skip!("schemaonly");
    let src = open(&f.source).await;

    let mut o = options();
    o.include_data = false;
    let result = backup::write_backup(&*src, &f.dump, &o, |_| {})
        .await
        .unwrap();
    assert_eq!(result.rows, 0);

    let script = std::fs::read_to_string(&f.dump).unwrap();
    assert!(script.contains("CREATE TABLE"));
    assert!(
        !script.contains("INSERT INTO"),
        "data leaked into a schema-only dump"
    );
}

#[tokio::test]
async fn progress_is_reported_while_backing_up() {
    let f = fixture_or_skip!("progress");
    let src = open(&f.source).await;

    let mut updates = Vec::new();
    backup::write_backup(&*src, &f.dump, &options(), |p| {
        updates.push((p.table.clone(), p.rows_written));
    })
    .await
    .unwrap();

    // Without progress, a large backup is indistinguishable from a hang.
    assert!(!updates.is_empty(), "no progress was reported");
    assert!(
        updates.iter().any(|(t, _)| t == "access_log"),
        "the 5000-row table reported no progress: {updates:?}"
    );
}

#[tokio::test]
async fn restore_stops_at_the_first_error_by_default() {
    let f = fixture_or_skip!("stoponerror");
    let dst = open(&f.target).await;

    let script = "CREATE TABLE ok (a int);\nTHIS IS NOT SQL;\nCREATE TABLE after (b int);";
    let err = backup::restore(
        &*dst,
        script,
        &RestoreOptions {
            stop_on_error: true,
        },
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("statement 2"), "{err}");

    // The statement after the failure must not have run.
    let exists = dst
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'after'",
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        exists.rows[0][0],
        Value::Int(0),
        "restore continued past the error"
    );
}

#[tokio::test]
async fn restore_can_be_told_to_continue_past_errors() {
    let f = fixture_or_skip!("continue");
    let dst = open(&f.target).await;

    let script = "CREATE TABLE ok (a int);\nTHIS IS NOT SQL;\nCREATE TABLE after (b int);";
    let result = backup::restore(
        &*dst,
        script,
        &RestoreOptions {
            stop_on_error: false,
        },
        |_, _| {},
    )
    .await
    .unwrap();

    assert_eq!(result.failed, 1);
    assert_eq!(result.errors.len(), 1);

    let exists = dst
        .query(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'after'",
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(exists.rows[0][0], Value::Int(1));
}

#[tokio::test]
async fn an_empty_script_is_rejected_rather_than_silently_doing_nothing() {
    let f = fixture_or_skip!("emptyscript");
    let dst = open(&f.target).await;

    assert!(backup::restore(
        &*dst,
        "",
        &RestoreOptions {
            stop_on_error: true
        },
        |_, _| {}
    )
    .await
    .is_err());
    assert!(backup::restore(
        &*dst,
        "-- just a comment\n",
        &RestoreOptions {
            stop_on_error: true
        },
        |_, _| {}
    )
    .await
    .is_err());
}
