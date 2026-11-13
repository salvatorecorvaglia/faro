//! Regressions for the audit safety fixes. Each case uses disposable data.
use faro_lib::driver::{self, Driver};
use faro_lib::model::{ConnectionConfig, Engine, GuardedStatement, SslMode};
use tokio_util::sync::CancellationToken;

struct DatabaseFile(std::path::PathBuf);
impl Drop for DatabaseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
async fn database() -> (Box<dyn Driver>, DatabaseFile) {
    let file =
        DatabaseFile(std::env::temp_dir().join(format!("faro-safety-{}.db", uuid::Uuid::new_v4())));
    drop(rusqlite::Connection::open(&file.0).unwrap());
    let driver = driver::connect(
        &ConnectionConfig {
            id: "safety".into(),
            name: "safety".into(),
            engine: Engine::Sqlite,
            host: String::new(),
            port: 0,
            username: String::new(),
            database: String::new(),
            file_path: Some(file.0.to_string_lossy().into_owned()),
            ssl_mode: SslMode::Prefer,
            color: None,
            read_only: false,
        },
        None,
    )
    .await
    .unwrap();
    (driver, file)
}

#[tokio::test]
async fn managed_write_preserves_open_editor_transaction() {
    let (driver, _file) = database().await;
    let token = CancellationToken::new();
    driver
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", token.clone())
        .await
        .unwrap();
    driver.execute("BEGIN", token.clone()).await.unwrap();
    let outcome = driver
        .apply_transaction(&[GuardedStatement {
            sql: "INSERT INTO items VALUES (2)".into(),
            expect: Some(1),
        }])
        .await;
    assert!(
        outcome.is_ok(),
        "managed write must use a separate session: {outcome:?}"
    );
    driver.execute("ROLLBACK", token.clone()).await.unwrap();
    let rows = driver
        .query("SELECT id FROM items ORDER BY id", 10, token)
        .await
        .unwrap();
    assert_eq!(rows.rows, vec![vec![faro_lib::model::Value::Int(2)]]);
}

#[tokio::test]
async fn stale_grid_update_is_rejected_without_overwriting_external_changes() {
    let (driver, _file) = database().await;
    let token = CancellationToken::new();
    driver
        .execute(
            "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)",
            token.clone(),
        )
        .await
        .unwrap();
    driver
        .execute("INSERT INTO items VALUES (1, 'original')", token.clone())
        .await
        .unwrap();
    let table = faro_lib::model::TableRef {
        schema: None,
        name: "items".into(),
    };
    let detail = driver.describe_table(&table).await.unwrap();
    // Preserve a real grid snapshot before another client changes the row.
    let snapshot = driver
        .query("SELECT id, name FROM items", 10, token.clone())
        .await
        .unwrap();
    let external = rusqlite::Connection::open(&_file.0).unwrap();
    external
        .execute("UPDATE items SET name = 'external' WHERE id = 1", [])
        .unwrap();
    let change: faro_lib::model::PendingChange = serde_json::from_value(serde_json::json!({
        "type": "update",
        "key": [{"column": "id", "value": {"kind": "stored", "value": snapshot.rows[0][0]}}],
        "cells": [{"column": "name", "value": {"kind": "text", "value": "local"}}],
        "original": [{"column": "name", "value": {"kind": "stored", "value": snapshot.rows[0][1]}}]
    }))
    .unwrap();
    let statements =
        faro_lib::dml::build_statements(&table, &detail, &[change], driver.dialect()).unwrap();
    let result = driver.apply_transaction(&statements).await;
    assert!(
        matches!(result, Err(faro_lib::error::FaroError::Conflict(_))),
        "stale update silently overwrote external data: {result:?}"
    );
    let rows = driver
        .query("SELECT name FROM items", 10, token)
        .await
        .unwrap();
    assert_eq!(
        rows.rows,
        vec![vec![faro_lib::model::Value::Text("external".into())]]
    );
}

#[tokio::test]
async fn managed_import_preserves_editor_transaction() {
    let (driver, _file) = database().await;
    let token = CancellationToken::new();
    driver
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", token.clone())
        .await
        .unwrap();
    driver.execute("BEGIN", token.clone()).await.unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender
        .send(faro_lib::model::ApplyChunk::Batch(vec![GuardedStatement {
            sql: "INSERT INTO items VALUES (3)".into(),
            expect: Some(1),
        }]))
        .await
        .unwrap();
    sender
        .send(faro_lib::model::ApplyChunk::Done)
        .await
        .unwrap();
    assert_eq!(driver.apply_streaming(receiver).await.unwrap(), 1);
    driver.execute("ROLLBACK", token.clone()).await.unwrap();
    assert_eq!(
        driver
            .query("SELECT id FROM items", 10, token)
            .await
            .unwrap()
            .rows,
        vec![vec![faro_lib::model::Value::Int(3)]]
    );
}

