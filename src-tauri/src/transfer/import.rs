//! Reading files into a table.
//!
//! Import is a two-step flow on purpose: parse and preview first, then insert
//! once the user has confirmed the column mapping. Guessing a mapping and
//! writing straight away would put wrong data in the right-looking columns —
//! the kind of mistake that is very hard to notice and very hard to undo.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{FaroError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportFormat {
    Csv,
    Tsv,
    Json,
    Xlsx,
}

impl ImportFormat {
    /// Guess from the file extension, which is all the user gives us.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "csv" => Some(ImportFormat::Csv),
            "tsv" | "tab" => Some(ImportFormat::Tsv),
            "json" => Some(ImportFormat::Json),
            "xlsx" | "xlsm" => Some(ImportFormat::Xlsx),
            _ => None,
        }
    }
}

/// What a file looks like, before anything is written.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    /// Column names from the file's header row.
    pub columns: Vec<String>,
    /// The first rows, for the user to eyeball.
    pub sample_rows: Vec<Vec<String>>,
    /// Rows found, excluding the header. `None` when not counted cheaply.
    pub total_rows: Option<usize>,
    /// Type guessed per column from the sample, purely advisory.
    pub inferred_types: Vec<String>,
}

/// Which file column feeds which table column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMapping {
    /// Index into the file's columns.
    pub source_index: usize,
    /// Name of the table column to write.
    pub target_column: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub format: ImportFormat,
    #[serde(default = "default_true")]
    pub has_header: bool,
    pub mappings: Vec<ColumnMapping>,
    /// Treat these exact strings as NULL rather than as text.
    #[serde(default = "default_null_tokens")]
    pub null_tokens: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Empty and the common spellings tools use for "no value". `NULL` is
/// deliberately not included by default: it is a legitimate string in plenty of
/// datasets, and the user can add it.
fn default_null_tokens() -> Vec<String> {
    vec![String::new()]
}

const SAMPLE_ROWS: usize = 20;

/// Parse a file into columns and rows of text.
///
/// Everything is read as text; coercion to the column's real type happens at
/// insert time, where the declared type is known.
pub fn read_rows(
    path: &Path,
    format: ImportFormat,
    has_header: bool,
) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    match format {
        ImportFormat::Csv => read_delimited(path, b',', has_header),
        ImportFormat::Tsv => read_delimited(path, b'\t', has_header),
        ImportFormat::Json => read_json(path),
        ImportFormat::Xlsx => read_xlsx(path, has_header),
    }
}

pub fn preview(path: &Path, format: ImportFormat, has_header: bool) -> Result<ImportPreview> {
    let (columns, rows) = read_rows(path, format, has_header)?;

    let inferred_types = (0..columns.len())
        .map(|i| infer_type(rows.iter().filter_map(|r| r.get(i).map(String::as_str))))
        .collect();

    Ok(ImportPreview {
        sample_rows: rows.iter().take(SAMPLE_ROWS).cloned().collect(),
        total_rows: Some(rows.len()),
        inferred_types,
        columns,
    })
}

