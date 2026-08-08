//! Built-in backup and restore.
//!
//! Faro generates dumps itself rather than shelling out to `pg_dump` or
//! `mysqldump`. That keeps the promise of no setup — it works the same on every
//! engine and every machine, with nothing to install — at the cost of being
//! slower on very large databases and not capturing every exotic server-side
//! object. See the README for the honest limits.
//!
//! One important constraint: type names in the generated DDL are the source
//! engine's own (`int8`, `VARCHAR(50)`, `uniqueidentifier`). A dump therefore
//! restores into the same engine, not across engines. Rewriting types between
//! dialects would be guesswork, and guessing wrong about a column type is worse
//! than declining.

use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::driver::{Dialect, Driver};
use crate::error::{FaroError, Result};
use crate::model::{TableDetail, TableKind, TableRef};
use crate::sql;

/// Rows per generated `INSERT`. Large enough that restore is not one statement
/// per row, small enough that a single statement stays parseable by every
/// engine's limits.
const ROWS_PER_INSERT: usize = 200;

/// Rows fetched per query while walking a table.
const FETCH_BATCH: u64 = 2_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupOptions {
    /// Tables to include. Empty means every table in the schema.
    #[serde(default)]
    pub tables: Vec<TableRef>,
    #[serde(default = "default_true")]
    pub include_schema: bool,
    #[serde(default = "default_true")]
    pub include_data: bool,
    /// Emit `DROP TABLE IF EXISTS` before each `CREATE`.
    #[serde(default)]
    pub drop_existing: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub tables: usize,
    pub rows: u64,
    pub bytes: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub table: String,
    pub table_index: usize,
    pub table_count: usize,
    pub rows_written: u64,
}

