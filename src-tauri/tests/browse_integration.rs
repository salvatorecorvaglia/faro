//! Server-side sort, filter and paging, exercised against the seeded SQLite
//! fixture through the same SQL the `browse_table` command composes.
//!
//! Run `./scripts/seed.sh` first. Skips itself when the fixture is absent.

// `Dialect` needs no import: its methods resolve through the `&dyn Dialect`
// that `Driver::dialect()` hands back.
use faro_lib::driver::{self, Driver};
use faro_lib::model::{ConnectionConfig, Engine, SslMode, Value};
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

#[tokio::test]
async fn like_wildcards_in_a_search_term_are_escaped() {
    let d = driver_or_skip!();

    // A user searching for the literal text "50%" must not match everything
    // starting with "50". The escape is what makes that true.
    let escaped = d
        .query(
            r"SELECT * FROM access_log WHERE CAST(path AS TEXT) LIKE '%\%%' ESCAPE '\'",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        escaped.rows.len(),
        0,
        "no path contains a literal percent sign"
    );

    let unescaped = d
        .query(
            "SELECT * FROM access_log WHERE CAST(path AS TEXT) LIKE '%'",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        !unescaped.rows.is_empty(),
        "a bare % should match everything"
    );
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
