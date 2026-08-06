use tauri::State;

use super::AppState;
use crate::dml;
use crate::error::Result;
use crate::model::{ApplyResult, GuardedStatement, PendingChange, TableRef};

/// The SQL that a set of staged changes would run, without running it.
///
/// The grid shows this before anything is applied. Someone about to modify
/// production data deserves to see the exact statements first, and the preview
/// goes through the identical code path as the apply — a preview generated
/// differently from what executes would be worse than none at all.
#[tauri::command]
pub async fn preview_changes(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    changes: Vec<PendingChange>,
) -> Result<Vec<GuardedStatement>> {
    let driver = state.registry.get(&connection_id).await?;
    let detail = driver.describe_table(&table).await?;
    dml::build_statements(&table, &detail, &changes, driver.dialect())
}

/// Apply staged changes atomically.
///
/// The table is re-described first rather than trusting a shape the frontend
/// cached: if a column was dropped or the primary key changed since the grid
/// loaded, the statements must be built against what the database has now.
#[tauri::command]
pub async fn apply_changes(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    changes: Vec<PendingChange>,
) -> Result<ApplyResult> {
    let driver = state.registry.get(&connection_id).await?;
    let detail = driver.describe_table(&table).await?;
    let statements = dml::build_statements(&table, &detail, &changes, driver.dialect())?;

    let rows_affected = driver.apply_transaction(&statements).await?;

    Ok(ApplyResult { statements_run: statements.len(), rows_affected })
}
