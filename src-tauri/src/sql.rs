//! SQL text utilities that are independent of any engine.

/// How one engine delimits strings and identifiers.
///
/// The read-only guard is a lexical test, so it has to tokenize the way the
/// engine actually will. These rules genuinely differ, and the differences are
/// not cosmetic: `"` opens an identifier on Postgres and a *string* on MySQL,
/// `\` escapes inside a literal on MySQL and ClickHouse and nowhere else, and
/// `[bracketed]` identifiers are T-SQL's alone.
///
/// Applying one engine's rules to another is what let a crafted identifier hide
/// a write keyword from [`is_read_only_statement`]: the lexer treated `\` as an
/// escape inside `"` on every engine, so on Postgres or SQL Server — where it is
/// an ordinary character — the engine closed the identifier at a quote the lexer
/// had already swallowed, and the two disagreed about where the statement ended.
/// See `a_quoting_rule_from_another_dialect_cannot_hide_a_write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexRules {
    /// `\` escapes the next character inside a quoted region.
    pub backslash_escapes: bool,
    /// `"` opens a string literal rather than a quoted identifier.
    pub double_quote_is_string: bool,
    /// `` ` `` opens a quoted identifier.
    pub backtick_idents: bool,
    /// `[` opens a quoted identifier, closed by `]`.
    pub bracket_idents: bool,
}

impl LexRules {
    /// SQL-standard quoting: `'` for strings, `"` for identifiers, a doubled
    /// delimiter to escape either, and no special meaning for `\`.
    /// Postgres, SQLite, DuckDB, CockroachDB and Redshift.
    pub const STANDARD: Self = Self {
        backslash_escapes: false,
        double_quote_is_string: false,
        backtick_idents: false,
        bracket_idents: false,
    };

    /// MySQL and MariaDB: `\` escapes inside a literal, `"` is a second string
    /// delimiter (absent `ANSI_QUOTES`), and identifiers take backticks.
    pub const MYSQL: Self = Self {
        backslash_escapes: true,
        double_quote_is_string: true,
        backtick_idents: true,
        bracket_idents: false,
    };

    /// T-SQL: standard quoting plus `[bracketed]` identifiers, where a literal
    /// `]` is escaped by doubling.
    pub const TSQL: Self = Self {
        backslash_escapes: false,
        double_quote_is_string: false,
        backtick_idents: false,
        bracket_idents: true,
    };

    /// ClickHouse: standard identifier quoting, but `\` escapes inside a
    /// literal and backticks are accepted for identifiers.
    pub const CLICKHOUSE: Self = Self {
        backslash_escapes: true,
        double_quote_is_string: false,
        backtick_idents: true,
        bracket_idents: false,
    };

    /// Every delimiter every supported engine uses, all at once.
    ///
    /// For the paths that genuinely do not know the engine — splitting a `.sql`
    /// dump chosen from the file picker before any connection is in play. It is
    /// the right default *for splitting*, where recognising more quoting than
    /// the engine does is harmless, but it must never be the only lexing behind
    /// a safety decision: recognising a delimiter the engine does not means
    /// skipping text the engine will execute. [`is_read_only_statement`]
    /// therefore checks this *and* [`STANDARD`](Self::STANDARD), and refuses if
    /// either reading finds a write.
    pub const PERMISSIVE: Self = Self {
        backslash_escapes: true,
        double_quote_is_string: true,
        backtick_idents: true,
        bracket_idents: true,
    };

    /// The closing delimiter for a region opened by `c`, and whether `\`
    /// escapes inside it. `None` when `c` does not open one.
    fn opens(&self, c: char) -> Option<(char, bool)> {
        match c {
            '\'' => Some(('\'', self.backslash_escapes)),
            // Only a *string* honours backslash escapes. A quoted identifier
            // escapes its delimiter by doubling on every engine here.
            '"' => Some(('"', self.backslash_escapes && self.double_quote_is_string)),
            '`' if self.backtick_idents => Some(('`', false)),
            '[' if self.bracket_idents => Some((']', false)),
            _ => None,
        }
    }
}

/// Consume a quoted region, the opening delimiter having already been read.
///
/// `out` receives the consumed text when the caller is rebuilding the original
/// string (statement splitting); keyword scanning passes `None` and discards it.
fn consume_quoted(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    close: char,
    backslash_escapes: bool,
    mut out: Option<&mut String>,
) {
    macro_rules! emit {
        ($c:expr) => {
            if let Some(o) = out.as_deref_mut() {
                o.push($c);
            }
        };
    }

    while let Some(n) = chars.next() {
        emit!(n);
        if backslash_escapes && n == '\\' {
            if let Some(esc) = chars.next() {
                emit!(esc);
            }
            continue;
        }
        if n == close {
            // A doubled delimiter is an escape, not the terminator.
            if chars.peek() == Some(&close) {
                let doubled = chars.next().unwrap_or(close);
                emit!(doubled);
                continue;
            }
            return;
        }
    }
}

