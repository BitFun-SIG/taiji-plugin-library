use super::common::{
    backup_domain_dir, backup_file_once, io_error, read_bounded_json, read_optional_bounded_json,
    relative_display, restore_unverified_file, stage_domain_dir, validate_regular_file,
};
use crate::service::session_projection_format::validate_runtime_event_log;
use crate::service::workspace::persistence::{
    current_workspace_storage_id, validate_workspace_persistence_data, WorkspacePersistenceData,
    WORKSPACE_PERSISTENCE_FORMAT_VERSION,
};
use crate::service::workspace::{PrimaryAssistantKey, WorkspaceInfo, WorkspaceKind};
use openbitfun_core_types::product_identity::product_id;
use openbitfun_core_types::validate_session_id;
use openbitfun_legacy_migration::{
    atomic_write_bytes, atomic_write_json, DomainContext, DomainScan, LegacyDomainAdapter,
    LegacyMigrationError, LegacyMigrationResult, MigrationRoots,
};
use openbitfun_product_domains::legacy_migration::{
    ConflictResolution, FindingSeverity, MigrationConflict, MigrationDiagnostic, MigrationDomainId,
    MigrationDomainResult, MigrationDomainState, ScanFinding,
};
use openbitfun_services_core::session::{
    OfflineSessionBundle, OfflineSessionImportStore, SessionRelationship, StoredDialogTurnFile,
    StoredSessionMetadataFile, SESSION_STORAGE_SCHEMA_VERSION,
};
use openbitfun_services_core::workspace_identity::{
    canonicalize_local_workspace_root, normalize_remote_workspace_path, LOCAL_WORKSPACE_SSH_HOST,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_SESSION_FILES: usize = 4096;
const MAX_SESSION_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SESSION_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_RUNTIME_EVENT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RUNTIME_DIRECTORIES: usize = 32_768;
const MAX_RUNTIME_DEPTH: usize = 16;

const SESSION_ROOT_FILES: &[&str] = &[
    "state.json",
    "prompt_cache.json",
    "turn-catalog.json",
    "token-anchors.json",
    "session-revert.json",
    "evidence-ledger.json",
];
const SESSION_OWNED_DIRECTORIES: &[&str] = &["snapshots", "artifacts", "tool-results"];

pub(crate) struct WorkspaceSessionsAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionImportAction {
    Import,
    Duplicate,
    TargetWins,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionManifestEntry {
    pub(crate) runtime_relative: String,
    pub(crate) session_id: String,
    pub(crate) action: SessionImportAction,
    pub(crate) expected_hash: String,
    pub(crate) turn_ids: BTreeSet<String>,
    pub(crate) relationship: Option<SessionRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeEventManifestEntry {
    pub(crate) session_id: String,
    pub(crate) action: SessionImportAction,
    pub(crate) expected_hash: String,
    pub(crate) turn_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceSessionsManifest {
    pub(crate) workspace_id_map: BTreeMap<String, String>,
    pub(crate) sessions: Vec<SessionManifestEntry>,
    pub(crate) runtime_events: Vec<RuntimeEventManifestEntry>,
    pub(crate) target_workspace_existed: bool,
    pub(crate) target_workspace_hash: Option<String>,
    pub(crate) skipped_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyWorkspacePersistenceData {
    workspaces: HashMap<String, WorkspaceInfo>,
    #[serde(default)]
    opened_workspace_ids: Vec<String>,
    current_workspace_id: Option<String>,
    #[serde(default)]
    recent_workspaces: Vec<String>,
    #[serde(default)]
    recent_assistant_workspaces: Vec<String>,
    #[serde(default)]
    primary_assistant_key: Option<PrimaryAssistantKey>,
    saved_at: chrono::DateTime<chrono::Utc>,
}

struct WorkspaceSessionsPlan {
    workspace_data: WorkspacePersistenceData,
    workspace_id_map: BTreeMap<String, String>,
    sessions: Vec<PlannedSession>,
    runtime_events: Vec<PlannedRuntimeEvent>,
    conflicts: Vec<MigrationConflict>,
    requires_relocation: Vec<String>,
    skipped_paths: Vec<String>,
    target_workspace_existed: bool,
    target_workspace_hash: Option<String>,
    logical_bytes: u64,
}

struct PlannedSession {
    runtime_relative: PathBuf,
    bundle: OfflineSessionBundle,
    auxiliary_files: Vec<(PathBuf, PathBuf)>,
    action: SessionImportAction,
    expected_hash: String,
}

struct PlannedRuntimeEvent {
    session_id: String,
    source_path: PathBuf,
    action: SessionImportAction,
    expected_hash: String,
    turn_ids: BTreeSet<String>,
}

impl LegacyDomainAdapter for WorkspaceSessionsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::WorkspaceSessions
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let plan = plan_workspace_sessions(roots)?;
        Ok(DomainScan {
            finding: ScanFinding {
                domain: self.domain(),
                code: "legacy_workspace_sessions_supported".to_string(),
                severity: if plan.conflicts.is_empty() {
                    FindingSeverity::Info
                } else {
                    FindingSeverity::Warning
                },
                entity_count: (plan.workspace_id_map.len()
                    + plan.sessions.len()
                    + plan.runtime_events.len()) as u64,
                logical_bytes: plan.logical_bytes,
                source_schema: Some("bitfun.workspace-session.v1".to_string()),
                migratable: true,
                detail: format!(
                    "{} workspaces, {} Sessions, and {} runtime event logs are owner-readable",
                    plan.workspace_id_map.len(),
                    plan.sessions.len(),
                    plan.runtime_events.len()
                ),
            },
            conflicts: plan.conflicts,
            target_schema: Some("openbitfun.workspace-session.current".to_string()),
            dependencies: Vec::new(),
        })
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let plan = plan_workspace_sessions(context.roots)?;
        let domain_root = stage_domain_dir(context, "workspace-sessions");
        atomic_write_json(
            &domain_root.join("workspace_data.json"),
            &plan.workspace_data,
        )?;

        let runtime = offline_runtime()?;
        for session in &plan.sessions {
            if session.action != SessionImportAction::Import {
                continue;
            }
            let sessions_root = domain_root.join("home").join(&session.runtime_relative);
            let store = OfflineSessionImportStore::new(&sessions_root);
            runtime
                .block_on(store.write_bundle(&session.bundle))
                .map_err(|error| owner_error("write staged Session", error))?;
            let staged_session = sessions_root.join(&session.bundle.metadata.session_id);
            for (relative, source) in &session.auxiliary_files {
                let bytes = fs::read(source).map_err(|error| io_error(source, error))?;
                atomic_write_bytes(&staged_session.join(relative), &bytes)?;
            }
            require_tree_hash(&staged_session, &session.expected_hash)?;
        }

        for event in &plan.runtime_events {
            if event.action != SessionImportAction::Import {
                continue;
            }
            let bytes = fs::read(&event.source_path)
                .map_err(|error| io_error(&event.source_path, error))?;
            atomic_write_bytes(
                &domain_root
                    .join("runtime-events")
                    .join(format!("{}.jsonl", event.session_id)),
                &bytes,
            )?;
        }

        let manifest = WorkspaceSessionsManifest {
            workspace_id_map: plan.workspace_id_map,
            sessions: plan.sessions.iter().map(session_manifest_entry).collect(),
            runtime_events: plan
                .runtime_events
                .iter()
                .map(|event| RuntimeEventManifestEntry {
                    session_id: event.session_id.clone(),
                    action: event.action,
                    expected_hash: event.expected_hash.clone(),
                    turn_ids: event.turn_ids.clone(),
                })
                .collect(),
            target_workspace_existed: plan.target_workspace_existed,
            target_workspace_hash: plan.target_workspace_hash,
            skipped_paths: plan.skipped_paths,
        };
        atomic_write_json(&workspace_sessions_manifest_path(context), &manifest)?;

        let imported = manifest
            .sessions
            .iter()
            .filter(|entry| entry.action == SessionImportAction::Import)
            .count()
            + manifest
                .runtime_events
                .iter()
                .filter(|entry| entry.action == SessionImportAction::Import)
                .count()
            + manifest.workspace_id_map.len();
        let skipped = manifest
            .sessions
            .iter()
            .filter(|entry| entry.action != SessionImportAction::Import)
            .count()
            + manifest
                .runtime_events
                .iter()
                .filter(|entry| entry.action != SessionImportAction::Import)
                .count();
        let warnings = manifest
            .skipped_paths
            .iter()
            .map(|path| MigrationDiagnostic {
                code: "session_path_not_migrated".to_string(),
                severity: FindingSeverity::Warning,
                domain: Some(self.domain()),
                relative_path: Some(path.clone()),
                message: "A non-owned or rebuildable Session path was left in the legacy source"
                    .to_string(),
                action: Some("Review the migration report before removing legacy data".to_string()),
            })
            .collect();
        Ok(MigrationDomainResult {
            domain: self.domain(),
            state: MigrationDomainState::Staged,
            imported: imported as u64,
            skipped: skipped as u64,
            conflicts: plan.conflicts.len() as u64,
            warnings,
            requires_relocation: plan.requires_relocation,
            ..MigrationDomainResult::default()
        })
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_workspace_sessions_manifest(context)?;
        let domain_root = stage_domain_dir(context, "workspace-sessions");
        let workspace_data: WorkspacePersistenceData = read_bounded_json(
            &context.layout.stage_root(),
            &domain_root.join("workspace_data.json"),
        )?;
        validate_workspace_persistence_data(
            &workspace_data,
            &context.roots.target_user_root.join("data/miniapps"),
        )
        .map_err(|error| owner_error("validate staged Workspace registry", error))?;

        let runtime = offline_runtime()?;
        for entry in imported_sessions(&manifest) {
            let sessions_root = domain_root
                .join("home")
                .join(path_from_manifest(&entry.runtime_relative)?);
            let store = OfflineSessionImportStore::new(&sessions_root);
            let bundle = runtime
                .block_on(store.load_bundle(&entry.session_id))
                .map_err(|error| owner_error("read staged Session", error))?
                .ok_or_else(|| {
                    LegacyMigrationError::InvalidRequest(format!(
                        "staged Session is missing: {}",
                        entry.session_id
                    ))
                })?;
            bundle
                .validate()
                .map_err(|error| owner_error("validate staged Session", error))?;
            require_tree_hash(&sessions_root.join(&entry.session_id), &entry.expected_hash)?;
        }
        for entry in imported_runtime_events(&manifest) {
            let path = domain_root
                .join("runtime-events")
                .join(format!("{}.jsonl", entry.session_id));
            validate_runtime_event_log(&path, &entry.session_id)
                .map_err(|error| owner_error("validate staged runtime event log", error))?;
            require_file_hash(&path, &entry.expected_hash)?;
        }
        validate_session_relationship_closure(&manifest)
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_workspace_sessions_manifest(context)?;
        let domain_root = stage_domain_dir(context, "workspace-sessions");
        let target_workspace = target_workspace_data_path(context.roots);
        let staged_workspace = domain_root.join("workspace_data.json");
        let staged_workspace_hash = hash_file(&staged_workspace)?;
        let workspace_already_applied = if target_workspace.exists() {
            validate_regular_file(&context.roots.target_user_root, &target_workspace)?;
            hash_file(&target_workspace)? == staged_workspace_hash
        } else {
            false
        };
        if !workspace_already_applied {
            verify_planned_file_state(
                &target_workspace,
                manifest.target_workspace_existed,
                manifest.target_workspace_hash.as_deref(),
            )?;
            backup_file_once(
                &target_workspace,
                &backup_domain_dir(context, "workspace-sessions").join("workspace_data.json"),
            )?;
            let workspace_bytes =
                fs::read(&staged_workspace).map_err(|error| io_error(&staged_workspace, error))?;
            atomic_write_bytes(&target_workspace, &workspace_bytes)?;
        }

        for entry in imported_sessions(&manifest) {
            let relative = path_from_manifest(&entry.runtime_relative)?;
            let staged = domain_root
                .join("home")
                .join(&relative)
                .join(&entry.session_id);
            let target = context
                .roots
                .target_home_root
                .join(&relative)
                .join(&entry.session_id);
            install_directory_idempotent(
                &staged,
                &target,
                &entry.expected_hash,
                &context.plan.run_id,
            )?;
        }
        for entry in imported_runtime_events(&manifest) {
            let staged = domain_root
                .join("runtime-events")
                .join(format!("{}.jsonl", entry.session_id));
            let target = context
                .roots
                .target_home_root
                .join("runtime-events")
                .join(format!("{}.jsonl", entry.session_id));
            install_file_idempotent(&staged, &target, &entry.expected_hash)?;
        }
        Ok(())
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_workspace_sessions_manifest(context)?;
        let expected: WorkspacePersistenceData = read_bounded_json(
            &context.layout.stage_root(),
            &stage_domain_dir(context, "workspace-sessions").join("workspace_data.json"),
        )?;
        let actual: WorkspacePersistenceData = read_bounded_json(
            &context.roots.target_user_root,
            &target_workspace_data_path(context.roots),
        )?;
        validate_workspace_persistence_data(
            &actual,
            &context.roots.target_user_root.join("data/miniapps"),
        )
        .map_err(|error| owner_error("validate committed Workspace registry", error))?;
        if serde_json::to_value(&expected).map_err(json_error)?
            != serde_json::to_value(&actual).map_err(json_error)?
        {
            return Err(LegacyMigrationError::InvalidRequest(
                "committed Workspace registry differs from the staged owner output".to_string(),
            ));
        }

        let runtime = offline_runtime()?;
        for entry in imported_sessions(&manifest) {
            let relative = path_from_manifest(&entry.runtime_relative)?;
            let sessions_root = context.roots.target_home_root.join(&relative);
            let store = OfflineSessionImportStore::new(&sessions_root);
            let bundle = runtime
                .block_on(store.load_bundle(&entry.session_id))
                .map_err(|error| owner_error("read committed Session", error))?
                .ok_or_else(|| {
                    LegacyMigrationError::InvalidRequest(format!(
                        "committed Session is missing: {}",
                        entry.session_id
                    ))
                })?;
            bundle
                .validate()
                .map_err(|error| owner_error("validate committed Session", error))?;
            require_tree_hash(&sessions_root.join(&entry.session_id), &entry.expected_hash)?;
        }
        for entry in imported_runtime_events(&manifest) {
            let path = context
                .roots
                .target_home_root
                .join("runtime-events")
                .join(format!("{}.jsonl", entry.session_id));
            validate_runtime_event_log(&path, &entry.session_id)
                .map_err(|error| owner_error("validate committed runtime event log", error))?;
            require_file_hash(&path, &entry.expected_hash)?;
        }
        Ok(())
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let Some(manifest) = read_optional_bounded_json::<WorkspaceSessionsManifest>(
            &context.layout.stage_root(),
            &workspace_sessions_manifest_path(context),
        )?
        else {
            return Ok(());
        };
        restore_unverified_file(
            &target_workspace_data_path(context.roots),
            &backup_domain_dir(context, "workspace-sessions").join("workspace_data.json"),
            manifest.target_workspace_existed,
        )?;
        let domain_root = stage_domain_dir(context, "workspace-sessions");
        for entry in imported_sessions(&manifest) {
            let relative = path_from_manifest(&entry.runtime_relative)?;
            remove_directory_if_matches(
                &domain_root
                    .join("home")
                    .join(&relative)
                    .join(&entry.session_id),
                &context
                    .roots
                    .target_home_root
                    .join(relative)
                    .join(&entry.session_id),
            )?;
        }
        for entry in imported_runtime_events(&manifest) {
            remove_file_if_matches(
                &domain_root
                    .join("runtime-events")
                    .join(format!("{}.jsonl", entry.session_id)),
                &context
                    .roots
                    .target_home_root
                    .join("runtime-events")
                    .join(format!("{}.jsonl", entry.session_id)),
            )?;
        }
        Ok(())
    }
}

fn plan_workspace_sessions(roots: &MigrationRoots) -> LegacyMigrationResult<WorkspaceSessionsPlan> {
    let source_workspace_path = source_workspace_data_path(roots);
    let legacy: LegacyWorkspacePersistenceData =
        read_bounded_json(&roots.legacy_user_root, &source_workspace_path)?;
    let target_workspace_path = target_workspace_data_path(roots);
    let target = read_optional_bounded_json::<WorkspacePersistenceData>(
        &roots.target_user_root,
        &target_workspace_path,
    )?;
    if let Some(target) = &target {
        validate_workspace_persistence_data(target, &roots.target_user_root.join("data/miniapps"))
            .map_err(|error| owner_error("read current Workspace registry", error))?;
    }

    let mut workspace_id_map = BTreeMap::new();
    let mut converted = Vec::new();
    let mut source_workspace_ids = legacy.workspaces.keys().cloned().collect::<Vec<_>>();
    source_workspace_ids.sort();
    let mut requires_relocation = Vec::new();
    for source_id in source_workspace_ids {
        let mut workspace = legacy.workspaces[&source_id].clone();
        normalize_legacy_workspace_for_current(&mut workspace)?;
        let target_id = current_workspace_storage_id(&workspace)
            .map_err(|error| owner_error("convert legacy Workspace id", error))?;
        workspace.id = target_id.clone();
        if workspace.workspace_kind != WorkspaceKind::Remote && !workspace.root_path.exists() {
            requires_relocation.push(target_id.clone());
        }
        workspace_id_map.insert(source_id, target_id.clone());
        converted.push((target_id, workspace));
    }

    let mut conflicts = Vec::new();
    let mut output_workspaces = target
        .as_ref()
        .map(|value| value.workspaces.clone())
        .unwrap_or_default();
    for (target_id, workspace) in converted {
        if let Some(existing) = output_workspaces.get(&target_id) {
            if serde_json::to_value(existing).map_err(json_error)?
                != serde_json::to_value(&workspace).map_err(json_error)?
            {
                conflicts.push(MigrationConflict {
                    domain: MigrationDomainId::WorkspaceSessions,
                    code: "workspace_target_wins".to_string(),
                    source_summary: format!("legacy Workspace {target_id}"),
                    target_summary: format!("current Workspace {target_id}"),
                    resolution: ConflictResolution::TargetWins,
                });
            }
        } else {
            output_workspaces.insert(target_id, workspace);
        }
    }

    let mut opened_workspace_ids = merge_reference_list(
        target.as_ref().map(|value| &value.opened_workspace_ids),
        &legacy.opened_workspace_ids,
        &workspace_id_map,
        &output_workspaces,
    );
    let recent_workspaces = merge_reference_list(
        target.as_ref().map(|value| &value.recent_workspaces),
        &legacy.recent_workspaces,
        &workspace_id_map,
        &output_workspaces,
    );
    let recent_assistant_workspaces = merge_reference_list(
        target
            .as_ref()
            .map(|value| &value.recent_assistant_workspaces),
        &legacy.recent_assistant_workspaces,
        &workspace_id_map,
        &output_workspaces,
    );
    let source_current = legacy
        .current_workspace_id
        .as_ref()
        .and_then(|id| workspace_id_map.get(id))
        .filter(|id| output_workspaces.contains_key(*id))
        .cloned();
    let current_workspace_id = target
        .as_ref()
        .and_then(|value| value.current_workspace_id.clone())
        .or(source_current);
    if let Some(current_id) = current_workspace_id.as_ref() {
        if !opened_workspace_ids.iter().any(|id| id == current_id) {
            opened_workspace_ids.push(current_id.clone());
        }
    }
    let workspace_data = WorkspacePersistenceData {
        format_version: WORKSPACE_PERSISTENCE_FORMAT_VERSION,
        product_id: product_id().to_string(),
        workspaces: output_workspaces,
        opened_workspace_ids,
        current_workspace_id,
        recent_workspaces,
        recent_assistant_workspaces,
        primary_assistant_key: target
            .as_ref()
            .and_then(|value| value.primary_assistant_key.clone())
            .or(legacy.primary_assistant_key),
        saved_at: target
            .as_ref()
            .map(|value| value.saved_at)
            .unwrap_or(legacy.saved_at),
    };
    validate_workspace_persistence_data(
        &workspace_data,
        &roots.target_user_root.join("data/miniapps"),
    )
    .map_err(|error| owner_error("convert legacy Workspace registry", error))?;

    let target_sessions = index_target_sessions(roots)?;
    let (sessions, mut skipped_paths) = plan_sessions(roots, &target_sessions, &mut conflicts)?;
    let runtime_events = plan_runtime_events(roots, &sessions, &mut conflicts, &mut skipped_paths)?;
    let session_bytes = sessions.iter().try_fold(0u64, |total, session| {
        expected_session_bytes(session).map(|bytes| total.saturating_add(bytes))
    })?;
    let event_bytes = runtime_events
        .iter()
        .map(|event| {
            fs::metadata(&event.source_path)
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        })
        .sum::<u64>();
    let workspace_bytes = fs::metadata(&source_workspace_path)
        .map_err(|error| io_error(&source_workspace_path, error))?
        .len();
    let target_workspace_existed = target_workspace_path.exists();
    let target_workspace_hash = target_workspace_existed
        .then(|| hash_file(&target_workspace_path))
        .transpose()?;

    Ok(WorkspaceSessionsPlan {
        workspace_data,
        workspace_id_map,
        sessions,
        runtime_events,
        conflicts,
        requires_relocation,
        skipped_paths,
        target_workspace_existed,
        target_workspace_hash,
        logical_bytes: workspace_bytes
            .saturating_add(session_bytes)
            .saturating_add(event_bytes),
    })
}

fn normalize_legacy_workspace_for_current(
    workspace: &mut WorkspaceInfo,
) -> LegacyMigrationResult<()> {
    if workspace.workspace_kind == WorkspaceKind::Remote {
        let normalized = normalize_remote_workspace_path(&workspace.root_path.to_string_lossy());
        if !normalized.starts_with('/') {
            return Err(LegacyMigrationError::UnsupportedSource(format!(
                "remote Workspace {} does not use an absolute POSIX root",
                workspace.id
            )));
        }
        workspace.root_path = PathBuf::from(normalized);
    } else {
        workspace.metadata.insert(
            "sshHost".to_string(),
            serde_json::Value::String(LOCAL_WORKSPACE_SSH_HOST.to_string()),
        );
        if workspace.root_path.exists() {
            let (canonical, _) = canonicalize_local_workspace_root(&workspace.root_path)
                .map_err(|error| owner_error("canonicalize legacy Workspace root", error))?;
            workspace.root_path = canonical;
        }
    }
    Ok(())
}

fn plan_sessions(
    roots: &MigrationRoots,
    target_sessions: &HashMap<String, Vec<PathBuf>>,
    conflicts: &mut Vec<MigrationConflict>,
) -> LegacyMigrationResult<(Vec<PlannedSession>, Vec<String>)> {
    let session_roots = find_session_roots(&roots.legacy_home_root)?;
    let mut sessions = Vec::new();
    let mut skipped_paths = Vec::new();
    let mut source_ids = HashMap::<String, String>::new();
    for sessions_root in session_roots {
        let runtime_relative = sessions_root
            .strip_prefix(&roots.legacy_home_root)
            .map_err(|_| LegacyMigrationError::PathEscape(sessions_root.clone()))?
            .to_path_buf();
        for session_dir in child_directories(&sessions_root)? {
            let session_id = file_name(&session_dir)?;
            validate_session_id(&session_id).map_err(|error| {
                LegacyMigrationError::UnsupportedSource(format!(
                    "legacy Session id is unsafe: {error}"
                ))
            })?;
            let metadata_path = session_dir.join("metadata.json");
            match fs::symlink_metadata(&metadata_path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    skipped_paths.push(relative_display(&roots.legacy_home_root, &session_dir));
                    continue;
                }
                Err(error) => return Err(io_error(&metadata_path, error)),
            }
            let metadata_file: StoredSessionMetadataFile =
                read_bounded_json(&roots.legacy_home_root, &metadata_path)?;
            if metadata_file.schema_version > SESSION_STORAGE_SCHEMA_VERSION {
                return Err(LegacyMigrationError::UnsupportedSource(format!(
                    "Session {session_id} uses unsupported schema {}",
                    metadata_file.schema_version
                )));
            }
            if metadata_file.metadata.session_id != session_id {
                return Err(LegacyMigrationError::UnsupportedSource(format!(
                    "Session directory id does not match its metadata: {session_id}"
                )));
            }
            let turns = read_session_turns(&roots.legacy_home_root, &session_dir, &session_id)?;
            let bundle = OfflineSessionBundle {
                metadata: metadata_file.metadata,
                turns,
            };
            bundle
                .validate()
                .map_err(|error| owner_error("validate legacy Session", error))?;
            let auxiliary_files = collect_auxiliary_session_files(
                &roots.legacy_home_root,
                &session_dir,
                &mut skipped_paths,
            )?;
            let expected_hash = expected_session_hash(&bundle, &auxiliary_files)?;
            if auxiliary_files
                .len()
                .saturating_add(bundle.turns.len())
                .saturating_add(1)
                > MAX_SESSION_FILES
            {
                return Err(LegacyMigrationError::ResourceLimit(format!(
                    "Session contains more than {MAX_SESSION_FILES} files: {}",
                    session_dir.display()
                )));
            }
            if expected_bundle_bytes(&bundle, &auxiliary_files)? > MAX_SESSION_BYTES {
                return Err(LegacyMigrationError::ResourceLimit(format!(
                    "Session exceeds {MAX_SESSION_BYTES} bytes: {}",
                    session_dir.display()
                )));
            }
            if let Some(previous_hash) =
                source_ids.insert(session_id.clone(), expected_hash.clone())
            {
                if previous_hash != expected_hash {
                    return Err(LegacyMigrationError::UnsupportedSource(format!(
                        "legacy Session id appears with different contents: {session_id}"
                    )));
                }
                skipped_paths.push(relative_display(&roots.legacy_home_root, &session_dir));
                continue;
            }

            let target_same_path = roots
                .target_home_root
                .join(&runtime_relative)
                .join(&session_id);
            let action = match target_sessions.get(&session_id) {
                None => SessionImportAction::Import,
                Some(paths)
                    if paths.len() == 1
                        && paths[0] == target_same_path
                        && hash_tree(&target_same_path)? == expected_hash =>
                {
                    SessionImportAction::Duplicate
                }
                Some(_) => {
                    conflicts.push(MigrationConflict {
                        domain: MigrationDomainId::WorkspaceSessions,
                        code: "session_target_wins".to_string(),
                        source_summary: format!("legacy Session {session_id}"),
                        target_summary: format!("current Session {session_id}"),
                        resolution: ConflictResolution::TargetWins,
                    });
                    SessionImportAction::TargetWins
                }
            };
            sessions.push(PlannedSession {
                runtime_relative: runtime_relative.clone(),
                bundle,
                auxiliary_files,
                action,
                expected_hash,
            });
        }
    }
    sessions.sort_by(|left, right| {
        (&left.runtime_relative, &left.bundle.metadata.session_id)
            .cmp(&(&right.runtime_relative, &right.bundle.metadata.session_id))
    });
    Ok((sessions, skipped_paths))
}

fn plan_runtime_events(
    roots: &MigrationRoots,
    sessions: &[PlannedSession],
    conflicts: &mut Vec<MigrationConflict>,
    skipped_paths: &mut Vec<String>,
) -> LegacyMigrationResult<Vec<PlannedRuntimeEvent>> {
    let source_root = roots.legacy_home_root.join("runtime-events");
    if !source_root.exists() {
        return Ok(Vec::new());
    }
    reject_linked_directory(&source_root)?;
    let known_sessions = sessions
        .iter()
        .map(|session| (session.bundle.metadata.session_id.as_str(), session.action))
        .collect::<HashMap<_, _>>();
    let mut planned = Vec::new();
    for entry in read_dir_sorted(&source_root)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(path));
        }
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            skipped_paths.push(relative_display(&roots.legacy_home_root, &path));
            continue;
        }
        if metadata.len() > MAX_RUNTIME_EVENT_BYTES {
            return Err(LegacyMigrationError::ResourceLimit(format!(
                "runtime event log exceeds {MAX_RUNTIME_EVENT_BYTES} bytes: {}",
                relative_display(&roots.legacy_home_root, &path)
            )));
        }
        validate_regular_file(&roots.legacy_home_root, &path)?;
        let session_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                LegacyMigrationError::UnsupportedSource(
                    "runtime event log name is not valid UTF-8".to_string(),
                )
            })?
            .to_string();
        let Some(session_action) = known_sessions.get(session_id.as_str()).copied() else {
            skipped_paths.push(relative_display(&roots.legacy_home_root, &path));
            continue;
        };
        let summary = validate_runtime_event_log(&path, &session_id)
            .map_err(|error| owner_error("read legacy runtime event log", error))?;
        let expected_hash = hash_file(&path)?;
        let target = roots
            .target_home_root
            .join("runtime-events")
            .join(format!("{session_id}.jsonl"));
        let action = if session_action == SessionImportAction::TargetWins {
            SessionImportAction::TargetWins
        } else if !target.exists() {
            SessionImportAction::Import
        } else if hash_file(&target)? == expected_hash {
            SessionImportAction::Duplicate
        } else {
            conflicts.push(MigrationConflict {
                domain: MigrationDomainId::WorkspaceSessions,
                code: "runtime_event_target_wins".to_string(),
                source_summary: format!("legacy runtime event log for {session_id}"),
                target_summary: format!("current runtime event log for {session_id}"),
                resolution: ConflictResolution::TargetWins,
            });
            SessionImportAction::TargetWins
        };
        planned.push(PlannedRuntimeEvent {
            session_id,
            source_path: path,
            action,
            expected_hash,
            turn_ids: summary.turn_ids,
        });
    }
    planned.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    Ok(planned)
}

