//! Conservative retained-memory budgets. Drivers may still allocate one native
//! row/frame before it can be measured; these are not process RSS limits.
use crate::{
    error::{FaroError, Result},
    model::{ResultSet, Value},
};
pub const RESULT_BYTES: usize = 32 * 1024 * 1024;
pub const RUN_BYTES: usize = 64 * 1024 * 1024;
pub const CACHE_BYTES: usize = 128 * 1024 * 1024;
pub const MATERIALIZED_BYTES: usize = 64 * 1024 * 1024;

pub fn json_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value::*;
    64usize.saturating_add(match value {
        String(text) => text.len().saturating_mul(2),
        Array(values) => values.iter().map(json_bytes).sum(),
        Object(values) => values
            .iter()
            .map(|(key, value)| {
                key.len()
                    .saturating_mul(2)
                    .saturating_add(json_bytes(value))
            })
            .sum(),
        _ => 0,
    })
}
pub fn value_bytes(value: &Value) -> usize {
    use Value::*;
    64usize.saturating_add(match value {
        Text(s) | Decimal(s) | Date(s) | Time(s) | Timestamp(s) | Uuid(s) | Unsupported(s) => {
            s.len().saturating_mul(2)
        }
        Bytes(bytes) => bytes.len(),
        Json(value) => json_bytes(value),
        Array(values) => values.iter().map(value_bytes).sum(),
        _ => 0,
    })
}
pub fn row_bytes(row: &[Value]) -> usize {
    24usize.saturating_add(row.iter().map(value_bytes).sum::<usize>())
}
pub fn result_bytes(result: &ResultSet) -> usize {
    result
        .rows
        .iter()
        .map(|row| row_bytes(row))
        .sum::<usize>()
        .saturating_add(
            result
                .columns
                .iter()
                .map(|column| 64 + 2 * (column.name.len() + column.type_name.len()))
                .sum::<usize>(),
        )
}
pub fn exceeded(scope: &str) -> FaroError {
    FaroError::ResourceLimit(format!("{scope} exceeds Faro's memory budget. Reduce the row limit, select fewer columns, or export in pages. Statements already sent to the database may have completed."))
}
#[derive(Default)]
pub struct RowBudget {
    rows: usize,
    columns: usize,
}
impl RowBudget {
    pub fn columns(&mut self, columns: &[crate::model::ColumnInfo]) -> Result<()> {
        self.columns = columns
            .iter()
            .map(|column| 64 + 2 * (column.name.len() + column.type_name.len()))
            .sum();
        if self.rows.saturating_add(self.columns) > RESULT_BYTES {
            return Err(exceeded("This result's columns"));
        }
        Ok(())
    }
    pub fn push(&mut self, row: &[Value]) -> Result<()> {
        self.rows = self.rows.saturating_add(row_bytes(row));
        if self.rows.saturating_add(self.columns) > RESULT_BYTES {
            return Err(exceeded("This result"));
        }
        Ok(())
    }
}
pub fn read_materialized(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MATERIALIZED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MATERIALIZED_BYTES {
        return Err(exceeded("This file requires whole-file parsing and"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn row_limit_cannot_hide_an_oversized_binary_or_text_value() {
        let row = vec![Value::Text("x".repeat(RESULT_BYTES / 2))];
        assert!(matches!(
            RowBudget::default().push(&row),
            Err(FaroError::ResourceLimit(_))
        ));
        assert_eq!(value_bytes(&Value::Bytes(vec![0; 128])), 192);
    }
    #[test]
    fn nested_json_and_arrays_are_included_in_the_budget() {
        assert!(
            value_bytes(&Value::Json(
                serde_json::json!({"nested": ["x".repeat(1000)]})
            )) > 2000
        );
        assert!(value_bytes(&Value::Array(vec![Value::Text("x".repeat(1000))])) > 2000);
    }
}