#[tokio::test]
async fn registry_retains_active_configuration_and_marks_credential_changes() {
    let (driver, _file) = database().await;
    let registry = faro_lib::registry::Registry::new();
    let active: ConnectionConfig = serde_json::from_value(serde_json::json!({
        "id": "active", "name": "Active", "engine": "sqlite", "filePath": _file.0
    }))
    .unwrap();
    registry
        .insert_configured(
            active.id.clone(),
            std::sync::Arc::from(driver),
            false,
            active.name.clone(),
            Some(active.clone()),
        )
        .await;
    let mut saved = active.clone();
    saved.read_only = true;
    assert!(!active.same_session(&saved));
    assert_eq!(registry.active_config("active").await, Some(active.clone()));
    assert!(!registry.session("active").await.unwrap().1);
    saved = active.clone();
    saved.name = "Renamed".into();
    saved.color = Some("blue".into());
    assert!(active.same_session(&saved));
    registry.credentials_changed("active").await;
    assert!(registry.status("active").await.2);
    registry.remove("active").await;
    assert_eq!(registry.status("active").await, (false, None, false));
}

#[tokio::test]
async fn managed_restore_preserves_editor_transaction() {
    let (driver, _file) = database().await;
    let token = CancellationToken::new();
    driver
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", token.clone())
        .await
        .unwrap();
    driver.execute("BEGIN", token.clone()).await.unwrap();
    let managed = driver.managed_session().await.unwrap();
    faro_lib::transfer::backup::restore(
        &*managed,
        "INSERT INTO items VALUES (4);",
        &faro_lib::transfer::backup::RestoreOptions {
            stop_on_error: true,
        },
        token.clone(),
        |_, _| {},
    )
    .await
    .unwrap();
    driver.execute("ROLLBACK", token.clone()).await.unwrap();
    assert_eq!(
        driver
            .query("SELECT id FROM items", 10, token)
            .await
            .unwrap()
            .rows,
        vec![vec![faro_lib::model::Value::Int(4)]]
    );
}

#[cfg(feature = "duckdb-engine")]
#[tokio::test]
async fn duckdb_managed_writes_preserve_editor_transaction() {
    let file = DatabaseFile(
        std::env::temp_dir().join(format!("faro-duck-safety-{}.duckdb", uuid::Uuid::new_v4())),
    );
    drop(duckdb::Connection::open(&file.0).unwrap());
    let config: ConnectionConfig = serde_json::from_value(serde_json::json!({
        "id": "duck", "name": "DuckDB", "engine": "duckdb", "filePath": file.0
    }))
    .unwrap();
    let driver = driver::connect(&config, None).await.unwrap();
    let token = CancellationToken::new();
    driver
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", token.clone())
        .await
        .unwrap();
    driver.execute("BEGIN", token.clone()).await.unwrap();
    driver
        .apply_transaction(&[GuardedStatement {
            sql: "INSERT INTO items VALUES (1)".into(),
            expect: Some(1),
        }])
        .await
        .unwrap();
    let managed = driver.managed_session().await.unwrap();
    faro_lib::transfer::backup::restore(
        &*managed,
        "INSERT INTO items VALUES (2);",
        &faro_lib::transfer::backup::RestoreOptions {
            stop_on_error: true,
        },
        token.clone(),
        |_, _| {},
    )
    .await
    .unwrap();
    driver.execute("ROLLBACK", token.clone()).await.unwrap();
    assert_eq!(
        driver
            .query("SELECT id FROM items ORDER BY id", 10, token)
            .await
            .unwrap()
            .rows,
        vec![
            vec![faro_lib::model::Value::Int(1)],
            vec![faro_lib::model::Value::Int(2)]
        ]
    );
}

#[tokio::test]
async fn snapshot_checks_float_values_and_case_sensitive_deletes() {
    use faro_lib::model::{CellEdit, EditValue, PendingChange, TableRef, Value};
    let (driver, _file) = database().await;
    let token = CancellationToken::new();
    driver.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, value REAL, name TEXT COLLATE NOCASE, note TEXT)", token.clone()).await.unwrap();
    driver
        .execute(
            "INSERT INTO items VALUES (1, 0.1, 'original', NULL)",
            token.clone(),
        )
        .await
        .unwrap();
    let table = TableRef {
        schema: None,
        name: "items".into(),
    };
    let detail = driver.describe_table(&table).await.unwrap();
    let key = vec![CellEdit {
        column: "id".into(),
        value: EditValue::Stored(Value::Int(1)),
    }];
    let changes = vec![PendingChange::Update {
        key: key.clone(),
        original: vec![CellEdit {
            column: "value".into(),
            value: EditValue::Stored(Value::Float(0.1)),
        }],
        cells: vec![CellEdit {
            column: "value".into(),
            value: EditValue::Text("0.2".into()),
        }],
    }];
    faro_lib::dml::validate_snapshots(&detail, &changes).unwrap();
    let statements =
        faro_lib::dml::build_statements(&table, &detail, &changes, driver.dialect()).unwrap();
    assert_eq!(driver.apply_transaction(&statements).await.unwrap(), 1);
    let snapshot = driver
        .query("SELECT * FROM items", 10, token.clone())
        .await
        .unwrap();
    let original = snapshot
        .columns
        .iter()
        .zip(&snapshot.rows[0])
        .map(|(column, value)| CellEdit {
            column: column.name.clone(),
            value: EditValue::Stored(value.clone()),
        })
        .collect();
    let changes = vec![PendingChange::Delete { key, original }];
    faro_lib::dml::validate_snapshots(&detail, &changes).unwrap();
    let statements =
        faro_lib::dml::build_statements(&table, &detail, &changes, driver.dialect()).unwrap();
    let external = rusqlite::Connection::open(&_file.0).unwrap();
    external
        .execute("UPDATE items SET name = 'ORIGINAL' WHERE id = 1", [])
        .unwrap();
    let outcome = driver.apply_transaction(&statements).await;
    assert!(
        matches!(outcome, Err(faro_lib::error::FaroError::Conflict(_))),
        "case-only change was lost: {outcome:?}"
    );
    external
        .execute(
            "UPDATE items SET name = 'original', note = 'changed' WHERE id = 1",
            [],
        )
        .unwrap();
    assert!(matches!(
        driver.apply_transaction(&statements).await,
        Err(faro_lib::error::FaroError::Conflict(_))
    ));
    assert_eq!(
        driver
            .query("SELECT COUNT(*) FROM items", 10, token)
            .await
            .unwrap()
            .rows,
        vec![vec![Value::Int(1)]]
    );
}

