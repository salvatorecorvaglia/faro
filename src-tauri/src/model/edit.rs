use serde::{Deserialize, Serialize};

/// A value the user typed into the grid, before it becomes a SQL literal.
///
/// Text and NULL are separate variants on purpose. In a grid, an emptied cell
/// is ambiguous — it could mean the empty string or NULL — and guessing would
/// silently write the wrong thing. The UI makes the user say which.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum EditValue {
    Null,
    /// Raw text as typed. Coerced against the column's declared type when the
    /// statement is generated.
    Text(String),
    /// Leave the column out of the statement entirely, so the database applies
    /// its own DEFAULT. Distinct from NULL: a DEFAULT of `now()` and an
    /// explicit NULL are very different outcomes.
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellEdit {
    pub column: String,
    pub value: EditValue,
}

/// One staged change. Applied only when the user explicitly confirms.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PendingChange {
    /// Change columns of the row identified by `key`.
    Update {
        /// Primary key columns and their *original* values, so the row is
        /// found even when the edit changes the key itself.
        key: Vec<CellEdit>,
        cells: Vec<CellEdit>,
    },
    Insert {
        cells: Vec<CellEdit>,
    },
    Delete {
        key: Vec<CellEdit>,
    },
}

/// A statement plus the number of rows it must affect.
///
/// The expectation is the safety net. A generated `UPDATE` should touch
/// exactly one row; if it touches none the row vanished underneath the user,
/// and if it touches several the key was not unique after all. Either way the
/// whole batch is rolled back rather than half-applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardedStatement {
    pub sql: String,
    /// `None` disables the check — used for inserts, where some engines report
    /// counts inconsistently.
    pub expect: Option<u64>,
}

/// What `apply_changes` did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub statements_run: usize,
    pub rows_affected: u64,
}
