use super::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

#[derive(Default)]
pub struct ExitGuard {
    pub ready: AtomicBool,
    pub approved: AtomicBool,
}

#[tauri::command]
pub fn set_exit_guard(guard: State<'_, ExitGuard>, ready: bool) {
    guard.ready.store(ready, Ordering::SeqCst);
}

#[tauri::command]
pub fn approve_exit(app: tauri::AppHandle, guard: State<'_, ExitGuard>) {
    guard.approved.store(true, Ordering::SeqCst);
    app.exit(0);
}

#[tauri::command]
pub fn running_operations(state: State<'_, AppState>) -> bool {
    state.registry.has_running_operations()
}