/// Write a dump of `tables` to `path`.
///
/// `on_progress` is called per batch so the UI can show a progress bar; a
/// backup of a large table is otherwise indistinguishable from a hang.
pub async fn write_backup(
    driver: &dyn Driver,
    path: &std::path::Path,
    options: &BackupOptions,
    mut on_progress: impl FnMut(BackupProgress),
) -> Result<BackupResult> {
    let dialect = driver.dialect();

    // A `.sql` dump of a document store would restore nowhere, so this declines
    // rather than writing a file that only looks like a backup.
    if !dialect.is_sql() {
        return Err(FaroError::Other(
            "Backup produces a SQL dump, which does not apply to this database. \
             Export a collection to JSON instead."
                .into(),
        ));
    }

    // Resolve the table list up front so progress can report a total.
    let tables = if options.tables.is_empty() {
        driver
            .list_tables(None)
            .await?
            .into_iter()
            // Views hold no data of their own and their definitions are
            // engine-specific text Faro does not currently read back.
            .filter(|t| t.kind == TableKind::Table)
            .map(|t| TableRef {
                schema: t.schema,
                name: t.name,
            })
            .collect()
    } else {
        options.tables.clone()
    };

    if tables.is_empty() {
        return Err(FaroError::Other("no tables selected to back up".into()));
    }

    let file = std::fs::File::create(path)?;
    let mut out = std::io::BufWriter::new(file);

    writeln!(out, "-- Faro backup")?;
    writeln!(out, "-- Generated {}", chrono::Utc::now().to_rfc3339())?;
    writeln!(
        out,
        "-- Type names are the source engine's own, so restore into the same engine."
    )?;
    writeln!(out)?;

    let mut details = Vec::with_capacity(tables.len());
    for table in &tables {
        details.push(driver.describe_table(table).await?);
    }

    // Order parents before children. Without this, a child table's rows are
    // inserted before the parent rows they reference and the restore fails on a
    // foreign key — which is exactly what happens with a plain alphabetical
    // order, where `book_stores` precedes `books`.
    details = order_by_dependency(details);

    // Schema first, all of it, before any data. A restore then has every table
    // in place before the first insert, so table order cannot break a
    // foreign key.
    if options.include_schema {
        for detail in &details {
            if options.drop_existing {
                writeln!(
                    out,
                    "DROP TABLE IF EXISTS {};",
                    dialect.qualify(detail.table.schema.as_deref(), &detail.table.name)
                )?;
            }
            // Prefer the engine's own DDL where it keeps one — it preserves
            // constraints a column-by-column rebuild cannot express.
            let ddl = match driver.table_ddl(&detail.table).await? {
                Some(native) => native,
                None => create_table_sql(detail, dialect),
            };
            writeln!(out, "{ddl}")?;
        }
    }

    let mut total_rows = 0u64;
    if options.include_data {
        for (index, detail) in details.iter().enumerate() {
            let written = write_table_data(driver, &mut out, detail, dialect, |rows| {
                on_progress(BackupProgress {
                    table: detail.table.name.clone(),
                    table_index: index,
                    table_count: details.len(),
                    rows_written: rows,
                });
            })
            .await?;
            total_rows += written;
        }
    }

    // Indexes and foreign keys last: inserting into an unindexed table is much
    // faster, and every referenced row exists by the time constraints apply.
    //
    // Foreign keys are skipped when the engine supplied its own DDL, since that
    // text already declares them inline and adding them again would fail.
    if options.include_schema {
        for detail in &details {
            let native = driver.table_ddl(&detail.table).await?.is_some();
            for stmt in secondary_objects_sql(detail, dialect) {
                if native && stmt.starts_with("ALTER TABLE") {
                    continue;
                }
                writeln!(out, "{stmt}")?;
            }
        }
    }

    out.flush()?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    Ok(BackupResult {
        tables: details.len(),
        rows: total_rows,
        bytes,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Page through a table, emitting batched `INSERT`s.
async fn write_table_data(
    driver: &dyn Driver,
    out: &mut impl Write,
    detail: &TableDetail,
    dialect: &dyn Dialect,
    mut on_batch: impl FnMut(u64),
) -> Result<u64> {
    let qualified = dialect.qualify(detail.table.schema.as_deref(), &detail.table.name);
    let columns: Vec<String> = detail
        .columns
        .iter()
        .map(|c| dialect.quote_ident(&c.name))
        .collect();

    // Ordering by the primary key is what makes paging stable; see `order_by_key`.
    let order = sql::order_by_key(&detail.primary_key, dialect);

    let base = format!("SELECT {} FROM {qualified}{order}", columns.join(", "));
    let prefix = format!("INSERT INTO {qualified} ({}) VALUES", columns.join(", "));

    writeln!(out, "\n-- Data for {}", detail.table.name)?;

    let mut offset = 0u64;
    let mut total = 0u64;

    loop {
        let paged = dialect.paginate(&base, FETCH_BATCH + 1, offset);
        let page = driver
            .query(
                &paged,
                FETCH_BATCH,
                tokio_util::sync::CancellationToken::new(),
            )
            .await?;

        if page.rows.is_empty() {
            break;
        }

        for chunk in page.rows.chunks(ROWS_PER_INSERT) {
            let tuples: Vec<String> = chunk
                .iter()
                .map(|row| {
                    let values: Vec<String> = row.iter().map(|v| dialect.literal(v)).collect();
                    format!("({})", values.join(", "))
                })
                .collect();
            writeln!(out, "{prefix}\n{};", tuples.join(",\n"))?;
        }

        let count = page.rows.len() as u64;
        total += count;
        offset += count;
        on_batch(total);

        if !page.truncated {
            break;
        }
    }

    Ok(total)
}

/// Generate `CREATE TABLE` from a table's columns and primary key.
pub fn create_table_sql(detail: &TableDetail, dialect: &dyn Dialect) -> String {
    let mut lines: Vec<String> = detail
        .columns
        .iter()
        .map(|c| {
            let mut line = format!("  {} {}", dialect.quote_ident(&c.name), c.type_name);
            if !c.nullable {
                line.push_str(" NOT NULL");
            }
            if let Some(default) = &c.default {
                // Defaults keep the engine's own syntax (`now()`,
                // `nextval(...)`) rather than being reinterpreted — but a
                // function call has to be parenthesised to be accepted back.
                line.push_str(&format!(" DEFAULT {}", wrap_default(default)));
            }
            line
        })
        .collect();

    if !detail.primary_key.is_empty() {
        let keys: Vec<String> = detail
            .primary_key
            .iter()
            .map(|k| dialect.quote_ident(k))
            .collect();
        lines.push(format!("  PRIMARY KEY ({})", keys.join(", ")));
    }

    format!(
        "CREATE TABLE {} (\n{}\n);",
        dialect.qualify(detail.table.schema.as_deref(), &detail.table.name),
        lines.join(",\n")
    )
}

/// Sort tables so every table follows the ones its foreign keys point at.
///
/// A topological sort over the foreign-key graph, restricted to the tables in
/// the dump — a reference to a table that is not being backed up cannot
/// constrain the order. Self-references are ignored, since a table cannot
/// precede itself.
///
/// Circular references between tables have no valid order; those tables are
/// appended in their original order rather than dropped. The restore may then
/// need foreign keys deferred, which is unavoidable for a genuine cycle.
fn order_by_dependency(details: Vec<TableDetail>) -> Vec<TableDetail> {
    let names: Vec<String> = details.iter().map(|d| d.table.name.clone()).collect();

    // Dependencies of each table, by index, kept only for tables in this dump.
    let deps: Vec<Vec<usize>> = details
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut out: Vec<usize> = d
                .foreign_keys
                .iter()
                .filter_map(|fk| names.iter().position(|n| *n == fk.referenced_table.name))
                .filter(|&target| target != i)
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();

    let count = details.len();
    let mut placed = vec![false; count];
    let mut order: Vec<usize> = Vec::with_capacity(count);

    // Take every table whose dependencies are already placed, one level at a
    // time, until no more can be placed.
    loop {
        let ready: Vec<usize> = (0..count)
            .filter(|&i| !placed[i] && deps[i].iter().all(|&d| placed[d]))
            .collect();
        if ready.is_empty() {
            break;
        }
        for i in ready {
            placed[i] = true;
            order.push(i);
        }
    }

    // Anything left is part of a cycle.
    order.extend((0..count).filter(|&i| !placed[i]));

    let mut slots: Vec<Option<TableDetail>> = details.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// Parenthesise a `DEFAULT` unless it is already a bare literal.
///
/// SQLite reports `DEFAULT (datetime('now'))` as `datetime('now')`, stripping
/// the parentheses it then requires back — emitting it verbatim is a syntax
/// error. Wrapping is accepted for expressions by both SQLite and Postgres, so
/// anything that is not obviously a literal gets parentheses.
fn wrap_default(default: &str) -> String {
    let t = default.trim();

    if t.is_empty() {
        return t.to_string();
    }
    // Already parenthesised.
    if t.starts_with('(') && t.ends_with(')') {
        return t.to_string();
    }
    // A quoted string literal.
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        return t.to_string();
    }
    // A number, with optional sign and decimal point.
    let numeric_body = t.strip_prefix('-').unwrap_or(t);
    if !numeric_body.is_empty() && numeric_body.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return t.to_string();
    }
    // A keyword literal.
    if matches!(
        t.to_ascii_uppercase().as_str(),
        "NULL" | "TRUE" | "FALSE" | "CURRENT_TIMESTAMP" | "CURRENT_DATE" | "CURRENT_TIME"
    ) {
        return t.to_string();
    }

    format!("({t})")
}

/// Indexes and foreign keys, emitted after the data is loaded.
pub fn secondary_objects_sql(detail: &TableDetail, dialect: &dyn Dialect) -> Vec<String> {
    let qualified = dialect.qualify(detail.table.schema.as_deref(), &detail.table.name);
    let mut out = Vec::new();

    for index in &detail.indexes {
        // Constraint-backed indexes are created by the table definition, not by
        // CREATE INDEX. Re-emitting one either duplicates it or, on SQLite,
        // fails outright because the name is reserved.
        if index.is_primary || index.is_constraint || index.columns.is_empty() {
            continue;
        }
        let columns: Vec<String> = index
            .columns
            .iter()
            .map(|c| dialect.quote_ident(c))
            .collect();
        out.push(format!(
            "CREATE {}INDEX {} ON {} ({});",
            if index.is_unique { "UNIQUE " } else { "" },
            dialect.quote_ident(&index.name),
            qualified,
            columns.join(", ")
        ));
    }

    for fk in &detail.foreign_keys {
        if fk.columns.is_empty() || fk.referenced_columns.is_empty() {
            continue;
        }
        let local: Vec<String> = fk.columns.iter().map(|c| dialect.quote_ident(c)).collect();
        let remote: Vec<String> = fk
            .referenced_columns
            .iter()
            .map(|c| dialect.quote_ident(c))
            .collect();
        out.push(format!(
            "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
            qualified,
            dialect.quote_ident(&fk.name),
            local.join(", "),
            dialect.qualify(
                fk.referenced_table.schema.as_deref(),
                &fk.referenced_table.name
            ),
            remote.join(", ")
        ));
    }

    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub statements: usize,
    pub failed: usize,
    /// Messages from statements that failed, when not stopping on error.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOptions {
    /// Abort at the first failure. On by default: a restore that limps past
    /// errors leaves a database nobody can trust.
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
}

/// Execute a dump file against a connection.
///
/// Statements are split with the same lexer the editor uses, so dollar-quoted
/// bodies and embedded semicolons survive.
pub async fn restore(
    driver: &dyn Driver,
    script: &str,
    options: &RestoreOptions,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<RestoreResult> {
    // Comment-only fragments survive the split — the dump's own header is one.
    // Dropping them here means the count reported to the user is the number of
    // statements that will actually run, and that a file of nothing but
    // comments is correctly recognized as having no SQL in it.
    let statements: Vec<String> = sql::split_statements(script)
        .into_iter()
        .filter(|s| {
            !s.lines()
                .all(|l| l.trim().is_empty() || l.trim_start().starts_with("--"))
        })
        .collect();

    if statements.is_empty() {
        return Err(FaroError::Other(
            "the file contains no SQL statements".into(),
        ));
    }

    let total = statements.len();
    let mut failed = 0usize;
    let mut errors = Vec::new();

    let dialect = driver.dialect();
    // Only worth a transaction when the caller wants to stop at the first
    // failure — "continue past errors" is a request for partial application, so
    // rolling the whole thing back would be the opposite of what was asked.
    //
    // `ddl_is_transactional` matters as much as `supports_transactions`: a dump
    // is mostly DDL, and on MySQL every `CREATE TABLE` commits implicitly, so
    // opening a transaction there would promise an atomicity it cannot deliver.
    let atomic =
        options.stop_on_error && dialect.supports_transactions() && dialect.ddl_is_transactional();

    if atomic {
        driver
            .execute(
                dialect.begin_statement(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await?;
    }

    for (index, stmt) in statements.iter().enumerate() {
        let result = driver
            .execute(stmt, tokio_util::sync::CancellationToken::new())
            .await;

        if let Err(e) = result {
            failed += 1;
            let message = format!("statement {}: {e}", index + 1);
            if options.stop_on_error {
                let outcome = if atomic {
                    let rolled_back = driver
                        .execute("ROLLBACK", tokio_util::sync::CancellationToken::new())
                        .await
                        .is_ok();
                    if rolled_back {
                        "Nothing was applied — the database is as it was.".to_string()
                    } else {
                        // Say so plainly. Believing a failed restore left no
                        // trace, when it did, is worse than knowing it is
                        // half-applied.
                        format!(
                            "The rollback then failed too, so {index} statements \
                             may remain applied. Check the database before retrying."
                        )
                    }
                } else {
                    format!("{index} of {total} statements had run and remain applied.")
                };
                return Err(FaroError::Other(format!(
                    "{message}\n\nRestore stopped. {outcome}"
                )));
            }
            errors.push(message);
        }

        on_progress(index + 1, total);
    }

    if atomic {
        driver
            .execute("COMMIT", tokio_util::sync::CancellationToken::new())
            .await?;
    }

    Ok(RestoreResult {
        statements: total,
        failed,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::dialect::{hex_bytes_x, paginate_limit_offset, quote_double};
    use crate::model::{ColumnDetail, ForeignKey, IndexInfo};

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

    fn col(name: &str, ty: &str, nullable: bool, default: Option<&str>) -> ColumnDetail {
        ColumnDetail {
            name: name.into(),
            type_name: ty.into(),
            nullable,
            default: default.map(String::from),
            is_primary_key: false,
            ordinal: 0,
        }
    }

    fn detail() -> TableDetail {
        TableDetail {
            table: TableRef {
                schema: None,
                name: "books".into(),
            },
            kind: TableKind::Table,
            columns: vec![
                col("id", "integer", false, None),
                col("title", "varchar(200)", false, None),
                col("created", "timestamptz", true, Some("now()")),
            ],
            primary_key: vec!["id".into()],
            foreign_keys: vec![ForeignKey {
                name: "books_author_fk".into(),
                columns: vec!["author_id".into()],
                referenced_table: TableRef {
                    schema: None,
                    name: "authors".into(),
                },
                referenced_columns: vec!["id".into()],
            }],
            indexes: vec![
                IndexInfo {
                    name: "books_pkey".into(),
                    columns: vec!["id".into()],
                    is_unique: true,
                    is_primary: true,
                    is_constraint: true,
                },
                IndexInfo {
                    name: "books_title_idx".into(),
                    columns: vec!["title".into()],
                    is_unique: false,
                    is_primary: false,
                    is_constraint: false,
                },
            ],
        }
    }

    #[test]
    fn create_table_includes_columns_types_and_key() {
        let sql = create_table_sql(&detail(), &TestDialect);
        assert!(sql.starts_with("CREATE TABLE \"books\" ("), "{sql}");
        assert!(sql.contains(r#""id" integer NOT NULL"#), "{sql}");
        assert!(sql.contains(r#""title" varchar(200) NOT NULL"#), "{sql}");
        assert!(sql.contains(r#"PRIMARY KEY ("id")"#), "{sql}");
        assert!(sql.ends_with(");"), "{sql}");
    }

    #[test]
    fn nullable_columns_omit_not_null() {
        let sql = create_table_sql(&detail(), &TestDialect);
        assert!(
            sql.contains(r#""created" timestamptz DEFAULT (now())"#),
            "{sql}"
        );
        assert!(!sql.contains("timestamptz NOT NULL"), "{sql}");
    }

    #[test]
    fn defaults_stay_expressions_rather_than_becoming_strings() {
        // `now()` is an expression, not a literal — quoting it would turn the
        // default into the string "now()". It is parenthesised instead, which is
        // what both SQLite and Postgres accept back.
        let sql = create_table_sql(&detail(), &TestDialect);
        assert!(sql.contains("DEFAULT (now())"), "{sql}");
        assert!(!sql.contains("DEFAULT 'now()'"), "{sql}");
    }

    #[test]
    fn a_table_without_a_primary_key_omits_the_clause() {
        let mut d = detail();
        d.primary_key.clear();
        let sql = create_table_sql(&d, &TestDialect);
        assert!(!sql.contains("PRIMARY KEY"), "{sql}");
    }

    #[test]
    fn secondary_objects_skip_the_primary_key_index() {
        // The PRIMARY KEY clause already created it; a second CREATE would fail.
        let out = secondary_objects_sql(&detail(), &TestDialect);
        assert!(!out.iter().any(|s| s.contains("books_pkey")), "{out:#?}");
        assert!(
            out.iter().any(|s| s.contains("books_title_idx")),
            "{out:#?}"
        );
    }

    #[test]
    fn secondary_objects_emit_indexes_and_foreign_keys() {
        let out = secondary_objects_sql(&detail(), &TestDialect);
        assert!(
            out.iter()
                .any(|s| s == r#"CREATE INDEX "books_title_idx" ON "books" ("title");"#),
            "{out:#?}"
        );
        assert!(out
            .iter()
            .any(|s| s.contains("ADD CONSTRAINT \"books_author_fk\" FOREIGN KEY")));
        assert!(out
            .iter()
            .any(|s| s.contains(r#"REFERENCES "authors" ("id")"#)));
    }

    #[test]
    fn unique_indexes_are_marked_unique() {
        let mut d = detail();
        d.indexes[1].is_unique = true;
        let out = secondary_objects_sql(&d, &TestDialect);
        assert!(
            out.iter().any(|s| s.starts_with("CREATE UNIQUE INDEX")),
            "{out:#?}"
        );
    }

    fn named(name: &str, refs: &[&str]) -> TableDetail {
        TableDetail {
            table: TableRef {
                schema: None,
                name: name.into(),
            },
            kind: TableKind::Table,
            columns: vec![col("id", "integer", false, None)],
            primary_key: vec!["id".into()],
            foreign_keys: refs
                .iter()
                .map(|r| ForeignKey {
                    name: format!("{name}_{r}_fk"),
                    columns: vec!["id".into()],
                    referenced_table: TableRef {
                        schema: None,
                        name: (*r).into(),
                    },
                    referenced_columns: vec!["id".into()],
                })
                .collect(),
            indexes: vec![],
        }
    }

    fn order_of(details: Vec<TableDetail>) -> Vec<String> {
        order_by_dependency(details)
            .into_iter()
            .map(|d| d.table.name)
            .collect()
    }

    #[test]
    fn dependency_order_puts_parents_before_children() {
        // Alphabetical order puts book_stores before books, which fails on a
        // foreign key during restore. This is the bug the sort exists to fix.
        let out = order_of(vec![
            named("book_stores", &["books"]),
            named("books", &["authors"]),
            named("authors", &[]),
        ]);
        assert_eq!(out, vec!["authors", "books", "book_stores"]);
    }

    #[test]
    fn tables_with_no_relationships_keep_their_order() {
        let out = order_of(vec![named("a", &[]), named("b", &[]), named("c", &[])]);
        assert_eq!(out, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_self_reference_does_not_deadlock_the_sort() {
        // A table cannot be required to precede itself.
        let out = order_of(vec![named("employees", &["employees"])]);
        assert_eq!(out, vec!["employees"]);
    }

    #[test]
    fn references_to_tables_outside_the_dump_are_ignored() {
        // A selective backup should not be reordered by a table it excludes.
        let out = order_of(vec![named("orders", &["customers"])]);
        assert_eq!(out, vec!["orders"]);
    }

    #[test]
    fn a_cycle_still_emits_every_table() {
        // No valid order exists, but silently dropping tables would be far
        // worse than emitting them in an imperfect order.
        let out = order_of(vec![named("a", &["b"]), named("b", &["a"])]);
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"a".to_string()) && out.contains(&"b".to_string()));
    }

    #[test]
    fn a_longer_chain_is_fully_ordered() {
        let out = order_of(vec![
            named("d", &["c"]),
            named("c", &["b"]),
            named("b", &["a"]),
            named("a", &[]),
        ]);
        assert_eq!(out, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn constraint_backed_indexes_are_never_recreated() {
        // SQLite reserves sqlite_autoindex_* names and refuses CREATE INDEX.
        let mut d = detail();
        d.indexes.push(IndexInfo {
            name: "sqlite_autoindex_books_1".into(),
            columns: vec!["title".into()],
            is_unique: true,
            is_primary: false,
            is_constraint: true,
        });
        let out = secondary_objects_sql(&d, &TestDialect);
        assert!(
            !out.iter().any(|s| s.contains("sqlite_autoindex")),
            "{out:#?}"
        );
    }

    #[test]
    fn expression_defaults_are_parenthesised_but_literals_are_not() {
        // SQLite reports `DEFAULT (datetime('now'))` without its parentheses,
        // then rejects the value if they are not put back.
        assert_eq!(wrap_default("datetime('now')"), "(datetime('now'))");
        assert_eq!(
            wrap_default("nextval('s'::regclass)"),
            "(nextval('s'::regclass))"
        );

        // Literals are already valid and stay as they are.
        assert_eq!(wrap_default("1"), "1");
        assert_eq!(wrap_default("-2.5"), "-2.5");
        assert_eq!(wrap_default("'abc'"), "'abc'");
        assert_eq!(wrap_default("NULL"), "NULL");
        assert_eq!(wrap_default("CURRENT_TIMESTAMP"), "CURRENT_TIMESTAMP");
        // Already wrapped, so not wrapped twice.
        assert_eq!(wrap_default("(datetime('now'))"), "(datetime('now'))");
    }

    #[test]
    fn identifiers_are_quoted_throughout_the_ddl() {
        let mut d = detail();
        d.table.name = "we\"ird".into();
        d.columns[0].name = "col\"umn".into();
        d.primary_key = vec!["col\"umn".into()];

        let sql = create_table_sql(&d, &TestDialect);
        assert!(sql.contains(r#""we""ird""#), "{sql}");
        assert!(sql.contains(r#""col""umn""#), "{sql}");
    }
}