fn read_session_turns(
    legacy_home_root: &Path,
    session_dir: &Path,
    session_id: &str,
) -> LegacyMigrationResult<Vec<openbitfun_services_core::session::DialogTurnData>> {
    let turns_dir = session_dir.join("turns");
    if !turns_dir.exists() {
        return Ok(Vec::new());
    }
    reject_linked_directory(&turns_dir)?;
    let mut turns = Vec::new();
    for entry in read_dir_sorted(&turns_dir)? {
        let path = entry.path();
        validate_regular_file(legacy_home_root, &path)?;
        let file_name = file_name(&path)?;
        let file_index = file_name
            .strip_prefix("turn-")
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                LegacyMigrationError::UnsupportedSource(format!(
                    "unsupported legacy Turn filename: {file_name}"
                ))
            })?;
        let stored: StoredDialogTurnFile = read_bounded_json(legacy_home_root, &path)?;
        if stored.schema_version > SESSION_STORAGE_SCHEMA_VERSION {
            return Err(LegacyMigrationError::UnsupportedSource(format!(
                "Session {session_id} Turn uses unsupported schema {}",
                stored.schema_version
            )));
        }
        if stored.turn.session_id != session_id || stored.turn.turn_index != file_index {
            return Err(LegacyMigrationError::UnsupportedSource(format!(
                "legacy Turn identity does not match its storage path in Session {session_id}"
            )));
        }
        turns.push(stored.turn);
    }
    turns.sort_by_key(|turn| turn.turn_index);
    Ok(turns)
}

