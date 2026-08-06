//! Round-tripping real data out to a file and back into a database.
//!
//! The plan's exit criterion for this phase: export a table to CSV, import it
//! back, and get the same values. Run `./scripts/seed.sh` first.

use faro_lib::driver::{self, Driver};
use faro_lib::model::{ConnectionConfig, Engine, GuardedStatement, SslMode, Value};
use faro_lib::transfer::export::{self, ExportFormat, ExportOptions};
use faro_lib::transfer::import::{self, ImportFormat};
use tokio_util::sync::CancellationToken;

struct Fixture {
    db: std::path::PathBuf,
    files: Vec<std::path::PathBuf>,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("tests/fixtures/faro_test.db");
        if !source.exists() {
            return None;
        }
        let db =
            std::env::temp_dir().join(format!("faro_transfer_{tag}_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        std::fs::copy(&source, &db).ok()?;
        Some(Self { db, files: vec![] })
    }

    fn file(&mut self, name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("faro_transfer_{}_{name}", std::process::id()));
        self.files.push(p.clone());
        p
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.db);
        for f in &self.files {
            let _ = std::fs::remove_file(f);
        }
    }
}

async fn open(f: &Fixture) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "t".into(),
        name: "t".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(f.db.to_string_lossy().into_owned()),
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

fn opts(format: ExportFormat, table: &str) -> ExportOptions {
    ExportOptions {
        format,
        include_header: true,
        table_name: Some(table.into()),
    }
}

