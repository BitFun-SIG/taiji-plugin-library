use openbitfun_legacy_migration::{
    atomic_write_json, probe_legacy_source, snapshot_sqlite_read_only, CancellationToken,
    CrashInjector, CrashPoint, DomainContext, DomainScan, LegacyDomainAdapter,
    LegacyMigrationError, LegacyMigrationResult, MigrationEngine, MigrationLayout, MigrationLock,
    MigrationRoots, NoCrashInjection, ProbeLimits,
};
use openbitfun_product_domains::legacy_migration::{
    FindingSeverity, MigrationDiagnostic, MigrationDomainId, MigrationDomainResult,
    MigrationDomainState, MigrationGroupId, MigrationPhase, MigrationRunReport, MigrationRunStatus,
    MigrationSelection, ScanFinding,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CallCounts {
    scan: usize,
    stage: usize,
    validate_stage: usize,
    commit: usize,
    validate_commit: usize,
    finalize_result: usize,
    rollback: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FinalizeBehavior {
    #[default]
    PassThrough,
    AddRedactedMetadata,
    Fail,
}

struct FakeAdapter {
    domain: MigrationDomainId,
    calls: Arc<Mutex<BTreeMap<MigrationDomainId, CallCounts>>>,
    finalize_behavior: FinalizeBehavior,
}

impl LegacyDomainAdapter for FakeAdapter {
    fn domain(&self) -> MigrationDomainId {
        self.domain
    }

    fn scan(&self, _roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        self.update(|counts| counts.scan += 1);
        Ok(DomainScan {
            finding: ScanFinding {
                domain: self.domain,
                code: "fake_source_supported".to_string(),
                entity_count: 1,
                logical_bytes: 16,
                source_schema: Some("fake.v1".to_string()),
                migratable: true,
                ..ScanFinding::default()
            },
            conflicts: Vec::new(),
            target_schema: Some("fake.current".to_string()),
            dependencies: Vec::new(),
        })
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        self.update(|counts| counts.stage += 1);
        atomic_write_json(
            &stage_path(context, self.domain),
            &serde_json::json!({"domain": format!("{:?}", self.domain), "entities": ["stable-id"]}),
        )?;
        Ok(MigrationDomainResult {
            domain: self.domain,
            state: MigrationDomainState::Staged,
            imported: 1,
            ..MigrationDomainResult::default()
        })
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        self.update(|counts| counts.validate_stage += 1);
        require_file(&stage_path(context, self.domain))
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        self.update(|counts| counts.commit += 1);
        let staged = fs::read(stage_path(context, self.domain)).map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!("missing fake stage: {error}"))
        })?;
        let value: serde_json::Value = serde_json::from_slice(&staged).map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!("invalid fake stage: {error}"))
        })?;
        // Rewriting the same owner record is intentional: recovery can repeat a
        // commit whose target write succeeded before its journal marker did.
        atomic_write_json(&target_path(context, self.domain), &value)
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        self.update(|counts| counts.validate_commit += 1);
        require_file(&target_path(context, self.domain))
    }

    fn finalize_result(
        &self,
        _context: &DomainContext<'_>,
        staged: &MigrationDomainResult,
    ) -> LegacyMigrationResult<MigrationDomainResult> {
        self.update(|counts| counts.finalize_result += 1);
        match self.finalize_behavior {
            FinalizeBehavior::PassThrough => Ok(staged.clone()),
            FinalizeBehavior::AddRedactedMetadata => {
                let mut finalized = staged.clone();
                finalized.imported = 2;
                finalized.skipped = 1;
                finalized.warnings.push(MigrationDiagnostic {
                    code: "credential_requires_reauthentication".to_string(),
                    severity: FindingSeverity::Warning,
                    domain: Some(self.domain),
                    message: "A credential must be entered again.".to_string(),
                    ..MigrationDiagnostic::default()
                });
                finalized.requires_reauthentication = vec!["account-1".to_string()];
                Ok(finalized)
            }
            FinalizeBehavior::Fail => Err(LegacyMigrationError::InvalidRequest(
                "final result is unavailable".to_string(),
            )),
        }
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        self.update(|counts| counts.rollback += 1);
        let path = target_path(context, self.domain);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(LegacyMigrationError::InvalidRequest(format!(
                "failed to roll back fake owner file {}: {error}",
                path.display()
            ))),
        }
    }
}

