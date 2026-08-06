use tauri::State;

use super::AppState;
use crate::error::Result;
use crate::model::{HistoryEntry, SavedQuery};

/// How many history rows to keep. Roughly a year of heavy use, and small
/// enough that the table stays fast without any maintenance from the user.
const HISTORY_CAP: i64 = 10_000;

/// Default page size for the history panel.
const HISTORY_PAGE: i64 = 300;

// -- Saved queries ---------------------------------------------------------

#[tauri::command]
pub fn list_saved_queries(state: State<'_, AppState>) -> Result<Vec<SavedQuery>> {
    state.store.list_saved_queries()
}

/// Create or update a saved query. Returns the stored row, so the caller picks
/// up the generated id and timestamps.
#[tauri::command]
pub fn save_query(state: State<'_, AppState>, mut query: SavedQuery) -> Result<SavedQuery> {
    if query.id.is_empty() {
        query.id = uuid::Uuid::new_v4().to_string();
    }
    if query.name.trim().is_empty() {
        query.name = default_query_name(&query.sql);
    }
    // Treat a blank folder as "no folder" so an empty input field does not
    // create a folder literally named "".
    if query
        .folder
        .as_deref()
        .map(str::trim)
        .is_some_and(str::is_empty)
    {
        query.folder = None;
    }

    state.store.upsert_saved_query(&query)?;
    Ok(state.store.get_saved_query(&query.id)?.unwrap_or(query))
}

#[tauri::command]
pub fn delete_saved_query(state: State<'_, AppState>, id: String) -> Result<()> {
    state.store.delete_saved_query(&id)
}

// -- History ---------------------------------------------------------------

#[tauri::command]
pub fn list_history(
    state: State<'_, AppState>,
    search: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<HistoryEntry>> {
    // An empty search box means "everything", not "match the empty string".
    let search = search.filter(|s| !s.trim().is_empty());
    state
        .store
        .list_history(search.as_deref(), limit.unwrap_or(HISTORY_PAGE))
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<()> {
    state.store.clear_history()
}

#[tauri::command]
pub fn delete_history_entry(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.store.delete_history_entry(id)
}

/// Trim history to its cap. Called after each write.
pub fn prune(state: &AppState) {
    // Best effort: failing to prune must never fail the query the user ran.
    let _ = state.store.prune_history(HISTORY_CAP);
}

/// First meaningful line of the SQL, as a fallback name for an unnamed query.
fn default_query_name(sql: &str) -> String {
    let line = sql
        .lines()
        .map(str::trim)
        // Skip comments and blanks so the name is the actual statement.
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .unwrap_or("Untitled query");

    let mut name: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        name.push('…');
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_an_unnamed_query_after_its_first_statement() {
        assert_eq!(
            default_query_name("SELECT * FROM users"),
            "SELECT * FROM users"
        );
    }

    #[test]
    fn skips_leading_comments_and_blank_lines() {
        let sql = "\n-- daily report\n\nSELECT count(*) FROM orders";
        assert_eq!(default_query_name(sql), "SELECT count(*) FROM orders");
    }

    #[test]
    fn truncates_a_very_long_line() {
        let sql = "SELECT ".to_string() + &"a, ".repeat(80);
        let name = default_query_name(&sql);
        assert_eq!(name.chars().count(), 61, "60 chars plus the ellipsis");
        assert!(name.ends_with('…'));
    }

    #[test]
    fn falls_back_when_there_is_nothing_but_comments() {
        assert_eq!(default_query_name("-- just a note"), "Untitled query");
        assert_eq!(default_query_name(""), "Untitled query");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        // Slicing by byte would panic mid-character on multi-byte input.
        let sql = "SELECT '".to_string() + &"日".repeat(80) + "'";
        let name = default_query_name(&sql);
        assert_eq!(name.chars().count(), 61);
    }
}
