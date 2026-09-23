//! Generating `UPDATE`/`INSERT`/`DELETE` from staged grid edits.
//!
//! This is the module that can destroy someone's data, so it is deliberately
//! pure — no connection, no async — and carries the bulk of the test suite.
//!
//! Three rules hold everywhere below:
//!
//! 1. Column names are checked against the table's real columns before they
//!    reach the SQL. Names cannot be parameterized, so an allowlist is the
//!    only thing standing between a crafted name and injection.
//! 2. Every statement is anchored to the full primary key, and carries the
//!    row count it must affect. One row, or the batch is rolled back.
//! 3. A table with no primary key produces no statements at all. There is no
//!    way to address exactly one of its rows.

use crate::driver::Dialect;
use crate::error::{FaroError, Result};
use crate::model::{
    CellEdit, EditValue, GuardedStatement, PendingChange, TableDetail, TableRef, Value,
};

/// Turn staged changes into statements, in an order that is safe to apply.
///
/// Deletes run before inserts so that removing a row and adding one with the
/// same key in a single batch does not collide on a unique index.
pub fn build_statements(
    table: &TableRef,
    detail: &TableDetail,
    changes: &[PendingChange],
    dialect: &dyn Dialect,
) -> Result<Vec<GuardedStatement>> {
    if changes.is_empty() {
        return Ok(vec![]);
    }

    // The central safety check. Without a primary key there is no way to write
    // a WHERE clause that provably matches one row.
    if !detail.is_editable() {
        return Err(FaroError::Other(format!(
            "\"{}\" cannot be edited: it has no primary key, so a row cannot be identified uniquely",
            table.name
        )));
    }

    let mut deletes = Vec::new();
    let mut updates = Vec::new();
    let mut inserts = Vec::new();

    for change in changes {
        match change {
            PendingChange::Delete { key } => {
                deletes.push(build_delete(table, detail, key, dialect)?);
            }
            PendingChange::Update { key, cells } => {
                if let Some(stmt) = build_update(table, detail, key, cells, dialect)? {
                    updates.push(stmt);
                }
            }
            PendingChange::Insert { cells } => {
                inserts.push(build_insert(table, detail, cells, dialect)?);
            }
        }
    }

    let mut out = deletes;
    out.extend(updates);
    out.extend(inserts);
    Ok(out)
}

