//! Reading catalog query results — the drivers' metadata queries.
//!
//! Each of these was written out separately in several drivers: two identical
//! pairs of cell readers under different names in ClickHouse and SQL Server,
//! and the same grouping loop in four `schema_snapshot`s. One copy each now.

use crate::model::{TableColumns, Value};

/// Read a catalog cell as text, where the column is known to be character
/// data. Anything absent or of another kind reads as empty.
pub(super) fn cell_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Text(s)) => s.clone(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Decimal(d)) => d.clone(),
        _ => String::new(),
    }
}

/// Read a catalog cell as an integer. Engines return counts and positions as
/// integers, decimals or digit strings depending on width; anything that does
/// not parse reads as zero.
pub(super) fn cell_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Int(i)) => *i,
        Some(Value::Decimal(d)) | Some(Value::Text(d)) => d.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Group `(table, column)` pairs into one entry per table, for autocomplete.
///
/// The rows must arrive ordered by table — every caller's catalog query sorts
/// by table, then position — so each table's columns are contiguous and keep
/// their declared order.
pub(super) fn group_columns(
    rows: impl IntoIterator<Item = (String, String)>,
    schema: Option<String>,
) -> Vec<TableColumns> {
    let mut out: Vec<TableColumns> = Vec::new();
    for (table, column) in rows {
        match out.last_mut().filter(|t| t.name == table) {
            Some(entry) => entry.columns.push(column),
            None => out.push(TableColumns {
                schema: schema.clone(),
                name: table,
                columns: vec![column],
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_group_by_table_in_their_arriving_order() {
        let rows =
            [("a", "id"), ("a", "name"), ("b", "id")].map(|(t, c)| (t.to_string(), c.to_string()));
        let out = group_columns(rows, Some("public".into()));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "a");
        assert_eq!(out[0].columns, ["id", "name"]);
        assert_eq!(out[0].schema.as_deref(), Some("public"));
        assert_eq!(out[1].columns, ["id"]);
        assert!(group_columns(std::iter::empty(), None).is_empty());
    }

    #[test]
    fn cells_read_whatever_width_the_engine_chose() {
        assert_eq!(cell_text(Some(&Value::Text("x".into()))), "x");
        assert_eq!(cell_text(Some(&Value::Int(7))), "7");
        assert_eq!(cell_text(None), "");
        assert_eq!(cell_int(Some(&Value::Int(7))), 7);
        assert_eq!(cell_int(Some(&Value::Decimal("42".into()))), 42);
        assert_eq!(cell_int(Some(&Value::Text("9".into()))), 9);
        assert_eq!(cell_int(Some(&Value::Text("x".into()))), 0);
        assert_eq!(cell_int(Some(&Value::Null)), 0);
    }
}