impl FakeAdapter {
    fn update(&self, update: impl FnOnce(&mut CallCounts)) {
        let mut calls = self
            .calls
            .lock()
            .expect("fake call state should not poison");
        update(calls.entry(self.domain).or_default());
    }
}

struct CrashOnce {
    point: CrashPoint,
    fired: AtomicBool,
}

impl CrashInjector for CrashOnce {
    fn should_crash(&self, point: CrashPoint) -> bool {
        point == self.point && !self.fired.swap(true, Ordering::AcqRel)
    }
}

#[test]
fn engine_recovers_after_commit_before_journal_and_deduplicates_repeated_runs() {
    let temp = test_tempdir();
    let roots = fixture_roots(temp.path());
    seed_supported_source(&roots);
    let source = probe_legacy_source(&roots, ProbeLimits::default())
        .expect("probe should succeed")
        .expect("source should be present");
    let calls = Arc::new(Mutex::new(BTreeMap::new()));
    let engine = fake_engine(roots.clone(), Arc::clone(&calls));
    let selection = MigrationSelection {
        groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
    };
    let plan = engine
        .plan(&source, selection, &CancellationToken::default())
        .expect("plan should succeed");
    let crash = CrashOnce {
        point: CrashPoint::AfterCommit(MigrationDomainId::Settings),
        fired: AtomicBool::new(false),
    };

    assert!(matches!(
        engine.execute(&plan, &CancellationToken::default(), &crash),
        Err(LegacyMigrationError::InjectedCrash(
            CrashPoint::AfterCommit(MigrationDomainId::Settings)
        ))
    ));
    let recovered = engine
        .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
        .expect("retry should recover");
    assert_eq!(recovered.status, MigrationRunStatus::Completed);
    assert!(recovered
        .domain_results
        .iter()
        .all(|result| result.state == MigrationDomainState::Verified));

    let before_repeat = calls.lock().expect("calls should not poison").clone();
    let repeated = engine
        .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
        .expect("completed run should be readable");
    assert_eq!(repeated, recovered);
    assert_eq!(
        *calls.lock().expect("calls should not poison"),
        before_repeat,
        "a completed plan must not execute owner writes again"
    );
    assert_eq!(
        before_repeat
            .get(&MigrationDomainId::Settings)
            .expect("settings calls")
            .commit,
        2,
        "the owner idempotently retries an ambiguous commit"
    );
    assert!(roots.legacy_user_root.join("config/app.json").exists());
}

#[test]
fn engine_persists_owner_metadata_finalized_after_commit_validation() {
    let temp = test_tempdir();
    let roots = fixture_roots(temp.path());
    seed_supported_source(&roots);
    let source = probe_legacy_source(&roots, ProbeLimits::default())
        .expect("probe should succeed")
        .expect("source should be present");
    let calls = Arc::new(Mutex::new(BTreeMap::new()));
    let engine = fake_engine_with_finalize(
        roots.clone(),
        Arc::clone(&calls),
        FinalizeBehavior::AddRedactedMetadata,
    );
    let plan = engine
        .plan(
            &source,
            MigrationSelection {
                groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
            },
            &CancellationToken::default(),
        )
        .expect("plan should succeed");

    let report = engine
        .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
        .expect("execution should succeed");

    assert_eq!(report.status, MigrationRunStatus::CompletedWithWarnings);
    assert_eq!(report.requires_reauthentication, ["account-1"]);
    let settings = report
        .domain_results
        .iter()
        .find(|result| result.domain == MigrationDomainId::Settings)
        .expect("settings result should exist");
    assert_eq!(settings.state, MigrationDomainState::Verified);
    assert_eq!(settings.imported, 2);
    assert_eq!(settings.skipped, 1);
    assert_eq!(settings.warnings.len(), 1);
    assert_eq!(
        calls
            .lock()
            .expect("calls should not poison")
            .get(&MigrationDomainId::Settings)
            .expect("settings calls")
            .finalize_result,
        1
    );

    let persisted: MigrationRunReport = serde_json::from_slice(
        &fs::read(MigrationLayout::new(&roots, &plan.run_id).report_path())
            .expect("report should be persisted"),
    )
    .expect("persisted report should be valid");
    assert_eq!(persisted, report);
}

