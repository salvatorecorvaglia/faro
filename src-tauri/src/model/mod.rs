//! Engine-agnostic domain types shared between the drivers and the frontend.
//!
//! Everything crossing the IPC boundary is defined here so the UI never has to
//! know which engine produced a value.

mod browse;
mod connection;
mod edit;
mod library;
mod result;
mod schema;
mod value;

pub use browse::{BrowseOptions, ColumnFilter, FilterOp};
pub use connection::{ConnectionConfig, Engine, SslMode};
pub use edit::{ApplyResult, CellEdit, EditValue, GuardedStatement, PendingChange};
pub use library::{HistoryEntry, NewHistoryEntry, SavedQuery};
pub use result::{ColumnInfo, ExecResult, QueryOutcome, ResultSet};
pub use schema::{
    ColumnDetail, ForeignKey, IndexInfo, SchemaInfo, TableColumns, TableDetail, TableInfo,
    TableKind, TableRef,
};
pub use value::{quote_sql_string, quote_sql_string_backslash, Value};
