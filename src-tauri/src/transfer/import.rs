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

/// Rows read before the preview stops.
///
/// Enough to fill the sample table and to feed [`infer_type`], which looks at
/// 200 values itself. Reading further only refines a label the preview already
/// calls advisory.
const PREVIEW_ROWS: usize = 200;

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
        ImportFormat::Csv => read_delimited(path, sniff_delimiter(path)?, has_header),
        ImportFormat::Tsv => read_delimited(path, b'\t', has_header),
        ImportFormat::Json => read_json(path),
        ImportFormat::Xlsx => read_xlsx(path, has_header),
    }
}

/// Look at the head of a file: its columns, its first rows, and a guess at each
/// column's type.
///
/// Reads only [`PREVIEW_ROWS`], not the whole file. It used to call `read_rows`,
/// which materializes every row — so previewing a multi-gigabyte CSV, the exact
/// case the batched reader exists for, allocated the whole thing to show twenty
/// lines of it.
///
/// The cost is that `total_rows` is only known when the file turned out to be
/// short enough to finish. `None` means "not counted", which is what the field
/// has always meant, and the UI already words itself around it.
pub fn preview(path: &Path, format: ImportFormat, has_header: bool) -> Result<ImportPreview> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut complete = true;

    let (columns, _) = read_rows_batched(path, format, has_header, SAMPLE_ROWS, &mut |chunk| {
        rows.extend(chunk.iter().cloned());
        if rows.len() >= PREVIEW_ROWS {
            complete = false;
            return Ok(false);
        }
        Ok(true)
    })?;

    let inferred_types = (0..columns.len())
        .map(|i| infer_type(rows.iter().filter_map(|r| r.get(i).map(String::as_str))))
        .collect();

    Ok(ImportPreview {
        sample_rows: rows.iter().take(SAMPLE_ROWS).cloned().collect(),
        total_rows: complete.then_some(rows.len()),
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
/// `batch` rows are ever live. JSON scans the schema and then streams rows in
/// a second pass. XLSX still materializes a bounded worksheet, so it falls back
/// to reading fully and are handed to `sink` in batches for a uniform caller.
///
/// `sink` returns `false` to stop reading — what the preview uses to look at
/// the first rows of a file without paying for the rest of it. Returning an
/// error stops it too, but says something went wrong; the two are different
/// outcomes and the caller needs to tell them apart.
///
/// Returns the column names and the total number of rows passed to `sink`.
pub fn read_rows_batched(
    path: &Path,
    format: ImportFormat,
    has_header: bool,
    batch: usize,
    sink: &mut dyn FnMut(&[Vec<String>]) -> Result<bool>,
) -> Result<(Vec<String>, usize)> {
    read_rows_batched_cancelled(
        path,
        format,
        has_header,
        batch,
        sink,
        &tokio_util::sync::CancellationToken::new(),
    )
}

pub fn read_rows_batched_cancelled(
    path: &Path,
    format: ImportFormat,
    has_header: bool,
    batch: usize,
    sink: &mut dyn FnMut(&[Vec<String>]) -> Result<bool>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Vec<String>, usize)> {
    if cancel.is_cancelled() {
        return Err(FaroError::Cancelled);
    }
    if matches!(format, ImportFormat::Json) {
        return read_json_batched(path, batch, sink, cancel);
    }
    let delimiter = match format {
        ImportFormat::Csv => Some(sniff_delimiter(path)?),
        ImportFormat::Tsv => Some(b'\t'),
        ImportFormat::Json | ImportFormat::Xlsx => None,
    };

    let Some(delimiter) = delimiter else {
        let (columns, rows) = read_rows(path, format, has_header)?;
        let mut total = 0usize;
        for chunk in rows.chunks(batch.max(1)) {
            total += chunk.len();
            if !sink(chunk)? {
                break;
            }
        }
        return Ok((columns, total));
    };

    let (columns, mut reader) = delimited_reader(path, delimiter, has_header)?;

    let mut buffer: Vec<Vec<String>> = Vec::with_capacity(batch.max(1));
    let mut total = 0usize;
    let mut batch_bytes = 0usize;
    for record in reader.records() {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        let record = record.map_err(|e| FaroError::Io(e.to_string()))?;
        let bytes = record
            .iter()
            .map(|value| 32 + value.len() * 2)
            .sum::<usize>();
        if bytes > crate::budget::RESULT_BYTES {
            return Err(crate::budget::exceeded("A delimited row"));
        }
        if !buffer.is_empty() && batch_bytes.saturating_add(bytes) > crate::budget::RESULT_BYTES {
            total += buffer.len();
            if !sink(&buffer)? {
                return Ok((columns, total));
            }
            buffer.clear();
            batch_bytes = 0;
        }
        batch_bytes += bytes;
        buffer.push(record.iter().map(String::from).collect());
        if buffer.len() >= batch.max(1) {
            total += buffer.len();
            let keep_going = sink(&buffer)?;
            buffer.clear();
            batch_bytes = 0;
            if !keep_going {
                return Ok((columns, total));
            }
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

/// Work out which character separates a `.csv` file's fields.
///
/// "CSV" is not one format. Excel writes `;` wherever the decimal separator is
/// a comma — Italy, Germany, France and most of Europe — and reading such a file
/// with `,` produced one column per row, holding the whole line. The first line
/// decides: whichever of `,` `;` tab `|` occurs most often outside quotes, with
/// `,` kept on a tie or when none appears.
fn sniff_delimiter(path: &Path) -> Result<u8> {
    use std::io::Read;

    // A header line is short; this bounds the read for a file with none.
    const SNIFF_BYTES: u64 = 64 * 1024;
    let mut head = Vec::new();
    std::fs::File::open(path)?
        .take(SNIFF_BYTES)
        .read_to_end(&mut head)?;
    Ok(delimiter_of(&head))
}

/// The delimiter a line of text uses; see [`sniff_delimiter`].
fn delimiter_of(head: &[u8]) -> u8 {
    const CANDIDATES: &[u8] = b",;\t|";
    let mut counts = [0usize; CANDIDATES.len()];
    let mut in_quotes = false;
    for &b in head {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'\n' | b'\r' if !in_quotes => break,
            _ if !in_quotes => {
                if let Some(i) = CANDIDATES.iter().position(|&c| c == b) {
                    counts[i] += 1;
                }
            }
            _ => {}
        }
    }
    // `max_by_key` keeps the last maximum, so iterate in reverse to let the
    // earliest candidate — the comma — win a tie.
    (0..CANDIDATES.len())
        .rev()
        .max_by_key(|&i| counts[i])
        .filter(|&i| counts[i] > 0)
        .map(|i| CANDIDATES[i])
        .unwrap_or(b',')
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

/// Parse a JSON array one value at a time. Preserve sink errors verbatim.
fn visit_json(path: &Path, sink: &mut dyn FnMut(serde_json::Value) -> Result<bool>) -> Result<()> {
    use serde::de::{DeserializeSeed, SeqAccess, Visitor};
    struct ArrayVisitor<'a> {
        sink: &'a mut dyn FnMut(serde_json::Value) -> Result<bool>,
        failure: &'a mut Option<FaroError>,
        stopped: &'a mut bool,
    }
    impl<'de> Visitor<'de> for ArrayVisitor<'_> {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a JSON array of objects")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<(), A::Error> {
            while let Some(value) = sequence.next_element::<serde_json::Value>()? {
                if crate::budget::json_bytes(&value) > crate::budget::RESULT_BYTES {
                    *self.failure = Some(crate::budget::exceeded("A JSON row"));
                    return Err(serde::de::Error::custom("JSON row exceeds memory budget"));
                }
                match (self.sink)(value) {
                    Ok(true) => {}
                    Ok(false) => {
                        *self.stopped = true;
                        return Err(serde::de::Error::custom("preview complete"));
                    }
                    Err(error) => {
                        *self.failure = Some(error);
                        return Err(serde::de::Error::custom("JSON sink failed"));
                    }
                }
            }
            Ok(())
        }
    }
    impl<'de> DeserializeSeed<'de> for ArrayVisitor<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> std::result::Result<(), D::Error> {
            deserializer.deserialize_seq(self)
        }
    }
    let mut failure = None;
    let mut stopped = false;
    let mut deserializer =
        serde_json::Deserializer::from_reader(std::io::BufReader::new(std::fs::File::open(path)?));
    let result = ArrayVisitor {
        sink,
        failure: &mut failure,
        stopped: &mut stopped,
    }
    .deserialize(&mut deserializer);
    if let Some(error) = failure {
        return Err(error);
    }
    if stopped {
        return Ok(());
    }
    result.map_err(|error| {
        FaroError::Other(format!(
            "expected the JSON file to contain an array of objects: {error}"
        ))
    })?;
    deserializer.end()?;
    Ok(())
}

fn read_json_batched(
    path: &Path,
    batch: usize,
    sink: &mut dyn FnMut(&[Vec<String>]) -> Result<bool>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Vec<String>, usize)> {
    let mut columns = Vec::<String>::new();
    let mut seen = std::collections::HashSet::new();
    let mut column_bytes = 0usize;
    visit_json(path, &mut |value| {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        if let Some(object) = value.as_object() {
            for key in object.keys() {
                if seen.insert(key.clone()) {
                    column_bytes = column_bytes.saturating_add(key.len() * 4 + 64);
                    if column_bytes > crate::budget::RESULT_BYTES || columns.len() >= 10_000 {
                        return Err(crate::budget::exceeded("The JSON column list"));
                    }
                    columns.push(key.clone());
                }
            }
        }
        Ok(true)
    })?;
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut total = 0usize;
    let mut stopped = false;
    visit_json(path, &mut |value| {
        if cancel.is_cancelled() {
            return Err(FaroError::Cancelled);
        }
        let row: Vec<String> = columns
            .iter()
            .map(|key| match value.get(key) {
                None | Some(serde_json::Value::Null) => String::new(),
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(value) => value.to_string(),
            })
            .collect();
        let size = row.iter().map(|value| 32 + value.len() * 2).sum::<usize>();
        if size > crate::budget::RESULT_BYTES {
            return Err(crate::budget::exceeded("A JSON row"));
        }
        if !rows.is_empty() && bytes.saturating_add(size) > crate::budget::RESULT_BYTES {
            total += rows.len();
            if !sink(&rows)? {
                stopped = true;
                return Ok(false);
            }
            rows.clear();
            bytes = 0;
        }
        bytes += size;
        rows.push(row);
        if rows.len() >= batch.max(1) {
            total += rows.len();
            if !sink(&rows)? {
                stopped = true;
                return Ok(false);
            }
            rows.clear();
            bytes = 0;
        }
        Ok(true)
    })?;
    if !stopped && !rows.is_empty() {
        total += rows.len();
        sink(&rows)?;
    }
    Ok((columns, total))
}

fn read_json(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let bytes = crate::budget::read_materialized(path)?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes)?;

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
    use calamine::{open_workbook, Reader, Xlsx};

    if std::fs::metadata(path)?.len() > crate::budget::MATERIALIZED_BYTES as u64 {
        return Err(crate::budget::exceeded("This workbook"));
    }
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)
        .map_err(|e| FaroError::Io(e.to_string()))?;
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        expanded = expanded.saturating_add(
            archive
                .by_index(index)
                .map_err(|e| FaroError::Io(e.to_string()))?
                .size(),
        );
        if expanded > crate::budget::MATERIALIZED_BYTES as u64 {
            return Err(crate::budget::exceeded("This workbook's expanded contents"));
        }
    }
    drop(archive);
    let mut workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| FaroError::Io(format!("could not open workbook: {e}")))?;

    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| FaroError::Other("the workbook has no sheets".into()))?;

    {
        let mut cells = workbook
            .worksheet_cells_reader(&sheet_name)
            .map_err(|error| FaroError::Io(error.to_string()))?;
        let mut bytes = 0usize;
        let mut first = (u32::MAX, u32::MAX);
        let mut last = (0u32, 0u32);
        while let Some(cell) = cells
            .next_cell()
            .map_err(|error| FaroError::Io(error.to_string()))?
        {
            let (row, column) = cell.get_position();
            first = (first.0.min(row), first.1.min(column));
            last = (last.0.max(row), last.1.max(column));
            let size = match cell.get_value() {
                calamine::DataRef::SharedString(text) => text.len() * 2,
                calamine::DataRef::String(text)
                | calamine::DataRef::DateTimeIso(text)
                | calamine::DataRef::DurationIso(text) => text.len() * 2,
                _ => 32,
            };
            bytes = bytes.saturating_add(64 + size);
            if bytes > crate::budget::MATERIALIZED_BYTES {
                return Err(crate::budget::exceeded("The expanded workbook cells"));
            }
        }
        if first.0 != u32::MAX {
            let dense = (last.0 - first.0 + 1) as u64 * (last.1 - first.1 + 1) as u64;
            if dense > (crate::budget::MATERIALIZED_BYTES / 64) as u64 {
                return Err(crate::budget::exceeded("The worksheet dimensions"));
            }
        }
    }
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
            Ok(true)
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
    fn sparse_workbook_dimensions_are_checked_before_dense_allocation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sparse.xlsx");
        let mut workbook = rust_xlsxwriter::Workbook::new();
        let sheet = workbook.add_worksheet();
        sheet.write_string(0, 0, "header").unwrap();
        sheet.write_string(999_999, 9999, "end").unwrap();
        workbook.save(&path).unwrap();
        assert!(matches!(
            read_rows(&path, ImportFormat::Xlsx, true),
            Err(FaroError::ResourceLimit(_))
        ));
    }

    #[test]
    fn streamed_json_preserves_sink_errors_and_can_stop_early() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rows.json");
        std::fs::write(&path, r#"[{"a":1},{"b":2},{"a":3}]"#).unwrap();
        let error = read_rows_batched(&path, ImportFormat::Json, true, 1, &mut |_| {
            Err(FaroError::Cancelled)
        })
        .unwrap_err();
        assert!(matches!(error, FaroError::Cancelled));
        let mut calls = 0;
        let (columns, rows) = read_rows_batched(&path, ImportFormat::Json, true, 1, &mut |_| {
            calls += 1;
            Ok(false)
        })
        .unwrap();
        assert_eq!(columns, vec!["a", "b"]);
        assert_eq!(rows, 1);
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_semicolon_separated_csv_is_read_as_columns() {
        // What Excel writes in Italy, Germany and most of Europe. Read with a
        // comma, each line came back as a single column holding all of it —
        // and a decimal comma inside a value would have split it.
        let path = write_temp("semicolon.csv", "id;price;name\n1;9,99;Ada\n2;0,5;Grace\n");
        let (columns, rows) = read_rows(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(columns, ["id", "price", "name"]);
        assert_eq!(rows[0], ["1", "9,99", "Ada"]);

        // The batched reader, which imports actually use, agrees.
        let path = write_temp("semicolon_batched.csv", "id;name\n1;Ada\n");
        let (columns, _, rows) = batches(&path, ImportFormat::Csv, true, 10);
        let _ = std::fs::remove_file(&path);
        assert_eq!(columns, ["id", "name"]);
        assert_eq!(rows[0], ["1", "Ada"]);
    }

    #[test]
    fn the_delimiter_is_judged_from_the_first_line_outside_quotes() {
        assert_eq!(delimiter_of(b"a,b,c\n1;2;3;4;5"), b',');
        assert_eq!(delimiter_of(b"a;b;c\n"), b';');
        assert_eq!(delimiter_of(b"a\tb\tc"), b'\t');
        assert_eq!(delimiter_of(b"a|b|c\r\n"), b'|');
        // Separators inside a quoted header name do not count.
        assert_eq!(delimiter_of(b"\"x;y;z\",b,c\n"), b',');
        // A tie, or nothing at all, keeps the comma.
        assert_eq!(delimiter_of(b"a,b;c\n"), b',');
        assert_eq!(delimiter_of(b"single\n"), b',');
        assert_eq!(delimiter_of(b""), b',');
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
        // Short enough to have been read to the end, so the count is exact.
        assert_eq!(p.total_rows, Some(50));
        assert_eq!(p.sample_rows.len(), SAMPLE_ROWS);
    }

    #[test]
    fn preview_stops_before_reading_a_long_file() {
        // The point of the change: a preview must not cost the whole file.
        // Beyond the cap the row count is reported as unknown rather than
        // guessed, and the sample is still there.
        let mut csv = String::from("id\n");
        for i in 0..(PREVIEW_ROWS * 3) {
            csv.push_str(&format!("{i}\n"));
        }
        let path = write_temp("long.csv", &csv);
        let p = preview(&path, ImportFormat::Csv, true).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(p.total_rows, None, "the whole file was counted");
        assert_eq!(p.sample_rows.len(), SAMPLE_ROWS);
        assert_eq!(p.inferred_types, vec!["integer"]);
    }

    #[test]
    fn a_sink_that_stops_is_not_an_error() {
        // Stopping early and failing are different outcomes; only one of them
        // should surface to the user as a broken import.
        let body: String = (1..=100).map(|i| format!("{i}\n")).collect();
        let path = write_temp("batched_stop.csv", &format!("id\n{body}"));

        let mut seen = 0usize;
        let (columns, total) =
            read_rows_batched(&path, ImportFormat::Csv, true, 10, &mut |chunk| {
                seen += chunk.len();
                Ok(seen < 30)
            })
            .expect("stopping early must not be an error");

        assert_eq!(columns, ["id"]);
        assert_eq!(seen, 30, "read past the stop");
        assert_eq!(total, 30, "total must count only what was delivered");
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
