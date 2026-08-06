use std::path::PathBuf;
use tauri::State;

use super::AppState;
use crate::error::{FaroError, Result};
use crate::model::{ResultSet, TableRef};
use crate::transfer::export::{self, ExportOptions};
use crate::transfer::import::{self, ImportFormat, ImportOptions, ImportPreview};

/// How many rows to read per page when exporting a whole table, and how many
/// to insert per statement when importing. Large enough to be efficient, small
/// enough that memory stays bounded on a big table.
const BATCH: u64 = 5_000;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    pub rows: u64,
    pub path: String,
}

/// Write an in-hand result set to a file.
#[tauri::command]
pub async fn export_result(
    path: String,
    result: ResultSet,
    options: ExportOptions,
) -> Result<TransferResult> {
    let target = PathBuf::from(&path);
    let rows = export::write_file(&target, &result, &options, None)?;
    Ok(TransferResult { rows, path })
}

/// Export an entire table, reading it in pages rather than loading it all.
///
/// The grid only ever holds one page, so exporting what is on screen would
/// silently truncate a large table. This re-reads from the database instead.
#[tauri::command]
pub async fn export_table(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    path: String,
    mut options: ExportOptions,
) -> Result<TransferResult> {
    let driver = state.registry.get(&connection_id).await?;
    let dialect = driver.dialect();
    let qualified = dialect.qualify(table.schema.as_deref(), &table.name);

    if options.table_name.is_none() {
        options.table_name = Some(table.name.clone());
    }

    let base = format!("SELECT * FROM {qualified}");
    let mut combined: Option<ResultSet> = None;
    let mut offset = 0u64;

    // Accumulate pages into one result. Formats like XLSX and JSON need the
    // whole set to produce a valid file, so streaming per page is not an
    // option; the batching keeps the query side bounded.
    loop {
        let paged = dialect.paginate(&base, BATCH + 1, offset);
        let page = driver
            .query(&paged, BATCH, tokio_util::sync::CancellationToken::new())
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

    let result = combined.unwrap_or(ResultSet {
        columns: vec![],
        rows: vec![],
        truncated: false,
        elapsed_ms: 0,
    });

    let target = PathBuf::from(&path);
    let rows = export::write_file(&target, &result, &options, Some(dialect))?;
    Ok(TransferResult { rows, path })
}

/// Inspect a file without writing anything.
#[tauri::command]
pub async fn preview_import(path: String, has_header: bool) -> Result<ImportPreview> {
    let source = PathBuf::from(&path);
    let format = ImportFormat::from_path(&source).ok_or_else(|| {
        FaroError::Other(
            "unrecognized file type — Faro can import .csv, .tsv, .json and .xlsx".into(),
        )
    })?;
    import::preview(&source, format, has_header)
}

/// Insert a file's rows into a table.
///
/// Runs as one transaction: a file that fails halfway leaves the table exactly
/// as it was, rather than half-loaded with no clear way to tell how far it got.
#[tauri::command]
pub async fn import_file(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    path: String,
    options: ImportOptions,
) -> Result<TransferResult> {
    let driver = state.registry.get(&connection_id).await?;
    let detail = driver.describe_table(&table).await?;
    let dialect = driver.dialect();

    if options.mappings.is_empty() {
        return Err(FaroError::Other(
            "no columns are mapped — choose which file columns to import".into(),
        ));
    }

    // Validate targets against the real table before reading the file, so a
    // bad mapping fails immediately rather than after a long parse.
    for mapping in &options.mappings {
        if !detail
            .columns
            .iter()
            .any(|c| c.name == mapping.target_column)
        {
            return Err(FaroError::Other(format!(
                "no column named \"{}\" on {}",
                mapping.target_column, table.name
            )));
        }
    }

    let source = PathBuf::from(&path);
    let (_, rows) = import::read_rows(&source, options.format, options.has_header)?;

    let qualified = dialect.qualify(table.schema.as_deref(), &table.name);
    let column_list: Vec<String> = options
        .mappings
        .iter()
        .map(|m| dialect.quote_ident(&m.target_column))
        .collect();

    let type_of = |name: &str| {
        detail
            .columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.type_name.clone())
            .unwrap_or_default()
    };

    let mut statements = Vec::new();
    for chunk in rows.chunks(500) {
        let mut tuples = Vec::with_capacity(chunk.len());
        for row in chunk {
            let values: Vec<String> = options
                .mappings
                .iter()
                .map(|m| {
                    let raw = row.get(m.source_index).map(String::as_str).unwrap_or("");
                    let value = if options.null_tokens.iter().any(|t| t == raw) {
                        crate::model::EditValue::Null
                    } else {
                        crate::model::EditValue::Text(raw.to_string())
                    };
                    crate::dml::literal(&value, &type_of(&m.target_column), dialect)
                })
                .collect();
            tuples.push(format!("({})", values.join(", ")));
        }

        statements.push(crate::model::GuardedStatement {
            sql: format!(
                "INSERT INTO {qualified} ({}) VALUES {}",
                column_list.join(", "),
                tuples.join(", ")
            ),
            // Row counts for multi-row inserts vary by engine, and a failure
            // raises an error rather than reporting the wrong count.
            expect: None,
        });
    }

    let affected = driver.apply_transaction(&statements).await?;
    Ok(TransferResult {
        rows: if affected > 0 {
            affected
        } else {
            rows.len() as u64
        },
        path,
    })
}

/// Default filename offered by the save dialog.
#[tauri::command]
pub fn suggested_export_name(base: String, format: export::ExportFormat) -> String {
    export::suggested_filename(&base, format)
}
