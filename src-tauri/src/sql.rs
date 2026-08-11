//! SQL text utilities that are independent of any engine.

/// Split a script into individual statements.
///
/// Deliberately a lexer rather than a full parse: Faro must handle dialect
/// syntax no general-purpose SQL parser accepts (engine-specific DDL,
/// extensions), and a user's script should still run even when it cannot be
/// fully parsed. So this tracks only what is needed to know whether a `;` is a
/// real separator — string literals, quoted identifiers, comments, and
/// dollar-quoted bodies.
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
///
/// `offset` is a **UTF-16 code unit** index, because that is what the caller
/// has: CodeMirror — like every JavaScript string API — counts positions in
/// UTF-16. Rust slices by UTF-8 bytes, and the two agree only while the text is
/// pure ASCII. One accented character or emoji earlier in the script shifts
/// them apart, and the cursor then resolves to the wrong statement — which for
/// a script mixing `SELECT` and `DELETE` means running the wrong one.
pub fn statement_at(script: &str, offset_utf16: usize) -> Option<String> {
    let offset = utf16_to_byte_offset(script, offset_utf16);
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

/// Convert a UTF-16 code unit index into a UTF-8 byte index.
///
/// Clamps to the end of the string rather than failing: an out-of-range cursor
/// means "past the last statement", which the caller already handles.
fn utf16_to_byte_offset(s: &str, offset_utf16: usize) -> usize {
    let mut units = 0usize;
    for (byte_index, c) in s.char_indices() {
        if units >= offset_utf16 {
            return byte_index;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// The bare, upper-cased word tokens of a statement: everything outside string
/// literals, quoted identifiers, dollar-quoted bodies and comments.
///
/// Keyword *matching* must not look at quoted text. `SELECT * FROM t WHERE note
/// = 'delete everything'` contains the word DELETE but writes nothing, and a
/// column legitimately named `"insert"` is not a statement keyword. Anything
/// that decides what a statement *does* has to work from bare tokens only.
pub fn bare_words(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = sql.chars().peekable();

    // Flush whatever word has accumulated.
    macro_rules! flush {
        () => {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word).to_ascii_uppercase());
            }
        };
    }

    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                flush!();
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                flush!();
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            '\'' | '"' | '`' | '[' => {
                flush!();
                // `[ident]` is SQL Server's quoting and closes on `]`.
                let close = if c == '[' { ']' } else { c };
                while let Some(n) = chars.next() {
                    if n == close {
                        // A doubled delimiter is an escape, not the end.
                        if chars.peek() == Some(&close) {
                            chars.next();
                            continue;
                        }
                        break;
                    }
                    if n == '\\' {
                        chars.next();
                    }
                }
            }
            '$' => {
                flush!();
                if let Some(tag) = read_dollar_tag(&mut chars) {
                    let mut sink = String::new();
                    consume_until_tag(&mut chars, &tag, &mut sink);
                }
            }
            c if c.is_alphanumeric() || c == '_' => word.push(c),
            _ => flush!(),
        }
    }
    flush!();
    out
}

/// Statements that can never write, whatever follows the leading keyword.
const ALWAYS_READ_ONLY: [&str; 3] = ["SHOW", "DESCRIBE", "DESC"];

/// Bare tokens that mean a statement can modify data, schema, or session state.
///
/// `INTO` is here for T-SQL's `SELECT … INTO new_table`, which creates and
/// populates a table while looking like a read. Postgres spells the same thing
/// `SELECT … INTO`, and MySQL has `SELECT … INTO OUTFILE`. `SELECT … INTO @var`
/// is harmless, but refusing all of them is the right trade for an opt-in
/// safety setting: the cost is a rare rejected read, the alternative is a
/// silent write on a connection the user marked read-only.
const WRITE_TOKENS: [&str; 24] = [
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "TRUNCATE", "DROP", "CREATE",
    "ALTER", "RENAME", "GRANT", "REVOKE", "CALL", "EXEC", "EXECUTE", "INTO", "OUTFILE", "DUMPFILE",
    "ATTACH", "DETACH", "VACUUM", "REINDEX", "COPY",
];