/// Split a script into individual statements.
///
/// Deliberately a lexer rather than a full parse: Faro must handle dialect
/// syntax no general-purpose SQL parser accepts (engine-specific DDL,
/// extensions), and a user's script should still run even when it cannot be
/// fully parsed. So this tracks only what is needed to know whether a `;` is a
/// real separator — string literals, quoted identifiers, comments, and
/// dollar-quoted bodies.
pub fn split_statements(script: &str) -> Vec<String> {
    split_statements_with(script, LexRules::PERMISSIVE)
}

/// Split a script into statements using one engine's quoting rules.
///
/// Recognising a delimiter the engine does not is harmless here — it can only
/// merge statements that should have been split, and the merged text is then
/// handed to the engine as written. Failing to recognise one is what would be
/// dangerous, which is why the no-dialect entry point above is
/// [`LexRules::PERMISSIVE`].
pub fn split_statements_with(script: &str, rules: LexRules) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = script.chars().peekable();

    while let Some(c) = chars.next() {
        // Line comment: consume to end of line.
        if c == '-' && chars.peek() == Some(&'-') {
            current.push(c);
            for n in chars.by_ref() {
                current.push(n);
                if n == '\n' {
                    break;
                }
            }
            continue;
        }

        // Block comment: consume to the closing */.
        if c == '/' && chars.peek() == Some(&'*') {
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
            continue;
        }

        // Quoted regions, delimited the way this engine delimits them.
        if let Some((close, backslash)) = rules.opens(c) {
            current.push(c);
            consume_quoted(&mut chars, close, backslash, Some(&mut current));
            continue;
        }

        // Postgres dollar quoting: $$ ... $$ or $tag$ ... $tag$. Function
        // bodies routinely contain semicolons, so this must be respected.
        if c == '$' {
            match read_dollar_tag(&mut chars) {
                Some(tag) => {
                    current.push_str(&tag);
                    consume_until_tag(&mut chars, &tag, &mut current);
                }
                None => current.push(c),
            }
            continue;
        }

        if c == ';' {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
            continue;
        }

        current.push(c);
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

/// Whether a fragment holds nothing but comments and whitespace.
///
/// Splitting keeps comments with the statement they precede, so a script that
/// ends in a comment — `SELECT 1;\n-- done` — yields a trailing fragment with
/// no SQL in it. Sent to an engine that is an error on some (MySQL: "Query was
/// empty") and a pointless round trip on the rest, so callers that execute
/// statements drop these first.
pub fn is_comment_only(sql: &str) -> bool {
    leading_trivia_len(sql) == sql.len()
}

/// Byte length of the comments and whitespace in front of a fragment's first
/// piece of SQL — the whole length when there is none.
fn leading_trivia_len(sql: &str) -> usize {
    let mut chars = sql.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        if c == '-' && next == Some('-') {
            for (_, n) in chars.by_ref() {
                if n == '\n' {
                    break;
                }
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            chars.next();
            let mut prev = '\0';
            for (_, n) in chars.by_ref() {
                if prev == '*' && n == '/' {
                    break;
                }
                prev = n;
            }
            continue;
        }
        if !c.is_whitespace() {
            return i;
        }
    }
    sql.len()
}

/// The statements of a script that actually carry SQL, with any comment-only
/// fragment removed. See [`is_comment_only`].
pub fn executable_statements(script: &str, rules: LexRules) -> Vec<String> {
    split_statements_with(script, rules)
        .into_iter()
        .filter(|s| !is_comment_only(s))
        .collect()
}

/// Find the statement surrounding `offset`, for "run the statement under the
/// cursor". Returns the trimmed statement, or None if there is none to run.
///
/// A cursor between two statements belongs to the *earlier* one while it is
/// still on that statement's line — resting just after its `;`, or in a
/// trailing comment — which is where a cursor ends up after typing a
/// statement. Once a line break separates them, it belongs to the next one.
/// Resolving the first case to the next statement is how "run this statement"
/// on `SELECT 1;⎸` ran the `DELETE` on the line below.
///
/// `offset` is a **UTF-16 code unit** index, because that is what the caller
/// has: CodeMirror — like every JavaScript string API — counts positions in
/// UTF-16. Rust slices by UTF-8 bytes, and the two agree only while the text is
/// pure ASCII. One accented character or emoji earlier in the script shifts
/// them apart, and the cursor then resolves to the wrong statement — which for
/// a script mixing `SELECT` and `DELETE` means running the wrong one.
pub fn statement_at(script: &str, offset_utf16: usize) -> Option<String> {
    let offset = utf16_to_byte_offset(script, offset_utf16);
    // Whether the cursor is still on the line where `end` sits.
    let same_line = |end: usize| !script[end..offset].contains('\n');

    let mut cursor = 0usize;
    let mut previous: Option<(String, usize)> = None;
    for stmt in executable_statements(script, LexRules::PERMISSIVE) {
        // Locate this statement in the original text to map cursor positions.
        let start = script[cursor..].find(&stmt)? + cursor;
        let end = start + stmt.len();
        // Where its SQL begins. Splitting attaches a comment to the statement
        // that follows it, so `SELECT 1; -- note` puts the note at the head of
        // the *next* statement's text even though it sits on this one's line.
        let code_start = start + leading_trivia_len(&stmt);
        // `<= end` so a cursor resting just past the last character still counts.
        if offset <= end {
            if let Some((prev, prev_end)) = previous {
                if offset < code_start && same_line(prev_end) {
                    return Some(prev);
                }
            }
            return Some(stmt);
        }
        cursor = end;
        previous = Some((stmt, end));
    }

    // Past the last statement: still its statement while on its line.
    previous
        .filter(|(_, end)| same_line(*end))
        .map(|(stmt, _)| stmt)
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
    bare_words_with(sql, LexRules::PERMISSIVE)
}

/// The bare word tokens of a statement under one engine's quoting rules.
///
/// Unlike splitting, getting this wrong in the permissive direction is exactly
/// what is dangerous: every delimiter recognised here is text *not* scanned for
/// keywords, so crediting the engine with quoting it does not have hides
/// whatever follows. Anything deciding whether a statement may run must pass
/// the engine's real rules, or check every plausible set — see
/// [`is_read_only_statement`].
pub fn bare_words_with(sql: &str, rules: LexRules) -> Vec<String> {
    bare_words_up_to(sql, rules, usize::MAX)
}

/// The statement's first bare word, upper-cased — its leading keyword.
///
/// Skips comments, quoted text and punctuation the same way [`bare_words`]
/// does, so `-- note\nSELECT …` and `/* hint */ (SELECT …)` both lead with
/// `SELECT`. Stops at the first word rather than tokenizing the whole
/// statement, since callers ask this of every statement they run.
pub fn leading_word(sql: &str) -> Option<String> {
    bare_words_up_to(sql, LexRules::PERMISSIVE, 1).pop()
}

/// [`bare_words_with`], stopping once `max` words have been collected.
fn bare_words_up_to(sql: &str, rules: LexRules, max: usize) -> Vec<String> {
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
        if out.len() >= max {
            break;
        }
        if c == '-' && chars.peek() == Some(&'-') {
            flush!();
            for n in chars.by_ref() {
                if n == '\n' {
                    break;
                }
            }
            continue;
        }

        if c == '/' && chars.peek() == Some(&'*') {
            flush!();
            chars.next();
            let mut prev = '\0';
            for n in chars.by_ref() {
                if prev == '*' && n == '/' {
                    break;
                }
                prev = n;
            }
            continue;
        }

        if let Some((close, backslash)) = rules.opens(c) {
            flush!();
            consume_quoted(&mut chars, close, backslash, None);
            continue;
        }

        if c == '$' {
            flush!();
            if let Some(tag) = read_dollar_tag(&mut chars) {
                let mut sink = String::new();
                consume_until_tag(&mut chars, &tag, &mut sink);
            }
            continue;
        }

        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }

        flush!();
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
    // No engine is named here, and no single rule set is safe without one.
    // Every delimiter a lexer credits the engine with is text it stops scanning
    // for keywords, so guessing generously is exactly how a write hides. Both
    // readings have to agree the statement is a read.
    //
    // The cost is a rare false refusal — a T-SQL `SELECT [drop] FROM t`, or a
    // MySQL string literal containing the word DELETE — which is the same trade
    // `WRITE_TOKENS` already makes for `INTO`. Callers that know the engine
    // should pass its rules to [`is_read_only_statement_with`] instead and pay
    // nothing.
    is_read_only_statement_with(sql, LexRules::STANDARD)
        && is_read_only_statement_with(sql, LexRules::PERMISSIVE)
}