fn collect_auxiliary_session_files(
    legacy_home_root: &Path,
    session_dir: &Path,
    skipped_paths: &mut Vec<String>,
) -> LegacyMigrationResult<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    for entry in read_dir_sorted(session_dir)? {
        let path = entry.path();
        let name = file_name(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(path));
        }
        if metadata.is_file() {
            if name == "metadata.json" {
                continue;
            }
            if SESSION_ROOT_FILES.contains(&name.as_str()) {
                validate_owned_file(legacy_home_root, session_dir, &path, &mut files)?;
            } else {
                skipped_paths.push(relative_display(legacy_home_root, &path));
            }
        } else if metadata.is_dir() {
            if name == "turns" {
                continue;
            }
            if SESSION_OWNED_DIRECTORIES.contains(&name.as_str()) {
                collect_owned_directory(legacy_home_root, session_dir, &path, 0, &mut files)?;
            } else {
                skipped_paths.push(relative_display(legacy_home_root, &path));
            }
        } else {
            skipped_paths.push(relative_display(legacy_home_root, &path));
        }
    }
    enforce_session_limits(session_dir, &files)?;
    Ok(files)
}

fn collect_owned_directory(
    legacy_home_root: &Path,
    session_dir: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> LegacyMigrationResult<()> {
    if depth > MAX_RUNTIME_DEPTH {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "Session directory depth exceeds {MAX_RUNTIME_DEPTH}: {}",
            relative_display(legacy_home_root, directory)
        )));
    }
    reject_linked_directory(directory)?;
    for entry in read_dir_sorted(directory)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(path));
        }
        if metadata.is_dir() {
            collect_owned_directory(legacy_home_root, session_dir, &path, depth + 1, files)?;
        } else if metadata.is_file() {
            validate_owned_file(legacy_home_root, session_dir, &path, files)?;
        }
    }
    Ok(())
}

