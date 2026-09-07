//! Local-only product entry points for the standalone legacy Data Migrator.
//!
//! This module may inspect the retired BitFun roots and create authenticated
//! handoffs. It never writes migrated product data; only the sibling Data
//! Migrator executes a plan after Desktop has shut down.

use openbitfun_core::legacy_migration::adapters_for_groups;
use openbitfun_core_types::product_identity::product_id;
use openbitfun_legacy_migration::{
    launch_trusted_executable, probe_legacy_source, CancellationToken, HandoffStore,
    LegacyMigrationError, LegacyMigrationResult, MigrationEngine, MigrationOnboardingStore,
    MigrationRoots, PlatformExecutableTrustVerifier, ProbeLimits, TrustedInstallationResolver,
};
use openbitfun_product_domains::legacy_migration::{
    LegacySourceDescriptor, MigrationOnboardingState, MigrationPromptChoice, MigrationRunReport,
    MigrationSelection, MigratorHandoffRequest, MigratorProtocolCapabilities, MigratorRequestMode,
    MigratorRequestOrigin, ScanFinding, CURRENT_MIGRATION_FORMAT_VERSION,
};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

const RELEASE_CHANNEL: &str = match option_env!("OPENBITFUN_RELEASE_CHANNEL") {
    Some(value) => value,
    None => "stable",
};
const DESKTOP_BINARY_NAME: &str = match option_env!("OPENBITFUN_DESKTOP_BINARY_NAME") {
    Some(value) => value,
    None => "openbitfun-desktop",
};
const DATA_MIGRATOR_BINARY_NAME: &str = match option_env!("OPENBITFUN_DATA_MIGRATOR_BINARY_NAME") {
    Some(value) => value,
    None => "openbitfun-data-migrator",
};
const HANDOFF_LIFETIME_MS: i64 = 10 * 60 * 1000;

static STARTUP_HANDLED_RUN_ID: OnceLock<Option<String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupProbeDisposition {
    Continue { handled_run_id: Option<String> },
    MigratorLaunched,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationCommandError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl LegacyMigrationCommandError {
    fn new(code: &str, message: &str, recoverable: bool) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            recoverable,
        }
    }

    fn from_legacy(error: &LegacyMigrationError) -> Self {
        match error {
            LegacyMigrationError::UnsupportedSource(_) => Self::new(
                "unsupported_source",
                "The discovered BitFun data format is not supported by this Data Migrator.",
                false,
            ),
            LegacyMigrationError::InvalidRequest(_) | LegacyMigrationError::InvalidPlan(_) => {
                Self::new(
                    "invalid_migration_request",
                    "The migration request is invalid or no longer current.",
                    true,
                )
            }
            LegacyMigrationError::UntrustedExecutable(_)
            | LegacyMigrationError::TrustedInstallationUnavailable(_) => Self::new(
                "data_migrator_unavailable",
                "The signed Data Migrator sibling is missing or cannot be trusted. Repair or update this OpenBitFun installation.",
                false,
            ),
            LegacyMigrationError::LockUnavailable => Self::new(
                "migration_locked",
                "Another Data Migrator currently owns the migration lock.",
                true,
            ),
            LegacyMigrationError::PathUnavailable(_)
            | LegacyMigrationError::SourceEqualsTarget(_)
            | LegacyMigrationError::PathEscape(_)
            | LegacyMigrationError::LinkedPath(_)
            | LegacyMigrationError::ResourceLimit(_) => Self::new(
                "migration_storage_unavailable",
                "The local migration storage or retired BitFun source failed a safety check.",
                false,
            ),
            LegacyMigrationError::ProcessInspection(_) => Self::new(
                "process_inspection_failed",
                "OpenBitFun could not inspect local data-writing processes.",
                true,
            ),
            LegacyMigrationError::Cancelled => Self::new(
                "cancelled",
                "The read-only migration operation was cancelled.",
                true,
            ),
            LegacyMigrationError::InjectedCrash(_)
            | LegacyMigrationError::Domain { .. }
            | LegacyMigrationError::Io { .. }
            | LegacyMigrationError::Json { .. }
            | LegacyMigrationError::Sqlite { .. } => Self::new(
                "migration_operation_failed",
                "OpenBitFun could not safely inspect or prepare the local migration state.",
                true,
            ),
        }
    }
}