#[test]
fn progress_reports_real_domain_phases_counts_and_cancel_boundaries() {
    let temp = test_tempdir();
    let roots = fixture_roots(temp.path());
    seed_supported_source(&roots);
    let source = probe_legacy_source(&roots, ProbeLimits::default())
        .expect("probe should succeed")
        .expect("source should be present");
    let engine = fake_engine(roots, Arc::new(Mutex::new(BTreeMap::new())));
    let plan = engine
        .plan(
            &source,
            MigrationSelection {
                groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
            },
            &CancellationToken::default(),
        )
        .expect("plan should succeed");
    let mut progress = Vec::new();

    engine
        .execute_with_progress(
            &plan,
            &CancellationToken::default(),
            &NoCrashInjection,
            |event| progress.push(event),
        )
        .expect("execution should succeed");

    for phase in [
        MigrationPhase::Stage,
        MigrationPhase::ValidateStage,
        MigrationPhase::Commit,
        MigrationPhase::ValidateCommit,
        MigrationPhase::Finalize,
    ] {
        assert!(
            progress.iter().any(|event| event.phase == phase),
            "progress should include {phase:?}"
        );
    }
    assert!(progress
        .iter()
        .filter(|event| event.phase == MigrationPhase::Commit)
        .all(|event| !event.safe_to_cancel));
    assert!(progress
        .iter()
        .any(|event| event.code == "domain_verified" && event.safe_to_cancel));
    assert!(progress
        .iter()
        .all(|event| event.processed <= event.total && event.total == plan.steps.len() as u64));
}

#[test]
fn engine_rolls_back_unverified_commit_when_owner_finalization_fails() {
    let temp = test_tempdir();
    let roots = fixture_roots(temp.path());
    seed_supported_source(&roots);
    let source = probe_legacy_source(&roots, ProbeLimits::default())
        .expect("probe should succeed")
        .expect("source should be present");
    let calls = Arc::new(Mutex::new(BTreeMap::new()));
    let engine =
        fake_engine_with_finalize(roots.clone(), Arc::clone(&calls), FinalizeBehavior::Fail);
    let plan = engine
        .plan(
            &source,
            MigrationSelection {
                groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
            },
            &CancellationToken::default(),
        )
        .expect("plan should succeed");

    assert!(matches!(
        engine.execute(&plan, &CancellationToken::default(), &NoCrashInjection),
        Err(LegacyMigrationError::Domain {
            domain: MigrationDomainId::Settings,
            ..
        })
    ));
    assert!(!target_path_for_roots(&roots, MigrationDomainId::Settings).exists());
    let settings_calls = calls
        .lock()
        .expect("calls should not poison")
        .get(&MigrationDomainId::Settings)
        .copied()
        .expect("settings calls");
    assert_eq!(settings_calls.finalize_result, 1);
    assert_eq!(settings_calls.rollback, 1);

    let persisted: MigrationRunReport = serde_json::from_slice(
        &fs::read(MigrationLayout::new(&roots, &plan.run_id).report_path())
            .expect("failed report should be persisted"),
    )
    .expect("persisted report should be valid");
    assert_eq!(persisted.status, MigrationRunStatus::FailedRecoverable);
    assert_eq!(
        persisted
            .domain_results
            .iter()
            .find(|result| result.domain == MigrationDomainId::Settings)
            .expect("settings result should exist")
            .state,
        MigrationDomainState::Failed
    );
}

#[test]
fn sqlite_wal_snapshot_contains_committed_rows_without_changing_source_files() {
    let temp = test_tempdir();
    let source = temp.path().join("source.sqlite");
    let destination = temp.path().join("stage/snapshot.sqlite");
    let connection = Connection::open(&source).expect("source SQLite should open");
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;\
             PRAGMA wal_autocheckpoint=0;\
             CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL);\
             INSERT INTO items(value) VALUES ('first'), ('second');",
        )
        .expect("WAL fixture should be created");
    let source_files = sqlite_family(&source);
    assert!(source_files
        .iter()
        .any(|path| path.to_string_lossy().ends_with("-wal")));
    let before = source_files
        .iter()
        .filter(|path| !path.to_string_lossy().ends_with("-shm"))
        .map(|path| (path.clone(), sha256_file(path)))
        .collect::<BTreeMap<_, _>>();

    snapshot_sqlite_read_only(&source, &destination).expect("snapshot should succeed");

    let snapshot = Connection::open(&destination).expect("snapshot should open");
    let count: i64 = snapshot
        .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
        .expect("snapshot rows should be readable");
    assert_eq!(count, 2);
    drop(snapshot);
    for (path, expected_hash) in before {
        assert_eq!(
            sha256_file(&path),
            expected_hash,
            "source changed: {}",
            path.display()
        );
    }
    assert!(!destination.with_extension("sqlite-shm").exists());
    assert!(!destination.with_extension("sqlite-wal").exists());
}

