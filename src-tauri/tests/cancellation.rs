//! Does cancelling a query actually free the connection, or just make the UI
//! stop waiting for it?
//!
//! `registry::cancel_query` only ever cancels a client-side token — nothing
//! here or in `driver/fetch.rs` speaks a server-side cancel protocol. For the
//! pooled sqlx engines (Postgres, MySQL, MariaDB, SQLite), the theory is that
//! dropping the row stream on cancellation returns the connection to its
//! pool in a clean state, so a query issued right after does not queue
//! behind the abandoned one. That theory had no test — this is it.
//!
//! Run against SQLite because it needs no Docker fixture: `driver/fetch.rs`'s
//! `impl_fetch_capped!` macro generates the identical cancellation logic for
//! Postgres, MySQL and MariaDB too, so this exercises the shared mechanism
//! all four engines rely on, not something SQLite-specific.

use faro_lib::driver::{self, Driver};
use faro_lib::error::FaroError;
use faro_lib::model::{ConnectionConfig, Engine, SslMode, TableRef};
use faro_lib::transfer::backup::{self, BackupOptions, RestoreOptions};
use tokio_util::sync::CancellationToken;

async fn open_in_memory() -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        engine: Engine::Sqlite,
        host: String::new(),
        port: 0,
        username: String::new(),
        database: String::new(),
        file_path: Some(":memory:".into()),
        ssl_mode: SslMode::Prefer,
        color: None,
        read_only: false,
    };
    driver::connect(&config, None)
        .await
        .expect("connect failed")
}

// Multi-threaded on purpose: the cancel has to land while the query is still
// streaming, and on the single-threaded runtime that depended on the query
// future returning `Pending` at least once. An in-memory SQLite stream can hand
// back many rows without ever doing so, which left the canceller unpolled and
// made this test fail sporadically under parallel load.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_query_frees_the_connection_for_the_next_one() {
    let d = open_in_memory().await;
    let none = CancellationToken::new();

    d.execute("CREATE TABLE big(n INTEGER)", none.clone())
        .await
        .expect("create table");
    // Large enough that streaming it takes measurably longer than issuing a
    // cancel does — otherwise the whole result could already be read by the
    // time the cancellation lands, and the test would prove nothing.
    d.execute(
        "WITH RECURSIVE seq(n) AS (\
             SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 500000\
         ) INSERT INTO big SELECT n FROM seq",
        none.clone(),
    )
    .await
    .expect("seed rows");

    let cancel = CancellationToken::new();
    // Cancel from a task on another worker, so it runs whether or not the query
    // future ever yields. Streaming half a million rows takes far longer than
    // this sleep, so the cancel reliably arrives mid-stream.
    let canceller = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            cancel.cancel();
        }
    });

    let result = d
        .query("SELECT n FROM big", 1_000_000, cancel.clone())
        .await;
    canceller.await.expect("canceller task panicked");

    assert!(
        matches!(result, Err(FaroError::Cancelled)),
        "expected the query to observe the cancellation, got {result:?}",
    );

    // The real assertion: the pool's single connection must be usable again
    // right away, not stuck behind a stream that was never fully drained.
    let after = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        d.query("SELECT 1 AS one", 10, CancellationToken::new()),
    )
    .await
    .expect("a query after cancellation must not hang waiting for the connection")
    .expect("a query after cancellation must succeed");

    assert_eq!(after.rows.len(), 1);
}

#[tokio::test]
async fn cancelling_a_backup_stops_mid_table_and_leaves_the_connection_usable() {
    let d = open_in_memory().await;
    let none = CancellationToken::new();

    d.execute("CREATE TABLE big(n INTEGER)", none.clone())
        .await
        .expect("create table");
    d.execute(
        "WITH RECURSIVE seq(n) AS (\
             SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 300000\
         ) INSERT INTO big SELECT n FROM seq",
        none,
    )
    .await
    .expect("seed rows");

    let path = std::env::temp_dir().join(format!("faro_cancel_backup_{}.sql", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let options = BackupOptions {
        tables: vec![TableRef {
            schema: None,
            name: "big".into(),
        }],
        include_schema: true,
        include_data: true,
        drop_existing: false,
    };
    let cancel = CancellationToken::new();
    let backup_fut = backup::write_backup(&*d, &path, &options, cancel.clone(), |_| {});
    let trigger = async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(backup_fut, trigger);
    let _ = std::fs::remove_file(&path);

    assert!(
        matches!(result, Err(FaroError::Cancelled)),
        "expected the backup to observe the cancellation, got {result:?}",
    );

    // Same real assertion as the query test above: the connection must still
    // be usable, not stuck behind the abandoned table dump.
    let after = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        d.query("SELECT 1 AS one", 10, CancellationToken::new()),
    )
    .await
    .expect("a query after a cancelled backup must not hang")
    .expect("a query after a cancelled backup must succeed");
    assert_eq!(after.rows.len(), 1);
}

#[tokio::test]
async fn cancelling_a_restore_stops_it_and_rolls_back() {
    let d = open_in_memory().await;
    d.execute("CREATE TABLE big(n INTEGER)", CancellationToken::new())
        .await
        .expect("create table");

    // One large statement, so cancellation has to interrupt it mid-flight —
    // not just skip starting the next one.
    let script = "WITH RECURSIVE seq(n) AS (\
         SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 500000\
     ) INSERT INTO big SELECT n FROM seq;";

    let cancel = CancellationToken::new();
    let restore_fut = backup::restore(
        &*d,
        script,
        &RestoreOptions {
            stop_on_error: true,
        },
        cancel.clone(),
        |_, _| {},
    );
    let trigger = async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(restore_fut, trigger);

    let err = result.expect_err("expected the restore to observe the cancellation");
    assert!(
        err.to_string().contains("Restore cancelled"),
        "wrong error: {err}"
    );
    // stop_on_error wraps this in a transaction, so cancelling mid-statement
    // must roll it back rather than leave a half-written table.
    assert!(
        err.to_string().contains("Nothing was applied"),
        "expected a rollback, got: {err}"
    );

    let after = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        d.query(
            "SELECT COUNT(*) AS n FROM big",
            10,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("a query after a cancelled restore must not hang")
    .expect("a query after a cancelled restore must succeed");
    assert_eq!(
        after.rows[0][0],
        faro_lib::model::Value::Int(0),
        "the cancelled insert should have been rolled back"
    );
}

#[tokio::test]
async fn cancelling_before_a_query_starts_never_touches_the_connection() {
    let d = open_in_memory().await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = d.query("SELECT 1", 10, cancel).await;
    assert!(matches!(result, Err(FaroError::Cancelled)));

    // Confirms the early-exit path above didn't leave the pool's connection
    // checked out.
    let after = d
        .query("SELECT 1 AS one", 10, CancellationToken::new())
        .await
        .expect("a fresh query must still succeed");
    assert_eq!(after.rows.len(), 1);
}
