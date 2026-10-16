use tauri::State;

use super::AppState;
use crate::error::Result;
use crate::model::{
    BrowseOptions, ResultSet, SchemaInfo, TableColumns, TableDetail, TableInfo, TableRef,
};

#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<SchemaInfo>> {
    state
        .registry
        .get(&connection_id)
        .await?
        .list_schemas()
        .await
}

#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    connection_id: String,
    schema: Option<String>,
) -> Result<Vec<TableInfo>> {
    state
        .registry
        .get(&connection_id)
        .await?
        .list_tables(schema.as_deref())
        .await
}

#[tauri::command]
pub async fn describe_table(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
) -> Result<TableDetail> {
    state
        .registry
        .get(&connection_id)
        .await?
        .describe_table(&table)
        .await
}

/// Every table and column in a schema, for editor autocomplete.
#[tauri::command]
pub async fn schema_snapshot(
    state: State<'_, AppState>,
    connection_id: String,
    schema: Option<String>,
) -> Result<Vec<TableColumns>> {
    state
        .registry
        .get(&connection_id)
        .await?
        .schema_snapshot(schema.as_deref())
        .await
}

/// Read a page of a table with sorting and filtering applied by the engine.
///
/// The composition lives in `sql`/`driver` rather than here: MongoDB has no
/// `SELECT` to compose, so it overrides `Driver::browse` entirely.
#[tauri::command]
pub async fn browse_table(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    options: BrowseOptions,
    query_id: String,
) -> Result<std::sync::Arc<ResultSet>> {
    let driver = state.registry.get(&connection_id).await?;
    let query = state.registry.begin_query(&connection_id, &query_id);
    let page = std::sync::Arc::new(driver.browse(&table, &options, query.token()).await?);
    drop(query);
    // Kept so "export this page" reads it from here; see `results`.
    state
        .results
        .insert(query_id, crate::results::Cached::Page(page.clone()));
    Ok(page)
}
