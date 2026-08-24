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
use faro_lib::model::{ConnectionConfig, Engine, SslMode};
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

#[tokio::test]
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
    let query = d.query("SELECT n FROM big", 1_000_000, cancel.clone());
    let trigger = async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(query, trigger);

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