#[tokio::test]
async fn a_table_round_trips_through_csv() {
    let mut f = fixture_or_skip!("roundtrip");
    let d = open(&f).await;

    // Export every author.
    let source = d
        .query(
            "SELECT id, name, email, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let path = f.file("authors.csv");
    let written = export::write_file(
        &path,
        &source,
        &opts(ExportFormat::Csv, "authors"),
        Some(d.dialect()),
    )
    .unwrap();
    assert_eq!(written, 5);

    // Read it back and confirm the file describes what left the database.
    let (columns, rows) = import::read_rows(&path, ImportFormat::Csv, true).unwrap();
    assert_eq!(columns, vec!["id", "name", "email", "bio"]);
    assert_eq!(rows.len(), 5);

    // Import into a fresh table and compare values.
    d.execute(
        "CREATE TABLE authors_copy (id INTEGER PRIMARY KEY, name TEXT, email TEXT, bio TEXT)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let statements: Vec<GuardedStatement> = rows
        .iter()
        .map(|row| {
            let values: Vec<String> = row
                .iter()
                .map(|v| {
                    if v.is_empty() {
                        "NULL".to_string()
                    } else {
                        format!("'{}'", v.replace('\'', "''"))
                    }
                })
                .collect();
            GuardedStatement {
                sql: format!(
                    "INSERT INTO authors_copy (id, name, email, bio) VALUES ({})",
                    values.join(", ")
                ),
                expect: None,
            }
        })
        .collect();

    d.apply_transaction(&statements).await.unwrap();

    let copied = d
        .query(
            "SELECT id, name FROM authors_copy ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(copied.rows.len(), 5);
    assert_eq!(copied.rows[0][1], Value::Text("Ada Lovelace".into()));

    // The awkward values must have survived the whole trip.
    let names: Vec<String> = copied
        .rows
        .iter()
        .filter_map(|r| match &r[1] {
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
}

#[tokio::test]
async fn null_survives_a_csv_round_trip_as_null() {
    let mut f = fixture_or_skip!("nulls");
    let d = open(&f).await;

    // `bio` is NULL for one author; it must come back NULL, not as "".
    let source = d
        .query(
            "SELECT id, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let null_count = source.rows.iter().filter(|r| r[1] == Value::Null).count();
    assert!(null_count > 0, "fixture should contain a NULL bio");

    let path = f.file("nulls.csv");
    export::write_file(&path, &source, &opts(ExportFormat::Csv, "authors"), None).unwrap();

    let (_, rows) = import::read_rows(&path, ImportFormat::Csv, true).unwrap();
    let empties = rows.iter().filter(|r| r[1].is_empty()).count();
    assert_eq!(
        empties, null_count,
        "NULLs did not come back as empty fields"
    );
}

#[tokio::test]
async fn exporting_a_table_reads_past_the_first_page() {
    let mut f = fixture_or_skip!("paging");
    let d = open(&f).await;
    let dialect = d.dialect();

    // access_log has 5000 rows — well past the 1000-row page the grid holds.
    // Exporting what is on screen would silently truncate to a fifth.
    let base = "SELECT * FROM access_log";
    let mut all: Option<faro_lib::model::ResultSet> = None;
    let mut offset = 0u64;
    loop {
        let page = d
            .query(
                &dialect.paginate(base, 2001, offset),
                2000,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let more = page.truncated;
        let n = page.rows.len() as u64;
        match &mut all {
            None => all = Some(page),
            Some(acc) => acc.rows.extend(page.rows),
        }
        if !more || n == 0 {
            break;
        }
        offset += n;
    }

    let combined = all.unwrap();
    assert_eq!(combined.rows.len(), 5000, "paged export lost rows");

    let path = f.file("access.csv");
    let written = export::write_file(
        &path,
        &combined,
        &opts(ExportFormat::Csv, "access_log"),
        None,
    )
    .unwrap();
    assert_eq!(written, 5000);

    let (_, rows) = import::read_rows(&path, ImportFormat::Csv, true).unwrap();
    assert_eq!(rows.len(), 5000);
}

#[tokio::test]
async fn json_export_round_trips_through_the_importer() {
    let mut f = fixture_or_skip!("json");
    let d = open(&f).await;

    let source = d
        .query(
            "SELECT id, name FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let path = f.file("authors.json");
    export::write_file(&path, &source, &opts(ExportFormat::Json, "authors"), None).unwrap();

    let (columns, rows) = import::read_rows(&path, ImportFormat::Json, true).unwrap();
    assert_eq!(columns, vec!["id", "name"]);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0][1], "Ada Lovelace");
}

#[tokio::test]
async fn sql_export_is_valid_sql_the_database_accepts() {
    let mut f = fixture_or_skip!("sqlfmt");
    let d = open(&f).await;

    let source = d
        .query(
            "SELECT id, name, email, bio FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let path = f.file("authors.sql");
    export::write_file(
        &path,
        &source,
        &opts(ExportFormat::Sql, "authors_copy"),
        Some(d.dialect()),
    )
    .unwrap();

    d.execute(
        "CREATE TABLE authors_copy (id INTEGER PRIMARY KEY, name TEXT, email TEXT, bio TEXT)",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    // Feeding the generated statements straight back is the real test of the
    // quoting and escaping.
    let text = std::fs::read_to_string(&path).unwrap();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        d.execute(line.trim_end_matches(';'), CancellationToken::new())
            .await
            .unwrap_or_else(|e| panic!("generated SQL was rejected: {e}\n{line}"));
    }

    let copied = d
        .query(
            "SELECT COUNT(*) FROM authors_copy",
            1,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(copied.rows[0][0], Value::Int(5));
}

#[tokio::test]
async fn xlsx_export_reads_back_through_calamine() {
    let mut f = fixture_or_skip!("xlsx");
    let d = open(&f).await;

    let source = d
        .query(
            "SELECT id, name FROM authors ORDER BY id",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let path = f.file("authors.xlsx");
    export::write_file(&path, &source, &opts(ExportFormat::Xlsx, "authors"), None).unwrap();

    let (columns, rows) = import::read_rows(&path, ImportFormat::Xlsx, true).unwrap();
    assert_eq!(columns, vec!["id", "name"]);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0][1], "Ada Lovelace");
}
