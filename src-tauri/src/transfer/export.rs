//! Writing result sets out to files.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::driver::Dialect;
use crate::error::{FaroError, Result};
use crate::model::{ResultSet, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Csv,
    /// Tab-separated. Useful for pasting straight into a spreadsheet.
    Tsv,
    Json,
    /// `INSERT` statements, for moving data into another database.
    Sql,
    Xlsx,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
            ExportFormat::Xlsx => "xlsx",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    #[serde(default = "default_true")]
    pub include_header: bool,
    /// Table name used in generated `INSERT` statements.
    #[serde(default)]
    pub table_name: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Render a cell for a text-based export.
///
/// NULL becomes an empty field in CSV and TSV, which is the convention every
/// spreadsheet and loader understands. Bytes are hex so the output stays valid
/// text rather than embedding raw binary in a CSV.
fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bytes(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        Value::Json(j) => j.to_string(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(cell_text).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(d) => d.clone(),
        Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::Uuid(s)
        | Value::Unsupported(s) => s.clone(),
    }
}

/// Rows read per query while walking a table.
///
/// Large enough to be efficient, small enough that one query's buffer stays
/// bounded. The accumulated result is not bounded — XLSX and JSON both need the
/// whole set before they can produce a valid file.
const READ_BATCH: u64 = 5_000;

/// Read an entire table into one `ResultSet`, a page at a time.
///
/// Split out of the `export_table` command so the paging can be tested without a
/// Tauri `State`: getting this wrong produces a file that looks complete and is
/// not, which is the kind of bug that has to be caught by a test rather than by
/// a user noticing later.
///
/// Ordering by the primary key is what makes the pages a partition of the table
/// rather than five arbitrary samples of it — see `sql::order_by_key`.
pub async fn read_table_paged(
    driver: &dyn crate::driver::Driver,
    table: &crate::model::TableRef,
) -> Result<ResultSet> {
    let dialect = driver.dialect();
    let qualified = dialect.qualify(table.schema.as_deref(), &table.name);
    let detail = driver.describe_table(table).await?;
    let order = crate::sql::order_by_key(&detail.primary_key, dialect);

    let base = format!("SELECT * FROM {qualified}{order}");
    let mut combined: Option<ResultSet> = None;
    let mut offset = 0u64;

    loop {
        let paged = dialect.paginate(&base, READ_BATCH + 1, offset);
        let page = driver
            .query(
                &paged,
                READ_BATCH,
                tokio_util::sync::CancellationToken::new(),
            )
            .await?;

        let more = page.truncated;
        let count = page.rows.len() as u64;

        match &mut combined {
            None => combined = Some(page),
            Some(acc) => acc.rows.extend(page.rows),
        }

        if !more || count == 0 {
            break;
        }
        offset += count;
    }

    let mut result = combined.unwrap_or(ResultSet {
        columns: vec![],
        rows: vec![],
        truncated: false,
        elapsed_ms: 0,
    });
    // The flag came from the first page, where it meant "more pages follow".
    // Every page has now been read, so the set is complete; leaving it set would
    // tell the caller it was looking at a partial table.
    result.truncated = false;
    Ok(result)
}

pub fn write_file(
    path: &Path,
    result: &ResultSet,
    options: &ExportOptions,
    dialect: Option<&dyn Dialect>,
) -> Result<u64> {
    match options.format {
        ExportFormat::Csv => write_delimited(path, result, options, b','),
        ExportFormat::Tsv => write_delimited(path, result, options, b'\t'),
        ExportFormat::Json => write_json(path, result),
        ExportFormat::Sql => write_sql(path, result, options, dialect),
        ExportFormat::Xlsx => write_xlsx(path, result, options),
    }
}

fn write_delimited(
    path: &Path,
    result: &ResultSet,
    options: &ExportOptions,
    delimiter: u8,
) -> Result<u64> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_path(path)
        .map_err(|e| FaroError::Io(e.to_string()))?;

    if options.include_header {
        writer
            .write_record(result.columns.iter().map(|c| &c.name))
            .map_err(|e| FaroError::Io(e.to_string()))?;
    }

    for row in &result.rows {
        writer
            .write_record(row.iter().map(cell_text))
            .map_err(|e| FaroError::Io(e.to_string()))?;
    }

    writer.flush().map_err(|e| FaroError::Io(e.to_string()))?;
    Ok(result.rows.len() as u64)
}

fn write_json(path: &Path, result: &ResultSet) -> Result<u64> {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = result
        .rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, col) in result.columns.iter().enumerate() {
                let value = row
                    .get(i)
                    .map(json_value)
                    .unwrap_or(serde_json::Value::Null);
                obj.insert(unique_key(&obj, &col.name), value);
            }
            obj
        })
        .collect();

    let file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(std::io::BufWriter::new(file), &rows)?;
    Ok(result.rows.len() as u64)
}