#[tokio::test]
async fn snapshot_backup_keeps_later_pages_consistent_with_earlier_pages() {
    use faro_lib::{
        model::TableRef,
        transfer::backup::{write_backup_with_consistency, BackupOptions},
    };
    let (driver, file) = database().await;
    let external = rusqlite::Connection::open(&file.0).unwrap();
    external.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE records(id INTEGER PRIMARY KEY, value TEXT); WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 2001) INSERT INTO records SELECT n, 'before' FROM seq;").unwrap();
    let external = std::sync::Mutex::new(external);
    let snapshot = driver
        .snapshot_session()
        .await
        .unwrap()
        .expect("SQLite supports a read snapshot");
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join("backup.sql");
    let mut changed = false;
    let result = write_backup_with_consistency(
        &*snapshot,
        &destination,
        &BackupOptions {
            tables: vec![TableRef {
                schema: None,
                name: "records".into(),
            }],
            include_schema: true,
            include_data: true,
            drop_existing: false,
        },
        CancellationToken::new(),
        true,
        |progress| {
            if !changed && progress.rows_written >= 2000 {
                external
                    .lock()
                    .unwrap()
                    .execute("UPDATE records SET value = 'after' WHERE id = 2001", [])
                    .unwrap();
                changed = true;
            }
        },
    )
    .await
    .unwrap();
    snapshot
        .execute("ROLLBACK", CancellationToken::new())
        .await
        .unwrap();
    assert!(changed);
    assert!(result.consistent_snapshot);
    assert_eq!(result.rows, 2001);
    let text = std::fs::read_to_string(destination).unwrap();
    assert!(text.contains("consistent read snapshot"));
    assert!(!text.contains("'after'"));
    assert_eq!(
        external
            .lock()
            .unwrap()
            .query_row("SELECT value FROM records WHERE id = 2001", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "after"
    );
}

#[cfg(feature = "duckdb-engine")]
#[tokio::test]
async fn duckdb_snapshot_ignores_concurrent_editor_writes() {
    let config: ConnectionConfig = serde_json::from_value(serde_json::json!({"id":"snapshot", "name":"snapshot", "engine":"duckdb", "filePath":":memory:"})).unwrap();
    let driver = driver::connect(&config, None).await.unwrap();
    driver
        .execute(
            "CREATE TABLE items(id INTEGER); INSERT INTO items VALUES (1)",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let snapshot = driver.snapshot_session().await.unwrap().unwrap();
    let before = snapshot
        .query("SELECT * FROM items", 10, CancellationToken::new())
        .await
        .unwrap();
    driver
        .execute("UPDATE items SET id = 2", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .query("SELECT * FROM items", 10, CancellationToken::new())
            .await
            .unwrap()
            .rows,
        before.rows
    );
    snapshot
        .execute("ROLLBACK", CancellationToken::new())
        .await
        .unwrap();
    assert_ne!(
        driver
            .query("SELECT * FROM items", 10, CancellationToken::new())
            .await
            .unwrap()
            .rows,
        before.rows
    );
}

#[tokio::test]
async fn a_single_large_cell_is_refused_before_it_is_retained_or_sent_to_the_ui() {
    let (driver, _file) = database().await;
    let sql = format!(
        "SELECT zeroblob({}) AS payload",
        faro_lib::budget::RESULT_BYTES + 1
    );
    let result = driver.query(&sql, 1, CancellationToken::new()).await;
    assert!(matches!(
        result,
        Err(faro_lib::error::FaroError::ResourceLimit(_))
    ));
    assert_eq!(
        driver
            .query("SELECT 1", 1, CancellationToken::new())
            .await
            .unwrap()
            .rows,
        vec![vec![faro_lib::model::Value::Int(1)]]
    );
}