/// Whether a statement is safe to run on a connection the user marked
/// read-only.
///
/// Distinct from [`crate::driver::returns_rows`], which answers a different
/// question — *does this produce a result set* — and is used to route a
/// statement to `query` or `execute`. Using that for the safety gate is what
/// let `SELECT … INTO t2 FROM t1` and `WITH c AS (…) DELETE FROM c` through:
/// both are writes whose leading keyword says "read".
///
/// This is a lexical guess and cannot be otherwise without a full per-dialect
/// parser, so it is a first line of defence. Every engine that can enforce
/// read-only itself is also told to; see each driver's `connect`.
pub fn is_read_only_statement(sql: &str) -> bool {
    let words = bare_words(sql);
    let Some(first) = words.first() else {
        // No bare tokens at all: a comment, or only a literal. Nothing to run.
        return true;
    };

    if ALWAYS_READ_ONLY.contains(&first.as_str()) {
        return true;
    }

    if !crate::driver::returns_rows(sql) {
        return false;
    }

    // `PRAGMA x = y` writes; `PRAGMA x` reads.
    if first == "PRAGMA" {
        return !sql.contains('=');
    }

    !words.iter().any(|w| WRITE_TOKENS.contains(&w.as_str()))
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
        dialect.paginate(&sql, crate::driver::probe_limit(limit), options.offset)
    } else {
        sql
    }
}

/// A stable `ORDER BY` for paging a table, ready to append to a `SELECT`.
///
/// Anything that walks a table in `LIMIT`/`OFFSET` pages needs this. Without an
/// ordering, a database is free to return rows in a different order for each
/// page, which both duplicates and skips rows across a page boundary — so a
/// paged read of a large table silently produces a wrong copy of it. For a
/// backup that means a dump that is quietly not the table.
///
/// The primary key is the cheap answer. Failing that, ordering by every
/// *sortable* column also gives a total order — it costs a sort, but a slow
/// correct backup beats a fast wrong one. Columns whose type has no ordering
/// operator are left out (see [`is_orderable`]); if that leaves nothing, there
/// is no ordering to impose and paging such a table stays best-effort.
pub fn stable_order_by(
    primary_key: &[String],
    columns: &[crate::model::ColumnDetail],
    dialect: &dyn crate::driver::Dialect,
) -> String {
    let keys: Vec<String> = if !primary_key.is_empty() {
        primary_key.iter().map(|k| dialect.quote_ident(k)).collect()
    } else {
        columns
            .iter()
            .filter(|c| is_orderable(&c.type_name))
            .map(|c| dialect.quote_ident(&c.name))
            .collect()
    };

    if keys.is_empty() {
        return String::new();
    }
    format!(" ORDER BY {}", keys.join(", "))
}

