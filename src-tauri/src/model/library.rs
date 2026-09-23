use serde::{Deserialize, Serialize};

/// A query the user chose to keep.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    /// One level of grouping. `None` means the query sits at the top.
    #[serde(default)]
    pub folder: Option<String>,
    pub sql: String,
    /// Connection this was saved against, offered when reopening it. Kept even
    /// if that connection is later deleted — a dangling id is harmless, while
    /// losing the association would silently change which database a saved
    /// query targets.
    #[serde(default)]
    pub connection_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One execution, recorded whether it succeeded or failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub sql: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Copied at write time rather than joined at read time, so history stays
    /// readable after the connection it ran against is deleted.
    #[serde(default)]
    pub connection_name: Option<String>,
    pub executed_at: String,
    pub duration_ms: i64,
    pub row_count: i64,
    /// The engine's message when this run failed. Failures are worth keeping —
    /// they are often exactly what the user wants to revisit.
    #[serde(default)]
    pub error: Option<String>,
    pub succeeded: bool,
}

/// A new history row, before the store assigns an id and timestamp.
#[derive(Debug, Clone)]
pub struct NewHistoryEntry {
    pub sql: String,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub duration_ms: i64,
    pub row_count: i64,
    pub error: Option<String>,
}