#[test]
fn lock_contention_and_source_target_aliases_fail_closed() {
    let temp = test_tempdir();
    let roots = fixture_roots(temp.path());
    let layout = MigrationLayout::new(&roots, "lock-test");
    let _first = MigrationLock::acquire(&layout).expect("first lock should succeed");
    assert!(matches!(
        MigrationLock::acquire(&layout),
        Err(LegacyMigrationError::LockUnavailable)
    ));

    let mut aliased = roots;
    aliased.target_user_root = aliased.legacy_user_root.clone();
    assert!(matches!(
        aliased.validate_distinct(),
        Err(LegacyMigrationError::SourceEqualsTarget(_))
    ));
}

fn fake_engine(
    roots: MigrationRoots,
    calls: Arc<Mutex<BTreeMap<MigrationDomainId, CallCounts>>>,
) -> MigrationEngine {
    fake_engine_with_finalize(roots, calls, FinalizeBehavior::PassThrough)
}

fn fake_engine_with_finalize(
    roots: MigrationRoots,
    calls: Arc<Mutex<BTreeMap<MigrationDomainId, CallCounts>>>,
    settings_finalize_behavior: FinalizeBehavior,
) -> MigrationEngine {
    let adapters = [
        MigrationDomainId::Settings,
        MigrationDomainId::Credentials,
        MigrationDomainId::CrossReferenceRepair,
    ]
    .into_iter()
    .map(|domain| {
        Box::new(FakeAdapter {
            domain,
            calls: Arc::clone(&calls),
            finalize_behavior: if domain == MigrationDomainId::Settings {
                settings_finalize_behavior
            } else {
                FinalizeBehavior::PassThrough
            },
        }) as Box<dyn LegacyDomainAdapter>
    });
    MigrationEngine::new(roots, adapters).expect("fake engine should be valid")
}

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

fn seed_supported_source(roots: &MigrationRoots) {
    let path = roots.legacy_user_root.join("config/app.json");
    fs::create_dir_all(path.parent().expect("configuration should have a parent"))
        .expect("legacy config directory should be created");
    fs::write(path, r#"{"version":"0.2.19"}"#).expect("legacy configuration should be written");
}

fn stage_path(context: &DomainContext<'_>, domain: MigrationDomainId) -> PathBuf {
    context.layout.stage_root().join(format!("{domain:?}.json"))
}

fn target_path(context: &DomainContext<'_>, domain: MigrationDomainId) -> PathBuf {
    target_path_for_roots(context.roots, domain)
}

fn target_path_for_roots(roots: &MigrationRoots, domain: MigrationDomainId) -> PathBuf {
    roots
        .target_user_root
        .join("data/fake-owner")
        .join(format!("{domain:?}.json"))
}

fn require_file(path: &Path) -> LegacyMigrationResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(LegacyMigrationError::InvalidRequest(format!(
            "expected fake owner file: {}",
            path.display()
        )))
    }
}

fn sqlite_family(database: &Path) -> Vec<PathBuf> {
    let mut paths = vec![database.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", database.display()));
        if path.exists() {
            paths.push(path);
        }
    }
    paths
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("fixture file should remain readable");
    hex::encode(Sha256::digest(bytes))
}

fn test_tempdir() -> tempfile::TempDir {
    match std::env::var_os("OPENBITFUN_TEST_TMPDIR") {
        Some(root) => tempfile::Builder::new()
            .prefix("legacy-migration-")
            .tempdir_in(root)
            .expect("temporary directory should be created in the requested root"),
        None => tempfile::tempdir().expect("temporary directory should be created"),
    }
}