/// JSON representation of a cell.
///
/// Decimals stay strings: emitting them as JSON numbers would round anything
/// past 2^53, which is exactly what the Decimal variant exists to prevent.
fn json_value(value: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::Json(j) => j.clone(),
        Value::Array(items) => J::Array(items.iter().map(json_value).collect()),
        other => J::String(cell_text(other)),
    }
}

/// SQL permits duplicate column names in a result; a JSON object does not.
fn unique_key(obj: &serde_json::Map<String, serde_json::Value>, name: &str) -> String {
    if !obj.contains_key(name) {
        return name.to_string();
    }
    let mut n = 2;
    while obj.contains_key(&format!("{name}_{n}")) {
        n += 1;
    }
    format!("{name}_{n}")
}

fn write_sql(
    path: &Path,
    result: &ResultSet,
    options: &ExportOptions,
    dialect: Option<&dyn Dialect>,
) -> Result<u64> {
    let table = options.table_name.as_deref().unwrap_or("exported_data");
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);

    let quote_ident = |s: &str| match dialect {
        Some(d) => d.quote_ident(s),
        None => format!("\"{}\"", s.replace('"', "\"\"")),
    };

    let columns: Vec<String> = result
        .columns
        .iter()
        .map(|c| quote_ident(&c.name))
        .collect();
    let prefix = format!(
        "INSERT INTO {} ({}) VALUES ",
        quote_ident(table),
        columns.join(", ")
    );

    for row in &result.rows {
        let values: Vec<String> = row
            .iter()
            .map(|v| match dialect {
                Some(d) => d.literal(v),
                // Fall back to SQLite-style byte literals when no connection
                // is in play, e.g. exporting a detached result.
                None => v.to_sql_literal(&crate::driver::dialect::hex_bytes_x),
            })
            .collect();
        writeln!(file, "{prefix}({});", values.join(", "))?;
    }

    file.flush()?;
    Ok(result.rows.len() as u64)
}

fn write_xlsx(path: &Path, result: &ResultSet, options: &ExportOptions) -> Result<u64> {
    use rust_xlsxwriter::{Format, Workbook};

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();

    let mut row_cursor = 0u32;
    if options.include_header {
        let bold = Format::new().set_bold();
        for (i, col) in result.columns.iter().enumerate() {
            sheet
                .write_string_with_format(0, i as u16, &col.name, &bold)
                .map_err(|e| FaroError::Io(e.to_string()))?;
        }
        // Freeze the header so it stays visible while scrolling a long export.
        sheet
            .set_freeze_panes(1, 0)
            .map_err(|e| FaroError::Io(e.to_string()))?;
        row_cursor = 1;
    }

    for (r, row) in result.rows.iter().enumerate() {
        let excel_row = row_cursor + r as u32;
        for (c, value) in row.iter().enumerate() {
            let col = c as u16;
            // Numbers are written as numbers so they sort and sum in the
            // spreadsheet — but decimals stay text, since Excel's f64 storage
            // would silently round the digits Faro worked to preserve.
            match value {
                // Leave the cell genuinely empty rather than writing "" — a
                // blank and an empty string are different things in a
                // spreadsheet, and formulas treat them differently.
                Value::Null => continue,
                Value::Int(i) => sheet.write_number(excel_row, col, *i as f64),
                Value::Float(f) => sheet.write_number(excel_row, col, *f),
                Value::Bool(b) => sheet.write_boolean(excel_row, col, *b),
                other => sheet.write_string(excel_row, col, cell_text(other)),
            }
            .map_err(|e| FaroError::Io(e.to_string()))?;
        }
    }

    workbook
        .save(path)
        .map_err(|e| FaroError::Io(e.to_string()))?;
    Ok(result.rows.len() as u64)
}

