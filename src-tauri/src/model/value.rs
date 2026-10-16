use serde::{Deserialize, Serialize};

/// A single cell, normalized across every engine.
///
/// The variants are chosen so the frontend can render without knowing the
/// source engine, while `ColumnInfo::type_name` still carries the native type
/// for display.
///
/// Three deliberate choices:
/// - `Decimal` stays a string end to end. Routing money through `f64` loses
///   precision, and there is no going back once it does.
/// - `Bytes` carries the raw buffer but the UI renders a size badge, never the
///   contents inline; a single BLOB column would otherwise wreck the grid.
/// - An `Int` too large for a JavaScript number crosses IPC as a `decimal`;
///   see the `Serialize` impl below.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

/// Largest integer magnitude a JavaScript `number` holds exactly (2^53 − 1).
const MAX_SAFE_JS_INT: i64 = (1 << 53) - 1;

/// Largest binary value sent to the UI whole.
///
/// The UI never shows a binary value's contents — the grid draws a size badge
/// — but each one crossed IPC as a JSON array of numbers, about four times its
/// size: a page of thumbnails or documents could be hundreds of megabytes of
/// JSON for nothing on screen. Past this, only the size is sent. Keys, hashes
/// and UUIDs fit under it, so a binary primary key still reaches the UI whole
/// and can be sent back to find its row.
pub const MAX_INLINE_BYTES: usize = 256;

/// The wire form of [`Value`]: the same tagged shape the derive produced,
/// borrowed rather than cloned.
#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
enum Wire<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(std::borrow::Cow<'a, str>),
    Text(&'a str),
    Bytes(&'a [u8]),
    /// A binary value too large to send whole: its size in bytes.
    LargeBytes(usize),
    Date(&'a str),
    Time(&'a str),
    Timestamp(&'a str),
    Uuid(&'a str),
    Json(&'a serde_json::Value),
    Array(&'a [Value]),
    Unsupported(&'a str),
}

/// Serialized by hand for one reason: integer precision across IPC.
///
/// Every value reaches the UI as JSON, and JavaScript numbers are doubles, so
/// an `i64` past 2^53 was silently rounded on arrival — a BIGINT id displayed
/// wrong, was exported wrong (a query-result export sends the rows back from
/// the UI), and was used wrong as the primary key of a grid edit, where the
/// rounded key could match a *different* row. Those values go out as
/// `decimal` instead, which is a string end to end on both sides and is
/// rendered into SQL verbatim, so nothing downstream has to change. Ints that
/// fit keep their numeric form.
impl Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Value::Null => Wire::Null,
            Value::Bool(b) => Wire::Bool(*b),
            Value::Int(i) if i.unsigned_abs() > MAX_SAFE_JS_INT as u64 => {
                Wire::Decimal(std::borrow::Cow::Owned(i.to_string()))
            }
            Value::Int(i) => Wire::Int(*i),
            Value::Float(f) => Wire::Float(*f),
            Value::Decimal(d) => Wire::Decimal(std::borrow::Cow::Borrowed(d)),
            Value::Text(t) => Wire::Text(t),
            Value::Bytes(b) if b.len() > MAX_INLINE_BYTES => Wire::LargeBytes(b.len()),
            Value::Bytes(b) => Wire::Bytes(b),
            Value::Date(d) => Wire::Date(d),
            Value::Time(t) => Wire::Time(t),
            Value::Timestamp(t) => Wire::Timestamp(t),
            Value::Uuid(u) => Wire::Uuid(u),
            Value::Json(j) => Wire::Json(j),
            Value::Array(a) => Wire::Array(a),
            Value::Unsupported(u) => Wire::Unsupported(u),
        };
        wire.serialize(s)
    }
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

/// Whether text is a bare numeric literal: an optional leading `-`, digits, and
/// at most one decimal point.
///
/// Deliberately strict: no hex, no underscores, no exponent forms, no leading
/// `+`. This is what decides whether text may be emitted into SQL *unquoted* —
/// a `Value::Decimal`, or a value typed into a numeric column — so anything
/// unusual must fail it and be quoted instead.
///
/// One copy for every caller: DML generation and ClickHouse decoding each kept
/// an identical version under a different name.
pub fn is_plain_number(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    if body.is_empty() {
        return false;
    }
    let mut seen_dot = false;
    let mut seen_digit = false;
    for c in body.chars() {
        match c {
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    seen_digit
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

    fn wire(v: &Value) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn ints_that_fit_a_javascript_number_stay_numeric() {
        for i in [0, 42, -7, MAX_SAFE_JS_INT, -MAX_SAFE_JS_INT] {
            assert_eq!(
                wire(&Value::Int(i)),
                serde_json::json!({ "kind": "int", "value": i }),
                "{i}"
            );
        }
    }

    #[test]
    fn ints_past_2_pow_53_cross_ipc_as_exact_decimal_text() {
        // As a JSON number, 9007199254740993 arrives in JavaScript as
        // 9007199254740992 — a different id.
        for i in [
            MAX_SAFE_JS_INT + 2,
            i64::MAX,
            i64::MIN,
            -(MAX_SAFE_JS_INT + 2),
        ] {
            assert_eq!(
                wire(&Value::Int(i)),
                serde_json::json!({ "kind": "decimal", "value": i.to_string() }),
                "{i}"
            );
        }
    }

    #[test]
    fn the_wire_shape_is_unchanged_for_every_other_variant() {
        assert_eq!(wire(&Value::Null), serde_json::json!({ "kind": "null" }));
        assert_eq!(
            wire(&Value::Text("a".into())),
            serde_json::json!({ "kind": "text", "value": "a" })
        );
        assert_eq!(
            wire(&Value::Bytes(vec![1, 2])),
            serde_json::json!({ "kind": "bytes", "value": [1, 2] })
        );
        assert_eq!(
            wire(&Value::Array(vec![Value::Int(i64::MAX)])),
            serde_json::json!({
                "kind": "array",
                "value": [{ "kind": "decimal", "value": i64::MAX.to_string() }]
            })
        );
    }

    #[test]
    fn a_large_binary_value_crosses_ipc_as_its_size_only() {
        // The UI shows a size badge for every binary value, so sending a large
        // one whole was megabytes of JSON for nothing on screen.
        assert_eq!(
            wire(&Value::Bytes(vec![7; MAX_INLINE_BYTES + 1])),
            serde_json::json!({ "kind": "largeBytes", "value": MAX_INLINE_BYTES + 1 })
        );
        // Keys, hashes and UUIDs still arrive whole, so they can be sent back.
        let uuid = vec![1u8; 16];
        assert_eq!(
            wire(&Value::Bytes(uuid.clone())),
            serde_json::json!({ "kind": "bytes", "value": uuid })
        );
        assert_eq!(
            wire(&Value::Bytes(vec![0; MAX_INLINE_BYTES]))["kind"],
            "bytes"
        );
    }

    #[test]
    fn a_huge_int_round_trips_through_the_ui_without_losing_digits() {
        // `export_result` receives rows back from the frontend. The value must
        // come back as the same digits, rendered verbatim into SQL.
        let json = serde_json::to_string(&Value::Int(i64::MAX)).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Value::Decimal(i64::MAX.to_string()));
        assert_eq!(
            back.to_sql_literal(&hex, &quote_sql_string),
            i64::MAX.to_string()
        );
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