fn build_update(
    table: &TableRef,
    detail: &TableDetail,
    key: &[CellEdit],
    cells: &[CellEdit],
    dialect: &dyn Dialect,
) -> Result<Option<GuardedStatement>> {
    // An update with nothing to set is not an error — the user may have typed
    // a value and then typed the original back. Emitting `SET` with no
    // assignments would be a syntax error, so drop it.
    if cells.is_empty() {
        return Ok(None);
    }

    let assignments = cells
        .iter()
        .map(|c| {
            let column = column_or_err(detail, &c.column)?;
            Ok(format!(
                "{} = {}",
                dialect.quote_ident(&column.name),
                literal(&c.value, &column.type_name, dialect)
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Some(GuardedStatement {
        sql: format!(
            "UPDATE {} SET {} WHERE {}",
            dialect.qualify(table.schema.as_deref(), &table.name),
            assignments.join(", "),
            where_key(detail, key, dialect)?
        ),
        expect: Some(1),
    }))
}

fn build_delete(
    table: &TableRef,
    detail: &TableDetail,
    key: &[CellEdit],
    dialect: &dyn Dialect,
) -> Result<GuardedStatement> {
    Ok(GuardedStatement {
        sql: format!(
            "DELETE FROM {} WHERE {}",
            dialect.qualify(table.schema.as_deref(), &table.name),
            where_key(detail, key, dialect)?
        ),
        expect: Some(1),
    })
}

fn build_insert(
    table: &TableRef,
    detail: &TableDetail,
    cells: &[CellEdit],
    dialect: &dyn Dialect,
) -> Result<GuardedStatement> {
    // Columns left as Default are omitted so the database supplies its own —
    // which is what makes an auto-increment key work without the user filling
    // it in.
    let supplied: Vec<&CellEdit> = cells
        .iter()
        .filter(|c| c.value != EditValue::Default)
        .collect();

    if supplied.is_empty() {
        // Every column defaulted. The syntax for that differs per engine, and
        // it is almost never what someone meant by "add a row".
        return Err(FaroError::Other(
            "cannot insert a row with no values — fill in at least one column".into(),
        ));
    }

    let mut names = Vec::with_capacity(supplied.len());
    let mut values = Vec::with_capacity(supplied.len());
    for c in supplied {
        let column = column_or_err(detail, &c.column)?;
        names.push(dialect.quote_ident(&column.name));
        values.push(literal(&c.value, &column.type_name, dialect));
    }

    Ok(GuardedStatement {
        sql: format!(
            "INSERT INTO {} ({}) VALUES ({})",
            dialect.qualify(table.schema.as_deref(), &table.name),
            names.join(", "),
            values.join(", ")
        ),
        // Left unchecked: engines vary in what they report for INSERT, and a
        // failed insert raises an error rather than reporting zero rows.
        expect: None,
    })
}

/// Build the `WHERE` that identifies exactly one row.
///
/// Every primary key column must be present. A partial key would match a range
/// of rows, and the row-count guard would catch it only after the statement had
/// already run against them.
fn where_key(detail: &TableDetail, key: &[CellEdit], dialect: &dyn Dialect) -> Result<String> {
    let mut clauses = Vec::with_capacity(detail.primary_key.len());

    for pk_name in &detail.primary_key {
        let cell = key.iter().find(|c| &c.column == pk_name).ok_or_else(|| {
            FaroError::Other(format!(
                "cannot identify the row: primary key column \"{pk_name}\" is missing"
            ))
        })?;

        let column = column_or_err(detail, pk_name)?;
        let quoted = dialect.quote_ident(pk_name);

        clauses.push(match &cell.value {
            // `= NULL` is never true. A NULL in a primary key should be
            // impossible, but if one appears, matching zero rows and tripping
            // the guard is far better than matching everything.
            EditValue::Null => format!("{quoted} IS NULL"),
            EditValue::Default => {
                return Err(FaroError::Other(format!(
                    "primary key column \"{pk_name}\" has no value to match on"
                )))
            }
            v => format!("{quoted} = {}", literal(v, &column.type_name, dialect)),
        });
    }

    Ok(clauses.join(" AND "))
}

/// Look up a column, rejecting anything not actually on the table.
///
/// This allowlist is what makes interpolating a name into SQL safe.
fn column_or_err<'a>(
    detail: &'a TableDetail,
    name: &str,
) -> Result<&'a crate::model::ColumnDetail> {
    detail
        .columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| FaroError::Other(format!("no column named \"{name}\" on this table")))
}

/// Render an edited value as a SQL literal, guided by the column's type.
///
/// Numbers and booleans are emitted unquoted so they compare and store
/// correctly; everything else is quoted text and left to the engine to cast.
/// Text that does not parse as the declared type is still emitted quoted, so
/// the *database* rejects it with a real type error rather than Faro guessing.
pub fn literal(value: &EditValue, type_name: &str, dialect: &dyn Dialect) -> String {
    let text = match value {
        EditValue::Null | EditValue::Default => return "NULL".into(),
        EditValue::Text(t) => t,
    };

    let kind = classify(type_name);
    match kind {
        TypeClass::Integer => match text.trim().parse::<i64>() {
            Ok(n) => n.to_string(),
            Err(_) => dialect.literal(&Value::Text(text.clone())),
        },
        TypeClass::Number => {
            let t = text.trim();
            // Emitted verbatim, not through f64: reparsing a decimal as a
            // float would round it before it ever reached the database.
            if is_numeric_literal(t) {
                t.to_string()
            } else {
                dialect.literal(&Value::Text(text.clone()))
            }
        }
        TypeClass::Boolean => match parse_bool(text) {
            Some(b) => if b { "TRUE" } else { "FALSE" }.into(),
            None => dialect.literal(&Value::Text(text.clone())),
        },
        TypeClass::Text => dialect.literal(&Value::Text(text.clone())),
    }
}

#[derive(Debug, PartialEq)]
enum TypeClass {
    Integer,
    Number,
    Boolean,
    Text,
}

