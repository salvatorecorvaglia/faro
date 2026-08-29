use std::path::PathBuf;
use tauri::State;

use super::AppState;
use crate::error::{FaroError, Result};
use crate::model::{EditValue, GuardedStatement, ResultSet, TableRef};
use crate::transfer::export::{self, ExportFormat, ExportOptions};
use crate::transfer::import::{self, ImportFormat, ImportOptions, ImportPreview};

/// Rows per generated `INSERT`, and the batch size the file is read in.
///
/// Large enough that a restore is not one statement per row, small enough that
/// a single statement stays within every engine's limits.
const ROWS_PER_INSERT: usize = 500;

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
    // Writing is synchronous and can be large (XLSX builds the whole workbook),
    // so it goes to the blocking pool rather than stalling a runtime worker.
    let rows =
        tokio::task::spawn_blocking(move || export::write_file(&target, &result, &options, None))
            .await
            .map_err(|e| FaroError::Io(format!("could not write the file: {e}")))??;
    Ok(TransferResult { rows, path })
}

/// Export an entire table, reading it in pages rather than loading it all.
///
/// The grid only ever holds one page, so exporting what is on screen would
/// silently truncate a large table. This re-reads from the database instead.
///
/// CSV, TSV and SQL are streamed straight to disk page by page, since their
/// writers never needed the whole table in memory to begin with. XLSX and
/// JSON still read every page into one `ResultSet` first — producing either
/// genuinely requires the complete row set before the first byte can be
/// written.
///
/// `query_id` is registered the same way a query's is, so `cancel_query` can
/// stop a full-table export mid-flight too.
#[tauri::command]
pub async fn export_table(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    path: String,
    mut options: ExportOptions,
    query_id: String,
) -> Result<TransferResult> {
    let driver = state.registry.get(&connection_id).await?;

    if options.table_name.is_none() {
        options.table_name = Some(table.name.clone());
    }

    let target = PathBuf::from(&path);
    let cancel = state.registry.begin_query(&connection_id, &query_id).await;

    let outcome = match options.format {
        ExportFormat::Csv | ExportFormat::Tsv | ExportFormat::Sql => {
            export::export_table_streaming(
                &*driver,
                &table,
                &target,
                &options,
                Some(driver.dialect()),
                cancel,
            )
            .await
        }
        ExportFormat::Json | ExportFormat::Xlsx => {
            // These two need the complete row set before the first byte can be
            // written, so the read stays here and only the write is offloaded.
            // The driver goes along so the dialect can be borrowed inside;
            // `Arc<dyn Driver>` is Send + Sync, so it may cross the boundary.
            match export::read_table_paged(&*driver, &table, cancel).await {
                Ok(result) => {
                    let driver = driver.clone();
                    let options = options.clone();
                    tokio::task::spawn_blocking(move || {
                        export::write_file(&target, &result, &options, Some(driver.dialect()))
                    })
                    .await
                    .map_err(|e| FaroError::Io(format!("could not write the file: {e}")))?
                }
                Err(e) => Err(e),
            }
        }
    };

    state.registry.end_query(&connection_id, &query_id).await;
    let rows = outcome?;
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
    let driver = state.registry.get_writable(&connection_id).await?;
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

    let qualified = dialect.qualify(table.schema.as_deref(), &table.name);
    let columns_sql = options
        .mappings
        .iter()
        .map(|m| dialect.quote_ident(&m.target_column))
        .collect::<Vec<_>>()
        .join(", ");

    // Resolve each mapping's target type once, in mapping order.
    //
    // `dml::literal` needs the declared type for every cell, and this used to
    // scan the table's whole column list per cell — so the cost of an import
    // grew with rows × mapped columns × table width.
    let types: Vec<String> = options
        .mappings
        .iter()
        .map(|m| {
            detail
                .columns
                .iter()
                .find(|c| c.name == m.target_column)
                .map(|c| c.type_name.clone())
                .unwrap_or_default()
        })
        .collect();

    // Parse and generate on a blocking thread, in bounded batches.
    //
    // Both halves used to happen on the async runtime with the entire file
    // materialized first: every row as a `Vec<String>` *and* then every
    // generated statement, so peak memory was a large multiple of the file.
    // Batching keeps only `ROWS_PER_INSERT` parsed rows alive at a time.
    let driver_for_build = driver.clone();
    let mappings = options.mappings.clone();
    let null_tokens = options.null_tokens.clone();
    let format = options.format;
    let has_header = options.has_header;

    let (statements, row_count) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<GuardedStatement>, usize)> {
            let dialect = driver_for_build.dialect();
            let mut statements = Vec::new();

            let (_, total) = import::read_rows_batched(
                &source,
                format,
                has_header,
                ROWS_PER_INSERT,
                &mut |chunk| {
                    let mut tuples = Vec::with_capacity(chunk.len());
                    for row in chunk {
                        let values: Vec<String> = mappings
                            .iter()
                            .enumerate()
                            .map(|(i, m)| {
                                let raw = row.get(m.source_index).map(String::as_str).unwrap_or("");
                                let value = if null_tokens.iter().any(|t| t == raw) {
                                    EditValue::Null
                                } else {
                                    EditValue::Text(raw.to_string())
                                };
                                let type_name = types.get(i).map(String::as_str).unwrap_or("");
                                crate::dml::literal(&value, type_name, dialect)
                            })
                            .collect();
                        tuples.push(format!("({})", values.join(", ")));
                    }

                    statements.push(GuardedStatement {
                        sql: format!(
                            "INSERT INTO {qualified} ({columns_sql}) VALUES {}",
                            tuples.join(", ")
                        ),
                        // Row counts for multi-row inserts vary by engine, and a
                        // failure raises an error rather than reporting the
                        // wrong count.
                        expect: None,
                    });
                    Ok(())
                },
            )?;

            Ok((statements, total))
        })
        .await
        .map_err(|e| FaroError::Io(format!("could not read the file: {e}")))??;

    // Still one transaction: a file that fails halfway leaves the table exactly
    // as it was, which is the promise this command makes.
    let affected = driver.apply_transaction(&statements).await?;
    Ok(TransferResult {
        rows: if affected > 0 {
            affected
        } else {
            row_count as u64
        },
        path,
    })
}

/// Default filename offered by the save dialog.
#[tauri::command]
pub fn suggested_export_name(base: String, format: export::ExportFormat) -> String {
    export::suggested_filename(&base, format)
}