fn validate_owned_file(
    legacy_home_root: &Path,
    session_dir: &Path,
    path: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> LegacyMigrationResult<()> {
    validate_regular_file(legacy_home_root, path)?;
    let relative = path
        .strip_prefix(session_dir)
        .map_err(|_| LegacyMigrationError::PathEscape(path.to_path_buf()))?
        .to_path_buf();
    files.push((relative, path.to_path_buf()));
    Ok(())
}

fn enforce_session_limits(
    session_dir: &Path,
    files: &[(PathBuf, PathBuf)],
) -> LegacyMigrationResult<()> {
    let mut total = 0u64;
    for (_, path) in files {
        let size = fs::metadata(path)
            .map_err(|error| io_error(path, error))?
            .len();
        if size > MAX_SESSION_FILE_BYTES {
            return Err(LegacyMigrationError::ResourceLimit(format!(
                "Session file exceeds {MAX_SESSION_FILE_BYTES} bytes: {}",
                path.display()
            )));
        }
        total = total.saturating_add(size);
    }
    if total > MAX_SESSION_BYTES {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "Session exceeds {MAX_SESSION_BYTES} bytes: {}",
            session_dir.display()
        )));
    }
    Ok(())
}

fn index_target_sessions(
    roots: &MigrationRoots,
) -> LegacyMigrationResult<HashMap<String, Vec<PathBuf>>> {
    let mut by_id = HashMap::<String, Vec<PathBuf>>::new();
    for sessions_root in find_session_roots(&roots.target_home_root)? {
        for session_dir in child_directories(&sessions_root)? {
            by_id
                .entry(file_name(&session_dir)?)
                .or_default()
                .push(session_dir);
        }
    }
    for paths in by_id.values_mut() {
        paths.sort();
    }
    Ok(by_id)
}

