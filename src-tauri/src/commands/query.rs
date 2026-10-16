use tauri::State;

use super::AppState;
use crate::error::Result;
use crate::model::{NewHistoryEntry, QueryOutcome};
use crate::sql;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub sql: String,
    /// `None` when this statement failed; `error` then explains why.
    pub outcome: Option<QueryOutcome>,
    /// The structured error — kind, message and the engine's own code — rather
    /// than its text alone. Flattened to a string, a failing statement lost
    /// what every other command's error keeps: whether it was a read-only
    /// refusal, a cancellation or a database error, and the SQLSTATE that
    /// identifies it.
    pub error: Option<crate::error::FaroError>,
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
) -> Result<std::sync::Arc<RunResult>> {
    let driver = state.registry.get(&connection_id).await?;
    // Clamped, not trusted: the limit comes from the frontend and feeds both
    // the row cap and the `limit + 1` truncation probe.
    let limit = crate::driver::clamp_limit(limit);
    // Split — and, below, classify — with this engine's own quoting rules.
    // A lexer that credits an engine with a delimiter it does not have skips
    // text the engine will still execute, which is how a write once hid from
    // the read-only guard behind a backslash in a quoted identifier.
    let rules = driver.dialect().lex_rules();
    // Comment-only fragments are dropped: a script ending in a comment would
    // otherwise send an empty statement, which MySQL rejects outright.
    let statements = sql::executable_statements(&sql_text, rules);

    // A read-only connection runs reads and nothing else.
    //
    // The whole script is checked before any of it runs: refusing halfway would
    // leave the earlier statements applied, which is exactly the outcome the
    // setting exists to prevent.
    //
    // Skipped for engines whose query language is not SQL: MongoDB queries are
    // documents, a leading-keyword test means nothing against them, and the
    // Mongo driver already refuses every write form it does not recognise.
    if driver.dialect().is_sql() && state.registry.is_read_only(&connection_id).await {
        if let Some(write) = statements
            .iter()
            .find(|s| !sql::is_read_only_statement_with(s, rules))
        {
            let name = state
                .store
                .get_connection(&connection_id)
                .ok()
                .flatten()
                .map(|c| c.name)
                .unwrap_or_else(|| connection_id.clone());
            return Err(crate::error::FaroError::read_only(&name, Some(write)));
        }
    }

    let query = state.registry.begin_query(&connection_id, &query_id);
    let started = std::time::Instant::now();

    let mut results = Vec::with_capacity(statements.len());
    for stmt in statements {
        match driver.run(&stmt, limit, query.token()).await {
            Ok(outcome) => {
                results.push(StatementResult {
                    sql: stmt,
                    outcome: Some(outcome),
                    error: None,
                });
            }
            Err(e) => {
                results.push(StatementResult {
                    sql: stmt,
                    outcome: None,
                    error: Some(e),
                });
                break;
            }
        }
    }

    // Deregisters the query; the elapsed time below must not include it.
    drop(query);
    let total_elapsed_ms = started.elapsed().as_millis() as u64;

    record_history(
        &state,
        &connection_id,
        &sql_text,
        &results,
        total_elapsed_ms,
    );

    // Kept so an export can read the rows from here rather than have the UI
    // send them back; see `results`.
    let run = std::sync::Arc::new(RunResult {
        statements: results,
        total_elapsed_ms,
    });
    state
        .results
        .insert(query_id, crate::results::Cached::Run(run.clone()));
    Ok(run)
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
        row_count: results
            .iter()
            .filter_map(|r| r.outcome.as_ref())
            .map(|o| o.row_count())
            .sum::<u64>() as i64,
        error: results
            .iter()
            .find_map(|r| r.error.as_ref().map(|e| e.to_string())),
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
    Ok(state.registry.cancel_query(&connection_id, &query_id))
}

/// What running a script would do, in numbers: how many statements it holds and
/// how many of them write.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptSummary {
    pub statements: usize,
    pub writes: usize,
}

/// Count a script's statements and writes before it runs, so the UI can ask
/// before ⌘↵ runs a whole script that changes data.
///
/// ⌘↵ runs the entire editor contents. Pressed out of habit with the cursor on
/// one `SELECT`, it also ran the `DELETE` three lines below, with nothing in
/// between to stop it.
///
/// Classified with the connection's own quoting rules when it is open — the
/// same reading the read-only guard uses — and conservatively otherwise.
/// MongoDB queries are not SQL and Faro's Mongo support cannot write, so they
/// never count as writes.
#[tauri::command]
pub async fn script_summary(
    state: State<'_, AppState>,
    connection_id: Option<String>,
    sql_text: String,
) -> Result<ScriptSummary> {
    let driver = match connection_id {
        Some(id) => state.registry.get(&id).await.ok(),
        None => None,
    };
    let rules = driver
        .as_ref()
        .map(|d| d.dialect().lex_rules())
        .unwrap_or(sql::LexRules::PERMISSIVE);
    let statements = sql::executable_statements(&sql_text, rules);

    let writes = match &driver {
        Some(d) if !d.dialect().is_sql() => 0,
        Some(d) => sql::count_writes(&statements, Some(d.dialect().lex_rules())),
        None => sql::count_writes(&statements, None),
    };
    Ok(ScriptSummary {
        statements: statements.len(),
        writes,
    })
}

/// Extract the statement under the cursor, so the UI can show what will run.
///
/// `offset` is a UTF-16 code unit index — a CodeMirror document position,
/// passed through unconverted. See [`sql::statement_at`].
#[tauri::command]
pub fn statement_at_cursor(sql_text: String, offset: usize) -> Option<String> {
    sql::statement_at(&sql_text, offset)
}
