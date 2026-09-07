//! Offline, source-read-only import primitives for legacy BitFun data.

mod engine;
mod error;
mod paths;
mod probe;
mod sqlite;
mod storage;

pub use engine::{
    compute_plan_hash, CancellationToken, CrashInjector, CrashPoint, DomainContext, DomainScan,
    LegacyDomainAdapter, MigrationEngine, NoCrashInjection,
};
pub use error::{LegacyMigrationError, LegacyMigrationResult};
pub use paths::{MigrationRoots, LEGACY_PRODUCT_ID};
pub use probe::{probe_legacy_source, ProbeLimits};
pub use sqlite::{snapshot_sqlite_read_only, validate_sqlite};
pub use storage::{atomic_write_bytes, atomic_write_json, MigrationLayout, MigrationLock};
