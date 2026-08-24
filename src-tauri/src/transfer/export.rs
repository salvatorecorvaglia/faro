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
    /// Neutralize spreadsheet formulas in CSV and TSV output.
    ///
    /// On by default. Off is for feeding the file to another program that
    /// parses it as data, where the added apostrophe would be noise.
    #[serde(default = "default_true")]
    pub sanitize_formulas: bool,
}

fn default_true() -> bool {
    true
}

/// Largest integer magnitude an f64 represents exactly (2^53). Excel has no
/// integer type, so anything beyond this would be silently rounded on write.
const MAX_EXACT_INT: u64 = 1 << 53;

/// Characters that make a spreadsheet treat a cell as a formula rather than
/// text. `\t`, `\r` and `\n` are here because Excel strips leading whitespace
/// before deciding, so " =1+1" is still a formula to it.
const FORMULA_LEAD: [char; 7] = ['=', '+', '-', '@', '\t', '\r', '\n'];

/// Prefix a field with `'` if a spreadsheet would otherwise evaluate it.
///
/// CSV has no types, so Excel, LibreOffice and Sheets decide what a cell means
/// from its first character. A value read out of a database — which Faro does
/// not control and which may have been written by someone else — starting with
/// `=` becomes executable on open: `=HYPERLINK(...)` exfiltrates neighbouring
/// cells, and the legacy DDE forms can launch a process. The apostrophe is the
/// conventional fix; every spreadsheet strips it on display and treats the rest
/// as literal text.
///
/// Deliberately *not* applied to XLSX, where the writer sets a real cell type
/// and `write_string` cannot be reinterpreted as a formula.
fn sanitize_formula(field: String) -> String {
    match field.chars().next() {
        Some(c) if FORMULA_LEAD.contains(&c) => {
            let mut out = String::with_capacity(field.len() + 1);
            out.push('\'');
            out.push_str(&field);
            out
        }
        _ => field,
    }
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
/// bounded. Whether the *pages* accumulate beyond that depends on the caller:
/// `read_table_paged` does, because XLSX and JSON need the whole set before
/// they can produce a valid file; `export_table_streaming` does not, because
/// CSV, TSV and SQL can each be written one row at a time.
const READ_BATCH: u64 = 5_000;

/// Walk a table page by page, in primary-key order, handing each one to
/// `on_page` as it arrives.
///
/// The one place the paging logic — offsets, the truncation flag, the stable
/// ordering that makes pages a partition of the table rather than five
/// arbitrary samples of it (see `sql::order_by_key`) — is implemented, shared
/// by `read_table_paged` (which accumulates the pages) and
/// `export_table_streaming` (which writes each one straight to disk).
/// Getting this wrong produces a file that looks complete and is not, which
/// is the kind of bug that has to be caught by a test rather than by a user
/// noticing later — so it exists in exactly one place either way.
async fn walk_table_pages<F>(
    driver: &dyn crate::driver::Driver,
    table: &crate::model::TableRef,
    cancel: tokio_util::sync::CancellationToken,
    mut on_page: F,
) -> Result<()>
where
    F: FnMut(ResultSet) -> Result<()>,
{
    let dialect = driver.dialect();
    let qualified = dialect.qualify(table.schema.as_deref(), &table.name);
    let detail = driver.describe_table(table).await?;
    let order = crate::sql::stable_order_by(&detail.primary_key, &detail.columns, dialect);
    let base = format!("SELECT * FROM {qualified}{order}");

    let mut offset = 0u64;
    loop {
        let paged = dialect.paginate(&base, READ_BATCH + 1, offset);
        let page = driver.query(&paged, READ_BATCH, cancel.clone()).await?;

        let more = page.truncated;
        let count = page.rows.len() as u64;
        on_page(page)?;

        if !more || count == 0 {
            break;
        }
        offset += count;
    }
    Ok(())
}

/// Read an entire table into one `ResultSet`, a page at a time.
///
/// For XLSX and JSON, which need every row before either can produce a valid
/// file. CSV, TSV and SQL go through `export_table_streaming` instead, which
/// writes each page straight to disk rather than holding the whole table in
/// memory.
pub async fn read_table_paged(
    driver: &dyn crate::driver::Driver,
    table: &crate::model::TableRef,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<ResultSet> {
    let mut combined: Option<ResultSet> = None;
    walk_table_pages(driver, table, cancel, |page| {
        match &mut combined {
            None => combined = Some(page),
            Some(acc) => acc.rows.extend(page.rows),
        }
        Ok(())
    })
    .await?;

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

/// Export an entire table straight to disk, one page at a time — for CSV,
/// TSV and SQL, whose writers already emit one row at a time and never
/// needed the whole table in memory to begin with. `format` on `options`
/// must be one of those three; XLSX and JSON still go through
/// `read_table_paged` + `write_file` (see `export_table` in
/// `commands/transfer.rs`), since producing either genuinely does require
/// every row before the first byte can be written.
pub async fn export_table_streaming(
    driver: &dyn crate::driver::Driver,
    table: &crate::model::TableRef,
    path: &Path,
    options: &ExportOptions,
    dialect: Option<&dyn Dialect>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<u64> {
    let mut sink: Option<TableSink> = None;
    let mut total_rows = 0u64;

    walk_table_pages(driver, table, cancel, |page| {
        if sink.is_none() {
            sink = Some(TableSink::open(path, &page.columns, options, dialect)?);
        }
        let s = sink.as_mut().expect("just initialized above");
        for row in &page.rows {
            s.write_row(row)?;
        }
        total_rows += page.rows.len() as u64;
        Ok(())
    })
    .await?;

    // The loop always runs at least once (even a 0-row table gets one query,
    // for its column list), so the sink was always opened.
    if let Some(s) = sink {
        s.finish()?;
    }
    Ok(total_rows)
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

/// A CSV/TSV destination that can be written to one row at a time.
///
/// Shared by `write_delimited` (a whole `ResultSet` already in hand) and
/// `export_table_streaming` (one page at a time, never accumulated) so the
/// formula-sanitizing and cell-rendering logic exists in exactly one place.
struct DelimitedSink {
    writer: csv::Writer<std::fs::File>,
    sanitize: bool,
}

impl DelimitedSink {
    fn open(
        path: &Path,
        delimiter: u8,
        columns: &[crate::model::ColumnInfo],
        include_header: bool,
        sanitize: bool,
    ) -> Result<Self> {
        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_path(path)
            .map_err(|e| FaroError::Io(e.to_string()))?;

        if include_header {
            // Column names are database-supplied too, so they get the same
            // treatment as the data.
            writer
                .write_record(columns.iter().map(|c| {
                    if sanitize {
                        sanitize_formula(c.name.clone())
                    } else {
                        c.name.clone()
                    }
                }))
                .map_err(|e| FaroError::Io(e.to_string()))?;
        }

        Ok(Self { writer, sanitize })
    }

    fn write_row(&mut self, row: &[Value]) -> Result<()> {
        self.writer
            .write_record(row.iter().map(|v| {
                let text = cell_text(v);
                if self.sanitize {
                    sanitize_formula(text)
                } else {
                    text
                }
            }))
            .map_err(|e| FaroError::Io(e.to_string()))
    }

    fn finish(mut self) -> Result<()> {
        self.writer
            .flush()
            .map_err(|e| FaroError::Io(e.to_string()))
    }
}

fn write_delimited(
    path: &Path,
    result: &ResultSet,
    options: &ExportOptions,
    delimiter: u8,
) -> Result<u64> {
    let mut sink = DelimitedSink::open(
        path,
        delimiter,
        &result.columns,
        options.include_header,
        options.sanitize_formulas,
    )?;
    for row in &result.rows {
        sink.write_row(row)?;
    }
    sink.finish()?;
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

/// A SQL-dump destination that can be written to one row at a time.
///
/// Shared for the same reason as `DelimitedSink`.
struct SqlSink<'a> {
    file: std::io::BufWriter<std::fs::File>,
    prefix: String,
    dialect: Option<&'a dyn Dialect>,
}

impl<'a> SqlSink<'a> {
    fn open(
        path: &Path,
        columns: &[crate::model::ColumnInfo],
        options: &ExportOptions,
        dialect: Option<&'a dyn Dialect>,
    ) -> Result<Self> {
        let table = options.table_name.as_deref().unwrap_or("exported_data");
        let quote_ident = |s: &str| match dialect {
            Some(d) => d.quote_ident(s),
            None => format!("\"{}\"", s.replace('"', "\"\"")),
        };
        let column_list: Vec<String> = columns.iter().map(|c| quote_ident(&c.name)).collect();
        let prefix = format!(
            "INSERT INTO {} ({}) VALUES ",
            quote_ident(table),
            column_list.join(", ")
        );
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        Ok(Self {
            file,
            prefix,
            dialect,
        })
    }

    fn write_row(&mut self, row: &[Value]) -> Result<()> {
        let values: Vec<String> = row
            .iter()
            .map(|v| match self.dialect {
                Some(d) => d.literal(v),
                // Fall back to SQLite-style byte literals and standard string
                // quoting when no connection is in play, e.g. exporting a
                // detached result. A dump written without a dialect is
                // standard SQL by definition, so the standard rule is right.
                None => v.to_sql_literal(
                    &crate::driver::dialect::hex_bytes_x,
                    &crate::model::quote_sql_string,
                ),
            })
            .collect();
        writeln!(self.file, "{}({});", self.prefix, values.join(", "))?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.file.flush()?;
        Ok(())
    }
}

fn write_sql(
    path: &Path,
    result: &ResultSet,
    options: &ExportOptions,
    dialect: Option<&dyn Dialect>,
) -> Result<u64> {
    let mut sink = SqlSink::open(path, &result.columns, options, dialect)?;
    for row in &result.rows {
        sink.write_row(row)?;
    }
    sink.finish()?;
    Ok(result.rows.len() as u64)
}

/// Dispatches to whichever streaming sink `options.format` calls for.
///
/// Only `Csv`, `Tsv` and `Sql` are valid here — `export_table_streaming` is
/// never called for `Json`/`Xlsx`, which read the whole table first instead
/// (see `commands/transfer.rs`).
enum TableSink<'a> {
    Delimited(Box<DelimitedSink>),
    Sql(SqlSink<'a>),
}

impl<'a> TableSink<'a> {
    fn open(
        path: &Path,
        columns: &[crate::model::ColumnInfo],
        options: &ExportOptions,
        dialect: Option<&'a dyn Dialect>,
    ) -> Result<Self> {
        match options.format {
            ExportFormat::Csv => Ok(Self::Delimited(Box::new(DelimitedSink::open(
                path,
                b',',
                columns,
                options.include_header,
                options.sanitize_formulas,
            )?))),
            ExportFormat::Tsv => Ok(Self::Delimited(Box::new(DelimitedSink::open(
                path,
                b'\t',
                columns,
                options.include_header,
                options.sanitize_formulas,
            )?))),
            ExportFormat::Sql => Ok(Self::Sql(SqlSink::open(path, columns, options, dialect)?)),
            ExportFormat::Json | ExportFormat::Xlsx => unreachable!(
                "export_table_streaming is only called for Csv/Tsv/Sql; \
                 {:?} goes through read_table_paged + write_file instead",
                options.format
            ),
        }
    }

    fn write_row(&mut self, row: &[Value]) -> Result<()> {
        match self {
            Self::Delimited(s) => s.write_row(row),
            Self::Sql(s) => s.write_row(row),
        }
    }

    fn finish(self) -> Result<()> {
        match self {
            Self::Delimited(s) => s.finish(),
            Self::Sql(s) => s.finish(),
        }
    }
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
                // Excel stores every number as an f64, so an integer past 2^53
                // cannot be written as a number without losing digits — the
                // same reason decimals stay text just below. A BIGINT id is
                // exactly the value where that matters, so it goes as text.
                Value::Int(i) if i.unsigned_abs() <= MAX_EXACT_INT => {
                    sheet.write_number(excel_row, col, *i as f64)
                }
                Value::Int(i) => sheet.write_string(excel_row, col, i.to_string()),
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
            sanitize_formulas: true,
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

    /// A result whose contents are hostile to a spreadsheet.
    fn formula_sample() -> ResultSet {
        ResultSet {
            columns: vec![ColumnInfo {
                name: "payload".into(),
                type_name: "text".into(),
            }],
            rows: vec![
                vec![Value::Text("=1+1".into())],
                vec![Value::Text(r#"=HYPERLINK("http://evil/","click")"#.into())],
                vec![Value::Text("+1234".into())],
                vec![Value::Text("-1+2".into())],
                vec![Value::Text("@SUM(A1:A9)".into())],
                vec![Value::Text("\t=1+1".into())],
                // Must be left alone: these are ordinary data.
                vec![Value::Text("Ada".into())],
                vec![Value::Text("a=b".into())],
                vec![Value::Null],
            ],
            truncated: false,
            elapsed_ms: 1,
        }
    }

    #[test]
    fn csv_neutralizes_spreadsheet_formulas() {
        let path = tmp("formula.csv");
        write_file(&path, &formula_sample(), &opts(ExportFormat::Csv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let lines: Vec<&str> = text.lines().skip(1).collect();
        // csv quotes a field once it contains the apostrophe-prefixed comma or
        // quote, so assert on the parsed field rather than the raw line.
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(text.as_bytes());
        let fields: Vec<String> = rdr
            .records()
            .map(|r| r.unwrap().get(0).unwrap().to_string())
            .collect();

        assert_eq!(fields[0], "'=1+1");
        assert_eq!(fields[1], r#"'=HYPERLINK("http://evil/","click")"#);
        assert_eq!(fields[2], "'+1234");
        assert_eq!(fields[3], "'-1+2");
        assert_eq!(fields[4], "'@SUM(A1:A9)");
        assert_eq!(fields[5], "'\t=1+1");
        // Untouched.
        assert_eq!(fields[6], "Ada");
        assert_eq!(fields[7], "a=b");
        assert_eq!(fields[8], "");
        assert_eq!(lines.len(), 9);
    }

    #[test]
    fn a_formula_shaped_column_name_is_neutralized_too() {
        let mut rs = formula_sample();
        rs.columns[0].name = "=cmd".into();
        let path = tmp("formula_header.csv");
        write_file(&path, &rs, &opts(ExportFormat::Csv), None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(text.starts_with("'=cmd\n"), "{text}");
    }

    #[test]
    fn formula_sanitizing_can_be_turned_off() {
        let path = tmp("formula_off.csv");
        let options = ExportOptions {
            sanitize_formulas: false,
            ..opts(ExportFormat::Csv)
        };
        write_file(&path, &formula_sample(), &options, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(text.contains("\n=1+1\n"), "{text}");
    }

    #[test]
    fn json_and_sql_exports_are_not_apostrophized() {
        // Only the spreadsheet-facing formats need this; adding an apostrophe
        // to JSON or a SQL literal would corrupt the value.
        let json_path = tmp("formula.json");
        write_file(
            &json_path,
            &formula_sample(),
            &opts(ExportFormat::Json),
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&json_path).unwrap();
        let _ = std::fs::remove_file(&json_path);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[0]["payload"], "=1+1", "JSON value was altered");

        let sql_path = tmp("formula.sql");
        write_file(&sql_path, &formula_sample(), &opts(ExportFormat::Sql), None).unwrap();
        let text = std::fs::read_to_string(&sql_path).unwrap();
        let _ = std::fs::remove_file(&sql_path);
        // The literal must hold `=1+1`, not `'=1+1`.
        assert!(text.contains(r#"VALUES ('=1+1')"#), "{text}");
        assert!(!text.contains(r#"VALUES ('''=1+1')"#), "{text}");
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
