//! Server-side sort, filter and paging, exercised against the seeded SQLite
//! fixture through the same SQL the `browse_table` command composes.
//!
//! Run `./scripts/seed.sh` first. Skips itself when the fixture is absent.

// `Dialect` needs no import: its methods resolve through the `&dyn Dialect`
// that `Driver::dialect()` hands back.
use faro_lib::driver::{self, Driver};
use faro_lib::model::{
    BrowseOptions, ColumnFilter, ConnectionConfig, Engine, FilterOp, SslMode, TableRef, Value,
};
use tokio_util::sync::CancellationToken;

fn fixture_path() -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("tests/fixtures/faro_test.db");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

/// A SQLite connection config with no file chosen yet.
fn base_config() -> ConnectionConfig {
    ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: None,
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    }
}

async fn open() -> Option<Box<dyn Driver>> {
    let mut config = base_config();
    config.file_path = Some(fixture_path()?);
    Some(
        driver::connect(&config, None)
            .await
            .expect("connect failed"),
    )
}

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

fn text_at(rows: &[Vec<Value>], row: usize, col: usize) -> String {
    match &rows[row][col] {
        Value::Text(s) => s.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

/// SQLite is schema-less as far as Faro is concerned, so `schema` stays `None`.
fn table(name: &str) -> TableRef {
    TableRef {
        schema: None,
        name: name.into(),
    }
}

/// Browse options carrying a single `Contains` filter — the operator whose
/// escaping is under test.
fn contains(column: &str, needle: &str) -> BrowseOptions {
    BrowseOptions {
        sort_column: None,
        sort_desc: false,
        filters: vec![ColumnFilter {
            column: column.into(),
            op: FilterOp::Contains,
            value: needle.into(),
        }],
        limit: Some(100),
        offset: 0,
    }
}

#[tokio::test]
async fn sorting_descending_reverses_the_order() {
    let d = driver_or_skip!();
    let q = d.dialect().quote_ident("name");

    let asc = d
        .query(
            &format!("SELECT name FROM authors ORDER BY {q} ASC"),
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let desc = d
        .query(
            &format!("SELECT name FROM authors ORDER BY {q} DESC"),
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(asc.rows.len(), desc.rows.len());
    assert_eq!(
        text_at(&asc.rows, 0, 0),
        text_at(&desc.rows, desc.rows.len() - 1, 0)
    );
}

#[tokio::test]
async fn a_filter_narrows_the_result_server_side() {
    let d = driver_or_skip!();

    let all = d
        .query("SELECT * FROM authors", 100, CancellationToken::new())
        .await
        .unwrap();
    let filtered = d
        .query(
            "SELECT * FROM authors WHERE CAST(\"name\" AS TEXT) LIKE '%Lovelace%'",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(all.rows.len(), 5);
    assert_eq!(filtered.rows.len(), 1);
}

/// A writable copy of the fixture, so a test can add its own rows without
/// leaking state into tests running in parallel.
///
/// Needed because the shared fixture cannot express the case below: none of its
/// values contain a literal `%` or `_`, which is exactly what distinguishes
/// working escaping from broken escaping.
struct TempDb {
    path: std::path::PathBuf,
}

impl TempDb {
    fn from_fixture(tag: &str) -> Option<Self> {
        let source = fixture_path()?;
        let path =
            std::env::temp_dir().join(format!("faro_browse_{tag}_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::copy(&source, &path).ok()?;
        Some(Self { path })
    }

    async fn driver(&self) -> Box<dyn Driver> {
        let mut config = base_config();
        config.file_path = Some(self.path.to_string_lossy().into_owned());
        driver::connect(&config, None)
            .await
            .expect("connect failed")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[tokio::test]
async fn like_wildcards_in_a_search_term_are_escaped() {
    let Some(db) = TempDb::from_fixture("like_escape") else {
        eprintln!("skipping: run ./scripts/seed.sh to create the fixture");
        return;
    };
    let d = db.driver().await;

    // Two rows per operator that differ *only* in whether the wildcard is taken
    // literally, so the assertions below have real discriminating power: with the
    // `ESCAPE` clause each search matches exactly its literal row, and without it
    // the escaped pattern matches nothing at all.
    for sql in [
        "CREATE TABLE labels (id INTEGER PRIMARY KEY, name TEXT)",
        "INSERT INTO labels (name) VALUES ('50%'), ('50off'), ('a_b'), ('axb')",
    ] {
        d.execute(sql, CancellationToken::new())
            .await
            .expect("could not set up the labels table");
    }

    // '%' is the multi-character wildcard. Unescaped, '%50%%' matches both
    // '50%' and '50off'.
    let percent = d
        .browse(
            &table("labels"),
            &contains("name", "50%"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        percent.rows.len(),
        1,
        "searching for the literal text '50%' should match only '50%', got {:?}",
        percent.rows
    );

    // '_' is the single-character wildcard. Unescaped, '%a_b%' matches both
    // 'a_b' and 'axb'.
    let underscore = d
        .browse(
            &table("labels"),
            &contains("name", "a_b"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        underscore.rows.len(),
        1,
        "searching for the literal text 'a_b' should match only 'a_b', got {:?}",
        underscore.rows
    );

    // The control: the wildcards really do match more than one row here, so the
    // assertions above are about escaping and not about a sparse table.
    let wild = d
        .query(
            "SELECT * FROM labels WHERE name LIKE '%50%%' OR name LIKE '%a_b%'",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(wild.rows.len(), 4, "all four rows match as wildcards");
}

#[tokio::test]
async fn paging_a_sorted_table_returns_disjoint_ordered_pages() {
    let d = driver_or_skip!();
    let dialect = d.dialect();
    let base = "SELECT path FROM access_log ORDER BY path ASC";

    let p1 = d
        .query(&dialect.paginate(base, 6, 0), 5, CancellationToken::new())
        .await
        .unwrap();
    let p2 = d
        .query(&dialect.paginate(base, 6, 5), 5, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(p1.rows.len(), 5);
    assert_eq!(p2.rows.len(), 5);

    // Ordering must survive the paging wrapper, or page two is arbitrary.
    let last_of_first = text_at(&p1.rows, 4, 0);
    let first_of_second = text_at(&p2.rows, 0, 0);
    assert!(
        last_of_first < first_of_second,
        "pages overlap or lost their order: {last_of_first} !< {first_of_second}"
    );
}

#[tokio::test]
async fn sorting_places_nulls_consistently() {
    let d = driver_or_skip!();

    // `bio` is NULL for one author. Whatever the engine's null ordering, the
    // row count must not change — a sort that drops rows would be a silent
    // data loss the user could not see.
    let sorted = d
        .query(
            "SELECT name, bio FROM authors ORDER BY bio ASC",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(sorted.rows.len(), 5);
    assert_eq!(
        sorted.rows.iter().filter(|r| r[1] == Value::Null).count(),
        1
    );
}
