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
) -> Result<BackupResult> {
    let driver = state.registry.get(&connection_id).await?;
    let target = PathBuf::from(&path);
    let cancel = state.registry.begin_query(&connection_id, &query_id).await;

    let result = backup::write_backup(
        &*driver,
        &target,
        &options,
        cancel,
        |progress: BackupProgress| {
            // A dropped listener must not fail the backup.
            let _ = app.emit(BACKUP_PROGRESS, &progress);
        },
    )
    .await;

    state.registry.end_query(&connection_id, &query_id).await;
    result
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
    let script = std::fs::read_to_string(&path)?;
    let cancel = state.registry.begin_query(&connection_id, &query_id).await;

    let result = backup::restore(&*driver, &script, &options, cancel, |done, total| {
        let _ = app.emit(RESTORE_PROGRESS, RestoreProgress { done, total });
    })
    .await;

    state.registry.end_query(&connection_id, &query_id).await;
    result
}

/// Statement count for a dump file, so the UI can warn before running it.
#[tauri::command]
pub fn inspect_backup(path: String) -> Result<BackupFileInfo> {
    let script = std::fs::read_to_string(&path)?;
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
        first_lines: script.lines().take(6).map(String::from).collect(),
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
