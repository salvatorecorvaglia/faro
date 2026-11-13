use std::path::PathBuf;
use tauri::{Emitter, State};

use super::AppState;
use crate::error::Result;
use crate::transfer::backup::{
    self, BackupOptions, BackupProgress, BackupResult, RestoreOptions, RestoreResult,
};

/// Event name the frontend listens on for backup progress.
const BACKUP_PROGRESS: &str = "faro://backup-progress";
const RESTORE_PROGRESS: &str = "faro://restore-progress";

/// Write a dump of the connected database.
///
/// Progress is emitted as events rather than returned at the end: a backup of a
/// large table takes long enough that silence is indistinguishable from a hang.
///
/// `query_id` is registered with the same connection-scoped cancellation
/// registry a query uses, so the existing `cancel_query` command can stop a
/// backup mid-flight too — there is nothing backup-specific about it.
#[tauri::command]
pub async fn backup_database(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    connection_id: String,
    path: String,
    options: BackupOptions,
    query_id: String,
    allow_best_effort: Option<bool>,
) -> Result<BackupResult> {
    let driver = state.registry.get(&connection_id).await?;
    let target = PathBuf::from(&path);
    let query = state.registry.begin_query(&connection_id, &query_id);

    let _operation = state
        .registry
        .managed_submission(&connection_id, query.token())
        .await?;
    let snapshot = driver.snapshot_session().await?;
    let consistent = snapshot.is_some();
    if !consistent && !allow_best_effort.unwrap_or(false) {
        return Err(crate::error::FaroError::Other(
            "A consistent snapshot is unavailable for this connection. Enable best-effort backup explicitly to continue; concurrent changes may appear in that backup.".into()
        ));
    }
    let reader: &dyn crate::driver::Driver = snapshot.as_deref().unwrap_or(&*driver);
    let result = backup::write_backup_with_consistency(
        reader,
        &target,
        &options,
        query.token(),
        consistent,
        |progress: BackupProgress| {
            let _ = app.emit(
                BACKUP_PROGRESS,
                serde_json::json!({ "queryId": query_id, "progress": progress }),
            );
        },
    )
    .await;
    // Close the read transaction even after cancellation or a disk error.
    if let Some(snapshot) = snapshot {
        let cleanup = snapshot.end_snapshot().await;
        if result.is_ok() {
            cleanup?;
        }
    }
    result.map(|mut result| {
        result.consistent_snapshot = consistent;
        result
    })
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RestoreProgress {
    done: usize,
    total: usize,
}

/// Execute a dump file against the connected database.
#[tauri::command]
pub async fn restore_database(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    connection_id: String,
    path: String,
    options: RestoreOptions,
    query_id: String,
) -> Result<RestoreResult> {
    let driver = state.registry.get_writable(&connection_id).await?;
    let query = state.registry.begin_query(&connection_id, &query_id);
    // Read on a blocking thread: a dump is arbitrarily large and this is a
    // synchronous whole-file read that was stalling an async runtime worker.
    let script = read_to_string_blocking(path.clone()).await?;

    let _operation = state
        .registry
        .managed_submission(&connection_id, query.token())
        .await?;
    let driver = driver.managed_session().await?;
    backup::restore(&*driver, &script, &options, query.token(), |done, total| {
        let _ = app.emit(
            RESTORE_PROGRESS,
            serde_json::json!({ "queryId": query_id, "progress": RestoreProgress { done, total } }),
        );
    })
    .await
}

/// Read a whole file without blocking the async runtime.
///
/// `tokio::fs` is not available — the runtime is built without the `fs`
/// feature — so the synchronous read is handed to the blocking pool, which is
/// what `tokio::fs` would do underneath anyway.
async fn read_to_string_blocking(path: String) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        let bytes = crate::budget::read_materialized(std::path::Path::new(&path))?;
        String::from_utf8(bytes).map_err(|e| crate::error::FaroError::Io(e.to_string()))
    })
    .await
    .map_err(|e| crate::error::FaroError::Io(format!("could not read the file: {e}")))?
}

/// Statement count for a dump file, so the UI can warn before running it.
///
/// Async so the read lands on the blocking pool. As a synchronous command it
/// ran on the IPC thread and froze the window for as long as reading the dump
/// took, which for a large backup is exactly when the user is watching.
#[tauri::command]
pub async fn inspect_backup(path: String) -> Result<BackupFileInfo> {
    let script = read_to_string_blocking(path).await?;
    let statements = crate::sql::split_statements(&script);

    Ok(BackupFileInfo {
        statements: statements.len(),
        bytes: script.len() as u64,
        // Counted rather than parsed: a `CREATE TABLE` present means the dump
        // carries schema, which decides whether the target must be empty.
        has_schema: statements.iter().any(|s| {
            s.trim_start()
                .to_ascii_uppercase()
                .starts_with("CREATE TABLE")
        }),
        first_lines: script
            .lines()
            .take(6)
            .map(|line| line.chars().take(512).collect())
            .collect(),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFileInfo {
    pub statements: usize,
    pub bytes: u64,
    pub has_schema: bool,
    pub first_lines: Vec<String>,
}