/// Suggest a filename for a result, without illegal path characters.
pub fn suggested_filename(base: &str, format: ExportFormat) -> String {
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = if cleaned.trim_matches('_').is_empty() {
        "export"
    } else {
        cleaned.trim_matches('_')
    };
    format!("{stem}.{}", format.extension())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ColumnInfo;

    fn sample() -> ResultSet {
        ResultSet {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    type_name: "int4".into(),
                },
                ColumnInfo {
                    name: "name".into(),
                    type_name: "text".into(),
                },
                ColumnInfo {
                    name: "amount".into(),
                    type_name: "numeric".into(),
                },
            ],
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Text("Ada".into()),
                    Value::Decimal("12345678901234567890.12".into()),
                ],
                vec![Value::Int(2), Value::Null, Value::Null],
                vec![
                    Value::Int(3),
                    Value::Text("O'Brien, \"quoted\"".into()),
                    Value::Float(1.5),
                ],
            ],
            truncated: false,
            elapsed_ms: 1,
        }
    }

    fn opts(format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            include_header: true,
            table_name: Some("people".into()),
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("faro_export_{}_{name}", std::process::id()))
    }

    #[test]
    fn csv_quotes_separators_and_leaves_null_empty() {
        let path = tmp("basic.csv");
        write_file(&path, &sample(), &opts(ExportFormat::Csv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(text.starts_with("id,name,amount\n"));
        // A comma and a quote inside a value must not break the column layout.
        assert!(text.contains(r#""O'Brien, ""quoted""""#), "{text}");
        // NULL is an empty field, which is what loaders expect.
        assert!(text.contains("2,,\n"), "{text}");
    }

    #[test]
    fn csv_can_omit_the_header() {
        let path = tmp("noheader.csv");
        let mut o = opts(ExportFormat::Csv);
        o.include_header = false;
        write_file(&path, &sample(), &o, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(!text.starts_with("id,name"));
        assert!(text.starts_with("1,Ada"));
    }

    #[test]
    fn csv_preserves_decimal_digits() {
        let path = tmp("decimal.csv");
        write_file(&path, &sample(), &opts(ExportFormat::Csv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.contains("12345678901234567890.12"),
            "precision lost: {text}"
        );
    }

    #[test]
    fn tsv_uses_tabs() {
        let path = tmp("basic.tsv");
        write_file(&path, &sample(), &opts(ExportFormat::Tsv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(text.starts_with("id\tname\tamount\n"), "{text}");
    }

    #[test]
    fn json_keeps_decimals_as_strings() {
        let path = tmp("basic.json");
        write_file(&path, &sample(), &opts(ExportFormat::Json), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.len(), 3);
        // A JSON number here would round the value past 2^53.
        assert!(parsed[0]["amount"].is_string(), "{}", parsed[0]);
        assert_eq!(parsed[0]["id"], 1);
        assert!(parsed[1]["name"].is_null());
    }

    #[test]
    fn json_disambiguates_duplicate_column_names() {
        let mut r = sample();
        r.columns[1].name = "id".into();
        let path = tmp("dupes.json");
        write_file(&path, &r, &opts(ExportFormat::Json), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        // Neither column may be dropped just because SQL allowed the collision.
        assert!(parsed[0].get("id").is_some());
        assert!(parsed[0].get("id_2").is_some());
    }

    #[test]
    fn sql_export_escapes_quotes_and_names_the_table() {
        let path = tmp("basic.sql");
        write_file(&path, &sample(), &opts(ExportFormat::Sql), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(
            text.contains(r#"INSERT INTO "people" ("id", "name", "amount")"#),
            "{text}"
        );
        assert!(text.contains("'O''Brien, \"quoted\"'"), "{text}");
        assert!(text.contains("NULL"), "{text}");
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn xlsx_is_written_and_is_a_real_workbook() {
        let path = tmp("basic.xlsx");
        let rows = write_file(&path, &sample(), &opts(ExportFormat::Xlsx), None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(rows, 3);
        // xlsx is a zip; the magic bytes confirm a real file was produced.
        assert_eq!(&bytes[..2], b"PK", "not a zip container");
    }

    #[test]
    fn an_empty_result_still_writes_a_valid_file() {
        let empty = ResultSet {
            columns: sample().columns,
            rows: vec![],
            truncated: false,
            elapsed_ms: 0,
        };
        for format in [
            ExportFormat::Csv,
            ExportFormat::Json,
            ExportFormat::Sql,
            ExportFormat::Xlsx,
        ] {
            let path = tmp(&format!("empty.{}", format.extension()));
            let rows = write_file(&path, &empty, &opts(format), None).unwrap();
            assert_eq!(rows, 0);
            assert!(path.exists(), "{format:?} produced no file");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn suggested_filenames_are_path_safe() {
        assert_eq!(
            suggested_filename("authors", ExportFormat::Csv),
            "authors.csv"
        );
        assert_eq!(
            suggested_filename("public.my table/x", ExportFormat::Json),
            "public_my_table_x.json"
        );
        assert_eq!(suggested_filename("", ExportFormat::Sql), "export.sql");
        assert_eq!(suggested_filename("///", ExportFormat::Csv), "export.csv");
    }

    #[test]
    fn bytes_export_as_hex_rather_than_raw_binary() {
        let r = ResultSet {
            columns: vec![ColumnInfo {
                name: "b".into(),
                type_name: "bytea".into(),
            }],
            rows: vec![vec![Value::Bytes(vec![0xde, 0xad])]],
            truncated: false,
            elapsed_ms: 0,
        };
        let path = tmp("bytes.csv");
        write_file(&path, &r, &opts(ExportFormat::Csv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(text.contains("dead"), "{text}");
    }
}