fn find_session_roots(home_root: &Path) -> LegacyMigrationResult<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    for name in ["projects", "remote_ssh", "personal_assistant"] {
        let root = home_root.join(name);
        if root.exists() {
            find_session_roots_recursive(&root, 0, &mut visited, &mut found)?;
        }
    }
    found.sort();
    Ok(found)
}

fn find_session_roots_recursive(
    directory: &Path,
    depth: usize,
    visited: &mut usize,
    found: &mut Vec<PathBuf>,
) -> LegacyMigrationResult<()> {
    if depth > MAX_RUNTIME_DEPTH {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "workspace runtime depth exceeds {MAX_RUNTIME_DEPTH}: {}",
            directory.display()
        )));
    }
    *visited = visited.saturating_add(1);
    if *visited > MAX_RUNTIME_DIRECTORIES {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "workspace runtime contains more than {MAX_RUNTIME_DIRECTORIES} directories"
        )));
    }
    reject_linked_directory(directory)?;
    if directory.file_name().and_then(|value| value.to_str()) == Some("sessions") {
        found.push(directory.to_path_buf());
        return Ok(());
    }
    for child in child_directories(directory)? {
        find_session_roots_recursive(&child, depth + 1, visited, found)?;
    }
    Ok(())
}

fn child_directories(directory: &Path) -> LegacyMigrationResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in read_dir_sorted(directory)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(path));
        }
        if metadata.is_dir() {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn read_dir_sorted(directory: &Path) -> LegacyMigrationResult<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| io_error(directory, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error(directory, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn reject_linked_directory(path: &Path) -> LegacyMigrationResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(LegacyMigrationError::LinkedPath(path.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(LegacyMigrationError::UnsupportedSource(format!(
            "expected a directory at {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn merge_reference_list(
    target: Option<&Vec<String>>,
    source: &[String],
    id_map: &BTreeMap<String, String>,
    workspaces: &HashMap<String, WorkspaceInfo>,
) -> Vec<String> {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for id in target
        .into_iter()
        .flatten()
        .cloned()
        .chain(source.iter().filter_map(|id| id_map.get(id)).cloned())
    {
        if workspaces.contains_key(&id) && seen.insert(id.clone()) {
            merged.push(id);
        }
    }
    merged
}

fn expected_session_hash(
    bundle: &OfflineSessionBundle,
    auxiliary_files: &[(PathBuf, PathBuf)],
) -> LegacyMigrationResult<String> {
    let mut entries = Vec::new();
    entries.push((
        PathBuf::from("metadata.json"),
        serde_json::to_vec(&StoredSessionMetadataFile::new(bundle.metadata.clone()))
            .map_err(json_error)?,
    ));
    for turn in &bundle.turns {
        entries.push((
            PathBuf::from("turns").join(format!("turn-{:04}.json", turn.turn_index)),
            serde_json::to_vec(&StoredDialogTurnFile::new(turn.clone())).map_err(json_error)?,
        ));
    }
    for (relative, source) in auxiliary_files {
        entries.push((
            relative.clone(),
            fs::read(source).map_err(|error| io_error(source, error))?,
        ));
    }
    Ok(hash_entries(entries))
}

fn expected_session_bytes(session: &PlannedSession) -> LegacyMigrationResult<u64> {
    expected_bundle_bytes(&session.bundle, &session.auxiliary_files)
}

fn expected_bundle_bytes(
    bundle: &OfflineSessionBundle,
    auxiliary_files: &[(PathBuf, PathBuf)],
) -> LegacyMigrationResult<u64> {
    let metadata_bytes =
        serde_json::to_vec(&StoredSessionMetadataFile::new(bundle.metadata.clone()))
            .map_err(json_error)?
            .len() as u64;
    let turn_bytes = bundle.turns.iter().try_fold(0u64, |total, turn| {
        serde_json::to_vec(&StoredDialogTurnFile::new(turn.clone()))
            .map(|bytes| total.saturating_add(bytes.len() as u64))
            .map_err(json_error)
    })?;
    let auxiliary_bytes = auxiliary_files.iter().try_fold(
        0u64,
        |total, (_, path)| -> LegacyMigrationResult<u64> {
            let bytes = fs::metadata(path)
                .map_err(|error| io_error(path, error))?
                .len();
            Ok(total.saturating_add(bytes))
        },
    )?;
    Ok(metadata_bytes
        .saturating_add(turn_bytes)
        .saturating_add(auxiliary_bytes))
}

pub(crate) fn target_wins_session_ids(
    roots: &MigrationRoots,
) -> LegacyMigrationResult<BTreeSet<String>> {
    Ok(plan_workspace_sessions(roots)?
        .sessions
        .into_iter()
        .filter(|session| session.action == SessionImportAction::TargetWins)
        .map(|session| session.bundle.metadata.session_id)
        .collect())
}

fn hash_entries(mut entries: Vec<(PathBuf, Vec<u8>)>) -> String {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, bytes) in entries {
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn hash_tree(root: &Path) -> LegacyMigrationResult<String> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, 0, &mut entries)?;
    Ok(hash_entries(entries))
}

fn collect_tree_entries(
    root: &Path,
    directory: &Path,
    depth: usize,
    entries: &mut Vec<(PathBuf, Vec<u8>)>,
) -> LegacyMigrationResult<()> {
    if depth > MAX_RUNTIME_DEPTH {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "target tree depth exceeds {MAX_RUNTIME_DEPTH}: {}",
            root.display()
        )));
    }
    reject_linked_directory(directory)?;
    for entry in read_dir_sorted(directory)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(path));
        }
        if metadata.is_dir() {
            collect_tree_entries(root, &path, depth + 1, entries)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| LegacyMigrationError::PathEscape(path.clone()))?
                .to_path_buf();
            entries.push((
                relative,
                fs::read(&path).map_err(|error| io_error(&path, error))?,
            ));
        }
        if entries.len() > MAX_SESSION_FILES {
            return Err(LegacyMigrationError::ResourceLimit(format!(
                "target tree contains more than {MAX_SESSION_FILES} files: {}",
                root.display()
            )));
        }
    }
    Ok(())
}