/// Read a file's rows in bounded batches, handing each batch to `sink`.
///
/// The whole-file [`read_rows`] materializes a `Vec<Vec<String>>` for every row
/// before anything is inserted, so importing a large CSV meant holding the file
/// in memory as several million individually-allocated `String`s — far larger
/// than the file itself, and with `panic = "abort"` in release an allocation
/// failure takes the app down rather than surfacing as an error.
///
/// Delimited files stream: `csv::Reader` yields records one at a time, so only
/// `batch` rows are ever live. JSON and XLSX cannot — both formats have to be
/// parsed as a whole document before any row is addressable — so they fall back
/// to reading fully and are handed to `sink` in batches for a uniform caller.
///
/// Returns the column names and the total number of rows passed to `sink`.
pub fn read_rows_batched(
    path: &Path,
    format: ImportFormat,
    has_header: bool,
    batch: usize,
    sink: &mut dyn FnMut(&[Vec<String>]) -> Result<()>,
) -> Result<(Vec<String>, usize)> {
    let delimiter = match format {
        ImportFormat::Csv => Some(b','),
        ImportFormat::Tsv => Some(b'\t'),
        ImportFormat::Json | ImportFormat::Xlsx => None,
    };

    let Some(delimiter) = delimiter else {
        let (columns, rows) = read_rows(path, format, has_header)?;
        let total = rows.len();
        for chunk in rows.chunks(batch.max(1)) {
            sink(chunk)?;
        }
        return Ok((columns, total));
    };

    let (columns, mut reader) = delimited_reader(path, delimiter, has_header)?;

    let mut buffer: Vec<Vec<String>> = Vec::with_capacity(batch.max(1));
    let mut total = 0usize;
    for record in reader.records() {
        let record = record.map_err(|e| FaroError::Io(e.to_string()))?;
        buffer.push(record.iter().map(String::from).collect());
        if buffer.len() >= batch.max(1) {
            total += buffer.len();
            sink(&buffer)?;
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        total += buffer.len();
        sink(&buffer)?;
    }

    Ok((columns, total))
}

/// Open a delimited file and resolve its column names, leaving the reader
/// positioned at the first data record.
///
/// Shared by the whole-file and batched readers so the header handling — which
/// has to re-open the file when there is no header row — cannot drift apart.
fn delimited_reader(
    path: &Path,
    delimiter: u8,
    has_header: bool,
) -> Result<(Vec<String>, csv::Reader<std::fs::File>)> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        // Ragged rows are common in hand-edited files; rejecting the whole
        // import over one short line helps nobody.
        .flexible(true)
        .from_path(path)
        .map_err(|e| FaroError::Io(e.to_string()))?;

    let columns: Vec<String> = if has_header {
        reader
            .headers()
            .map_err(|e| FaroError::Io(e.to_string()))?
            .iter()
            .map(String::from)
            .collect()
    } else {
        // Without a header, name columns positionally so they can still be
        // mapped.
        let width = reader
            .records()
            .next()
            .and_then(|r| r.ok())
            .map(|r| r.len())
            .unwrap_or(0);
        // Re-open: the peek above consumed a record.
        reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_path(path)
            .map_err(|e| FaroError::Io(e.to_string()))?;
        (1..=width).map(|i| format!("Column {i}")).collect()
    };

    Ok((columns, reader))
}

fn read_delimited(
    path: &Path,
    delimiter: u8,
    has_header: bool,
) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let (columns, mut reader) = delimited_reader(path, delimiter, has_header)?;

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| FaroError::Io(e.to_string()))?;
        rows.push(record.iter().map(String::from).collect());
    }

    Ok((columns, rows))
}

fn read_json(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let text = std::fs::read_to_string(path)?;
    let parsed: serde_json::Value = serde_json::from_str(&text)?;

    let array = parsed.as_array().ok_or_else(|| {
        FaroError::Other("expected the JSON file to contain an array of objects".into())
    })?;

    // Union of keys across all objects, in first-seen order, so a row missing
    // an optional field does not drop that column for everyone.
    let mut columns: Vec<String> = Vec::new();
    for item in array {
        if let Some(obj) = item.as_object() {
            for key in obj.keys() {
                if !columns.iter().any(|c| c == key) {
                    columns.push(key.clone());
                }
            }
        }
    }

    let rows = array
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|key| match item.get(key) {
                    None | Some(serde_json::Value::Null) => String::new(),
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                })
                .collect()
        })
        .collect();

    Ok((columns, rows))
}

fn read_xlsx(path: &Path, has_header: bool) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    use calamine::{open_workbook_auto, Reader};

    let mut workbook = open_workbook_auto(path)
        .map_err(|e| FaroError::Io(format!("could not open workbook: {e}")))?;

    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| FaroError::Other("the workbook has no sheets".into()))?;

    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|e| FaroError::Io(format!("could not read sheet: {e}")))?;

    let mut iter = range.rows();
    let columns: Vec<String> = if has_header {
        iter.next()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .unwrap_or_default()
    } else {
        let width = range.width();
        (1..=width).map(|i| format!("Column {i}")).collect()
    };

    let rows = iter
        .map(|row| row.iter().map(|c| c.to_string()).collect())
        .collect();

    Ok((columns, rows))
}