/// Whether a column type can appear in `ORDER BY` on every engine Faro speaks.
///
/// Conservative by design: this only decides whether a column *helps* make
/// paging deterministic, so wrongly excluding one costs a little determinism,
/// while wrongly including one makes the query fail outright. Postgres has no
/// default ordering for `json`, `xml` or the geometric types; T-SQL refuses the
/// legacy LOB types in `ORDER BY`.
fn is_orderable(type_name: &str) -> bool {
    let t = type_name.to_ascii_lowercase();
    const UNORDERABLE: [&str; 10] = [
        "json",
        "xml",
        "point",
        "polygon",
        "line",
        "circle",
        "geometry",
        "geography",
        "image",
        "hstore",
    ];
    // `ntext`/`text` are LOBs in T-SQL but ordinary sortable strings in
    // Postgres and SQLite, where `text` is the everyday string type. Excluding
    // them everywhere would drop the most useful ordering column on those
    // engines, so only the exact T-SQL LOB spellings are refused.
    if t == "ntext" {
        return false;
    }
    !UNORDERABLE.iter().any(|u| t.contains(u))
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
                    like_clause(&col, &format!("%{}%", escape_like(&f.value)), dialect)
                }
                FilterOp::StartsWith => {
                    like_clause(&col, &format!("{}%", escape_like(&f.value)), dialect)
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// A `CAST(... AS text) LIKE ... ESCAPE ...` comparison, built through the
/// dialect so both the pattern and the escape character are quoted correctly.
///
/// The `ESCAPE` clause has to go through `quote_string` like any other literal.
/// Writing `ESCAPE '\'` directly — as this did — produces an *unterminated*
/// literal on MySQL and ClickHouse, where the backslash escapes the closing
/// quote. And the cast target is not portable: MySQL rejects `TEXT` outright.
///
/// The clause itself is omitted where the dialect has no `ESCAPE` at all; see
/// [`crate::driver::Dialect::supports_like_escape`].
fn like_clause(col: &str, pattern: &str, dialect: &dyn crate::driver::Dialect) -> String {
    let pat = dialect.quote_string(pattern);
    let text = dialect.text_cast_type();
    let cmp = format!("CAST({col} AS {text}) LIKE {pat}");
    if dialect.supports_like_escape() {
        let esc = dialect.quote_string(&LIKE_ESCAPE.to_string());
        format!("{cmp} ESCAPE {esc}")
    } else {
        cmp
    }
}

/// Escape character used with `LIKE`.
///
/// Named rather than inlined because it has to appear identically in the escaped
/// pattern and in the `ESCAPE` clause; the two drifting apart would break every
/// text filter at once.
const LIKE_ESCAPE: char = '\\';

/// Neutralize LIKE wildcards so a user searching for "50%" does not match
/// everything starting with "50".
///
/// Must be paired with an explicit `ESCAPE` clause. Postgres and MySQL happen to
/// treat backslash as the default escape, but SQLite, DuckDB and SQL Server have
/// **no** default — there, an unaccompanied `\%` matches a literal backslash
/// followed by anything, so the filter silently returns the wrong rows.
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

    #[test]
    fn statement_at_counts_utf16_like_the_editor_does() {
        // The offset arrives from CodeMirror, which counts UTF-16 code units.
        // 'é' is 2 UTF-8 bytes but 1 UTF-16 unit; '𝒮' is 4 bytes and 2 units.
        // Reading the offset as bytes lands short and picks the statement
        // before the cursor.
        let script = "SELECT 'é𝒮é';\nDELETE FROM t";

        // In UTF-16: S-E-L-E-C-T(6) space(7) '(8) é(9) 𝒮(11) é(12) '(13) ;(14)
        // so the DELETE begins at unit 15.
        let utf16_len: usize = script.chars().map(|c| c.len_utf16()).sum();
        assert!(script.len() > utf16_len, "fixture must be non-ASCII");

        // A cursor inside the first statement resolves to it.
        assert_eq!(
            statement_at(script, 4).as_deref(),
            Some("SELECT 'é𝒮é'"),
            "cursor in the first statement"
        );
        // A cursor inside the DELETE must resolve to the DELETE, not the SELECT.
        assert_eq!(
            statement_at(script, utf16_len - 1).as_deref(),
            Some("DELETE FROM t"),
            "cursor in the second statement"
        );
        // And at the very end.
        assert_eq!(
            statement_at(script, utf16_len).as_deref(),
            Some("DELETE FROM t")
        );
    }

    #[test]
    fn utf16_offsets_map_to_byte_offsets() {
        assert_eq!(utf16_to_byte_offset("abc", 0), 0);
        assert_eq!(utf16_to_byte_offset("abc", 2), 2);
        // 'é' = 2 bytes, 1 unit.
        assert_eq!(utf16_to_byte_offset("éb", 1), 2);
        // '𝒮' = 4 bytes, 2 units (a surrogate pair).
        assert_eq!(utf16_to_byte_offset("𝒮b", 2), 4);
        // Past the end clamps rather than panicking.
        assert_eq!(utf16_to_byte_offset("ab", 99), 2);
    }

    // -- Stable paging order -----------------------------------------------

    fn col(name: &str, type_name: &str) -> crate::model::ColumnDetail {
        crate::model::ColumnDetail {
            name: name.into(),
            type_name: type_name.into(),
            nullable: true,
            default: None,
            is_primary_key: false,
            ordinal: 0,
        }
    }

    #[test]
    fn the_primary_key_is_the_paging_order_when_there_is_one() {
        let cols = [col("id", "int4"), col("name", "text")];
        assert_eq!(
            stable_order_by(&["id".into()], &cols, &TestDialect),
            r#" ORDER BY "id""#
        );
    }

    #[test]
    fn a_composite_key_orders_by_every_part() {
        let cols = [col("a", "int4"), col("b", "int4")];
        assert_eq!(
            stable_order_by(&["a".into(), "b".into()], &cols, &TestDialect),
            r#" ORDER BY "a", "b""#
        );
    }

    #[test]
    fn a_keyless_table_orders_by_its_sortable_columns() {
        // Without this a paged backup of a keyless table silently duplicates
        // and drops rows across page boundaries.
        let cols = [col("a", "int4"), col("b", "text")];
        assert_eq!(
            stable_order_by(&[], &cols, &TestDialect),
            r#" ORDER BY "a", "b""#
        );
    }

    #[test]
    fn unorderable_column_types_are_left_out_of_the_order() {
        // Postgres has no default ordering for json/xml/geometric types, so
        // including them would make the query fail rather than sort.
        let cols = [
            col("a", "int4"),
            col("doc", "json"),
            col("meta", "jsonb"),
            col("x", "xml"),
            col("p", "point"),
        ];
        assert_eq!(
            stable_order_by(&[], &cols, &TestDialect),
            r#" ORDER BY "a""#
        );
    }

    #[test]
    fn a_table_with_nothing_sortable_pages_unordered() {
        // No total order is available; better than emitting SQL that errors.
        let cols = [col("doc", "jsonb")];
        assert_eq!(stable_order_by(&[], &cols, &TestDialect), "");
        assert_eq!(stable_order_by(&[], &[], &TestDialect), "");
    }

    #[test]
    fn ordinary_text_columns_stay_orderable() {
        // `text` is the everyday string type on Postgres and SQLite; only
        // T-SQL's `ntext` LOB is refused.
        assert!(is_orderable("text"));
        assert!(is_orderable("varchar(200)"));
        assert!(is_orderable("TIMESTAMP"));
        assert!(!is_orderable("ntext"));
        assert!(!is_orderable("JSONB"));
    }

    // -- Read-only classification ------------------------------------------

    #[test]
    fn bare_words_ignores_quoted_and_commented_text() {
        assert_eq!(
            bare_words("SELECT * FROM t WHERE note = 'delete everything'"),
            ["SELECT", "FROM", "T", "WHERE", "NOTE"]
        );
        assert_eq!(
            bare_words(r#"SELECT "insert" FROM t"#),
            ["SELECT", "FROM", "T"]
        );
        assert_eq!(bare_words("SELECT 1 -- DROP TABLE t"), ["SELECT", "1"]);
        assert_eq!(bare_words("SELECT /* DELETE */ 1"), ["SELECT", "1"]);
        assert_eq!(bare_words("SELECT [drop] FROM t"), ["SELECT", "FROM", "T"]);
        assert_eq!(
            bare_words("SELECT `update` FROM t"),
            ["SELECT", "FROM", "T"]
        );
    }

    #[test]
    fn plain_reads_are_allowed() {
        for sql in [
            "SELECT * FROM t",
            "  select 1",
            "WITH c AS (SELECT 1) SELECT * FROM c",
            "EXPLAIN SELECT * FROM t",
            "SHOW TABLES",
            "SHOW CREATE TABLE t",
            "DESCRIBE t",
            "PRAGMA table_info(t)",
            "VALUES (1)",
            "SELECT * FROM t WHERE note = 'please delete me'",
            "SELECT insert_date, update_count FROM t",
        ] {
            assert!(is_read_only_statement(sql), "should be allowed: {sql}");
        }
    }

    #[test]
    fn writes_disguised_as_reads_are_refused() {
        for sql in [
            // The two holes the leading-keyword check let through.
            "SELECT * INTO t2 FROM t1",
            "WITH c AS (SELECT id FROM t) DELETE FROM t WHERE id IN (SELECT id FROM c)",
            // And the rest of the family.
            "WITH c AS (SELECT 1) INSERT INTO t SELECT * FROM c",
            "WITH c AS (SELECT 1) UPDATE t SET a = 1",
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            "EXPLAIN ANALYZE INSERT INTO t VALUES (1)",
            "PRAGMA journal_mode = WAL",
            "SELECT * FROM t; DROP TABLE t",
        ] {
            assert!(!is_read_only_statement(sql), "should be refused: {sql}");
        }
    }

    #[test]
    fn ordinary_writes_are_still_refused() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "DROP TABLE t",
            "CREATE TABLE t (a int)",
            "TRUNCATE t",
            "GRANT ALL ON t TO u",
            "EXEC sp_who",
        ] {
            assert!(!is_read_only_statement(sql), "should be refused: {sql}");
        }
    }

    #[test]
    fn a_comment_only_statement_is_harmless() {
        assert!(is_read_only_statement("-- just a note"));
        assert!(is_read_only_statement(""));
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

    /// Stands in for MySQL, MariaDB and ClickHouse: the engines that treat `\`
    /// as an escape inside a string literal.
    struct BackslashDialect;
    impl Dialect for BackslashDialect {
        fn quote_ident(&self, i: &str) -> String {
            crate::driver::dialect::quote_backtick(i)
        }
        fn paginate(&self, sql: &str, l: u64, o: u64) -> String {
            paginate_limit_offset(sql, l, o)
        }
        fn quote_bytes(&self, b: &[u8]) -> String {
            hex_bytes_x(b)
        }
        fn quote_string(&self, s: &str) -> String {
            crate::model::quote_sql_string_backslash(s)
        }
        fn text_cast_type(&self) -> &'static str {
            "CHAR"
        }
    }

    /// Stands in for ClickHouse: backslash escaping *and* no `ESCAPE` clause.
    struct NoEscapeDialect;
    impl Dialect for NoEscapeDialect {
        fn quote_ident(&self, i: &str) -> String {
            crate::driver::dialect::quote_backtick(i)
        }
        fn paginate(&self, sql: &str, l: u64, o: u64) -> String {
            paginate_limit_offset(sql, l, o)
        }
        fn quote_bytes(&self, b: &[u8]) -> String {
            hex_bytes_x(b)
        }
        fn quote_string(&self, s: &str) -> String {
            crate::model::quote_sql_string_backslash(s)
        }
        fn supports_like_escape(&self) -> bool {
            false
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
    fn like_filters_declare_their_escape_character() {
        // Escaping the wildcard is only half the job: SQLite, DuckDB and SQL
        // Server have no default LIKE escape, so without this clause `\%`
        // matches a literal backslash and the filter returns the wrong rows.
        let known = ["a"];
        for op in [FilterOp::Contains, FilterOp::StartsWith] {
            let out = build_where(&[filter("a", op, "50%")], &known, &TestDialect);
            assert!(out.ends_with(r"ESCAPE '\'"), "{op:?} produced {out}");
        }
    }

    #[test]
    fn non_like_filters_have_no_escape_clause() {
        // ESCAPE is only legal on LIKE; appending it to `=` would be a syntax
        // error on every engine.
        let known = ["a"];
        for op in [
            FilterOp::Equals,
            FilterOp::NotEquals,
            FilterOp::GreaterThan,
            FilterOp::LessThan,
            FilterOp::IsNull,
            FilterOp::IsNotNull,
        ] {
            let out = build_where(&[filter("a", op, "x")], &known, &TestDialect);
            assert!(!out.contains("ESCAPE"), "{op:?} produced {out}");
        }
    }

    #[test]
    fn escaped_pattern_and_escape_clause_agree() {
        // A backslash in the search text must be doubled in the pattern and the
        // clause must name that same character, or the two disagree and the
        // engine reads the pattern differently than intended.
        let known = ["a"];
        let out = build_where(
            &[filter("a", FilterOp::Contains, r"c:\tmp")],
            &known,
            &TestDialect,
        );
        assert!(out.contains(r"'%c:\\tmp%'"), "got {out}");
        assert!(out.ends_with(r"ESCAPE '\'"), "got {out}");
    }

    #[test]
    fn empty_filters_produce_no_clause() {
        assert_eq!(build_where(&[], &["a"], &TestDialect), "");
    }

    // -- Backslash escaping (the MySQL/MariaDB/ClickHouse injection) --------

    #[test]
    fn a_trailing_backslash_cannot_escape_its_closing_quote() {
        // The injection primitive. With only `'` doubled, filter one's value
        // of `\` swallows the `' AND ` that follows and filter two's value
        // becomes bare SQL.
        let known = ["a", "b"];
        let out = build_where(
            &[
                filter("a", FilterOp::Equals, r"\"),
                filter("b", FilterOp::Equals, "x' OR 1=1 -- "),
            ],
            &known,
            &BackslashDialect,
        );
        assert_eq!(
            out, r"`a` = '\\' AND `b` = 'x'' OR 1=1 -- '",
            "backslash left unescaped: {out}"
        );
    }

    #[test]
    fn standard_dialects_do_not_double_backslashes() {
        // The mirror image: doubling on Postgres/SQLite would corrupt the value.
        let out = build_where(
            &[filter("a", FilterOp::Equals, r"C:\tmp")],
            &["a"],
            &TestDialect,
        );
        assert_eq!(out, r#""a" = 'C:\tmp'"#);
    }

    #[test]
    fn like_escape_clause_is_quoted_for_the_dialect() {
        // `ESCAPE '\'` is an *unterminated* literal on MySQL and ClickHouse,
        // so the clause has to be quoted like any other string.
        let out = build_where(
            &[filter("a", FilterOp::Contains, "50%")],
            &["a"],
            &BackslashDialect,
        );
        assert!(out.ends_with(r"ESCAPE '\\'"), "got {out}");
        assert!(out.contains(r"'%50\\%%'"), "got {out}");
    }

    #[test]
    fn a_dialect_without_escape_still_escapes_the_pattern() {
        // ClickHouse rejects `ESCAPE` as a syntax error, so the clause is
        // dropped there — but the wildcard must still be neutralized, since
        // ClickHouse reads backslash as the escape character regardless.
        for op in [FilterOp::Contains, FilterOp::StartsWith] {
            let out = build_where(&[filter("a", op, "50%")], &["a"], &NoEscapeDialect);
            assert!(!out.contains("ESCAPE"), "{op:?} produced {out}");
            assert!(out.contains(r"50\\%"), "{op:?} produced {out}");
        }
    }

    #[test]
    fn text_cast_target_follows_the_dialect() {
        // MySQL rejects CAST(x AS TEXT); it needs CHAR.
        let mysqlish = build_where(
            &[filter("a", FilterOp::Contains, "x")],
            &["a"],
            &BackslashDialect,
        );
        assert!(mysqlish.starts_with("CAST(`a` AS CHAR) LIKE"), "{mysqlish}");

        let standard = build_where(
            &[filter("a", FilterOp::Contains, "x")],
            &["a"],
            &TestDialect,
        );
        assert!(
            standard.starts_with(r#"CAST("a" AS TEXT) LIKE"#),
            "{standard}"
        );
    }

    #[test]
    fn every_filter_op_emits_a_balanced_literal_on_backslash_dialects() {
        // Nothing a user can type into a filter box may leave an odd number of
        // unescaped delimiters behind.
        let hostile = [
            r"\",
            r"\\",
            r"'",
            r"\'",
            r"x\' OR 1=1 -- ",
            r"'; DROP TABLE t; --",
        ];
        let ops = [
            FilterOp::Equals,
            FilterOp::NotEquals,
            FilterOp::GreaterThan,
            FilterOp::LessThan,
            FilterOp::Contains,
            FilterOp::StartsWith,
        ];
        for value in hostile {
            for op in ops {
                let out = build_where(&[filter("a", op, value)], &["a"], &BackslashDialect);
                let body: String = out.chars().collect();
                // Walk the clause: outside a literal, quotes must alternate;
                // a backslash must always be followed by another backslash.
                let mut chars = body.chars().peekable();
                let mut in_literal = false;
                while let Some(c) = chars.next() {
                    match c {
                        '\\' if in_literal => assert_eq!(
                            chars.next(),
                            Some('\\'),
                            "lone backslash for {op:?}/{value:?} in {out}"
                        ),
                        '\'' if in_literal && chars.peek() == Some(&'\'') => {
                            chars.next();
                        }
                        '\'' => in_literal = !in_literal,
                        _ => {}
                    }
                }
                assert!(
                    !in_literal,
                    "unterminated literal for {op:?}/{value:?}: {out}"
                );
            }
        }
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