/// Bucket a declared type name.
///
/// Matching is loose and case-insensitive because engines spell these
/// differently (`int4`, `INTEGER`, `BIGINT UNSIGNED`, `NUMERIC(10,2)`), and an
/// unrecognized type falling through to quoted text is the safe outcome.
fn classify(type_name: &str) -> TypeClass {
    let t = type_name.to_ascii_lowercase();

    if t.contains("bool") {
        return TypeClass::Boolean;
    }
    // Checked before the integer arm: "numeric" and "decimal" must not be
    // truncated to an integer, and neither must a float.
    if t.contains("numeric")
        || t.contains("decimal")
        || t.contains("real")
        || t.contains("double")
        || t.contains("float")
        || t.starts_with("money")
    {
        return TypeClass::Number;
    }
    if t.contains("int") || t.starts_with("serial") || t.contains("bigserial") {
        return TypeClass::Integer;
    }
    TypeClass::Text
}

/// Whether text is a plain SQL numeric literal.
///
/// Deliberately strict: no hex, no underscores, no exponent-only forms, and no
/// leading `+`. Anything unusual falls through to a quoted literal and lets the
/// database decide, which is safer than emitting it raw.
fn is_numeric_literal(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
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

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "1" => Some(true),
        "false" | "f" | "no" | "n" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::dialect::{hex_bytes_x, paginate_limit_offset, quote_double};
    use crate::model::{ColumnDetail, TableKind};

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

    fn col(name: &str, type_name: &str, pk: bool) -> ColumnDetail {
        ColumnDetail {
            name: name.into(),
            type_name: type_name.into(),
            nullable: !pk,
            default: None,
            is_primary_key: pk,
            ordinal: 0,
        }
    }

    fn detail(pk: Vec<&str>, columns: Vec<ColumnDetail>) -> TableDetail {
        TableDetail {
            table: TableRef {
                schema: None,
                name: "books".into(),
            },
            kind: TableKind::Table,
            columns,
            primary_key: pk.into_iter().map(String::from).collect(),
            foreign_keys: vec![],
            indexes: vec![],
        }
    }

    fn standard() -> TableDetail {
        detail(
            vec!["id"],
            vec![
                col("id", "int4", true),
                col("title", "text", false),
                col("price", "numeric(10,2)", false),
                col("in_stock", "boolean", false),
            ],
        )
    }

    fn table() -> TableRef {
        TableRef {
            schema: None,
            name: "books".into(),
        }
    }

    fn cell(column: &str, text: &str) -> CellEdit {
        CellEdit {
            column: column.into(),
            value: EditValue::Text(text.into()),
        }
    }

    fn null_cell(column: &str) -> CellEdit {
        CellEdit {
            column: column.into(),
            value: EditValue::Null,
        }
    }

    // -- The central safety property -------------------------------------

    #[test]
    fn a_table_without_a_primary_key_produces_no_statements() {
        let d = detail(vec![], vec![col("a", "int4", false)]);
        let changes = vec![PendingChange::Update {
            key: vec![cell("a", "1")],
            cells: vec![cell("a", "2")],
        }];

        let err = build_statements(&table(), &d, &changes, &TestDialect).unwrap_err();
        assert!(err.to_string().contains("no primary key"), "{err}");
    }

    #[test]
    fn a_view_is_never_editable() {
        let mut d = standard();
        d.kind = TableKind::View;
        let changes = vec![PendingChange::Delete {
            key: vec![cell("id", "1")],
        }];
        assert!(build_statements(&table(), &d, &changes, &TestDialect).is_err());
    }

    #[test]
    fn every_mutation_expects_exactly_one_row() {
        let d = standard();
        let changes = vec![
            PendingChange::Update {
                key: vec![cell("id", "1")],
                cells: vec![cell("title", "x")],
            },
            PendingChange::Delete {
                key: vec![cell("id", "2")],
            },
        ];

        let stmts = build_statements(&table(), &d, &changes, &TestDialect).unwrap();
        for s in &stmts {
            assert_eq!(s.expect, Some(1), "unguarded statement: {}", s.sql);
        }
    }

    #[test]
    fn a_partial_composite_key_is_rejected() {
        // Anchoring on half a key would match a range of rows.
        let d = detail(
            vec!["book_id", "store_id"],
            vec![
                col("book_id", "int4", true),
                col("store_id", "int4", true),
                col("qty", "int4", false),
            ],
        );
        let changes = vec![PendingChange::Update {
            key: vec![cell("book_id", "1")],
            cells: vec![cell("qty", "5")],
        }];

        let err = build_statements(&table(), &d, &changes, &TestDialect).unwrap_err();
        assert!(err.to_string().contains("store_id"), "{err}");
    }

    #[test]
    fn an_unknown_column_is_rejected_rather_than_interpolated() {
        // The allowlist is the injection defence for names, which cannot be
        // bound as parameters.
        let d = standard();
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("title\" = '', evil", "x")],
        }];

        let err = build_statements(&table(), &d, &changes, &TestDialect).unwrap_err();
        assert!(err.to_string().contains("no column named"), "{err}");
    }

    // -- Statement shape ---------------------------------------------------

    #[test]
    fn builds_an_update_anchored_on_the_key() {
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "7")],
            cells: vec![cell("title", "New"), cell("price", "9.99")],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();

        assert_eq!(
            stmts[0].sql,
            r#"UPDATE "books" SET "title" = 'New', "price" = 9.99 WHERE "id" = 7"#
        );
    }

    #[test]
    fn an_update_uses_the_original_key_even_when_the_key_changes() {
        // Editing the primary key itself must still find the row by its old
        // value, or the update silently matches nothing.
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "7")],
            cells: vec![cell("id", "8")],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();
        assert_eq!(
            stmts[0].sql,
            r#"UPDATE "books" SET "id" = 8 WHERE "id" = 7"#
        );
    }

    #[test]
    fn builds_a_composite_key_where_clause() {
        let d = detail(
            vec!["book_id", "store_id"],
            vec![
                col("book_id", "int4", true),
                col("store_id", "int4", true),
                col("qty", "int4", false),
            ],
        );
        let changes = vec![PendingChange::Delete {
            key: vec![cell("book_id", "1"), cell("store_id", "2")],
        }];
        let stmts = build_statements(&table(), &d, &changes, &TestDialect).unwrap();

        assert_eq!(
            stmts[0].sql,
            r#"DELETE FROM "books" WHERE "book_id" = 1 AND "store_id" = 2"#
        );
    }

    #[test]
    fn an_update_with_no_changed_cells_is_dropped() {
        // `SET` with no assignments is a syntax error; typing a value and
        // undoing it should simply produce nothing.
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();
        assert!(stmts.is_empty());
    }

    #[test]
    fn insert_omits_defaulted_columns_so_the_database_supplies_them() {
        let changes = vec![PendingChange::Insert {
            cells: vec![
                CellEdit {
                    column: "id".into(),
                    value: EditValue::Default,
                },
                cell("title", "New book"),
            ],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();

        assert_eq!(
            stmts[0].sql,
            r#"INSERT INTO "books" ("title") VALUES ('New book')"#
        );
        assert_eq!(stmts[0].expect, None);
    }

    #[test]
    fn an_insert_with_every_column_defaulted_is_rejected() {
        let changes = vec![PendingChange::Insert {
            cells: vec![CellEdit {
                column: "id".into(),
                value: EditValue::Default,
            }],
        }];
        assert!(build_statements(&table(), &standard(), &changes, &TestDialect).is_err());
    }

    #[test]
    fn deletes_are_ordered_before_inserts() {
        // Removing a row and adding one with the same key in a single batch
        // must not collide on the unique index.
        let changes = vec![
            PendingChange::Insert {
                cells: vec![cell("title", "new")],
            },
            PendingChange::Delete {
                key: vec![cell("id", "1")],
            },
        ];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();

        assert!(stmts[0].sql.starts_with("DELETE"), "{}", stmts[0].sql);
        assert!(stmts[1].sql.starts_with("INSERT"), "{}", stmts[1].sql);
    }

    #[test]
    fn no_changes_produce_no_statements() {
        let stmts = build_statements(&table(), &standard(), &[], &TestDialect).unwrap();
        assert!(stmts.is_empty());
    }

    // -- NULL handling -----------------------------------------------------

    #[test]
    fn null_is_written_as_the_keyword_not_the_string() {
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![null_cell("title")],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();
        assert!(
            stmts[0].sql.contains(r#""title" = NULL"#),
            "{}",
            stmts[0].sql
        );
    }

    #[test]
    fn an_empty_string_is_not_null() {
        // The distinction the UI forces the user to make must survive to here.
        let changes = vec![PendingChange::Update {
            key: vec![cell("id", "1")],
            cells: vec![cell("title", "")],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();
        assert!(stmts[0].sql.contains(r#""title" = ''"#), "{}", stmts[0].sql);
    }

    #[test]
    fn a_null_key_matches_with_is_null_not_equals() {
        // `= NULL` is never true; IS NULL at least matches zero rows honestly
        // and trips the row-count guard.
        let changes = vec![PendingChange::Delete {
            key: vec![null_cell("id")],
        }];
        let stmts = build_statements(&table(), &standard(), &changes, &TestDialect).unwrap();
        assert_eq!(stmts[0].sql, r#"DELETE FROM "books" WHERE "id" IS NULL"#);
    }

    // -- Literal coercion --------------------------------------------------

    #[test]
    fn integers_are_unquoted() {
        assert_eq!(
            literal(&EditValue::Text("42".into()), "int4", &TestDialect),
            "42"
        );
        assert_eq!(
            literal(&EditValue::Text("-7".into()), "bigint", &TestDialect),
            "-7"
        );
    }

    #[test]
    fn decimals_keep_every_digit() {
        // Round-tripping through f64 would round this before it was written.
        let big = "12345678901234567890.0987654321";
        assert_eq!(
            literal(&EditValue::Text(big.into()), "numeric(40,10)", &TestDialect),
            big
        );
    }

    #[test]
    fn booleans_accept_the_usual_spellings() {
        for t in ["true", "TRUE", "t", "yes", "1"] {
            assert_eq!(
                literal(&EditValue::Text(t.into()), "boolean", &TestDialect),
                "TRUE"
            );
        }
        for f in ["false", "F", "no", "0"] {
            assert_eq!(
                literal(&EditValue::Text(f.into()), "boolean", &TestDialect),
                "FALSE"
            );
        }
    }

    #[test]
    fn text_is_quoted_and_escaped() {
        assert_eq!(
            literal(&EditValue::Text("O'Brien".into()), "text", &TestDialect),
            "'O''Brien'"
        );
    }

    #[test]
    fn a_non_numeric_value_in_a_numeric_column_is_quoted_for_the_database_to_reject() {
        // Faro does not decide this is invalid — it hands the engine something
        // it can refuse with a real type error.
        assert_eq!(
            literal(&EditValue::Text("abc".into()), "int4", &TestDialect),
            "'abc'"
        );
        assert_eq!(
            literal(&EditValue::Text("".into()), "int4", &TestDialect),
            "''"
        );
    }

    #[test]
    fn numeric_literals_reject_anything_unusual() {
        // Emitting these raw would be an injection seam.
        assert!(!is_numeric_literal("1 OR 1=1"));
        assert!(!is_numeric_literal("0x1f"));
        assert!(!is_numeric_literal("1e10"));
        assert!(!is_numeric_literal("1_000"));
        assert!(!is_numeric_literal("--1"));
        assert!(!is_numeric_literal("1.2.3"));
        assert!(!is_numeric_literal(""));
        assert!(!is_numeric_literal("-"));
        assert!(!is_numeric_literal("."));

        assert!(is_numeric_literal("1"));
        assert!(is_numeric_literal("-1.5"));
        assert!(is_numeric_literal(".5"));
    }

    #[test]
    fn an_injection_attempt_in_a_numeric_column_is_quoted_not_inlined() {
        let out = literal(
            &EditValue::Text("1; DROP TABLE books".into()),
            "int4",
            &TestDialect,
        );
        assert_eq!(out, "'1; DROP TABLE books'");
    }

    #[test]
    fn type_classification_is_case_and_suffix_tolerant() {
        assert_eq!(classify("INTEGER"), TypeClass::Integer);
        assert_eq!(classify("int8"), TypeClass::Integer);
        assert_eq!(classify("BIGINT UNSIGNED"), TypeClass::Integer);
        assert_eq!(classify("NUMERIC(10,2)"), TypeClass::Number);
        assert_eq!(classify("double precision"), TypeClass::Number);
        assert_eq!(classify("BOOLEAN"), TypeClass::Boolean);
        assert_eq!(classify("bool"), TypeClass::Boolean);
        assert_eq!(classify("varchar(50)"), TypeClass::Text);
        assert_eq!(classify("timestamptz"), TypeClass::Text);
        // Unknown types fall through to quoted text, which is the safe default.
        assert_eq!(classify("geometry"), TypeClass::Text);
    }

    #[test]
    fn numeric_types_are_not_misread_as_integers() {
        // "numeric" contains no "int", but this guards the ordering of the
        // checks in classify.
        assert_eq!(classify("decimal"), TypeClass::Number);
        assert_eq!(
            literal(&EditValue::Text("1.5".into()), "decimal", &TestDialect),
            "1.5"
        );
    }
}
