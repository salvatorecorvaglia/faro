use std::path::PathBuf;
use tauri::State;

use super::AppState;
use crate::error::{FaroError, Result};
use crate::model::{ApplyChunk, EditValue, GuardedStatement, TableRef};
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

/// Write a result the backend already holds — a query's run, or a table page —
/// to a file.
///
/// Replaces `export_result`, which received the rows back from the UI. The UI
/// is not a faithful copy: large binary values reach it only as their size.
/// Reading from the result cache exports exactly what the database returned,
/// and nothing large crosses IPC a second time.
///
/// `statement` picks one result set of a multi-statement run; a page has
/// only `0`.
#[tauri::command]
pub async fn export_cached(
    state: State<'_, AppState>,
    query_id: String,
    statement: usize,
    path: String,
    options: ExportOptions,
) -> Result<TransferResult> {
    let cached = state.results.get(&query_id).ok_or_else(|| {
        FaroError::Other(
            "These results are no longer held — only the most recent ones are kept. \
             Run the query again to export them."
                .into(),
        )
    })?;
    let target = PathBuf::from(&path);
    // Writing is synchronous and can be large (XLSX builds the whole workbook),
    // so it goes to the blocking pool rather than stalling a runtime worker.
    let rows = tokio::task::spawn_blocking(move || {
        let result = cached
            .result_set(statement)
            .ok_or_else(|| FaroError::Other("that statement returned no rows to export".into()))?;
        export::write_file(&target, result, &options, None)
    })
    .await
    .map_err(|e| FaroError::Io(format!("could not write the file: {e}")))??;
    Ok(TransferResult { rows, path })
}

/// Export an entire table, reading it in pages rather than loading it all.
///
/// The grid only ever holds one page, so exporting what is on screen would
/// silently truncate a large table. This re-reads from the database instead.
///
/// CSV, TSV, SQL and JSON are streamed straight to disk page by page, since
/// their writers never needed the whole table in memory to begin with. XLSX
/// still reads every page into one `ResultSet` first — a workbook is built
/// whole before it is written — and stops reading as soon as the table proves
/// too large for a worksheet.
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
    let query = state.registry.begin_query(&connection_id, &query_id);

    let outcome = match options.format {
        ExportFormat::Csv | ExportFormat::Tsv | ExportFormat::Sql | ExportFormat::Json => {
            export::export_table_streaming(
                &*driver,
                &table,
                &target,
                &options,
                Some(driver.dialect()),
                query.token(),
            )
            .await
        }
        ExportFormat::Xlsx => {
            // A workbook needs the complete row set before the first byte can
            // be written, so the read stays here and only the write is
            // offloaded. The driver goes along so the dialect can be borrowed
            // inside; `Arc<dyn Driver>` is Send + Sync, so it may cross the
            // boundary.
            let max_rows = export::XLSX_MAX_ROWS - usize::from(options.include_header);
            match export::read_table_paged_capped(&*driver, &table, query.token(), Some(max_rows))
                .await
            {
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
    // On the blocking pool: CSV previews stop early, but JSON and XLSX have to
    // be parsed whole before their first row exists, and that ran on an async
    // runtime worker for as long as a large workbook took to read.
    tokio::task::spawn_blocking(move || import::preview(&source, format, has_header))
        .await
        .map_err(|e| FaroError::Io(format!("could not read the file: {e}")))?
}

/// The producer's error once the applier has given up and dropped its end of
/// the channel.
///
/// Never the real cause — the applier's own error is — so it is recognised
/// below and discarded in favour of that one.
const APPLIER_GONE: &str = "the applier stopped before the file was read";

/// Insert a file's rows into a table.
///
/// Runs as one transaction: a file that fails halfway leaves the table exactly
/// as it was, rather than half-loaded with no clear way to tell how far it got.
///
/// `query_id` is registered the same way a query's is, so `cancel_query` can
/// stop an import mid-flight — which matters most for exactly the large files
/// this streams.
#[tauri::command]
pub async fn import_file(
    state: State<'_, AppState>,
    connection_id: String,
    table: TableRef,
    path: String,
    options: ImportOptions,
    query_id: String,
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

    let query = state.registry.begin_query(&connection_id, &query_id);
    let cancel = query.token();

    // Parse on a blocking thread and apply on this one, joined by a short
    // channel.
    //
    // Both halves used to happen with the entire file materialized first: every
    // row as a `Vec<String>` and then every generated statement, so peak memory
    // was a large multiple of the file. Batching the *reads* alone did not fix
    // that — the statements were still all retained until the last one was
    // built. The channel is what supplies the missing back-pressure: with two
    // batches in flight the reader stops as soon as the applier falls behind,
    // so peak memory is a batch rather than the file, and the whole thing is
    // still one transaction.
    let (tx, rx) = tokio::sync::mpsc::channel::<ApplyChunk>(2);

    let driver_for_build = driver.clone();
    let mappings = options.mappings.clone();
    let null_tokens = options.null_tokens.clone();
    let format = options.format;
    let has_header = options.has_header;
    let cancel_for_build = cancel.clone();

    let parse = tokio::task::spawn_blocking(move || -> Result<usize> {
        let dialect = driver_for_build.dialect();

        let (_, total) = import::read_rows_batched(
            &source,
            format,
            has_header,
            ROWS_PER_INSERT,
            &mut |chunk| {
                // Checked per batch rather than per row: cancelling should stop
                // the import promptly, not instantly, and the applier rolls the
                // transaction back either way.
                if cancel_for_build.is_cancelled() {
                    return Err(FaroError::Cancelled);
                }

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

                let statement = GuardedStatement {
                    sql: format!(
                        "INSERT INTO {qualified} ({columns_sql}) VALUES {}",
                        tuples.join(", ")
                    ),
                    // Row counts for multi-row inserts vary by engine, and a
                    // failure raises an error rather than reporting the wrong
                    // count.
                    expect: None,
                };

                // Blocking on a full channel is the back-pressure, not a stall.
                tx.blocking_send(ApplyChunk::Batch(vec![statement]))
                    .map_err(|_| FaroError::Other(APPLIER_GONE.into()))?;
                Ok(true)
            },
        )?;

        // Only this earns a commit; see `Driver::apply_streaming`.
        tx.blocking_send(ApplyChunk::Done)
            .map_err(|_| FaroError::Other(APPLIER_GONE.into()))?;
        Ok(total)
    });

    // Both run at once: the applier drains the channel while the reader fills
    // it. Awaiting the reader first would deadlock on a file larger than the
    // channel.
    let applied = driver.apply_streaming(rx).await;
    let parsed = parse
        .await
        .map_err(|e| FaroError::Io(format!("could not read the file: {e}")))?;

    // Deregister before reporting, so a failed import does not leave the
    // connection looking busy.
    drop(query);

    let row_count = match parsed {
        Ok(total) => total,
        // The reader only noticed that the applier had gone; the applier knows
        // why. Prefer its error, and fall back if it somehow succeeded.
        Err(FaroError::Other(ref message)) if message == APPLIER_GONE => {
            return Err(applied.err().unwrap_or_else(|| {
                FaroError::Other("the import stopped for an unknown reason".into())
            }))
        }
        Err(e) => return Err(e),
    };

    let affected = applied?;
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
