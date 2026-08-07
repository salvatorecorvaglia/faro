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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = Store::open(&store::default_path().expect("could not locate a config directory"))
        .expect("could not open the Faro settings database");

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
        .run(tauri::generate_context!())
        .expect("error while running Faro");
}
