mod app_state;
mod commands;

use app_state::MigratorCoordinator;
use std::fmt;
use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    Bootstrap,
    EventLoop,
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bootstrap => formatter.write_str("Data Migrator bootstrap failed"),
            Self::EventLoop => formatter.write_str("Data Migrator event loop failed"),
        }
    }
}

impl std::error::Error for RunError {}

pub fn run(run_id: &str) -> Result<(), RunError> {
    let coordinator = MigratorCoordinator::bootstrap(run_id).map_err(|_| RunError::Bootstrap)?;
    tauri::Builder::default()
        .manage(coordinator)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let coordinator = window.app_handle().state::<MigratorCoordinator>();
                api.prevent_close();
                if coordinator.is_running() {
                    coordinator.cancel();
                    return;
                }
                if coordinator.close_and_restart().is_ok() {
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_migrator_bootstrap,
            commands::scan_legacy_migration,
            commands::prepare_legacy_migration,
            commands::retry_writer_check,
            commands::start_legacy_migration,
            commands::cancel_legacy_migration,
            commands::export_migration_diagnostics,
            commands::finish_legacy_migration,
        ])
        .run(tauri::generate_context!())
        .map_err(|_| RunError::EventLoop)
}
