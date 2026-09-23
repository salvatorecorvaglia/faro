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
    /// Both quoters differ per engine, so the dialect supplies them rather than
    /// hardcoding one. `quote_bytes` is `'\x..'` for Postgres and `X'..'` for
    /// SQLite/MySQL; `quote_string` has to double the backslash on the engines
    /// that treat it as an escape (MySQL, MariaDB, ClickHouse) and must not on
    /// the ones that do not.
    ///
    /// Takes trait objects rather than `impl Fn` because `Array` recurses:
    /// with generic parameters each level would instantiate at `&F`, `&&F`,
    /// … and blow the monomorphization recursion limit.
    pub fn to_sql_literal(
        &self,
        quote_bytes: &dyn Fn(&[u8]) -> String,
        quote_string: &dyn Fn(&str) -> String,
    ) -> String {
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
            Value::Json(j) => quote_string(&j.to_string()),
            Value::Array(items) => {
                let inner: Vec<String> = items
                    .iter()
                    .map(|v| v.to_sql_literal(quote_bytes, quote_string))
                    .collect();
                format!("ARRAY[{}]", inner.join(", "))
            }
            Value::Text(s)
            | Value::Date(s)
            | Value::Time(s)
            | Value::Timestamp(s)
            | Value::Uuid(s)
            | Value::Unsupported(s) => quote_string(s),
        }
    }
}

/// Single-quote a string literal, doubling embedded quotes per the SQL standard.
///
/// Correct only for engines where the backslash is an ordinary character —
/// Postgres (with `standard_conforming_strings`, the default since 9.1),
/// SQLite, DuckDB and SQL Server. Use [`quote_sql_string_backslash`] anywhere
/// else; see its comment for what goes wrong.
pub fn quote_sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Single-quote a string literal for engines that also treat `\` as an escape
/// character inside `'...'` — MySQL, MariaDB (default `sql_mode`, i.e. without
/// `NO_BACKSLASH_ESCAPES`) and ClickHouse.
///
/// Doubling only the quote is not enough there. A value ending in a backslash
/// escapes its own closing delimiter, the literal runs on past where the
/// generator believes it ended, and whatever follows — the rest of a `WHERE`
/// clause, the next column in an `INSERT` — is parsed as SQL rather than data.
/// That is a live injection wherever the value is user- or file-supplied.
pub fn quote_sql_string_backslash(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
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
        assert_eq!(v.to_sql_literal(&hex, &quote_sql_string), "'O''Brien'");
    }

    #[test]
    fn decimal_keeps_full_precision() {
        // The whole point of the Decimal variant: this must not become 0.1+0.2 float math.
        let v = Value::Decimal("12345678901234567890.123456789".into());
        assert_eq!(
            v.to_sql_literal(&hex, &quote_sql_string),
            "12345678901234567890.123456789"
        );
    }

    #[test]
    fn non_finite_floats_become_null() {
        assert_eq!(
            Value::Float(f64::NAN).to_sql_literal(&hex, &quote_sql_string),
            "NULL"
        );
        assert_eq!(
            Value::Float(f64::INFINITY).to_sql_literal(&hex, &quote_sql_string),
            "NULL"
        );
    }

    #[test]
    fn null_is_unquoted_keyword() {
        assert_eq!(Value::Null.to_sql_literal(&hex, &quote_sql_string), "NULL");
        assert!(Value::Null.is_null());
        assert!(!Value::Text(String::new()).is_null());
    }

    #[test]
    fn bytes_use_the_dialect_quoter() {
        let v = Value::Bytes(vec![0xde, 0xad]);
        assert_eq!(v.to_sql_literal(&hex, &quote_sql_string), "X'dead'");
    }

    #[test]
    fn standard_quoting_leaves_backslashes_alone() {
        // Postgres, SQLite, DuckDB and SQL Server: `\` is an ordinary
        // character, and doubling it would corrupt the value.
        assert_eq!(quote_sql_string(r"C:\tmp"), r"'C:\tmp'");
        assert_eq!(quote_sql_string(r"trailing\"), r"'trailing\'");
    }

    #[test]
    fn backslash_quoting_closes_the_escape_hole() {
        // The injection primitive: a value ending in a backslash must not be
        // able to escape its own closing quote.
        assert_eq!(quote_sql_string_backslash(r"trailing\"), r"'trailing\\'");
        assert_eq!(quote_sql_string_backslash(r"C:\tmp"), r"'C:\\tmp'");
        // Both escapes at once.
        assert_eq!(
            quote_sql_string_backslash(r"a\'; DROP TABLE t; --"),
            r"'a\\''; DROP TABLE t; --'"
        );
    }

    #[test]
    fn backslash_quoting_still_doubles_quotes() {
        assert_eq!(quote_sql_string_backslash("O'Brien"), "'O''Brien'");
    }

    #[test]
    fn the_quoted_literal_is_always_balanced() {
        // Whatever goes in, the result must be one complete literal: an odd
        // number of unescaped delimiters is exactly what an injection needs.
        for input in [
            r"\",
            r"\\",
            r"'",
            r"''",
            r"\'",
            r"'\",
            r"\\'",
            "plain",
            "",
            r"a\'; SELECT 1; --",
        ] {
            for quoted in [quote_sql_string_backslash(input)] {
                assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
                // Strip the delimiters, then every remaining backslash and
                // quote must be part of a doubled pair.
                let body = &quoted[1..quoted.len() - 1];
                let mut chars = body.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '\\' || c == '\'' {
                        assert_eq!(
                            chars.next(),
                            Some(c),
                            "unpaired {c:?} in {quoted:?} from input {input:?}"
                        );
                    }
                }
            }
        }
    }
}
