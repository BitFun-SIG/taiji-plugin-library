use crate::app_state::{CommandError, DiagnosticsExportView, MigratorCoordinator, MigratorView};
use openbitfun_product_domains::legacy_migration::{MigrationPromptChoice, MigrationSelection};
use serde::Deserialize;
use tauri::{AppHandle, State};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct EmptyRequest {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SelectionRequest {
    pub selection: MigrationSelection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ExecuteRequest {
    pub plan_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PromptChoiceRequest {
    pub choice: MigrationPromptChoice,
}

#[tauri::command]
pub(crate) fn get_migrator_bootstrap(
    state: State<'_, MigratorCoordinator>,
    request: EmptyRequest,
) -> MigratorView {
    let _ = request;
    state.snapshot()
}

#[tauri::command]
pub(crate) fn scan_legacy_migration(
    state: State<'_, MigratorCoordinator>,
    request: SelectionRequest,
) -> Result<MigratorView, CommandError> {
    state.scan(request.selection)
}

#[tauri::command]
pub(crate) fn prepare_legacy_migration(
    state: State<'_, MigratorCoordinator>,
    request: SelectionRequest,
) -> Result<MigratorView, CommandError> {
    state.prepare(request.selection)
}

#[tauri::command]
pub(crate) fn retry_writer_check(
    state: State<'_, MigratorCoordinator>,
    request: EmptyRequest,
) -> Result<MigratorView, CommandError> {
    let _ = request;
    state.refresh_blockers()
}

#[tauri::command]
pub(crate) fn start_legacy_migration(
    state: State<'_, MigratorCoordinator>,
    request: ExecuteRequest,
) -> Result<MigratorView, CommandError> {
    state.start(request.plan_hash)
}

#[tauri::command]
pub(crate) fn cancel_legacy_migration(
    state: State<'_, MigratorCoordinator>,
    request: EmptyRequest,
) -> MigratorView {
    let _ = request;
    state.cancel()
}

#[tauri::command]
pub(crate) fn export_migration_diagnostics(
    state: State<'_, MigratorCoordinator>,
    request: EmptyRequest,
) -> Result<DiagnosticsExportView, CommandError> {
    let _ = request;
    state.export_diagnostics()
}

#[tauri::command]
pub(crate) fn finish_legacy_migration(
    app: AppHandle,
    state: State<'_, MigratorCoordinator>,
    request: PromptChoiceRequest,
) -> Result<(), CommandError> {
    state.finish_and_restart(request.choice)?;
    app.exit(0);
    Ok(())
}
