//! SQL text utilities that are independent of any engine.

/// Split a script into individual statements.
///
/// Deliberately a lexer rather than a full parse: Faro must handle dialect
/// syntax that `sqlparser` rejects (engine-specific DDL, extensions), and a
/// user's script should still run even when it cannot be fully parsed. So this
/// tracks only what is needed to know whether a `;` is a real separator —
/// string literals, quoted identifiers, comments, and dollar-quoted bodies.
pub fn split_statements(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = script.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Line comment: consume to end of line.
            '-' if chars.peek() == Some(&'-') => {
                current.push(c);
                for n in chars.by_ref() {
                    current.push(n);
                    if n == '\n' {
                        break;
                    }
                }
            }
            // Block comment: consume to the closing */.
            '/' if chars.peek() == Some(&'*') => {
                current.push(c);
                current.push(chars.next().unwrap_or('*'));
                let mut prev = '\0';
                for n in chars.by_ref() {
                    current.push(n);
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            // Quoted regions. A doubled quote is an escape, not a terminator.
            '\'' | '"' | '`' => {
                current.push(c);
                loop {
                    match chars.next() {
                        Some(n) if n == c => {
                            current.push(n);
                            if chars.peek() == Some(&c) {
                                current.push(chars.next().unwrap_or(c));
                                continue;
                            }
                            break;
                        }
                        // Backslash escape (MySQL-style); consume the next char.
                        Some('\\') => {
                            current.push('\\');
                            if let Some(esc) = chars.next() {
                                current.push(esc);
                            }
                        }
                        Some(n) => current.push(n),
                        None => break,
                    }
                }
            }
            // Postgres dollar quoting: $$ ... $$ or $tag$ ... $tag$. Function
            // bodies routinely contain semicolons, so this must be respected.
            '$' => {
                let tag = read_dollar_tag(&mut chars);
                match tag {
                    Some(tag) => {
                        current.push_str(&tag);
                        consume_until_tag(&mut chars, &tag, &mut current);
                    }
                    None => current.push(c),
                }
            }
            ';' => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Read a `$tag$` opener starting just after the first `$`. Returns the full
/// delimiter including both dollars, or None if this is not a dollar quote.
fn read_dollar_tag(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let mut tag = String::from("$");
    let mut lookahead = chars.clone();

    loop {
        match lookahead.next() {
            Some('$') => {
                tag.push('$');
                // Commit the lookahead now that this is confirmed a tag.
                *chars = lookahead;
                return Some(tag);
            }
            // Tags are identifier-shaped; anything else means a bare `$`
            // (e.g. a `$1` placeholder), which is not a quote.
            Some(c) if c.is_alphanumeric() || c == '_' => tag.push(c),
            _ => return None,
        }
    }
}

fn consume_until_tag(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    tag: &str,
    out: &mut String,
) {
    let mut window = String::new();
    for c in chars.by_ref() {
        out.push(c);
        window.push(c);
        if window.len() > tag.len() {
            window.remove(0);
        }
        if window == tag {
            return;
        }
    }
}

/// Find the statement surrounding `offset`, for "run the statement under the
/// cursor". Returns the trimmed statement, or None if the cursor sits in
/// whitespace between statements.
pub fn statement_at(script: &str, offset: usize) -> Option<String> {
    let mut cursor = 0usize;
    for stmt in split_statements(script) {
        // Locate this statement in the original text to map cursor positions.
        let start = script[cursor..].find(&stmt)? + cursor;
        let end = start + stmt.len();
        // `<= end` so a cursor resting just past the last character still counts.
        if offset <= end {
            return Some(stmt);
        }
        cursor = end;
    }
    None
}

// -- Browse-page composition ----------------------------------------------

use crate::model::{BrowseOptions, ColumnFilter, FilterOp, TableRef};

/// Compose the `SELECT` for a table page: filters, ordering, and paging.
///
/// Split out from the command so it can be tested without a live connection —
/// this is where an injection or a mis-ordered page would originate.
///
/// `known` is the table's real column list. Any sort or filter naming a column
/// outside it is dropped rather than errored: a stale filter left over from a
/// schema change should not block browsing.
pub fn build_browse_sql(
    table: &TableRef,
    options: &BrowseOptions,
    known: &[&str],
    dialect: &dyn crate::driver::Dialect,
    limit: u64,
) -> String {
    let mut sql = format!(
        "SELECT * FROM {}",
        dialect.qualify(table.schema.as_deref(), &table.name)
    );

    let where_clause = build_where(&options.filters, known, dialect);
    if !where_clause.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clause);
    }

    if let Some(col) = options.sort_column.as_deref().filter(|c| known.contains(c)) {
        sql.push_str(&format!(
            " ORDER BY {} {}",
            dialect.quote_ident(col),
            if options.sort_desc { "DESC" } else { "ASC" }
        ));
    }

    // Only wrap for paging when there is an offset. At offset 0 the driver's
    // own `limit + 1` probe already caps the result, and wrapping would nest a
    // redundant subquery.
    if options.offset > 0 {
        dialect.paginate(&sql, limit + 1, options.offset)
    } else {
        sql
    }
}

/// Build a WHERE clause from validated column filters.
///
/// Filters whose column is unknown are dropped rather than errored: a stale
/// filter left over from a schema change should not block browsing.
fn build_where(
    filters: &[ColumnFilter],
    known: &[&str],
    dialect: &dyn crate::driver::Dialect,
) -> String {
    filters
        .iter()
        .filter(|f| known.contains(&f.column.as_str()))
        .map(|f| {
            let col = dialect.quote_ident(&f.column);
            let lit = crate::model::Value::Text(f.value.clone());
            let quoted = dialect.literal(&lit);
            match f.op {
                FilterOp::Equals => format!("{col} = {quoted}"),
                FilterOp::NotEquals => format!("{col} <> {quoted}"),
                FilterOp::GreaterThan => format!("{col} > {quoted}"),
                FilterOp::LessThan => format!("{col} < {quoted}"),
                FilterOp::IsNull => format!("{col} IS NULL"),
                FilterOp::IsNotNull => format!("{col} IS NOT NULL"),
                FilterOp::Contains => {
                    let pat = dialect.literal(&crate::model::Value::Text(format!(
                        "%{}%",
                        escape_like(&f.value)
                    )));
                    format!("CAST({col} AS TEXT) LIKE {pat}")
                }
                FilterOp::StartsWith => {
                    let pat = dialect.literal(&crate::model::Value::Text(format!(
                        "{}%",
                        escape_like(&f.value)
                    )));
                    format!("CAST({col} AS TEXT) LIKE {pat}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Neutralize LIKE wildcards so a user searching for "50%" does not match
/// everything starting with "50".
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_semicolons() {
        let out = split_statements("SELECT 1; SELECT 2");
        assert_eq!(out, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn ignores_trailing_and_repeated_semicolons() {
        assert_eq!(split_statements("SELECT 1;;;").len(), 1);
        assert_eq!(split_statements("   ;  ").len(), 0);
    }

    #[test]
    fn keeps_semicolons_inside_string_literals() {
        let out = split_statements("SELECT 'a;b'; SELECT 2");
        assert_eq!(out, vec!["SELECT 'a;b'", "SELECT 2"]);
    }

    #[test]
    fn handles_doubled_quote_escapes() {
        let out = split_statements("SELECT 'O''Brien; Esq'; SELECT 2");
        assert_eq!(out, vec!["SELECT 'O''Brien; Esq'", "SELECT 2"]);
    }

    #[test]
    fn keeps_semicolons_inside_quoted_identifiers() {
        let out = split_statements(r#"SELECT "we;ird" FROM t; SELECT 2"#);
        assert_eq!(out, vec![r#"SELECT "we;ird" FROM t"#, "SELECT 2"]);
    }

    #[test]
    fn keeps_semicolons_inside_comments() {
        let out = split_statements("SELECT 1 -- a; comment\n; SELECT 2");
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("-- a; comment"));

        let out = split_statements("SELECT 1 /* a; b */; SELECT 2");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn respects_dollar_quoted_function_bodies() {
        // The classic failure: splitting a PL/pgSQL body at its internal ';'.
        let script = r#"
CREATE FUNCTION f() RETURNS int AS $$
BEGIN
  RAISE NOTICE 'hi';
  RETURN 1;
END;
$$ LANGUAGE plpgsql;
SELECT f()
"#;
        let out = split_statements(script);
        assert_eq!(out.len(), 2, "function body was split: {out:#?}");
        assert!(out[0].contains("RETURN 1;"));
        assert_eq!(out[1], "SELECT f()");
    }

    #[test]
    fn respects_tagged_dollar_quotes() {
        let script = "SELECT $tag$a;b$tag$; SELECT 2";
        let out = split_statements(script);
        assert_eq!(out, vec!["SELECT $tag$a;b$tag$", "SELECT 2"]);
    }

    #[test]
    fn bare_dollar_placeholders_are_not_quotes() {
        // `$1` is a Postgres bind placeholder, not the start of a dollar quote.
        let out = split_statements("SELECT * FROM t WHERE a = $1; SELECT 2");
        assert_eq!(out, vec!["SELECT * FROM t WHERE a = $1", "SELECT 2"]);
    }

    #[test]
    fn statement_at_finds_the_cursor_statement() {
        let script = "SELECT 1;\nSELECT 2;\nSELECT 3";
        assert_eq!(statement_at(script, 3).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at(script, 14).as_deref(), Some("SELECT 2"));
        assert_eq!(statement_at(script, 25).as_deref(), Some("SELECT 3"));
    }

    #[test]
    fn statement_at_end_of_script_returns_last() {
        let script = "SELECT 1";
        assert_eq!(statement_at(script, 8).as_deref(), Some("SELECT 1"));
    }

    // -- Browse composition ------------------------------------------------

    use crate::driver::dialect::{hex_bytes_x, paginate_limit_offset, quote_double, Dialect};
    use crate::model::{ColumnFilter, FilterOp, Value};

    struct TestDialect;
    impl Dialect for TestDialect {
        fn quote_ident(&self, i: &str) -> String {
            quote_double(i)
        }
        fn paginate(&self, sql: &str, l: u64, o: u64) -> String {
            paginate_limit_offset(sql, l, o)
        }
        fn quote_bytes(&self, b: &[u8]) -> String {
            hex_bytes_x(b)
        }
    }

    fn filter(column: &str, op: FilterOp, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.into(),
            op,
            value: value.into(),
        }
    }

    #[test]
    fn builds_conjunction_of_filters() {
        let known = ["a", "b"];
        let out = build_where(
            &[
                filter("a", FilterOp::Equals, "1"),
                filter("b", FilterOp::IsNull, ""),
            ],
            &known,
            &TestDialect,
        );
        assert_eq!(out, r#""a" = '1' AND "b" IS NULL"#);
    }

    #[test]
    fn drops_filters_on_unknown_columns() {
        // The column allowlist is the injection defence for interpolated names.
        let known = ["a"];
        let out = build_where(
            &[
                filter("a", FilterOp::Equals, "1"),
                filter("evil\" OR 1=1 --", FilterOp::Equals, "x"),
            ],
            &known,
            &TestDialect,
        );
        assert_eq!(out, r#""a" = '1'"#);
    }

    #[test]
    fn escapes_quotes_in_filter_values() {
        let known = ["a"];
        let out = build_where(
            &[filter("a", FilterOp::Equals, "x' OR '1'='1")],
            &known,
            &TestDialect,
        );
        assert_eq!(out, r#""a" = 'x'' OR ''1''=''1'"#);
    }

    #[test]
    fn escapes_like_wildcards_in_search_text() {
        let known = ["a"];
        let out = build_where(
            &[filter("a", FilterOp::Contains, "50%")],
            &known,
            &TestDialect,
        );
        assert!(out.contains(r"'%50\%%'"), "got {out}");
    }

    #[test]
    fn empty_filters_produce_no_clause() {
        assert_eq!(build_where(&[], &["a"], &TestDialect), "");
    }

    #[test]
    fn literal_delegates_to_dialect_byte_quoting() {
        assert_eq!(TestDialect.literal(&Value::Bytes(vec![0x01])), "X'01'");
    }

    fn opts() -> BrowseOptions {
        BrowseOptions {
            sort_column: None,
            sort_desc: false,
            filters: vec![],
            limit: None,
            offset: 0,
        }
    }

    fn table() -> TableRef {
        TableRef {
            schema: None,
            name: "books".into(),
        }
    }

    #[test]
    fn plain_browse_is_a_bare_select() {
        let sql = build_browse_sql(&table(), &opts(), &["id"], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books""#);
    }

    #[test]
    fn sorting_appends_an_order_by() {
        let mut o = opts();
        o.sort_column = Some("id".into());
        o.sort_desc = true;
        let sql = build_browse_sql(&table(), &o, &["id"], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books" ORDER BY "id" DESC"#);
    }

    #[test]
    fn sorting_on_an_unknown_column_is_dropped() {
        // The allowlist is what makes interpolating the name safe at all.
        let mut o = opts();
        o.sort_column = Some("id; DROP TABLE books".into());
        let sql = build_browse_sql(&table(), &o, &["id"], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books""#);
    }

    #[test]
    fn filters_and_sort_compose_in_sql_order() {
        let mut o = opts();
        o.filters = vec![filter("id", FilterOp::GreaterThan, "5")];
        o.sort_column = Some("id".into());
        let sql = build_browse_sql(&table(), &o, &["id"], &TestDialect, 1000);
        assert_eq!(
            sql,
            r#"SELECT * FROM "books" WHERE "id" > '5' ORDER BY "id" ASC"#
        );
    }

    #[test]
    fn offset_wraps_for_paging_and_keeps_the_order_by() {
        let mut o = opts();
        o.sort_column = Some("id".into());
        o.offset = 20;
        let sql = build_browse_sql(&table(), &o, &["id"], &TestDialect, 10);
        // The ORDER BY must stay inside the subquery, or the page is arbitrary.
        assert!(sql.contains(r#"ORDER BY "id" ASC"#), "{sql}");
        assert!(sql.contains("LIMIT 11 OFFSET 20"), "{sql}");
    }

    #[test]
    fn offset_zero_does_not_wrap() {
        // The driver's own limit+1 probe already caps it; wrapping would nest
        // a pointless subquery.
        let sql = build_browse_sql(&table(), &opts(), &["id"], &TestDialect, 1000);
        assert!(!sql.contains("faro_q"), "{sql}");
    }

    #[test]
    fn schema_qualifies_when_the_dialect_supports_it() {
        let t = TableRef {
            schema: Some("public".into()),
            name: "books".into(),
        };
        let sql = build_browse_sql(&t, &opts(), &["id"], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "public"."books""#);
    }
}
