//! Grouping a catalog's one-row-per-column foreign key listing into one
//! `ForeignKey` per constraint.
//!
//! Every engine's foreign-key query returns exactly that shape — a composite
//! key spans several rows, one per column — so folding them back together is
//! the same operation everywhere. What differs between engines is only how a
//! row says which constraint it belongs to: MySQL and SQL Server's catalogs
//! name the constraint directly; SQLite's `PRAGMA foreign_key_list` has no
//! name, only a numeric id that is not even in declaration order, so its
//! driver groups by that id and invents a name afterward. `K` is that
//! grouping key, generic over both cases.

use crate::model::{ForeignKey, TableRef};

/// One row of a composite foreign key's per-column detail, before grouping.
pub(crate) struct FkColumnRow<K> {
    /// Ties a composite key's rows together — see the module docs for why
    /// this is generic rather than always the constraint name.
    pub group: K,
    pub column: String,
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    /// `None` when the catalog reported no counterpart for this column,
    /// which does happen (a foreign key with more local columns than
    /// referenced ones is invalid SQL, but a row a driver failed to decode
    /// cleanly should not crash the whole listing over one bad key).
    pub referenced_column: Option<String>,
}

/// Fold rows into one `ForeignKey` per distinct `group`, in the order each
/// group was first seen. `name_of` turns a group key into the constraint's
/// display name — identity for engines that group by name already, a
/// synthesized name for SQLite.
pub(crate) fn group_foreign_keys<K: PartialEq>(
    rows: Vec<FkColumnRow<K>>,
    name_of: impl Fn(&K) -> String,
) -> Vec<ForeignKey> {
    let mut groups: Vec<(K, ForeignKey)> = Vec::new();
    for row in rows {
        if let Some((_, fk)) = groups.iter_mut().find(|(k, _)| *k == row.group) {
            fk.columns.push(row.column);
            fk.referenced_columns.extend(row.referenced_column);
            continue;
        }
        let fk = ForeignKey {
            name: name_of(&row.group),
            columns: vec![row.column],
            referenced_table: TableRef {
                schema: row.referenced_schema,
                name: row.referenced_table,
            },
            referenced_columns: row.referenced_column.into_iter().collect(),
        };
        groups.push((row.group, fk));
    }
    groups.into_iter().map(|(_, fk)| fk).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_composite_columns_under_one_constraint() {
        let rows = vec![
            FkColumnRow {
                group: "fk_order_item".to_string(),
                column: "order_id".to_string(),
                referenced_schema: None,
                referenced_table: "orders".to_string(),
                referenced_column: Some("id".to_string()),
            },
            FkColumnRow {
                group: "fk_order_item".to_string(),
                column: "item_id".to_string(),
                referenced_schema: None,
                referenced_table: "orders".to_string(),
                referenced_column: Some("item".to_string()),
            },
        ];

        let fks = group_foreign_keys(rows, |name| name.clone());
        assert_eq!(fks.len(), 1);
        assert_eq!(fks[0].columns, vec!["order_id", "item_id"]);
        assert_eq!(fks[0].referenced_columns, vec!["id", "item"]);
    }

    #[test]
    fn keeps_distinct_constraints_separate_and_in_first_seen_order() {
        let rows = vec![
            FkColumnRow {
                group: 2,
                column: "b".to_string(),
                referenced_schema: None,
                referenced_table: "t2".to_string(),
                referenced_column: Some("id".to_string()),
            },
            FkColumnRow {
                group: 1,
                column: "a".to_string(),
                referenced_schema: None,
                referenced_table: "t1".to_string(),
                referenced_column: Some("id".to_string()),
            },
        ];

        // SQLite's ids arrive descending (see the module docs) — this is
        // exactly that case, and the output order must still follow the
        // rows, not the id values.
        let fks = group_foreign_keys(rows, |id: &i32| format!("fk_t_{id}"));
        assert_eq!(fks.len(), 2);
        assert_eq!(fks[0].name, "fk_t_2");
        assert_eq!(fks[1].name, "fk_t_1");
    }

    #[test]
    fn a_missing_referenced_column_does_not_panic_or_desync_the_columns() {
        let rows = vec![FkColumnRow {
            group: "fk_x".to_string(),
            column: "a".to_string(),
            referenced_schema: Some("dbo".to_string()),
            referenced_table: "t".to_string(),
            referenced_column: None,
        }];

        let fks = group_foreign_keys(rows, |name| name.clone());
        assert_eq!(fks[0].columns, vec!["a"]);
        assert!(fks[0].referenced_columns.is_empty());
        assert_eq!(fks[0].referenced_table.schema.as_deref(), Some("dbo"));
    }
}