/// Guess a column's type from its values. Advisory only — shown in the preview
/// so the user can spot a bad mapping, never used to transform data.
fn infer_type<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut seen = false;
    let mut all_int = true;
    let mut all_number = true;
    let mut all_bool = true;

    for v in values.take(200) {
        let t = v.trim();
        if t.is_empty() {
            continue;
        }
        seen = true;

        if t.parse::<i64>().is_err() {
            all_int = false;
        }
        if t.parse::<f64>().is_err() {
            all_number = false;
        }
        if !matches!(
            t.to_ascii_lowercase().as_str(),
            "true" | "false" | "t" | "f" | "yes" | "no" | "0" | "1"
        ) {
            all_bool = false;
        }
    }

    if !seen {
        return "empty".into();
    }
    // Checked before integer: a column of only 0s and 1s is more likely a flag
    // than a number, and the label is only a hint either way.
    if all_bool {
        return "boolean".into();
    }
    if all_int {
        return "integer".into();
    }
    if all_number {
        return "number".into();
    }
    "text".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("faro_import_{}_{name}", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    /// Collect every batch `read_rows_batched` produces, with its size.
    fn batches(
        path: &std::path::Path,
        format: ImportFormat,
        has_header: bool,
        batch: usize,
    ) -> (Vec<String>, Vec<usize>, Vec<Vec<String>>) {
        let mut sizes = Vec::new();
        let mut all = Vec::new();
        let (columns, total) = read_rows_batched(path, format, has_header, batch, &mut |chunk| {
            sizes.push(chunk.len());
            all.extend(chunk.iter().cloned());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            total,
            all.len(),
            "reported total disagrees with what was sent"
        );
        (columns, sizes, all)
    }

    #[test]
    fn batched_reading_bounds_each_chunk() {
        // The point of the batched reader: never hold more than `batch` parsed
        // rows at once, however large the file is.
        let body: String = (1..=10).map(|i| format!("{i},name{i}\n")).collect();
        let path = write_temp("batched.csv", &format!("id,name\n{body}"));

        let (columns, sizes, rows) = batches(&path, ImportFormat::Csv, true, 3);

        assert_eq!(columns, ["id", "name"]);
        assert_eq!(sizes, [3, 3, 3, 1], "chunks were not bounded at 3");
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], ["1", "name1"]);
        assert_eq!(rows[9], ["10", "name10"]);
    }

    #[test]
    fn batched_reading_agrees_with_reading_the_whole_file() {
        // The two paths must not drift: the batched one is what imports use.
        let path = write_temp("batched_same.csv", "id,name\n1,a\n2,b\n3,c\n");

        let (whole_columns, whole_rows) = read_rows(&path, ImportFormat::Csv, true).unwrap();
        let (columns, _, rows) = batches(&path, ImportFormat::Csv, true, 2);

        assert_eq!(columns, whole_columns);
        assert_eq!(rows, whole_rows);
    }

    #[test]
    fn batched_reading_names_columns_positionally_without_a_header() {
        // The header-less path re-opens the file, which is the fiddly bit the
        // whole-file and batched readers now share rather than duplicate.
        let path = write_temp("batched_noheader.csv", "1,a\n2,b\n3,c\n");

        let (columns, _, rows) = batches(&path, ImportFormat::Csv, false, 2);

        assert_eq!(columns, ["Column 1", "Column 2"]);
        assert_eq!(rows.len(), 3, "the first row must not be eaten as a header");
        assert_eq!(rows[0], ["1", "a"]);
    }

    #[test]
    fn batched_reading_handles_a_format_that_cannot_stream() {
        // JSON has to be parsed whole; it is still delivered in batches so the
        // caller has one shape to handle.
        let path = write_temp(
            "batched.json",
            r#"[{"id":1},{"id":2},{"id":3},{"id":4},{"id":5}]"#,
        );

        let (columns, sizes, rows) = batches(&path, ImportFormat::Json, true, 2);

        assert_eq!(columns, ["id"]);
        assert_eq!(sizes, [2, 2, 1]);
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn a_sink_error_stops_the_read() {
        // A failure building statements must abort rather than carry on
        // consuming the rest of a very large file.
        let body: String = (1..=100).map(|i| format!("{i}\n")).collect();
        let path = write_temp("batched_err.csv", &format!("id\n{body}"));

        let mut seen = 0usize;
        let result = read_rows_batched(&path, ImportFormat::Csv, true, 10, &mut |chunk| {
            seen += chunk.len();
            Err(FaroError::Other("stop".into()))
        });

        assert!(result.is_err());
        assert_eq!(seen, 10, "read continued past the failing batch");
    }

    #[test]
    fn detects_format_from_the_extension() {
        assert_eq!(
            ImportFormat::from_path(Path::new("a.csv")),
            Some(ImportFormat::Csv)
        );
        assert_eq!(
            ImportFormat::from_path(Path::new("a.TSV")),
            Some(ImportFormat::Tsv)
        );
        assert_eq!(
            ImportFormat::from_path(Path::new("a.json")),
            Some(ImportFormat::Json)
        );
        assert_eq!(
            ImportFormat::from_path(Path::new("a.xlsx")),
            Some(ImportFormat::Xlsx)
        );
        assert_eq!(ImportFormat::from_path(Path::new("a.txt")), None);
        assert_eq!(ImportFormat::from_path(Path::new("noext")), None);
    }

    #[test]
    fn reads_a_csv_with_a_header() {
        let path = write_temp("basic.csv", "id,name\n1,Ada\n2,Grace\n");
        let (cols, rows) = read_rows(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(cols, vec!["id", "name"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1", "Ada"]);
    }

    #[test]
    fn reads_a_csv_without_a_header() {
        let path = write_temp("noheader.csv", "1,Ada\n2,Grace\n");
        let (cols, rows) = read_rows(&path, ImportFormat::Csv, false).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(cols, vec!["Column 1", "Column 2"]);
        // Every line is data when there is no header — none may be swallowed.
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn csv_quoting_and_embedded_separators_survive() {
        let path = write_temp("quoted.csv", "id,name\n1,\"O'Brien, Ken\"\n");
        let (_, rows) = read_rows(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(rows[0][1], "O'Brien, Ken");
    }

    #[test]
    fn a_ragged_row_does_not_abort_the_import() {
        let path = write_temp("ragged.csv", "a,b,c\n1,2,3\n4,5\n");
        let (_, rows) = read_rows(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].len(), 2);
    }

    #[test]
    fn reads_tab_separated_files() {
        let path = write_temp("basic.tsv", "id\tname\n1\tAda\n");
        let (cols, rows) = read_rows(&path, ImportFormat::Tsv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(cols, vec!["id", "name"]);
        assert_eq!(rows[0], vec!["1", "Ada"]);
    }

    #[test]
    fn reads_json_objects() {
        let path = write_temp(
            "basic.json",
            r#"[{"id": 1, "name": "Ada"}, {"id": 2, "name": null}]"#,
        );
        let (cols, rows) = read_rows(&path, ImportFormat::Json, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(cols, vec!["id", "name"]);
        assert_eq!(rows[0], vec!["1", "Ada"]);
        // JSON null becomes an empty field, matching the CSV convention.
        assert_eq!(rows[1][1], "");
    }

    #[test]
    fn json_columns_are_the_union_across_rows() {
        // A field present on only some objects must still get a column, or its
        // data would silently vanish.
        let path = write_temp("sparse.json", r#"[{"a": 1}, {"a": 2, "b": 3}]"#);
        let (cols, rows) = read_rows(&path, ImportFormat::Json, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(cols, vec!["a", "b"]);
        assert_eq!(rows[0], vec!["1", ""]);
        assert_eq!(rows[1], vec!["2", "3"]);
    }

    #[test]
    fn json_that_is_not_an_array_is_rejected_clearly() {
        let path = write_temp("object.json", r#"{"a": 1}"#);
        let err = read_rows(&path, ImportFormat::Json, true).unwrap_err();
        let _ = std::fs::remove_file(&path);

        assert!(err.to_string().contains("array of objects"), "{err}");
    }

    #[test]
    fn preview_reports_counts_and_a_sample() {
        let mut csv = String::from("id,name\n");
        for i in 0..50 {
            csv.push_str(&format!("{i},Name {i}\n"));
        }
        let path = write_temp("big.csv", &csv);
        let p = preview(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(p.columns, vec!["id", "name"]);
        assert_eq!(p.total_rows, Some(50));
        assert_eq!(p.sample_rows.len(), SAMPLE_ROWS);
    }

    #[test]
    fn type_inference_labels_the_obvious_cases() {
        assert_eq!(infer_type(["1", "2", "3"].into_iter()), "integer");
        assert_eq!(infer_type(["1.5", "2"].into_iter()), "number");
        assert_eq!(infer_type(["true", "false"].into_iter()), "boolean");
        assert_eq!(infer_type(["Ada", "Grace"].into_iter()), "text");
        assert_eq!(infer_type(["", ""].into_iter()), "empty");
    }

    #[test]
    fn type_inference_ignores_blanks_but_not_mixed_content() {
        assert_eq!(infer_type(["1", "", "2"].into_iter()), "integer");
        // One non-numeric value makes the whole column text.
        assert_eq!(infer_type(["1", "abc"].into_iter()), "text");
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_empty_import() {
        let path = std::env::temp_dir().join("faro_definitely_not_here.csv");
        assert!(read_rows(&path, ImportFormat::Csv, true).is_err());
    }
}