/// How many of `statements` can write, judged with `rules` when the engine is
/// known and with every plausible reading when it is not.
///
/// Uses the same classifier as the read-only guard, so "this script changes
/// data" means exactly what a read-only connection would refuse.
pub fn count_writes(statements: &[String], rules: Option<LexRules>) -> usize {
    statements
        .iter()
        .filter(|s| match rules {
            Some(rules) => !is_read_only_statement_with(s, rules),
            None => !is_read_only_statement(s),
        })
        .count()
}

/// Whether a statement is safe to run read-only, judged with one engine's
/// quoting rules.
pub fn is_read_only_statement_with(sql: &str, rules: LexRules) -> bool {
    let words = bare_words_with(sql, rules);
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

    if first == "PRAGMA" {
        return pragma_reads(sql);
    }

    !words
        .iter()
        .any(|w| WRITE_TOKENS.contains(&w.as_str()) || SIDE_EFFECT_FUNCTIONS.contains(&w.as_str()))
}

/// Functions that change state while being called from what looks like a read.
///
/// `SELECT set_config('default_transaction_read_only', 'off', false)` has no
/// write keyword in it, so the token test waved it through — and it switches
/// off the server-side read-only guard Postgres was given at connect, after
/// which a `SELECT` of any function that writes succeeds. The rest are the
/// same shape: `nextval`/`setval` advance sequences, `dblink`/`dblink_exec`
/// run arbitrary SQL on another server, the `lo_*` family creates, writes and
/// deletes large objects (and `lo_import`/`lo_export` touch the server's
/// filesystem), and the `pg_*` administration functions kill sessions or
/// reconfigure the server.
///
/// Matched as bare words anywhere in the statement, so a column that happens
/// to share one of these names is refused too — the same trade `WRITE_TOKENS`
/// makes for `INTO`: a rare rejected read on an opt-in safety setting.
const SIDE_EFFECT_FUNCTIONS: [&str; 16] = [
    "SET_CONFIG",
    "NEXTVAL",
    "SETVAL",
    "DBLINK",
    "DBLINK_EXEC",
    "LO_IMPORT",
    "LO_EXPORT",
    "LO_UNLINK",
    "LO_CREATE",
    "LO_FROM_BYTEA",
    "LO_PUT",
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "PG_RELOAD_CONF",
    "PG_ROTATE_LOGFILE",
    "PG_SWITCH_WAL",
];