fn require_tree_hash(path: &Path, expected: &str) -> LegacyMigrationResult<()> {
    let actual = hash_tree(path)?;
    if actual != expected {
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "Session tree hash mismatch at {}",
            path.display()
        )));
    }
    Ok(())
}

fn hash_file(path: &Path) -> LegacyMigrationResult<String> {
    fs::read(path)
        .map(|bytes| format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
        .map_err(|error| io_error(path, error))
}

fn require_file_hash(path: &Path, expected: &str) -> LegacyMigrationResult<()> {
    if hash_file(path)? != expected {
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "file hash mismatch at {}",
            path.display()
        )));
    }
    Ok(())
}

fn install_directory_idempotent(
    staged: &Path,
    target: &Path,
    expected_hash: &str,
    run_id: &str,
) -> LegacyMigrationResult<()> {
    if target.exists() {
        if hash_tree(target)? == expected_hash {
            return Ok(());
        }
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "target changed after planning: {}",
            target.display()
        )));
    }
    let parent = target.parent().ok_or_else(|| {
        LegacyMigrationError::InvalidRequest(format!("target has no parent: {}", target.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temp = parent.join(format!(
        ".migration-{}-{}",
        safe_component(run_id),
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| io_error(&temp, error))?;
    }
    let install_result = copy_tree(staged, &temp)
        .and_then(|()| fs::rename(&temp, target).map_err(|error| io_error(target, error)));
    if install_result.is_err() && temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| io_error(&temp, error))?;
    }
    install_result
}

fn copy_tree(source: &Path, target: &Path) -> LegacyMigrationResult<()> {
    reject_linked_directory(source)?;
    fs::create_dir_all(target).map_err(|error| io_error(target, error))?;
    for entry in read_dir_sorted(source)? {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata =
            fs::symlink_metadata(&source_path).map_err(|error| io_error(&source_path, error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(LegacyMigrationError::LinkedPath(source_path));
        }
        if metadata.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&source_path).map_err(|error| io_error(&source_path, error))?;
            atomic_write_bytes(&target_path, &bytes)?;
        }
    }
    Ok(())
}

