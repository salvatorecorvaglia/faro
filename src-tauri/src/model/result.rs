use serde::{Deserialize, Serialize};

use super::value::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// The engine's own type name (`int8`, `VARCHAR(50)`, `uniqueidentifier`).
    /// Shown verbatim so users see database truth, not Faro's normalization.
    pub type_name: String,
}

/// The result of a statement that returned rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    /// True when the engine had more rows than the fetch limit.
    ///
    /// Determined by asking for `limit + 1` rows and seeing the extra one, so
    /// there is no `COUNT(*)` on a large table just to render a footer.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// The result of a statement that returned no rows (INSERT/UPDATE/DDL).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub rows_affected: u64,
    pub elapsed_ms: u64,
}

/// What a single statement produced. A script yields one of these per statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum QueryOutcome {
    Rows(ResultSet),
    Affected(ExecResult),
}

impl QueryOutcome {
    pub fn elapsed_ms(&self) -> u64 {
        match self {
            QueryOutcome::Rows(r) => r.elapsed_ms,
            QueryOutcome::Affected(e) => e.elapsed_ms,
        }
    }

    /// Row count for history entries: rows returned, or rows affected.
    pub fn row_count(&self) -> u64 {
        match self {
            QueryOutcome::Rows(r) => r.rows.len() as u64,
            QueryOutcome::Affected(e) => e.rows_affected,
        }
    }
}