impl From<LegacyMigrationError> for LegacyMigrationCommandError {
    fn from(error: LegacyMigrationError) -> Self {
        Self::from_legacy(&error)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct EmptyLegacyMigrationRequest {}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ScanLegacyMigrationRequest {
    pub selection: Option<MigrationSelection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PrepareLegacyMigrationRequest {
    pub selection: MigrationSelection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GetLegacyMigrationReportRequest {
    pub run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetLegacyMigrationPromptPreferenceRequest {
    pub choice: MigrationPromptChoice,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationStatusView {
    pub source: Option<LegacySourceDescriptor>,
    pub onboarding: MigrationOnboardingState,
    pub latest_report: Option<MigrationRunReport>,
    pub startup_report: Option<MigrationRunReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationScanView {
    pub source: LegacySourceDescriptor,
    pub selection: MigrationSelection,
    pub scanned_at_ms: i64,
    pub findings: Vec<ScanFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationHandoffView {
    pub run_id: String,
    pub mode: MigratorRequestMode,
}

pub(crate) fn run_startup_probe() -> LegacyMigrationResult<StartupProbeDisposition> {
    let roots = MigrationRoots::resolve_current_user()?;
    let onboarding = MigrationOnboardingStore::new(roots.clone());
    let restart_run_id = migration_restart_run_id(std::env::args_os().skip(1));
    if let Some(run_id) = restart_run_id {
        if onboarding.consume_handled_run_id(&run_id)? {
            return Ok(StartupProbeDisposition::Continue {
                handled_run_id: Some(run_id),
            });
        }
    }

    let state = onboarding.load()?;
    let Some(source) = probe_legacy_source(&roots, ProbeLimits::default())? else {
        return Ok(StartupProbeDisposition::Continue {
            handled_run_id: None,
        });
    };
    if !should_offer_onboarding(&state, &source) {
        return Ok(StartupProbeDisposition::Continue {
            handled_run_id: None,
        });
    }

    let request = new_handoff_request(
        MigratorRequestMode::Onboarding,
        MigratorRequestOrigin::FirstLaunch,
        &source,
        MigrationSelection::all(),
    );
    HandoffStore::new(roots.clone(), product_id(), RELEASE_CHANNEL)
        .write_request(&request, now_ms())?;
    onboarding.update(|state| {
        state.format_version = CURRENT_MIGRATION_FORMAT_VERSION;
        if state.source_fingerprint != source.source_fingerprint {
            state.choice = MigrationPromptChoice::Unset;
        }
        state.source_fingerprint = source.source_fingerprint.clone();
        state.detected_at_ms.get_or_insert_with(now_ms);
        state.last_prompted_version = Some(env!("CARGO_PKG_VERSION").to_string());
        state.run_id = Some(request.run_id.clone());
        state.handled_run_id = None;
    })?;
    launch_data_migrator(&request.run_id)?;
    Ok(StartupProbeDisposition::MigratorLaunched)
}

pub(crate) fn set_startup_handled_run_id(run_id: Option<String>) {
    let _ = STARTUP_HANDLED_RUN_ID.set(run_id);
}

#[tauri::command]
pub fn get_legacy_migration_status(
    request: EmptyLegacyMigrationRequest,
) -> Result<LegacyMigrationStatusView, LegacyMigrationCommandError> {
    let _ = request;
    migration_status().map_err(Into::into)
}

#[tauri::command]
pub async fn scan_legacy_migration(
    request: ScanLegacyMigrationRequest,
) -> Result<LegacyMigrationScanView, LegacyMigrationCommandError> {
    let selection = request.selection.unwrap_or_else(MigrationSelection::all);
    if selection.groups.is_empty() {
        return Err(LegacyMigrationCommandError::new(
            "empty_selection",
            "Select at least one migration group.",
            true,
        ));
    }
    tauri::async_runtime::spawn_blocking(move || scan_local_source(selection))
        .await
        .map_err(|_| {
            LegacyMigrationCommandError::new(
                "scan_worker_failed",
                "The local read-only migration scan stopped unexpectedly.",
                true,
            )
        })?
        .map_err(Into::into)
}

#[tauri::command]
pub fn prepare_legacy_migration(
    app: AppHandle,
    request: PrepareLegacyMigrationRequest,
) -> Result<LegacyMigrationHandoffView, LegacyMigrationCommandError> {
    if request.selection.groups.is_empty() {
        return Err(LegacyMigrationCommandError::new(
            "empty_selection",
            "Select at least one migration group.",
            true,
        ));
    }
    let handoff =
        prepare_settings_handoff(request.selection).map_err(LegacyMigrationCommandError::from)?;
    crate::request_desktop_exit(&app, 0, "legacy_migration_handoff");
    Ok(handoff)
}

#[tauri::command]
pub fn get_legacy_migration_report(
    request: GetLegacyMigrationReportRequest,
) -> Result<Option<MigrationRunReport>, LegacyMigrationCommandError> {
    let roots =
        MigrationRoots::resolve_current_user().map_err(LegacyMigrationCommandError::from)?;
    let store = MigrationOnboardingStore::new(roots);
    let report = match request.run_id.as_deref() {
        Some(run_id) => store.load_report(run_id),
        None => store.load_last_report(),
    }
    .map_err(LegacyMigrationCommandError::from)?;
    Ok(report.as_ref().map(redact_report_for_ui))
}

#[tauri::command]
pub fn set_legacy_migration_prompt_preference(
    request: SetLegacyMigrationPromptPreferenceRequest,
) -> Result<MigrationOnboardingState, LegacyMigrationCommandError> {
    if !matches!(
        request.choice,
        MigrationPromptChoice::Unset
            | MigrationPromptChoice::RemindLater
            | MigrationPromptChoice::DoNotRemind
    ) {
        return Err(LegacyMigrationCommandError::new(
            "invalid_prompt_preference",
            "Only reminder preferences can be changed from OpenBitFun settings.",
            false,
        ));
    }
    let roots =
        MigrationRoots::resolve_current_user().map_err(LegacyMigrationCommandError::from)?;
    MigrationOnboardingStore::new(roots)
        .update(|state| {
            state.format_version = CURRENT_MIGRATION_FORMAT_VERSION;
            state.choice = request.choice;
            state.handled_run_id = None;
        })
        .map_err(Into::into)
}

fn migration_status() -> LegacyMigrationResult<LegacyMigrationStatusView> {
    let roots = MigrationRoots::resolve_current_user()?;
    let onboarding_store = MigrationOnboardingStore::new(roots.clone());
    let onboarding = onboarding_store.load()?;
    let source = probe_legacy_source(&roots, ProbeLimits::default())?;
    let latest_report = onboarding_store
        .load_last_report()?
        .as_ref()
        .map(redact_report_for_ui);
    let startup_report = STARTUP_HANDLED_RUN_ID
        .get()
        .and_then(|run_id| run_id.as_deref())
        .map(|run_id| onboarding_store.load_report(run_id))
        .transpose()?
        .flatten()
        .as_ref()
        .map(redact_report_for_ui);
    Ok(LegacyMigrationStatusView {
        source,
        onboarding,
        latest_report,
        startup_report,
    })
}

fn scan_local_source(
    selection: MigrationSelection,
) -> LegacyMigrationResult<LegacyMigrationScanView> {
    let roots = MigrationRoots::resolve_current_user()?;
    let source = probe_legacy_source(&roots, ProbeLimits::default())?.ok_or_else(|| {
        LegacyMigrationError::UnsupportedSource(
            "no supported legacy BitFun data was discovered".to_string(),
        )
    })?;
    if !source.supported {
        return Err(LegacyMigrationError::UnsupportedSource(
            source.product_version.clone(),
        ));
    }
    let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection))?;
    let findings = engine
        .scan(&selection, &CancellationToken::default())?
        .into_iter()
        .map(|scan| scan.finding)
        .collect();
    let scanned_at_ms = now_ms();
    MigrationOnboardingStore::new(roots).update(|state| {
        state.format_version = CURRENT_MIGRATION_FORMAT_VERSION;
        state.source_fingerprint = source.source_fingerprint.clone();
        state.detected_at_ms.get_or_insert(scanned_at_ms);
        state.last_scanned_at_ms = Some(scanned_at_ms);
    })?;
    Ok(LegacyMigrationScanView {
        source,
        selection,
        scanned_at_ms,
        findings,
    })
}

fn prepare_settings_handoff(
    selection: MigrationSelection,
) -> LegacyMigrationResult<LegacyMigrationHandoffView> {
    let roots = MigrationRoots::resolve_current_user()?;
    let source = probe_legacy_source(&roots, ProbeLimits::default())?.ok_or_else(|| {
        LegacyMigrationError::UnsupportedSource(
            "no supported legacy BitFun data was discovered".to_string(),
        )
    })?;
    if !source.supported {
        return Err(LegacyMigrationError::UnsupportedSource(
            source.product_version.clone(),
        ));
    }
    let request = new_handoff_request(
        MigratorRequestMode::Execute,
        MigratorRequestOrigin::Settings,
        &source,
        selection,
    );
    HandoffStore::new(roots.clone(), product_id(), RELEASE_CHANNEL)
        .write_request(&request, now_ms())?;
    MigrationOnboardingStore::new(roots).update(|state| {
        state.format_version = CURRENT_MIGRATION_FORMAT_VERSION;
        state.source_fingerprint = source.source_fingerprint.clone();
        state.detected_at_ms.get_or_insert_with(now_ms);
        state.run_id = Some(request.run_id.clone());
        state.handled_run_id = None;
    })?;
    launch_data_migrator(&request.run_id)?;
    Ok(LegacyMigrationHandoffView {
        run_id: request.run_id,
        mode: request.mode,
    })
}

fn new_handoff_request(
    mode: MigratorRequestMode,
    origin: MigratorRequestOrigin,
    source: &LegacySourceDescriptor,
    selection: MigrationSelection,
) -> MigratorHandoffRequest {
    let created_at_ms = now_ms();
    let capabilities = MigratorProtocolCapabilities::current();
    MigratorHandoffRequest {
        protocol_version: capabilities.protocol_version,
        mode,
        origin,
        run_id: uuid::Uuid::new_v4().to_string(),
        nonce: uuid::Uuid::new_v4().to_string(),
        source_id: Some(source.source_id.clone()),
        source_fingerprint: Some(source.source_fingerprint.clone()),
        selection,
        caller_process_id: std::process::id(),
        product_id: product_id().to_string(),
        release_channel: RELEASE_CHANNEL.to_string(),
        created_at_ms,
        expires_at_ms: created_at_ms.saturating_add(HANDOFF_LIFETIME_MS),
        required_capabilities: capabilities.capabilities,
    }
}

fn launch_data_migrator(run_id: &str) -> LegacyMigrationResult<u32> {
    let current = std::env::current_exe().map_err(|error| LegacyMigrationError::Io {
        path: DESKTOP_BINARY_NAME.into(),
        source: error,
    })?;
    let executable = TrustedInstallationResolver::resolve_sibling(
        &current,
        DESKTOP_BINARY_NAME,
        DATA_MIGRATOR_BINARY_NAME,
        &PlatformExecutableTrustVerifier,
    )?;
    launch_trusted_executable(&executable, &[OsStr::new(run_id)])
}

fn should_offer_onboarding(
    state: &MigrationOnboardingState,
    source: &LegacySourceDescriptor,
) -> bool {
    if !source.supported || source.already_migrated {
        return false;
    }
    if state.source_fingerprint != source.source_fingerprint {
        return true;
    }
    !matches!(
        state.choice,
        MigrationPromptChoice::DoNotRemind | MigrationPromptChoice::MigrateNow
    )
}

fn migration_restart_run_id(arguments: impl IntoIterator<Item = OsString>) -> Option<String> {
    let mut values = arguments.into_iter();
    let mut run_id = None;
    while let Some(argument) = values.next() {
        if argument != OsStr::new("--legacy-migration-run-id") {
            continue;
        }
        let candidate = values.next()?.into_string().ok()?;
        if run_id.is_some() || uuid::Uuid::parse_str(&candidate).is_err() {
            return None;
        }
        run_id = Some(candidate);
    }
    run_id
}

fn redact_report_for_ui(report: &MigrationRunReport) -> MigrationRunReport {
    let mut redacted = report.clone();
    for diagnostic in &mut redacted.diagnostics {
        diagnostic.relative_path = None;
        diagnostic.message = diagnostic.code.replace('_', " ");
        diagnostic.action = None;
    }
    for result in &mut redacted.domain_results {
        for warning in &mut result.warnings {
            warning.relative_path = None;
            warning.message = warning.code.replace('_', " ");
            warning.action = None;
        }
    }
    redacted
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbitfun_product_domains::legacy_migration::MigrationRunStatus;

    fn source(fingerprint: &str) -> LegacySourceDescriptor {
        LegacySourceDescriptor {
            source_fingerprint: fingerprint.to_string(),
            supported: true,
            ..LegacySourceDescriptor::default()
        }
    }

    #[test]
    fn only_matching_source_preferences_suppress_onboarding() {
        let mut state = MigrationOnboardingState {
            source_fingerprint: "source-a".to_string(),
            choice: MigrationPromptChoice::DoNotRemind,
            ..MigrationOnboardingState::default()
        };
        assert!(!should_offer_onboarding(&state, &source("source-a")));
        assert!(should_offer_onboarding(&state, &source("source-b")));

        state.choice = MigrationPromptChoice::RemindLater;
        assert!(should_offer_onboarding(&state, &source("source-a")));
    }

    #[test]
    fn completed_source_does_not_offer_onboarding_again() {
        let mut completed = source("source-a");
        completed.already_migrated = true;
        assert!(!should_offer_onboarding(
            &MigrationOnboardingState::default(),
            &completed
        ));
    }

    #[test]
    fn restart_argument_requires_one_uuid_value() {
        let run_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            migration_restart_run_id([
                OsString::from("--unrelated"),
                OsString::from("value"),
                OsString::from("--legacy-migration-run-id"),
                OsString::from(&run_id),
            ]),
            Some(run_id.clone())
        );
        assert_eq!(
            migration_restart_run_id([
                OsString::from("--legacy-migration-run-id"),
                OsString::from("invalid"),
            ]),
            None
        );
        assert_eq!(
            migration_restart_run_id([
                OsString::from("--legacy-migration-run-id"),
                OsString::from(&run_id),
                OsString::from("--legacy-migration-run-id"),
                OsString::from(&run_id),
            ]),
            None
        );
    }

    #[test]
    fn settings_handoff_requires_every_current_migrator_capability() {
        let request = new_handoff_request(
            MigratorRequestMode::Execute,
            MigratorRequestOrigin::Settings,
            &source("source-a"),
            MigrationSelection::all(),
        );
        assert_eq!(
            request.required_capabilities,
            MigratorProtocolCapabilities::current().capabilities
        );
        assert_eq!(request.mode, MigratorRequestMode::Execute);
        assert_eq!(request.origin, MigratorRequestOrigin::Settings);
    }

    #[test]
    fn ui_report_removes_paths_and_free_text() {
        let report = MigrationRunReport {
            status: MigrationRunStatus::FailedRecoverable,
            diagnostics: vec![
                openbitfun_product_domains::legacy_migration::MigrationDiagnostic {
                    code: "legacy_item_failed".to_string(),
                    relative_path: Some("private/session.md".to_string()),
                    message: "private body".to_string(),
                    action: Some("private action".to_string()),
                    ..Default::default()
                },
            ],
            ..MigrationRunReport::default()
        };
        let redacted = redact_report_for_ui(&report);
        assert!(redacted.diagnostics[0].relative_path.is_none());
        assert_eq!(redacted.diagnostics[0].message, "legacy item failed");
        assert!(redacted.diagnostics[0].action.is_none());
    }
}
