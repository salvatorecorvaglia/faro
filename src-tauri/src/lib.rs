pub mod budget;
pub mod commands;
pub mod dml;
pub mod driver;
pub mod error;
pub mod model;
pub mod registry;
pub mod results;
pub mod secrets;
#[cfg(all(feature = "desktop-smoke", debug_assertions))]
mod smoke;
pub mod sql;
mod sqlx;
pub mod store;
pub mod transfer;

use commands::AppState;
use registry::Registry;
use results::ResultCache;
use std::sync::atomic::Ordering;
use store::Store;
use tauri::{Emitter, Manager};

/// Open the settings database, or explain why not.
///
/// Separated from `run` so the failure has somewhere to go. These were
/// `expect()`s, and with `panic = "abort"` in the release profile a panic is
/// not a catchable error — a corrupt or locked `faro.db`, or a home directory
/// the OS will not hand over, killed the process at launch with nothing on
/// screen and nothing in the log.
fn open_store() -> std::result::Result<Store, String> {
    #[cfg(all(feature = "desktop-smoke", debug_assertions))]
    if let Some(report) = std::env::var_os("FARO_SMOKE_REPORT") {
        let path = std::path::PathBuf::from(report);
        return Store::open(&path.with_extension("db")).map_err(|error| error.to_string());
    }
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
    #[cfg(all(feature = "desktop-smoke", debug_assertions))]
    let started = std::time::Instant::now();
    #[cfg(all(feature = "desktop-smoke", debug_assertions))]
    assert!(
        std::env::var_os("FARO_SMOKE_REPORT").is_some(),
        "A native smoke build requires FARO_SMOKE_REPORT"
    );
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
        .manage(commands::ExitGuard::default())
        .manage(AppState {
            store,
            registry: Registry::new(),
            results: ResultCache::new(),
        })
        .setup(move |app| {
            #[cfg(all(feature = "desktop-smoke", debug_assertions))]
            app.manage(smoke::Started(started));
            let _ = app;
            Ok(())
        })
        .invoke_handler(|invoke| {
            #[cfg(all(feature = "desktop-smoke", debug_assertions))]
            if invoke.message.command().starts_with("smoke_") {
                return smoke::handler(invoke);
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                commands::set_exit_guard,
                commands::approve_exit,
                commands::running_operations,
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
                commands::export_cached,
                commands::export_table,
                commands::preview_import,
                commands::import_file,
                commands::suggested_export_name,
                commands::backup_database,
                commands::restore_database,
                commands::inspect_backup,
                commands::run_query,
                commands::release_result,
                commands::cancel_query,
                commands::statement_at_cursor,
                commands::script_summary,
                commands::list_saved_queries,
                commands::save_query,
                commands::delete_saved_query,
                commands::list_history,
                commands::clear_history,
                commands::delete_history_entry,
            ];
            handler(invoke)
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Faro could not start: {e}");
            std::process::exit(1);
        })
        .run(|app, event| {
            let guard = app.state::<commands::ExitGuard>();
            let guarded =
                guard.ready.load(Ordering::SeqCst) && !guard.approved.load(Ordering::SeqCst);
            let request_confirmation = || {
                let running = app.state::<AppState>().registry.has_running_operations();
                let _ = app.emit("faro://quit-requested", running);
            };
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = &event
            {
                if guarded {
                    api.prevent_close();
                    request_confirmation();
                    return;
                }
            }
            if let tauri::RunEvent::ExitRequested { api, code, .. } = &event {
                // Tauri cannot prevent a restart. The updater confirms before
                // requesting it; ordinary native close/quit routes through here.
                if guarded && *code != Some(tauri::RESTART_EXIT_CODE) {
                    api.prevent_exit();
                    request_confirmation();
                    return;
                }
            }
            // Close every pool on the way out.
            //
            // Without this the process simply exited and left each connection
            // to time out server-side. `Registry::close_all` existed but was
            // never called from anywhere.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let state: tauri::State<'_, AppState> = app.state();
                // Bounded: closing waits on each server, and one that has gone
                // unreachable would otherwise hold quitting hostage until its
                // TCP timeout. Past the deadline the process exits anyway and
                // the server reaps the connection on its own, which is where
                // things stood before pools were closed at all.
                tauri::async_runtime::block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        state.registry.close_all(),
                    )
                    .await;
                });
            }
        });
}
