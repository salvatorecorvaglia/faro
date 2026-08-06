use serde::{Deserialize, Serialize};

/// A single cell, normalized across every engine.
///
/// The variants are chosen so the frontend can render without knowing the
/// source engine, while `ColumnInfo::type_name` still carries the native type
/// for display.
///
/// Two deliberate choices:
/// - `Decimal` stays a string end to end. Routing money through `f64` loses
///   precision, and there is no going back once it does.
/// - `Bytes` carries the raw buffer but the UI renders a size badge, never the
///   contents inline; a single BLOB column would otherwise wreck the grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact numerics, kept as text to preserve precision.
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    /// ISO-8601 date (`2026-08-05`).
    Date(String),
    /// ISO-8601 time (`13:45:00`).
    Time(String),
    /// ISO-8601 timestamp, with offset when the column is timezone-aware.
    Timestamp(String),
    Uuid(String),
    Json(serde_json::Value),
    Array(Vec<Value>),
    /// A type no driver knows how to decode. Carries the engine's type name so
    /// the user sees *something* truthful instead of a silent NULL.
    Unsupported(String),
}

impl Value {
    /// Whether this is SQL NULL, which the grid renders distinctly from an
    /// empty string.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Render as a SQL literal for dump generation and generated DML.
    ///
    /// `quote_bytes` differs per engine (`'\x..'` for Postgres, `X'..'` for
    /// SQLite/MySQL), so the dialect supplies it rather than hardcoding one.
    ///
    /// Takes a trait object rather than `impl Fn` because `Array` recurses:
    /// with a generic parameter each level would instantiate at `&F`, `&&F`,
    /// … and blow the monomorphization recursion limit.
    pub fn to_sql_literal(&self, quote_bytes: &dyn Fn(&[u8]) -> String) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Value::Int(i) => i.to_string(),
            // Non-finite floats have no portable literal; NULL is the only
            // honest option and is what most dump tools emit.
            Value::Float(f) if f.is_finite() => f.to_string(),
            Value::Float(_) => "NULL".into(),
            Value::Decimal(d) => d.clone(),
            Value::Bytes(b) => quote_bytes(b),
            Value::Json(j) => quote_sql_string(&j.to_string()),
            Value::Array(items) => {
                let inner: Vec<String> = items
                    .iter()
                    .map(|v| v.to_sql_literal(quote_bytes))
                    .collect();
                format!("ARRAY[{}]", inner.join(", "))
            }
            Value::Text(s)
            | Value::Date(s)
            | Value::Time(s)
            | Value::Timestamp(s)
            | Value::Uuid(s)
            | Value::Unsupported(s) => quote_sql_string(s),
        }
    }
}

/// Single-quote a string literal, doubling embedded quotes per the SQL standard.
pub fn quote_sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        format!(
            "X'{}'",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        )
    }

    #[test]
    fn escapes_embedded_quotes() {
        let v = Value::Text("O'Brien".into());
        assert_eq!(v.to_sql_literal(&hex), "'O''Brien'");
    }

    #[test]
    fn decimal_keeps_full_precision() {
        // The whole point of the Decimal variant: this must not become 0.1+0.2 float math.
        let v = Value::Decimal("12345678901234567890.123456789".into());
        assert_eq!(v.to_sql_literal(&hex), "12345678901234567890.123456789");
    }

    #[test]
    fn non_finite_floats_become_null() {
        assert_eq!(Value::Float(f64::NAN).to_sql_literal(&hex), "NULL");
        assert_eq!(Value::Float(f64::INFINITY).to_sql_literal(&hex), "NULL");
    }

    #[test]
    fn null_is_unquoted_keyword() {
        assert_eq!(Value::Null.to_sql_literal(&hex), "NULL");
        assert!(Value::Null.is_null());
        assert!(!Value::Text(String::new()).is_null());
    }

    #[test]
    fn bytes_use_the_dialect_quoter() {
        let v = Value::Bytes(vec![0xde, 0xad]);
        assert_eq!(v.to_sql_literal(&hex), "X'dead'");
    }
}