fn install_file_idempotent(
    staged: &Path,
    target: &Path,
    expected_hash: &str,
) -> LegacyMigrationResult<()> {
    if target.exists() {
        if hash_file(target)? == expected_hash {
            return Ok(());
        }
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "target changed after planning: {}",
            target.display()
        )));
    }
    let bytes = fs::read(staged).map_err(|error| io_error(staged, error))?;
    atomic_write_bytes(target, &bytes)
}

fn remove_directory_if_matches(staged: &Path, target: &Path) -> LegacyMigrationResult<()> {
    if staged.exists() && target.exists() && hash_tree(staged)? == hash_tree(target)? {
        fs::remove_dir_all(target).map_err(|error| io_error(target, error))?;
    }
    Ok(())
}

fn remove_file_if_matches(staged: &Path, target: &Path) -> LegacyMigrationResult<()> {
    if staged.exists() && target.exists() && hash_file(staged)? == hash_file(target)? {
        fs::remove_file(target).map_err(|error| io_error(target, error))?;
    }
    Ok(())
}

fn verify_planned_file_state(
    path: &Path,
    expected_exists: bool,
    expected_hash: Option<&str>,
) -> LegacyMigrationResult<()> {
    if path.exists() != expected_exists {
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "target changed after planning: {}",
            path.display()
        )));
    }
    if let Some(expected_hash) = expected_hash {
        require_file_hash(path, expected_hash)?;
    }
    Ok(())
}