/// SQLite pragmas that only ever report; their call form is a query.
///
/// Needed because `PRAGMA name(value)` is a third spelling of `PRAGMA name =
/// value` — SQLite accepts it for settings as well as for the introspection
/// pragmas — so the shape of the statement cannot tell the two apart and the
/// name has to.
const READ_ONLY_PRAGMAS: [&str; 16] = [
    "TABLE_INFO",
    "TABLE_XINFO",
    "TABLE_LIST",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "FOREIGN_KEY_CHECK",
    "DATABASE_LIST",
    "COLLATION_LIST",
    "COMPILE_OPTIONS",
    "FUNCTION_LIST",
    "MODULE_LIST",
    "PRAGMA_LIST",
    "INTEGRITY_CHECK",
    "QUICK_CHECK",
];

/// Whether a `PRAGMA` statement only reads.
///
/// `PRAGMA x` reads. `PRAGMA x = y` writes — and so does `PRAGMA x(y)`, which a
/// test for `=` alone waved straight through, making `PRAGMA journal_mode(WAL)`
/// and `PRAGMA writable_schema(1)` both look like reads.
fn pragma_reads(sql: &str) -> bool {
    let Some(name) = pragma_name(sql) else {
        // Not a shape this can read; treating it as a write is the safe side.
        return false;
    };
    if READ_ONLY_PRAGMAS.contains(&name.as_str()) {
        return true;
    }
    // Any other pragma is a read only when it supplies no value in either form.
    !sql.contains('=') && !sql.contains('(')
}

