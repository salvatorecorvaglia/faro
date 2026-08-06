use serde::{Deserialize, Serialize};

/// A fully-qualified table reference.
///
/// `schema` is optional because SQLite and DuckDB have no meaningful schema
/// layer; forcing a placeholder there would leak into every generated query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    /// Marks `pg_catalog`, `information_schema` and friends so the tree can
    /// collapse them by default instead of burying the user's own tables.
    pub is_system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
    /// Planner estimate, not an exact count — exact counts are too expensive to
    /// gather while painting a schema tree.
    pub estimated_rows: Option<i64>,
}

/// One table and its column names, flattened for autocomplete.
///
/// Separate from `TableDetail` on purpose: the editor needs names for every
/// table at once and nothing else, while `TableDetail` costs several queries
/// per table to assemble keys, indexes and defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableColumns {
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDetail {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_primary_key: bool,
    pub ordinal: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: TableRef,
    pub referenced_columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub is_unique: bool,
    pub is_primary: bool,
    /// Created implicitly to enforce a constraint, rather than declared with
    /// `CREATE INDEX`.
    ///
    /// Backup must not re-emit these: the constraint in the table definition
    /// creates them, and SQLite outright refuses a `CREATE INDEX` naming one
    /// (`sqlite_autoindex_*` is reserved).
    #[serde(default)]
    pub is_constraint: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDetail {
    pub table: TableRef,
    pub kind: TableKind,
    pub columns: Vec<ColumnDetail>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexInfo>,
}

impl TableDetail {
    /// A table without a primary key cannot be safely edited: there is no way
    /// to address exactly one row, and an `UPDATE` could silently hit many.
    /// The grid goes read-only when this is false.
    pub fn is_editable(&self) -> bool {
        self.kind == TableKind::Table && !self.primary_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(kind: TableKind, pk: Vec<&str>) -> TableDetail {
        TableDetail {
            table: TableRef { schema: None, name: "t".into() },
            kind,
            columns: vec![],
            primary_key: pk.into_iter().map(String::from).collect(),
            foreign_keys: vec![],
            indexes: vec![],
        }
    }

    #[test]
    fn tables_need_a_primary_key_to_be_editable() {
        assert!(detail(TableKind::Table, vec!["id"]).is_editable());
        assert!(!detail(TableKind::Table, vec![]).is_editable());
    }

    #[test]
    fn views_are_never_editable() {
        // Even with a PK-looking column set, a view has no addressable rows.
        assert!(!detail(TableKind::View, vec!["id"]).is_editable());
        assert!(!detail(TableKind::MaterializedView, vec!["id"]).is_editable());
    }
}
