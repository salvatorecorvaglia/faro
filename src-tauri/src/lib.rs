pub mod commands;
pub mod dml;
pub mod driver;
pub mod error;
pub mod model;
pub mod registry;
pub mod secrets;
pub mod sql;
pub mod store;
pub mod transfer;

use commands::AppState;
use registry::Registry;
use store::Store;
use tauri::Manager;

/// Open the settings database, or explain why not.
///
/// Separated from `run` so the failure has somewhere to go. These were
/// `expect()`s, and with `panic = "abort"` in the release profile a panic is
/// not a catchable error — a corrupt or locked `faro.db`, or a home directory
/// the OS will not hand over, killed the process at launch with nothing on
/// screen and nothing in the log.
fn open_store() -> std::result::Result<Store, String> {
    let path = store::default_path()
        .map_err(|e| format!("Faro could not prepare its configuration directory.\n\n{e}"))?;

    Store::open(&path).map_err(|e| {
        format!(
            "Faro could not open its settings database at {}.\n\n{e}\n\n\
             If the file is corrupt, moving it aside will let Faro start with \
             empty settings.",
            path.display()
        )
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = match open_store() {
        Ok(store) => store,
        Err(message) => {
            // Exit cleanly with a reason rather than aborting. There is no
            // window yet and the dialog plugin needs an initialized app, so
            // stderr is the only channel available at this point — but a
            // one-line explanation and exit code 1 still beats the abort that
            // `panic = "abort"` produced, which printed nothing useful and
            // looked to the user like the app silently failing to launch.
            eprintln!("Faro cannot start.\n\n{message}");
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState {
            store,
            registry: Registry::new(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_connections,
            commands::list_connection_status,
            commands::save_connection,
            commands::delete_connection,
            commands::connect,
            commands::test_connection,
            commands::disconnect,
            commands::connected_ids,
            commands::keychain_available,
            commands::list_engines,
            commands::list_schemas,
            commands::list_tables,
            commands::describe_table,
            commands::schema_snapshot,
            commands::browse_table,
            commands::preview_changes,
            commands::apply_changes,
            commands::export_result,
            commands::export_table,
            commands::preview_import,
            commands::import_file,
            commands::suggested_export_name,
            commands::backup_database,
            commands::restore_database,
            commands::inspect_backup,
            commands::run_query,
            commands::cancel_query,
            commands::statement_at_cursor,
            commands::list_saved_queries,
            commands::save_query,
            commands::delete_saved_query,
            commands::list_history,
            commands::clear_history,
            commands::delete_history_entry,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Faro could not start: {e}");
            std::process::exit(1);
        })
        .run(|app, event| {
            // Close every pool on the way out.
            //
            // Without this the process simply exited and left each connection
            // to time out server-side. `Registry::close_all` existed but was
            // never called from anywhere.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let state: tauri::State<'_, AppState> = app.state();
                tauri::async_runtime::block_on(state.registry.close_all());
            }
        });
}
