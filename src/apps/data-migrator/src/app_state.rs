use openbitfun_core::legacy_migration::adapters_for_groups;
use openbitfun_core_types::product_identity::product_id;
use openbitfun_legacy_migration::{
    blocking_writer_processes_for_product, launch_trusted_executable, probe_legacy_source,
    CancellationToken, HandoffDisposition, HandoffStore, LegacyMigrationError,
    LegacyMigrationResult, MigrationEngine, MigrationLayout, MigrationOnboardingStore,
    MigrationRoots, NoCrashInjection, PlatformExecutableTrustVerifier, ProbeLimits,
    TrustedInstallationResolver, WriterProcess,
};
use openbitfun_product_capabilities::{product_assembly_plan_for_profile, DeliveryProfile};
use openbitfun_product_domains::legacy_migration::{
    LegacySourceDescriptor, MigrationPhase, MigrationPlan, MigrationProgressEvent,
    MigrationPromptChoice, MigrationRunReport, MigrationRunStatus, MigrationSelection,
    MigratorHandoffRequest, MigratorProtocolCapabilities, MigratorRequestMode, ScanFinding,
    CURRENT_MIGRATION_FORMAT_VERSION,
};
use serde::Serialize;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl CommandError {
    fn new(code: &str, message: &str, recoverable: bool) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            recoverable,
        }
    }

    pub(crate) fn worker_failed() -> Self {
        Self::new(
            "worker_failed",
            "The migration worker stopped before returning a result.",
            true,
        )
    }

    fn operation_in_progress() -> Self {
        Self::new(
            "operation_in_progress",
            "Another migration operation is still running.",
            true,
        )
    }

    fn from_legacy(error: &LegacyMigrationError) -> Self {
        match error {
            LegacyMigrationError::PathUnavailable(_) => Self::new(
                "path_unavailable",
                "A required data location is unavailable for the current user.",
                true,
            ),
            LegacyMigrationError::SourceEqualsTarget(_) => Self::new(
                "source_equals_target",
                "The legacy and OpenBitFun data locations are not safely separated.",
                false,
            ),
            LegacyMigrationError::UnsupportedSource(_) => Self::new(
                "unsupported_source",
                "This legacy BitFun data format is not supported by this migrator.",
                false,
            ),
            LegacyMigrationError::InvalidRequest(_) => Self::new(
                "invalid_handoff",
                "The migration handoff could not be authenticated or has expired.",
                false,
            ),
            LegacyMigrationError::InvalidPlan(_) => Self::new(
                "invalid_plan",
                "The saved migration plan no longer matches this request or source.",
                true,
            ),
            LegacyMigrationError::PathEscape(_) | LegacyMigrationError::LinkedPath(_) => Self::new(
                "unsafe_source_path",
                "Migration stopped because a source path failed its safety check.",
                false,
            ),
            LegacyMigrationError::ResourceLimit(_) => Self::new(
                "resource_limit",
                "Migration stopped at a configured safety limit.",
                true,
            ),
            LegacyMigrationError::LockUnavailable => Self::new(
                "migration_locked",
                "Another migration process currently owns the migration lock.",
                true,
            ),
            LegacyMigrationError::Cancelled => Self::new(
                "cancelled",
                "Migration was cancelled at a safe boundary.",
                true,
            ),
            LegacyMigrationError::ProcessInspection(_) => Self::new(
                "process_inspection_failed",
                "OpenBitFun could not verify that all data-writing processes have stopped.",
                true,
            ),
            LegacyMigrationError::UntrustedExecutable(_)
            | LegacyMigrationError::TrustedInstallationUnavailable(_) => Self::new(
                "trusted_installation_unavailable",
                "The signed OpenBitFun installation could not be verified.",
                true,
            ),
            LegacyMigrationError::InjectedCrash(_) => Self::new(
                "migration_interrupted",
                "Migration was interrupted and can be resumed from its journal.",
                true,
            ),
            LegacyMigrationError::Domain { .. } => Self::new(
                "domain_failed",
                "One migration domain failed validation. Other verified domains remain intact.",
                true,
            ),
            LegacyMigrationError::Io { .. }
            | LegacyMigrationError::Json { .. }
            | LegacyMigrationError::Sqlite { .. } => Self::new(
                "storage_failed",
                "Migration could not safely read or write one of its data stores.",
                true,
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigratorView {
    pub delivery_profile: String,
    pub protocol: MigratorProtocolCapabilities,
    pub mode: MigratorRequestMode,
    pub source: Option<LegacySourceDescriptor>,
    pub selection: MigrationSelection,
    pub findings: Vec<ScanFinding>,
    pub plan: Option<MigrationPlan>,
    pub report: Option<MigrationRunReport>,
    pub progress: Option<MigrationProgressEvent>,
    pub blockers: Vec<WriterProcess>,
    pub status: MigrationRunStatus,
    pub running: bool,
    pub can_execute: bool,
    pub recovery: bool,
    pub error: Option<CommandError>,
}

#[derive(Debug)]
struct MigratorSession {
    roots: MigrationRoots,
    request: MigratorHandoffRequest,
    disposition: HandoffDisposition,
    source: Option<LegacySourceDescriptor>,
    selection: MigrationSelection,
    findings: Vec<ScanFinding>,
    plan: Option<MigrationPlan>,
    report: Option<MigrationRunReport>,
    progress: Option<MigrationProgressEvent>,
    blockers: Vec<WriterProcess>,
    status: MigrationRunStatus,
    running: bool,
    error: Option<CommandError>,
    cancellation: CancellationToken,
}

#[derive(Clone)]
pub(crate) struct MigratorCoordinator {
    session: Arc<Mutex<MigratorSession>>,
}

impl MigratorCoordinator {
    pub(crate) fn bootstrap(run_id: &str) -> LegacyMigrationResult<Self> {
        Self::bootstrap_with(
            run_id,
            MigrationRoots::resolve_current_user()?,
            product_id(),
            RELEASE_CHANNEL,
        )
    }

    fn bootstrap_with(
        run_id: &str,
        roots: MigrationRoots,
        expected_product_id: &str,
        expected_release_channel: &str,
    ) -> LegacyMigrationResult<Self> {
        let product_plan = product_assembly_plan_for_profile(DeliveryProfile::DataMigrator);
        if !product_plan.capability_set().ids().is_empty()
            || !product_plan.capability_assembly().agent_ids().is_empty()
            || !product_plan.feature_groups().is_empty()
        {
            return Err(LegacyMigrationError::InvalidRequest(
                "data migrator delivery profile unexpectedly selected runtime capabilities"
                    .to_string(),
            ));
        }

        let store = HandoffStore::new(roots.clone(), expected_product_id, expected_release_channel);
        let handoff = store.load_request(run_id, now_ms())?;
        let request = handoff.request().clone();
        let source = probe_bound_source(&roots, &request)?;
        let plan = store.load_authorized_plan(&handoff)?;
        let report = handoff
            .layout()
            .read_json::<MigrationRunReport>(&handoff.layout().report_path())?;
        let selection = plan
            .as_ref()
            .map(|plan| plan.selection.clone())
            .unwrap_or_else(|| request.selection.clone());
        let findings = plan
            .as_ref()
            .map(|plan| plan.findings.clone())
            .unwrap_or_default();
        let status = report
            .as_ref()
            .map(|report| report.status)
            .unwrap_or_else(|| {
                if plan.is_some() {
                    MigrationRunStatus::Planned
                } else if source.is_some() {
                    MigrationRunStatus::Discovered
                } else {
                    MigrationRunStatus::default()
                }
            });
        let blockers = writer_processes(request.caller_process_id)?;

        Ok(Self {
            session: Arc::new(Mutex::new(MigratorSession {
                roots,
                request,
                disposition: handoff.disposition(),
                source,
                selection,
                findings,
                plan,
                report,
                progress: None,
                blockers,
                status,
                running: false,
                error: None,
                cancellation: CancellationToken::default(),
            })),
        })
    }

    pub(crate) fn snapshot(&self) -> MigratorView {
        let session = self.lock();
        snapshot(&session)
    }

    pub(crate) fn scan(&self, selection: MigrationSelection) -> Result<MigratorView, CommandError> {
        let (roots, request, cancellation) = self.begin_operation(&selection)?;
        let coordinator = self.clone();
        self.spawn_worker("legacy-migration-scan", move || {
            coordinator.scan_background(roots, request, selection, cancellation);
        })
    }

    fn scan_background(
        &self,
        roots: MigrationRoots,
        request: MigratorHandoffRequest,
        selection: MigrationSelection,
        cancellation: CancellationToken,
    ) {
        let result = (|| {
            let source = probe_bound_source(&roots, &request)?.ok_or_else(|| {
                LegacyMigrationError::UnsupportedSource(
                    "no supported legacy BitFun data was discovered".to_string(),
                )
            })?;
            let engine = migration_engine(roots.clone(), &selection)?;
            let scans = engine.scan(&selection, &cancellation)?;
            Ok::<_, LegacyMigrationError>((
                source,
                scans
                    .into_iter()
                    .map(|scan| scan.finding)
                    .collect::<Vec<_>>(),
            ))
        })();

        let mut session = self.lock();
        session.running = false;
        match result {
            Ok((source, findings)) => {
                session.source = Some(source);
                session.selection = selection;
                session.findings = findings;
                session.plan = None;
                session.report = None;
                session.status = MigrationRunStatus::Scanned;
                session.progress = Some(MigrationProgressEvent {
                    run_id: session.request.run_id.clone(),
                    phase: MigrationPhase::Scan,
                    processed: session.selection.expanded_domains().len() as u64,
                    total: session.selection.expanded_domains().len() as u64,
                    safe_to_cancel: true,
                    code: "scan_completed".to_string(),
                    ..MigrationProgressEvent::default()
                });
                session.error = None;
            }
            Err(error) => {
                self.finish_error_locked(&mut session, &error);
            }
        }
    }

    pub(crate) fn prepare(
        &self,
        selection: MigrationSelection,
    ) -> Result<MigratorView, CommandError> {
        let (roots, request, cancellation) = self.begin_operation(&selection)?;
        let coordinator = self.clone();
        self.spawn_worker("legacy-migration-plan", move || {
            coordinator.prepare_background(roots, request, selection, cancellation);
        })
    }

    fn prepare_background(
        &self,
        roots: MigrationRoots,
        request: MigratorHandoffRequest,
        selection: MigrationSelection,
        cancellation: CancellationToken,
    ) {
        let result = (|| {
            let source = probe_bound_source(&roots, &request)?.ok_or_else(|| {
                LegacyMigrationError::UnsupportedSource(
                    "no supported legacy BitFun data was discovered".to_string(),
                )
            })?;
            let engine = migration_engine(roots, &selection)?;
            let plan = engine.plan_with_run_id(
                &source,
                selection.clone(),
                request.run_id.clone(),
                &cancellation,
            )?;
            let blockers = writer_processes(request.caller_process_id)?;
            Ok::<_, LegacyMigrationError>((source, plan, blockers))
        })();

        let mut session = self.lock();
        session.running = false;
        match result {
            Ok((source, plan, blockers)) => {
                session.source = Some(source);
                session.selection = selection;
                session.findings = plan.findings.clone();
                session.plan = Some(plan);
                session.report = None;
                session.blockers = blockers;
                session.status = MigrationRunStatus::Planned;
                session.progress = Some(MigrationProgressEvent {
                    run_id: session.request.run_id.clone(),
                    phase: MigrationPhase::Plan,
                    processed: session.selection.expanded_domains().len() as u64,
                    total: session.selection.expanded_domains().len() as u64,
                    safe_to_cancel: true,
                    code: "plan_ready".to_string(),
                    ..MigrationProgressEvent::default()
                });
                session.error = None;
            }
            Err(error) => {
                self.finish_error_locked(&mut session, &error);
            }
        }
    }

    pub(crate) fn refresh_blockers(&self) -> Result<MigratorView, CommandError> {
        let caller_process_id = self.lock().request.caller_process_id;
        match writer_processes(caller_process_id) {
            Ok(blockers) => {
                let mut session = self.lock();
                session.blockers = blockers;
                session.error = None;
                Ok(snapshot(&session))
            }
            Err(error) => {
                let mut session = self.lock();
                Err(self.finish_error_locked(&mut session, &error))
            }
        }
    }

    pub(crate) fn start(&self, plan_hash: String) -> Result<MigratorView, CommandError> {
        let (roots, request, plan, cancellation) = {
            let mut session = self.lock();
            if session.running {
                return Err(CommandError::operation_in_progress());
            }
            let plan = session.plan.clone().ok_or_else(|| {
                CommandError::new(
                    "plan_required",
                    "Run the preflight plan before starting migration.",
                    true,
                )
            })?;
            if plan.plan_hash != plan_hash {
                return Err(CommandError::new(
                    "stale_plan",
                    "The confirmed plan is no longer the active migration plan.",
                    true,
                ));
            }
            let store = HandoffStore::new(session.roots.clone(), product_id(), RELEASE_CHANNEL);
            let handoff = store
                .load_request(&session.request.run_id, now_ms())
                .map_err(|error| CommandError::from_legacy(&error))?;
            store
                .authorize_plan(&handoff, &plan, now_ms())
                .map_err(|error| CommandError::from_legacy(&error))?;

            session.cancellation = CancellationToken::default();
            session.running = true;
            session.error = None;
            session.status = MigrationRunStatus::WaitingForProcesses;
            session.progress = Some(MigrationProgressEvent {
                run_id: session.request.run_id.clone(),
                phase: MigrationPhase::Acquire,
                processed: 0,
                total: plan.steps.len() as u64,
                safe_to_cancel: true,
                code: "waiting_for_writer_processes".to_string(),
                ..MigrationProgressEvent::default()
            });
            (
                session.roots.clone(),
                session.request.clone(),
                plan,
                session.cancellation.clone(),
            )
        };

        let coordinator = self.clone();
        self.spawn_worker("legacy-data-migration", move || {
            coordinator.execute_background(roots, request, plan, cancellation);
        })
    }

    pub(crate) fn cancel(&self) -> MigratorView {
        let mut session = self.lock();
        session.cancellation.cancel();
        if let Some(progress) = &mut session.progress {
            progress.code = if progress.safe_to_cancel {
                "cancellation_requested".to_string()
            } else {
                "cancellation_pending_safe_boundary".to_string()
            };
        }
        snapshot(&session)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.lock().running
    }

    pub(crate) fn finish_and_restart(
        &self,
        choice: MigrationPromptChoice,
    ) -> Result<(), CommandError> {
        if self.is_running() {
            return Err(CommandError::operation_in_progress());
        }
        if choice == MigrationPromptChoice::Unset {
            return Err(CommandError::new(
                "invalid_prompt_choice",
                "Choose whether to migrate now, be reminded later, or stop reminders.",
                true,
            ));
        }
        if choice == MigrationPromptChoice::MigrateNow && self.lock().report.is_none() {
            return Err(CommandError::new(
                "migration_result_required",
                "A completed or recoverable migration report is required before finishing.",
                true,
            ));
        }

        let result = (|| {
            self.persist_prompt_choice(choice)?;
            self.restart_desktop()
        })();
        if let Err(error) = result {
            let mut session = self.lock();
            let command_error = self.finish_error_locked(&mut session, &error);
            return Err(command_error);
        }
        Ok(())
    }

    pub(crate) fn close_and_restart(&self) -> Result<(), CommandError> {
        let choice = if self.lock().report.is_some() {
            MigrationPromptChoice::MigrateNow
        } else {
            MigrationPromptChoice::RemindLater
        };
        self.finish_and_restart(choice)
    }

    fn begin_operation(
        &self,
        selection: &MigrationSelection,
    ) -> Result<(MigrationRoots, MigratorHandoffRequest, CancellationToken), CommandError> {
        let mut session = self.lock();
        if session.running {
            return Err(CommandError::operation_in_progress());
        }
        validate_selection(&session.request, selection)?;
        session.cancellation = CancellationToken::default();
        session.running = true;
        session.error = None;
        session.progress = Some(MigrationProgressEvent {
            run_id: session.request.run_id.clone(),
            phase: MigrationPhase::Scan,
            processed: 0,
            total: selection.expanded_domains().len() as u64,
            safe_to_cancel: true,
            code: "scanning_source".to_string(),
            ..MigrationProgressEvent::default()
        });
        Ok((
            session.roots.clone(),
            session.request.clone(),
            session.cancellation.clone(),
        ))
    }

    fn spawn_worker(
        &self,
        name: &str,
        worker: impl FnOnce() + Send + 'static,
    ) -> Result<MigratorView, CommandError> {
        if std::thread::Builder::new()
            .name(name.to_string())
            .spawn(worker)
            .is_err()
        {
            let mut session = self.lock();
            session.running = false;
            let error = CommandError::worker_failed();
            session.error = Some(error.clone());
            return Err(error);
        }

        Ok(self.snapshot())
    }

    fn execute_background(
        &self,
        roots: MigrationRoots,
        request: MigratorHandoffRequest,
        plan: MigrationPlan,
        cancellation: CancellationToken,
    ) {
        loop {
            if cancellation.is_cancelled() {
                self.finish_cancelled_before_execution(&plan);
                return;
            }
            match writer_processes(request.caller_process_id) {
                Ok(blockers) => {
                    let done = blockers.is_empty();
                    let mut session = self.lock();
                    session.blockers = blockers;
                    if let Some(progress) = &mut session.progress {
                        progress.code = if done {
                            "writer_processes_stopped".to_string()
                        } else {
                            "waiting_for_writer_processes".to_string()
                        };
                    }
                    drop(session);
                    if done {
                        break;
                    }
                }
                Err(error) => {
                    let mut session = self.lock();
                    session.running = false;
                    self.finish_error_locked(&mut session, &error);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        let engine = match migration_engine(roots.clone(), &plan.selection) {
            Ok(engine) => engine,
            Err(error) => {
                let mut session = self.lock();
                session.running = false;
                self.finish_error_locked(&mut session, &error);
                return;
            }
        };
        let coordinator = self.clone();
        let result = engine.execute_with_progress(
            &plan,
            &cancellation,
            &NoCrashInjection,
            move |progress| coordinator.record_progress(progress),
        );

        let mut session = self.lock();
        session.running = false;
        match result {
            Ok(report) => {
                session.status = report.status;
                session.report = Some(report);
                session.error = None;
                drop(session);
                let _ = self.persist_prompt_choice(MigrationPromptChoice::MigrateNow);
            }
            Err(error) => {
                let layout = MigrationLayout::new(&roots, &plan.run_id);
                if let Ok(Some(report)) =
                    layout.read_json::<MigrationRunReport>(&layout.report_path())
                {
                    session.status = report.status;
                    session.report = Some(report);
                } else if matches!(error, LegacyMigrationError::Cancelled) {
                    session.status = MigrationRunStatus::Cancelled;
                }
                self.finish_error_locked(&mut session, &error);
            }
        }
    }

    fn finish_cancelled_before_execution(&self, plan: &MigrationPlan) {
        let mut session = self.lock();
        session.running = false;
        session.status = MigrationRunStatus::Cancelled;
        session.progress = Some(MigrationProgressEvent {
            run_id: plan.run_id.clone(),
            phase: MigrationPhase::Acquire,
            processed: 0,
            total: plan.steps.len() as u64,
            safe_to_cancel: true,
            code: "migration_cancelled".to_string(),
            ..MigrationProgressEvent::default()
        });
        session.error = Some(CommandError::from_legacy(&LegacyMigrationError::Cancelled));
    }

    fn record_progress(&self, progress: MigrationProgressEvent) {
        let mut session = self.lock();
        session.status = status_for_phase(progress.phase);
        session.progress = Some(progress);
    }

    fn persist_prompt_choice(&self, choice: MigrationPromptChoice) -> LegacyMigrationResult<()> {
        let session = self.lock();
        let store = MigrationOnboardingStore::new(session.roots.clone());
        let request = &session.request;
        let source = session.source.as_ref();
        let has_report = session.report.is_some();
        store.update(|state| {
            state.format_version = CURRENT_MIGRATION_FORMAT_VERSION;
            if let Some(source) = source {
                state.source_fingerprint = source.source_fingerprint.clone();
                state.detected_at_ms.get_or_insert_with(now_ms);
            }
            state.choice = choice;
            state.last_prompted_version = Some(env!("CARGO_PKG_VERSION").to_string());
            state.run_id = Some(request.run_id.clone());
            state.handled_run_id = Some(request.run_id.clone());
            if has_report {
                state.last_report_run_id = Some(request.run_id.clone());
            }
        })?;
        Ok(())
    }

    fn restart_desktop(&self) -> LegacyMigrationResult<()> {
        let run_id = self.lock().request.run_id.clone();
        let current = std::env::current_exe().map_err(|error| LegacyMigrationError::Io {
            path: Path::new(DATA_MIGRATOR_BINARY_NAME).to_path_buf(),
            source: error,
        })?;
        let executable = TrustedInstallationResolver::resolve_sibling(
            &current,
            DATA_MIGRATOR_BINARY_NAME,
            DESKTOP_BINARY_NAME,
            &PlatformExecutableTrustVerifier,
        )?;
        let arguments = [OsStr::new("--legacy-migration-run-id"), OsStr::new(&run_id)];
        launch_trusted_executable(&executable, &arguments)?;
        Ok(())
    }

    fn finish_error_locked(
        &self,
        session: &mut MigratorSession,
        error: &LegacyMigrationError,
    ) -> CommandError {
        session.running = false;
        if matches!(error, LegacyMigrationError::Cancelled) {
            session.status = MigrationRunStatus::Cancelled;
            if let Some(progress) = &mut session.progress {
                progress.safe_to_cancel = true;
                progress.code = "migration_cancelled".to_string();
            }
        }
        let command_error = CommandError::from_legacy(error);
        session.error = Some(command_error.clone());
        command_error
    }

    fn lock(&self) -> MutexGuard<'_, MigratorSession> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn migration_engine(
    roots: MigrationRoots,
    selection: &MigrationSelection,
) -> LegacyMigrationResult<MigrationEngine> {
    MigrationEngine::new(roots, adapters_for_groups(selection))
}

fn writer_processes(caller_process_id: u32) -> LegacyMigrationResult<Vec<WriterProcess>> {
    blocking_writer_processes_for_product(caller_process_id, &[DESKTOP_BINARY_NAME])
}

fn validate_selection(
    request: &MigratorHandoffRequest,
    selection: &MigrationSelection,
) -> Result<(), CommandError> {
    if selection.groups.is_empty() {
        return Err(CommandError::new(
            "empty_selection",
            "Select at least one migration group.",
            true,
        ));
    }
    if request.mode == MigratorRequestMode::Execute && request.selection != *selection {
        return Err(CommandError::new(
            "selection_mismatch",
            "The selected groups differ from the scope confirmed in OpenBitFun.",
            false,
        ));
    }
    Ok(())
}

fn probe_bound_source(
    roots: &MigrationRoots,
    request: &MigratorHandoffRequest,
) -> LegacyMigrationResult<Option<LegacySourceDescriptor>> {
    let source = probe_legacy_source(roots, ProbeLimits::default())?;
    if let Some(source) = &source {
        if request
            .source_id
            .as_deref()
            .is_some_and(|source_id| source_id != source.source_id)
            || request
                .source_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| fingerprint != source.source_fingerprint)
        {
            return Err(LegacyMigrationError::InvalidRequest(
                "discovered source does not match the authenticated handoff".to_string(),
            ));
        }
    } else if request.source_id.is_some() || request.source_fingerprint.is_some() {
        return Err(LegacyMigrationError::InvalidRequest(
            "authenticated handoff source is no longer present".to_string(),
        ));
    }
    Ok(source)
}

fn snapshot(session: &MigratorSession) -> MigratorView {
    let plan = session.plan.as_ref().map(redact_plan_for_ui);
    let report = session.report.as_ref().map(redact_report_for_ui);
    MigratorView {
        delivery_profile: DeliveryProfile::DataMigrator.id().to_string(),
        protocol: MigratorProtocolCapabilities::current(),
        mode: session.request.mode,
        source: session.source.clone(),
        selection: session.selection.clone(),
        findings: session
            .findings
            .iter()
            .cloned()
            .map(redact_finding_for_ui)
            .collect(),
        can_execute: plan.is_some()
            && session
                .source
                .as_ref()
                .is_some_and(|source| source.supported)
            && !session.running,
        plan,
        report,
        progress: session.progress.clone(),
        blockers: session.blockers.clone(),
        status: session.status,
        running: session.running,
        recovery: session.disposition == HandoffDisposition::Recovery,
        error: session.error.clone(),
    }
}

fn redact_finding_for_ui(mut finding: ScanFinding) -> ScanFinding {
    finding.detail = finding.code.replace('_', " ");
    finding
}

fn redact_plan_for_ui(plan: &MigrationPlan) -> MigrationPlan {
    let mut redacted = plan.clone();
    redacted.findings = redacted
        .findings
        .into_iter()
        .map(redact_finding_for_ui)
        .collect();
    for conflict in &mut redacted.conflicts {
        conflict.source_summary = "Legacy item".to_string();
        conflict.target_summary = "Existing OpenBitFun item".to_string();
    }
    redacted
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

fn status_for_phase(phase: MigrationPhase) -> MigrationRunStatus {
    match phase {
        MigrationPhase::Discover => MigrationRunStatus::Discovered,
        MigrationPhase::Scan => MigrationRunStatus::Scanned,
        MigrationPhase::Plan => MigrationRunStatus::Planned,
        MigrationPhase::Acquire => MigrationRunStatus::WaitingForProcesses,
        MigrationPhase::Stage => MigrationRunStatus::Staging,
        MigrationPhase::ValidateStage => MigrationRunStatus::ValidatingStage,
        MigrationPhase::Commit => MigrationRunStatus::Committing,
        MigrationPhase::ValidateCommit => MigrationRunStatus::ValidatingCommit,
        MigrationPhase::Finalize => MigrationRunStatus::Completed,
    }
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
    use openbitfun_product_domains::legacy_migration::{
        MigratorProtocolCapability, MigratorRequestOrigin, CURRENT_MIGRATOR_PROTOCOL_VERSION,
    };
    use std::collections::BTreeSet;
    use std::fs;

    fn fixture_roots(root: &Path) -> MigrationRoots {
        MigrationRoots {
            legacy_user_root: root.join("legacy/user"),
            legacy_home_root: root.join("legacy/home"),
            legacy_skills_root: root.join("legacy/skills"),
            legacy_ssh_root: root.join("legacy/ssh"),
            target_user_root: root.join("target/user"),
            target_home_root: root.join("target/home"),
            target_skills_root: root.join("target/skills"),
            target_ssh_root: root.join("target/ssh"),
        }
    }

    fn handoff_request() -> MigratorHandoffRequest {
        let current = now_ms();
        MigratorHandoffRequest {
            protocol_version: CURRENT_MIGRATOR_PROTOCOL_VERSION,
            mode: MigratorRequestMode::Onboarding,
            origin: MigratorRequestOrigin::FirstLaunch,
            run_id: uuid::Uuid::new_v4().to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
            selection: MigrationSelection::all(),
            caller_process_id: u32::MAX,
            product_id: "openbitfun".to_string(),
            release_channel: "stable".to_string(),
            created_at_ms: current,
            expires_at_ms: current + 60_000,
            required_capabilities: BTreeSet::from([
                MigratorProtocolCapability::ReadOnlyScan,
                MigratorProtocolCapability::JournalRecovery,
            ]),
            ..MigratorHandoffRequest::default()
        }
    }

    fn write_probe_fixture(roots: &MigrationRoots) {
        let config = roots.legacy_user_root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("app.json"), br#"{"version":"0.2.19"}"#).unwrap();
    }

    #[test]
    fn bootstrap_consumes_the_real_non_agent_delivery_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let roots = fixture_roots(temporary.path());
        write_probe_fixture(&roots);
        let request = handoff_request();
        HandoffStore::new(roots.clone(), "openbitfun", "stable")
            .write_request(&request, now_ms())
            .unwrap();

        let coordinator =
            MigratorCoordinator::bootstrap_with(&request.run_id, roots, "openbitfun", "stable")
                .unwrap();
        let view = coordinator.snapshot();

        assert_eq!(view.delivery_profile, "data-migrator");
        assert_eq!(view.mode, MigratorRequestMode::Onboarding);
        assert!(view.source.is_some());
        assert!(!view.recovery);
    }

    #[test]
    fn execute_handoff_rejects_a_scope_change() {
        let mut request = handoff_request();
        request.mode = MigratorRequestMode::Execute;
        let mut changed = request.selection.clone();
        changed
            .groups
            .remove(&openbitfun_product_domains::legacy_migration::MigrationGroupId::Memory);

        let error = validate_selection(&request, &changed).unwrap_err();
        assert_eq!(error.code, "selection_mismatch");
    }

    #[test]
    fn cancelled_scan_finishes_in_an_explicit_cancelled_state() {
        let temporary = tempfile::tempdir().unwrap();
        let roots = fixture_roots(temporary.path());
        write_probe_fixture(&roots);
        let request = handoff_request();
        HandoffStore::new(roots.clone(), "openbitfun", "stable")
            .write_request(&request, now_ms())
            .unwrap();
        let coordinator = MigratorCoordinator::bootstrap_with(
            &request.run_id,
            roots.clone(),
            "openbitfun",
            "stable",
        )
        .unwrap();
        let selection = MigrationSelection::all();
        let (roots, request, cancellation) = coordinator.begin_operation(&selection).unwrap();
        cancellation.cancel();
        coordinator.scan_background(roots, request, selection, cancellation);

        let view = coordinator.snapshot();
        assert!(!view.running);
        assert_eq!(view.status, MigrationRunStatus::Cancelled);
        assert_eq!(
            view.error.map(|error| error.code).as_deref(),
            Some("cancelled")
        );
        assert_eq!(
            view.progress.map(|progress| progress.code).as_deref(),
            Some("migration_cancelled")
        );
    }

    #[test]
    fn command_errors_do_not_expose_storage_paths() {
        let error = LegacyMigrationError::Io {
            path: Path::new("C:/Users/private/secret.json").to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secret"),
        };
        let command = CommandError::from_legacy(&error);
        let serialized = serde_json::to_string(&command).unwrap();

        assert_eq!(command.code, "storage_failed");
        assert!(!serialized.contains("private"));
        assert!(!serialized.contains("secret"));
    }
}
