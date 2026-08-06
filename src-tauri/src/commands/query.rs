use tauri::State;

use super::AppState;
use crate::error::Result;
use crate::model::{NewHistoryEntry, QueryOutcome};
use crate::sql;

/// Default page size. Large enough that most results arrive whole, small enough
/// that a stray `SELECT *` on a huge table cannot lock up the UI.
const DEFAULT_LIMIT: u64 = 1000;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub sql: String,
    /// `None` when this statement failed; `error` then explains why.
    pub outcome: Option<QueryOutcome>,
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub statements: Vec<StatementResult>,
    pub total_elapsed_ms: u64,
}

/// Run SQL, which may contain several statements.
///
/// Statements run in order and stop at the first failure — continuing past an
/// error in a migration-style script would compound the damage. Results already
/// produced are still returned so the user sees how far it got.
///
/// Every run is recorded in history, successes and failures alike, and
/// cancellations too: "what did I run just before I killed it" is exactly the
/// question history exists to answer.
#[tauri::command]
pub async fn run_query(
    state: State<'_, AppState>,
    connection_id: String,
    sql_text: String,
    query_id: String,
    limit: Option<u64>,
) -> Result<RunResult> {
    let driver = state.registry.get(&connection_id).await?;
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let statements = sql::split_statements(&sql_text);

    let token = state.registry.begin_query(&connection_id, &query_id).await;
    let started = std::time::Instant::now();

    let mut results = Vec::with_capacity(statements.len());
    for stmt in statements {
        match driver.run(&stmt, limit, token.clone()).await {
            Ok(outcome) => {
                results.push(StatementResult { sql: stmt, outcome: Some(outcome), error: None });
            }
            Err(e) => {
                results.push(StatementResult {
                    sql: stmt,
                    outcome: None,
                    error: Some(e.to_string()),
                });
                break;
            }
        }
    }

    state.registry.end_query(&connection_id, &query_id).await;
    let total_elapsed_ms = started.elapsed().as_millis() as u64;

    record_history(&state, &connection_id, &sql_text, &results, total_elapsed_ms);

    Ok(RunResult { statements: results, total_elapsed_ms })
}

/// Write one history row for the whole submission.
///
/// One row rather than one per statement: the user submitted a single piece of
/// text and will want to re-run that same text, not reassemble it from parts.
fn record_history(
    state: &AppState,
    connection_id: &str,
    sql_text: &str,
    results: &[StatementResult],
    elapsed_ms: u64,
) {
    // The name is copied in so history stays readable after the connection is
    // deleted. A lookup failure is not worth failing the run over.
    let connection_name = state
        .store
        .get_connection(connection_id)
        .ok()
        .flatten()
        .map(|c| c.name);

    let entry = NewHistoryEntry {
        sql: sql_text.to_string(),
        connection_id: Some(connection_id.to_string()),
        connection_name,
        duration_ms: elapsed_ms as i64,
        row_count: results.iter().filter_map(|r| r.outcome.as_ref()).map(|o| o.row_count()).sum::<u64>()
            as i64,
        error: results.iter().find_map(|r| r.error.clone()),
    };

    // Best effort throughout: a history write must never break the query the
    // user actually asked for.
    if state.store.add_history(&entry).is_ok() {
        super::library::prune(state);
    }
}

/// Abort a running query. Returns false if it had already finished.
#[tauri::command]
pub async fn cancel_query(
    state: State<'_, AppState>,
    connection_id: String,
    query_id: String,
) -> Result<bool> {
    Ok(state.registry.cancel_query(&connection_id, &query_id).await)
}

/// Extract the statement under the cursor, so the UI can show what will run.
#[tauri::command]
pub fn statement_at_cursor(sql_text: String, offset: usize) -> Option<String> {
    sql::statement_at(&sql_text, offset)
}
