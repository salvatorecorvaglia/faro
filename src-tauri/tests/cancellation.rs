//! Native cancellation, transaction rollback and session reuse regressions.
//!
//! SQLite uses rusqlite's interrupt handle on its dedicated blocking session;
//! DuckDB uses its native interrupt handle. Network-driver cancellation under
//! real row locks is checked separately in `live_engines.rs` with the mandatory
//! live-service gate. These native tests do not stand in for a network server.

use faro_lib::driver::{self, Driver};
use faro_lib::error::FaroError;
use faro_lib::model::{ConnectionConfig, Engine, SslMode, TableRef};
use faro_lib::transfer::backup::{self, BackupOptions, RestoreOptions};
use tokio_util::sync::CancellationToken;

async fn open_in_memory() -> Box<dyn Driver> {
    open_memory(Engine::Sqlite).await
}

async fn open_memory(engine: Engine) -> Box<dyn Driver> {
    let config = ConnectionConfig {
        id: "test".into(),
        name: "test".into(),
        engine,
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

fn insert_chunk() -> faro_lib::model::ApplyChunk {
    faro_lib::model::ApplyChunk::Batch(vec![faro_lib::model::GuardedStatement {
        sql: "INSERT INTO cancellation_items VALUES (1)".into(),
        expect: Some(1),
    }])
}

async fn assert_no_imported_rows(driver: &dyn Driver) {
    let result = driver
        .query(
            "SELECT COUNT(*) FROM cancellation_items",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![faro_lib::model::Value::Int(0)]]);
}

#[tokio::test]
async fn cancelling_with_done_already_queued_never_commits() {
    let driver = open_in_memory().await;
    driver
        .execute(
            "CREATE TABLE cancellation_items(id INTEGER)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender.send(insert_chunk()).await.unwrap();
    sender
        .send(faro_lib::model::ApplyChunk::Done)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        driver.apply_streaming_cancelled(receiver, cancel).await,
        Err(FaroError::Cancelled)
    ));
    assert_no_imported_rows(&*driver).await;
}

#[tokio::test]
async fn cancelling_an_import_waiting_for_input_rolls_back_and_frees_session() {
    let driver: std::sync::Arc<dyn Driver> = std::sync::Arc::from(open_in_memory().await);
    driver
        .execute(
            "CREATE TABLE cancellation_items(id INTEGER)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender.send(insert_chunk()).await.unwrap();
    let cancel = CancellationToken::new();
    let work = tokio::spawn({
        let driver = driver.clone();
        let cancel = cancel.clone();
        async move { driver.apply_streaming_cancelled(receiver, cancel).await }
    });
    // Capacity is released only once the applier consumes the queued batch.
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), sender.reserve())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), work)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(FaroError::Cancelled)), "{result:?}");
    assert_no_imported_rows(&*driver).await;
    driver
        .apply_transaction(&[faro_lib::model::GuardedStatement {
            sql: "INSERT INTO cancellation_items VALUES (2)".into(),
            expect: Some(1),
        }])
        .await
        .unwrap();
}

#[tokio::test]
async fn cancellation_after_a_completed_commit_does_not_undo_success() {
    let driver = open_in_memory().await;
    driver
        .execute(
            "CREATE TABLE cancellation_items(id INTEGER)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender.send(insert_chunk()).await.unwrap();
    sender
        .send(faro_lib::model::ApplyChunk::Done)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        driver
            .apply_streaming_cancelled(receiver, cancel.clone())
            .await
            .unwrap(),
        1
    );
    cancel.cancel();
    assert_eq!(
        driver
            .query(
                "SELECT id FROM cancellation_items",
                10,
                CancellationToken::new()
            )
            .await
            .unwrap()
            .rows,
        vec![vec![faro_lib::model::Value::Int(1)]]
    );
}

#[cfg(feature = "duckdb-engine")]
#[tokio::test]
async fn cancelling_a_duckdb_import_interrupts_native_work_and_rolls_back() {
    let driver: std::sync::Arc<dyn Driver> =
        std::sync::Arc::from(open_memory(Engine::DuckDb).await);
    driver
        .execute(
            "CREATE TABLE cancellation_items(id BIGINT)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender.send(faro_lib::model::ApplyChunk::Batch(vec![faro_lib::model::GuardedStatement {
        sql: "INSERT INTO cancellation_items SELECT CAST(sum(i) AS BIGINT) FROM range(1000000000) t(i)".into(), expect: Some(1),
    }])).await.unwrap();
    let cancel = CancellationToken::new();
    let work = tokio::spawn({
        let driver = driver.clone();
        let cancel = cancel.clone();
        async move { driver.apply_streaming_cancelled(receiver, cancel).await }
    });
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), sender.reserve())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    // Done may already be buffered when cancellation arrives.
    sender
        .send(faro_lib::model::ApplyChunk::Done)
        .await
        .unwrap();
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), work)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.is_err(),
        "cancelled native insert committed: {result:?}"
    );
    assert_no_imported_rows(&*driver).await;
}

#[tokio::test]
async fn cancelling_an_import_queued_for_the_managed_connection_is_prompt() {
    let driver: std::sync::Arc<dyn Driver> = std::sync::Arc::from(open_in_memory().await);
    driver
        .execute(
            "CREATE TABLE cancellation_items(id INTEGER)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    sender.send(insert_chunk()).await.unwrap();
    let first_cancel = CancellationToken::new();
    let first = tokio::spawn({
        let driver = driver.clone();
        let token = first_cancel.clone();
        async move { driver.apply_streaming_cancelled(receiver, token).await }
    });
    drop(
        tokio::time::timeout(std::time::Duration::from_secs(5), sender.reserve())
            .await
            .unwrap()
            .unwrap(),
    );
    let (_second_sender, second_receiver) = tokio::sync::mpsc::channel(1);
    let second_cancel = CancellationToken::new();
    let queued = driver.apply_streaming_cancelled(second_receiver, second_cancel.clone());
    tokio::pin!(queued);
    assert!(futures::poll!(&mut queued).is_pending());
    second_cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), queued)
            .await
            .unwrap(),
        Err(FaroError::Cancelled)
    ));
    first_cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap(),
        Err(FaroError::Cancelled)
    ));
    assert_no_imported_rows(&*driver).await;
}

#[tokio::test]
async fn restore_cancelled_after_the_last_statement_rolls_back_before_commit() {
    let driver = open_in_memory().await;
    driver
        .execute(
            "CREATE TABLE final_restore (id INT)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let token = CancellationToken::new();
    let cancel = token.clone();
    let error = backup::restore(
        &*driver,
        "INSERT INTO final_restore VALUES (1)",
        &RestoreOptions {
            stop_on_error: true,
        },
        token,
        move |_, _| cancel.cancel(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("Restore cancelled"));
    let result = driver
        .query(
            "SELECT COUNT(*) FROM final_restore",
            10,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.rows[0][0], faro_lib::model::Value::Int(0));
}