/// The name a `PRAGMA` statement sets or reads, upper-cased.
///
/// Read from its exact position — the word after `PRAGMA`, or after the
/// `schema.` that may precede it. Accepting an allowlisted word *anywhere* in
/// the first few tokens let the value vouch for the name:
/// `PRAGMA user_version(table_info)` names `table_info` as its argument, sets
/// `user_version`, and passed as a read.
fn pragma_name(sql: &str) -> Option<String> {
    let body = &sql[leading_trivia_len(sql)..];
    if !body.get(..6)?.eq_ignore_ascii_case("PRAGMA") {
        return None;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let take_ident = |s: &str| -> String {
        let end = s.find(|c: char| !is_ident(c)).unwrap_or(s.len());
        s[..end].to_ascii_uppercase()
    };

    let rest = body[6..].trim_start();
    let first = take_ident(rest);
    let after = rest[first.len()..].trim_start();
    let name = match after.strip_prefix('.') {
        Some(qualified) => take_ident(qualified.trim_start()),
        None => first,
    };
    (!name.is_empty()).then_some(name)
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
///
/// `primary_key` makes paging deterministic. Without an `ORDER BY` a database
/// may return rows in a different order for each query, so Next could repeat
/// rows from the previous page and skip others entirely; sorting on a
/// non-unique column has the same problem among equal values. The key orders
/// an unsorted page and breaks ties in a sorted one. A keyless table is left
/// unordered — ordering by every column would sort the whole table for every
/// page, which is too high a price for browsing.
pub fn build_browse_sql(
    table: &TableRef,
    options: &BrowseOptions,
    known: &[&str],
    primary_key: &[String],
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

    let sort = options.sort_column.as_deref().filter(|c| known.contains(c));
    let mut order: Vec<String> = Vec::new();
    if let Some(col) = sort {
        order.push(format!(
            "{} {}",
            dialect.quote_ident(col),
            if options.sort_desc { "DESC" } else { "ASC" }
        ));
    }
    order.extend(
        primary_key
            .iter()
            .filter(|k| known.contains(&k.as_str()) && Some(k.as_str()) != sort)
            .map(|k| dialect.quote_ident(k)),
    );
    if !order.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
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

/// The condition selecting the rows that follow `last` in primary-key order —
/// keyset paging.
///
/// `OFFSET n` makes the engine produce and throw away `n` rows to reach each
/// page, so walking a table that way re-reads everything before every page:
/// quadratic in the table's size, which is what made backing up or exporting a
/// large table crawl as it went. Continuing from the last key seen costs one
/// index seek per page instead.
///
/// Spelled out as `a > x OR (a = x AND b > y)` rather than the row-value form
/// `(a, b) > (x, y)`, which SQL Server does not support.
///
/// `None` when a key value cannot be trusted to compare exactly against its
/// own literal — a float, a timestamp whose text form is not the engine's,
/// bytes — and the caller should fall back to an offset. Wrongly continuing
/// from a key that matches imprecisely would skip or repeat rows.
pub fn keyset_after(
    primary_key: &[String],
    last: &[&crate::model::Value],
    dialect: &dyn crate::driver::Dialect,
) -> Option<String> {
    use crate::model::Value;
    let exact = |v: &&Value| {
        matches!(
            v,
            Value::Int(_) | Value::Decimal(_) | Value::Text(_) | Value::Uuid(_)
        )
    };
    if primary_key.is_empty() || primary_key.len() != last.len() || !last.iter().all(exact) {
        return None;
    }

    let columns: Vec<String> = primary_key.iter().map(|k| dialect.quote_ident(k)).collect();
    let literals: Vec<String> = last.iter().map(|v| dialect.literal(v)).collect();
    let branches: Vec<String> = (0..columns.len())
        .map(|i| {
            let mut parts: Vec<String> = (0..i)
                .map(|j| format!("{} = {}", columns[j], literals[j]))
                .collect();
            parts.push(format!("{} > {}", columns[i], literals[i]));
            format!("({})", parts.join(" AND "))
        })
        .collect();
    Some(branches.join(" OR "))
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
pub(crate) fn escape_like(s: &str) -> String {
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
    fn a_cursor_just_after_a_semicolon_belongs_to_that_statement() {
        // The position a cursor is in right after typing a statement. It used
        // to resolve to the *next* statement — here, the DELETE.
        let script = "SELECT 1;\nDELETE FROM t";
        assert_eq!(statement_at(script, 9).as_deref(), Some("SELECT 1"));
        // Trailing spaces and a same-line comment keep it there too.
        let script = "SELECT 1;   -- note\nDELETE FROM t";
        assert_eq!(statement_at(script, 19).as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn a_cursor_on_the_next_line_belongs_to_the_next_statement() {
        let script = "SELECT 1;\n\n  DELETE FROM t";
        // Start of the blank line, and the indentation before DELETE.
        assert_eq!(statement_at(script, 10).as_deref(), Some("DELETE FROM t"));
        assert_eq!(statement_at(script, 12).as_deref(), Some("DELETE FROM t"));
    }

    #[test]
    fn a_cursor_after_the_final_semicolon_still_has_a_statement() {
        assert_eq!(statement_at("SELECT 1;", 9).as_deref(), Some("SELECT 1"));
        assert_eq!(statement_at("SELECT 1;  ", 11).as_deref(), Some("SELECT 1"));
        // A line further down there is nothing left to run.
        assert_eq!(statement_at("SELECT 1;\n", 10), None);
    }

    #[test]
    fn a_trailing_comment_is_never_the_statement_under_the_cursor() {
        let script = "SELECT 1;\n-- done";
        assert_eq!(statement_at(script, 12), None);
        assert_eq!(
            statement_at("SELECT 1; -- done", 16).as_deref(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn comment_only_fragments_are_recognised() {
        assert!(is_comment_only(""));
        assert!(is_comment_only("  \n "));
        assert!(is_comment_only("-- done"));
        assert!(is_comment_only("/* a */ -- b\n  /* c */"));
        assert!(!is_comment_only("-- note\nSELECT 1"));
        assert!(!is_comment_only("/* a */ SELECT 1"));
        assert!(!is_comment_only("'-- not a comment'"));
    }

    #[test]
    fn executable_statements_drop_the_comment_only_tail() {
        assert_eq!(
            executable_statements("SELECT 1;\n-- done\n", LexRules::STANDARD),
            vec!["SELECT 1"]
        );
        assert_eq!(
            executable_statements("-- header\nSELECT 1; /* x */ ;", LexRules::STANDARD),
            vec!["-- header\nSELECT 1"]
        );
    }

    #[test]
    fn the_leading_word_skips_comments_and_parentheses() {
        assert_eq!(leading_word("SELECT 1").as_deref(), Some("SELECT"));
        assert_eq!(
            leading_word("  -- note\nselect 1").as_deref(),
            Some("SELECT")
        );
        assert_eq!(
            leading_word("/* hint */ (SELECT 1)").as_deref(),
            Some("SELECT")
        );
        assert_eq!(leading_word("-- only a comment"), None);
        assert_eq!(leading_word(""), None);
    }

    #[test]
    fn a_leading_comment_does_not_make_a_read_look_like_a_write() {
        // Commented queries are ordinary; refusing them on a read-only
        // connection made the setting unusable for anyone who documents SQL.
        for sql in [
            "-- all users\nSELECT * FROM users",
            "/* report */ SELECT 1",
            "-- a\n-- b\nWITH c AS (SELECT 1) SELECT * FROM c",
        ] {
            assert!(is_read_only_statement(sql), "should be allowed: {sql}");
        }
        // And a comment in front of a write still leaves it a write.
        assert!(!is_read_only_statement("-- tidy up\nDELETE FROM t"));
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
    fn keyset_paging_continues_after_the_last_key() {
        use crate::model::Value;
        let pk = vec!["id".to_string()];
        assert_eq!(
            keyset_after(&pk, &[&Value::Int(42)], &TestDialect).as_deref(),
            Some(r#"("id" > 42)"#)
        );
    }

    #[test]
    fn a_composite_key_expands_without_row_value_syntax() {
        // `(a, b) > (x, y)` is not valid T-SQL, so the expanded form is used
        // everywhere.
        use crate::model::Value;
        let pk = vec!["book_id".to_string(), "store".to_string()];
        let last = [&Value::Int(7), &Value::Text("O'Hare".into())];
        assert_eq!(
            keyset_after(&pk, &last, &TestDialect).as_deref(),
            Some(r#"("book_id" > 7) OR ("book_id" = 7 AND "store" > 'O''Hare')"#)
        );
    }

    #[test]
    fn keys_that_do_not_compare_exactly_fall_back_to_an_offset() {
        use crate::model::Value;
        let pk = vec!["k".to_string()];
        for v in [
            Value::Float(0.1),
            Value::Timestamp("2026-01-01 00:00:00".into()),
            Value::Bytes(vec![1]),
            Value::Null,
        ] {
            assert_eq!(keyset_after(&pk, &[&v], &TestDialect), None, "{v:?}");
        }
        // No key, or a key the row does not fully carry.
        assert_eq!(keyset_after(&[], &[], &TestDialect), None);
        assert_eq!(keyset_after(&pk, &[], &TestDialect), None);
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
    fn a_quoting_rule_from_another_dialect_cannot_hide_a_write() {
        // The lexer used to apply MySQL's backslash-escape rule inside *every*
        // quote character on every engine. On Postgres, SQLite, DuckDB and SQL
        // Server `"` delimits an identifier and `\` means nothing, so the
        // engine closes the identifier at the quote the lexer had just
        // swallowed as an escape — and everything after it, including a write
        // keyword, became invisible to this check.
        //
        // On a standalone SQL Server that was a complete read-only bypass:
        // `ApplicationIntent=ReadOnly` is ignored there, and `simple_query`
        // runs the text as a multi-statement T-SQL batch.
        for sql in [
            // Double-quoted identifier: the engine ends it at the `"` after the
            // backslash, leaving `DROP TABLE t` as a second statement.
            r#"SELECT 1 AS "a\"; DROP TABLE t; --""#,
            // Same trick with no second statement at all — a data-modifying CTE
            // whose keyword hides behind the identifier.
            r#"WITH "a\" AS (SELECT 1) DELETE FROM t"#,
            // Bracket quoting is T-SQL's, and `]` is escaped there by doubling,
            // not by a backslash. Same shape, same outcome.
            r"SELECT 1 FROM [t\]; DROP TABLE t; --]",
        ] {
            assert!(!is_read_only_statement(sql), "should be refused: {sql}");
        }
    }

    #[test]
    fn the_pragma_call_form_is_a_write_too() {
        // `PRAGMA x = y` writes and `PRAGMA x` reads, but SQLite accepts a
        // third spelling: `PRAGMA x(y)` sets the value and contains no `=`, so
        // a substring test for `=` waved it straight through.
        for sql in [
            "PRAGMA journal_mode(WAL)",
            "PRAGMA writable_schema(1)",
            "PRAGMA journal_mode = WAL",
        ] {
            assert!(!is_read_only_statement(sql), "should be refused: {sql}");
        }

        // The reading forms must keep working.
        for sql in ["PRAGMA table_info(t)", "PRAGMA journal_mode"] {
            assert!(is_read_only_statement(sql), "should be allowed: {sql}");
        }
    }

    #[test]
    fn writes_in_a_script_are_counted_with_the_read_only_classifier() {
        let script = executable_statements(
            "-- tidy\nSELECT 1; DELETE FROM t; UPDATE t SET a = 1; SELECT 2",
            LexRules::STANDARD,
        );
        assert_eq!(script.len(), 4);
        assert_eq!(count_writes(&script, Some(LexRules::STANDARD)), 2);
        assert_eq!(count_writes(&script, None), 2);
        assert_eq!(count_writes(&["SELECT 1".into()], None), 0);
    }

    #[test]
    fn a_pragma_value_cannot_vouch_for_its_name() {
        // The allowlisted word is the *argument* here; the pragma being set is
        // user_version, which writes.
        for sql in [
            "PRAGMA user_version(table_info)",
            "PRAGMA main.application_id(index_list)",
            "PRAGMA table_info",
        ] {
            let expected = sql == "PRAGMA table_info";
            assert_eq!(is_read_only_statement(sql), expected, "{sql}");
        }
        // Schema-qualified and commented reads keep working.
        for sql in [
            "PRAGMA main.table_info(t)",
            "PRAGMA main . index_list(t)",
            "-- inspect\nPRAGMA table_info(t)",
        ] {
            assert!(is_read_only_statement(sql), "should be allowed: {sql}");
        }
    }

    #[test]
    fn functions_that_change_state_are_refused_inside_a_select() {
        // `set_config` switches off the server-side read-only guard, after
        // which any writing function called from a SELECT succeeds.
        for sql in [
            "SELECT set_config('default_transaction_read_only', 'off', false)",
            "SELECT nextval('orders_id_seq')",
            "SELECT setval('orders_id_seq', 1)",
            "SELECT * FROM dblink('remote', 'DELETE FROM t') AS x(a int)",
            "SELECT dblink_exec('remote', 'DROP TABLE t')",
            "SELECT lo_unlink(16400)",
            "SELECT pg_terminate_backend(123)",
            "SELECT PG_RELOAD_CONF()",
        ] {
            assert!(!is_read_only_statement(sql), "should be refused: {sql}");
        }
        // Mentioning one inside a string is still a read.
        assert!(is_read_only_statement("SELECT 'set_config' AS name"));
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
    fn each_engines_rules_are_applied_to_its_own_statements() {
        // The payloads from the bypass above, judged with the rules of the
        // engine that would actually run them.

        // Postgres/SQLite/DuckDB: `"` is an identifier and `\` is an ordinary
        // character, so the identifier ends at the second quote and the write
        // that follows is plainly visible.
        assert!(!is_read_only_statement_with(
            r#"SELECT 1 AS "a\"; DROP TABLE t; --""#,
            LexRules::STANDARD
        ));
        assert!(!is_read_only_statement_with(
            r#"WITH "a\" AS (SELECT 1) DELETE FROM t"#,
            LexRules::STANDARD
        ));

        // SQL Server: brackets quote identifiers, but `\` still escapes nothing,
        // so `[t\]` closes at that `]` and the DROP is in the open.
        assert!(!is_read_only_statement_with(
            r"SELECT 1 FROM [t\]; DROP TABLE t; --]",
            LexRules::TSQL
        ));

        // The same text on MySQL really is one harmless statement: there `"`
        // opens a *string* and `\` escapes the quote inside it, so everything
        // through the closing quote is an alias, not SQL. Refusing it would be
        // refusing a legitimate read.
        assert!(is_read_only_statement_with(
            r#"SELECT 1 AS "a\"; DROP TABLE t; --""#,
            LexRules::MYSQL
        ));

        // And a T-SQL column named after a keyword is a read, because brackets
        // genuinely are quoting there.
        assert!(is_read_only_statement_with(
            "SELECT [drop] FROM t",
            LexRules::TSQL
        ));
        // On Postgres the same text is not quoting at all, so it is refused —
        // the conservative direction, and `[drop]` is not valid there anyway.
        assert!(!is_read_only_statement_with(
            "SELECT [drop] FROM t",
            LexRules::STANDARD
        ));
    }

    #[test]
    fn splitting_follows_the_engines_quoting_too() {
        // Under standard rules the backslash does not protect the quote, so the
        // `;` after it is a real separator and the DROP becomes a statement of
        // its own — which the read-only check then sees on its own terms.
        let out = split_statements_with(r#"SELECT 1 AS "a\"; DROP TABLE t"#, LexRules::STANDARD);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!(out[1], "DROP TABLE t");

        // On MySQL the very same text is one statement, because there the
        // backslash really does escape the closing quote.
        let out = split_statements_with(r#"SELECT 1 AS "a\"; DROP TABLE t"#, LexRules::MYSQL);
        assert_eq!(out.len(), 1, "{out:#?}");
    }

    #[test]
    fn the_no_dialect_default_refuses_what_any_engine_might_run() {
        // `is_read_only_statement` names no engine, so it must not be fooled by
        // any single engine's quoting. Both readings have to agree.
        for sql in [
            r#"SELECT 1 AS "a\"; DROP TABLE t; --""#,
            r"SELECT 1 FROM [t\]; DROP TABLE t; --]",
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
        let sql = build_browse_sql(&table(), &opts(), &["id"], &[], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books""#);
    }

    #[test]
    fn sorting_appends_an_order_by() {
        let mut o = opts();
        o.sort_column = Some("id".into());
        o.sort_desc = true;
        let sql = build_browse_sql(&table(), &o, &["id"], &[], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books" ORDER BY "id" DESC"#);
    }

    #[test]
    fn sorting_on_an_unknown_column_is_dropped() {
        // The allowlist is what makes interpolating the name safe at all.
        let mut o = opts();
        o.sort_column = Some("id; DROP TABLE books".into());
        let sql = build_browse_sql(&table(), &o, &["id"], &[], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books""#);
    }

    #[test]
    fn an_unsorted_page_is_ordered_by_the_primary_key() {
        // Without an order, Next could repeat rows from the page before it and
        // skip others entirely.
        let pk = vec!["id".to_string()];
        let sql = build_browse_sql(&table(), &opts(), &["id", "name"], &pk, &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books" ORDER BY "id""#);
    }

    #[test]
    fn a_sort_on_another_column_is_tie_broken_by_the_key() {
        let mut o = opts();
        o.sort_column = Some("name".into());
        let pk = vec!["id".to_string()];
        let sql = build_browse_sql(&table(), &o, &["id", "name"], &pk, &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books" ORDER BY "name" ASC, "id""#);

        // Sorting on the key itself does not repeat it.
        o.sort_column = Some("id".into());
        o.sort_desc = true;
        let sql = build_browse_sql(&table(), &o, &["id", "name"], &pk, &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books" ORDER BY "id" DESC"#);
    }

    #[test]
    fn a_keyless_table_is_not_sorted_just_to_browse_it() {
        // Ordering by every column would sort the whole table for each page.
        let sql = build_browse_sql(&table(), &opts(), &["a", "b"], &[], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "books""#);
    }

    #[test]
    fn filters_and_sort_compose_in_sql_order() {
        let mut o = opts();
        o.filters = vec![filter("id", FilterOp::GreaterThan, "5")];
        o.sort_column = Some("id".into());
        let sql = build_browse_sql(&table(), &o, &["id"], &[], &TestDialect, 1000);
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
        let sql = build_browse_sql(&table(), &o, &["id"], &[], &TestDialect, 10);
        // The ORDER BY must stay inside the subquery, or the page is arbitrary.
        assert!(sql.contains(r#"ORDER BY "id" ASC"#), "{sql}");
        assert!(sql.contains("LIMIT 11 OFFSET 20"), "{sql}");
    }

    #[test]
    fn offset_zero_does_not_wrap() {
        // The driver's own limit+1 probe already caps it; wrapping would nest
        // a pointless subquery.
        let sql = build_browse_sql(&table(), &opts(), &["id"], &[], &TestDialect, 1000);
        assert!(!sql.contains("faro_q"), "{sql}");
    }

    #[test]
    fn schema_qualifies_when_the_dialect_supports_it() {
        let t = TableRef {
            schema: Some("public".into()),
            name: "books".into(),
        };
        let sql = build_browse_sql(&t, &opts(), &["id"], &[], &TestDialect, 1000);
        assert_eq!(sql, r#"SELECT * FROM "public"."books""#);
    }
}
