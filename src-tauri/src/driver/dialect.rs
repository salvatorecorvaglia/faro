use crate::model::Value;

/// Per-engine SQL syntax differences.
///
/// This trait exists because pagination is genuinely incompatible across
/// engines — SQL Server needs `OFFSET ... FETCH NEXT` (and refuses without an
/// `ORDER BY`), while most others take `LIMIT ... OFFSET`. Encoding those
/// differences once here keeps every feature above the driver layer free of
/// `match engine { ... }` branches.
pub trait Dialect: Send + Sync {
    /// Quote an identifier, escaping the closing quote character.
    fn quote_ident(&self, ident: &str) -> String;

    /// Wrap a `SELECT` so it returns at most `limit` rows starting at `offset`.
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String;

    /// Render a byte string as a literal.
    fn quote_bytes(&self, bytes: &[u8]) -> String;

    /// Render a text string as a literal.
    ///
    /// Defaults to the SQL-standard rule of doubling the quote. Engines that
    /// also honour backslash escapes inside `'...'` **must** override this —
    /// see [`crate::model::quote_sql_string_backslash`].
    fn quote_string(&self, s: &str) -> String {
        crate::model::quote_sql_string(s)
    }

    /// Whether this engine's own `CREATE TABLE` text also declares the table's
    /// secondary indexes.
    ///
    /// Only meaningful for engines that expose native DDL at all. MySQL's
    /// `SHOW CREATE TABLE` lists them inline as `KEY name (col)`, so a dump that
    /// also emits `CREATE INDEX` fails to restore with "Duplicate key name".
    /// SQLite keeps indexes as separate `sqlite_master` objects, so there they
    /// still have to be emitted separately. Foreign keys are inline in both.
    fn native_ddl_includes_indexes(&self) -> bool {
        false
    }

    /// The type to cast to when a value has to be compared as text, as in the
    /// `LIKE` used by the `Contains` and `StartsWith` browse filters.
    ///
    /// `TEXT` is not universal: MySQL rejects it in `CAST` and wants `CHAR`,
    /// and T-SQL's `TEXT` is a deprecated LOB type that cannot be compared.
    fn text_cast_type(&self) -> &'static str {
        "TEXT"
    }

    /// ClickHouse has no transactions, so restore and multi-statement applies
    /// must not try to open one.
    fn supports_transactions(&self) -> bool {
        true
    }

    /// Whether `CREATE TABLE` and friends can be rolled back.
    ///
    /// Separate from `supports_transactions` because MySQL and MariaDB have
    /// perfectly good transactions that DDL simply ignores: each `CREATE`,
    /// `DROP` or `ALTER` triggers an implicit commit, so a restore wrapped in a
    /// transaction there would report atomicity it cannot deliver. Better to
    /// know that and say so than to open a transaction for appearances.
    fn ddl_is_transactional(&self) -> bool {
        true
    }

    /// How this engine opens a transaction.
    ///
    /// Every engine here accepts bare `COMMIT` and `ROLLBACK`, but T-SQL spells
    /// the opener `BEGIN TRANSACTION` — a bare `BEGIN` there starts a statement
    /// block instead, which would silently not be a transaction at all.
    fn begin_statement(&self) -> &'static str {
        "BEGIN"
    }

    /// Whether the query language is SQL at all.
    ///
    /// False for MongoDB, whose queries are documents rather than statements.
    /// Callers use it to pick an editor mode, skip the SQL formatter, and refuse
    /// operations that only make sense as SQL — emitting a `.sql` dump for a
    /// document store would produce a file that restores nowhere.
    fn is_sql(&self) -> bool {
        true
    }

    /// Whether the engine has a real schema namespace. SQLite and DuckDB do not,
    /// which changes how tables are qualified everywhere.
    fn supports_schemas(&self) -> bool {
        true
    }

    /// Fully qualify a table for use in generated SQL.
    fn qualify(&self, schema: Option<&str>, table: &str) -> String {
        match schema.filter(|_| self.supports_schemas()) {
            Some(s) => format!("{}.{}", self.quote_ident(s), self.quote_ident(table)),
            None => self.quote_ident(table),
        }
    }

    fn literal(&self, value: &Value) -> String {
        value.to_sql_literal(&|b| self.quote_bytes(b), &|s| self.quote_string(s))
    }
}

/// Double-quote quoting (`"col"`), used by Postgres, SQLite, DuckDB and the
/// SQL standard generally.
pub fn quote_double(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Backtick quoting, used by MySQL and MariaDB.
pub fn quote_backtick(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

/// Bracket quoting, used by SQL Server. A literal `]` is escaped by doubling.
pub fn quote_bracket(ident: &str) -> String {
    format!("[{}]", ident.replace(']', "]]"))
}

/// `LIMIT n OFFSET m` — Postgres, SQLite, MySQL, DuckDB, ClickHouse.
pub fn paginate_limit_offset(sql: &str, limit: u64, offset: u64) -> String {
    let trimmed = sql.trim().trim_end_matches(';');
    if offset > 0 {
        format!("SELECT * FROM ({trimmed}) AS faro_q LIMIT {limit} OFFSET {offset}")
    } else {
        format!("SELECT * FROM ({trimmed}) AS faro_q LIMIT {limit}")
    }
}

pub fn hex_bytes_x(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 3);
    s.push_str("X'");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s.push('\'');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_the_closing_character() {
        // An identifier containing the quote char is the classic injection seam.
        assert_eq!(quote_double(r#"we"ird"#), r#""we""ird""#);
        assert_eq!(quote_backtick("we`ird"), "`we``ird`");
        assert_eq!(quote_bracket("we]ird"), "[we]]ird]");
    }

    #[test]
    fn pagination_wraps_so_inner_limits_survive() {
        // Naively appending LIMIT would produce "... LIMIT 5 LIMIT 10".
        let sql = "SELECT * FROM t LIMIT 5";
        let out = paginate_limit_offset(sql, 10, 0);
        assert_eq!(
            out,
            "SELECT * FROM (SELECT * FROM t LIMIT 5) AS faro_q LIMIT 10"
        );
    }

    #[test]
    fn pagination_strips_trailing_semicolon() {
        // A trailing ';' inside a subquery is a syntax error.
        let out = paginate_limit_offset("SELECT 1;", 10, 20);
        assert_eq!(out, "SELECT * FROM (SELECT 1) AS faro_q LIMIT 10 OFFSET 20");
    }

    #[test]
    fn hex_literal_is_zero_padded() {
        assert_eq!(hex_bytes_x(&[0x00, 0x0f, 0xff]), "X'000fff'");
    }
}
