//! Explicit opt-in native test bridge. Compiled only in a debug smoke build.
use crate::commands::ExitGuard;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tauri::{Manager, State};

pub struct Started(pub Instant);

#[tauri::command]
async fn smoke_action(
    app: tauri::AppHandle,
    started: State<'_, Started>,
    action: String,
) -> std::result::Result<serde_json::Value, String> {
    eprintln!("Native smoke action: {action}");
    match action.as_str() {
        "finish" => Ok(serde_json::json!(
            std::env::var("FARO_SMOKE_EXIT").unwrap_or_else(|_| "quit".into())
        )),
        "startup" => Ok(serde_json::json!({"nativeStartupMs": started.0.elapsed().as_millis()})),
        "close" => {
            app.get_webview_window("main")
                .ok_or("No main window")?
                .close()
                .map_err(|error| error.to_string())?;
            Ok(serde_json::Value::Null)
        }
        "quit" => {
            app.exit(0);
            Ok(serde_json::Value::Null)
        }
        "fixtures" => {
            let report = std::path::PathBuf::from(
                std::env::var_os("FARO_SMOKE_REPORT").ok_or("Missing smoke path")?,
            );
            let database = report.with_extension("sqlite");
            drop(rusqlite::Connection::open(&database).map_err(|error| error.to_string())?);
            let csv = report.with_extension("csv");
            let sql = report.with_extension("sql");
            std::fs::write(&csv, "id,name\n2,imported\n").map_err(|error| error.to_string())?;
            std::fs::write(&sql, "INSERT INTO smoke_rows VALUES (3, 'restored');")
                .map_err(|error| error.to_string())?;
            Ok(
                serde_json::json!({"database": database, "csv": csv, "sql": sql, "backup": report.with_extension("backup.sql")}),
            )
        }
        _ => Err("Unknown smoke action".into()),
    }
}

#[tauri::command]
fn smoke_report(
    app: tauri::AppHandle,
    guard: State<'_, ExitGuard>,
    result: serde_json::Value,
    finish: Option<bool>,
) -> std::result::Result<(), String> {
    let path = std::env::var_os("FARO_SMOKE_REPORT").ok_or("FARO_SMOKE_REPORT is required")?;
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if finish.unwrap_or(true) {
        guard.approved.store(true, Ordering::SeqCst);
        app.exit(if result["ok"] == true { 0 } else { 1 });
    }
    Ok(())
}

pub fn handler(invoke: tauri::ipc::Invoke) -> bool {
    let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool =
        tauri::generate_handler![smoke_action, smoke_report];
    handler(invoke)
}