fn validate_session_relationship_closure(
    manifest: &WorkspaceSessionsManifest,
) -> LegacyMigrationResult<()> {
    let sessions = manifest
        .sessions
        .iter()
        .map(|entry| (entry.session_id.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let event_turns = manifest
        .runtime_events
        .iter()
        .map(|entry| (entry.session_id.as_str(), &entry.turn_ids))
        .collect::<HashMap<_, _>>();
    for entry in &manifest.sessions {
        let Some(relationship) = &entry.relationship else {
            continue;
        };
        let Some(parent_session_id) = relationship.parent_session_id.as_deref() else {
            continue;
        };
        let parent = sessions.get(parent_session_id).ok_or_else(|| {
            LegacyMigrationError::UnsupportedSource(format!(
                "Session {} references a missing parent Session",
                entry.session_id
            ))
        })?;
        if let Some(parent_turn_id) = relationship.parent_dialog_turn_id.as_deref() {
            let in_persisted_turns = parent.turn_ids.contains(parent_turn_id);
            let in_runtime_events = event_turns
                .get(parent_session_id)
                .is_some_and(|turns| turns.contains(parent_turn_id));
            if !in_persisted_turns && !in_runtime_events {
                return Err(LegacyMigrationError::UnsupportedSource(format!(
                    "Session {} references a missing parent Turn",
                    entry.session_id
                )));
            }
        }
    }
    Ok(())
}

fn session_manifest_entry(session: &PlannedSession) -> SessionManifestEntry {
    SessionManifestEntry {
        runtime_relative: session
            .runtime_relative
            .to_string_lossy()
            .replace('\\', "/"),
        session_id: session.bundle.metadata.session_id.clone(),
        action: session.action,
        expected_hash: session.expected_hash.clone(),
        turn_ids: session
            .bundle
            .turns
            .iter()
            .map(|turn| turn.turn_id.clone())
            .collect(),
        relationship: session.bundle.metadata.relationship.clone(),
    }
}

fn imported_sessions(
    manifest: &WorkspaceSessionsManifest,
) -> impl Iterator<Item = &SessionManifestEntry> {
    manifest
        .sessions
        .iter()
        .filter(|entry| entry.action == SessionImportAction::Import)
}

fn imported_runtime_events(
    manifest: &WorkspaceSessionsManifest,
) -> impl Iterator<Item = &RuntimeEventManifestEntry> {
    manifest
        .runtime_events
        .iter()
        .filter(|entry| entry.action == SessionImportAction::Import)
}

pub(crate) fn read_workspace_sessions_manifest(
    context: &DomainContext<'_>,
) -> LegacyMigrationResult<WorkspaceSessionsManifest> {
    read_bounded_json(
        &context.layout.stage_root(),
        &workspace_sessions_manifest_path(context),
    )
}

fn workspace_sessions_manifest_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "workspace-sessions").join("manifest.json")
}

fn source_workspace_data_path(roots: &MigrationRoots) -> PathBuf {
    roots.legacy_user_root.join("data/workspace_data.json")
}

fn target_workspace_data_path(roots: &MigrationRoots) -> PathBuf {
    roots.target_user_root.join("data/workspace_data.json")
}

fn path_from_manifest(value: &str) -> LegacyMigrationResult<PathBuf> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(LegacyMigrationError::PathEscape(path));
    }
    Ok(path)
}

fn file_name(path: &Path) -> LegacyMigrationResult<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            LegacyMigrationError::UnsupportedSource(format!(
                "path component is not valid UTF-8: {}",
                path.display()
            ))
        })
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn offline_runtime() -> LegacyMigrationResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!(
                "failed to initialize offline Session writer: {error}"
            ))
        })
}

fn owner_error(context: &str, error: impl std::fmt::Display) -> LegacyMigrationError {
    LegacyMigrationError::InvalidRequest(format!("{context}: {error}"))
}

fn json_error(error: serde_json::Error) -> LegacyMigrationError {
    LegacyMigrationError::InvalidRequest(format!("JSON conversion failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_workspace_paths_are_normalized_with_posix_semantics() {
        let mut workspace: WorkspaceInfo = serde_json::from_value(serde_json::json!({
            "id": "legacy-remote",
            "name": "Remote fixture",
            "rootPath": "\\srv\\repo\\",
            "workspaceType": "Other",
            "workspaceKind": "remote",
            "status": "Inactive",
            "languages": [],
            "openedAt": "2026-01-01T00:00:00Z",
            "lastAccessed": "2026-01-01T00:00:00Z",
            "description": null,
            "tags": [],
            "statistics": null,
            "relatedPaths": [],
            "metadata": {
                "sshHost": "fixture.example",
                "connectionId": "fixture-connection"
            }
        }))
        .unwrap();

        normalize_legacy_workspace_for_current(&mut workspace).unwrap();
        assert_eq!(workspace.root_path.to_string_lossy(), "/srv/repo");
        assert!(current_workspace_storage_id(&workspace)
            .unwrap()
            .starts_with("remote_"));
    }
}
