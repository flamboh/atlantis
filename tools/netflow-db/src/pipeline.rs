//! End-to-end pipeline orchestration.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use jiff::{RoundMode, Timestamp, ToSpan, Unit, ZonedRound, civil::Date};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    config::{ConfigError, CsvSourceConfig},
    coverage::BucketCoverage,
    domain::{
        AddressSet, BucketKey, CanonicalBucket, DomainError, FlowSelection, Granularity,
        StatisticalBucket, StatisticalBucketIncludeProfile,
    },
    ingest::{self, IngestError, ProducerError},
    nfdump,
    provenance::{
        ExecutableRevision, ExpectedAbsence, FileSnapshot, InputRevision, ProvenanceError,
        capture_file_revision, csv_decoder_fingerprint, file_sha256, nfcapd_decoder_fingerprint,
        revision_for_locator, verify_file_snapshot,
    },
    publish::{PublishError, WriteBucketsProfile, write_buckets, write_buckets_profiled},
    registry::{Dataset, DatasetRegistry, DatasetSource, RegistryError, is_safe_path_component},
    storage::{
        BucketCoverageRow, DailyProductCompletionState, DatabaseOperationLock, DatasetMetadata,
        InputBucket, InputEvidenceRow, InputEvidenceState, InputKind, InputStatus, ProductIdentity,
        STATS_TABLE_NAMES, SourceDefinition, StatsBucketKey, StorageError,
        bind_nfcapd_source_layout, bind_product_identity, cached_content_fingerprint,
        canonical_path, complete_input_scan, connect_pipeline_writer, current_product_fingerprint,
        daily_product_completion_state, database_operation_lock_path, delete_stats_bucket_keys,
        delete_stats_time_range, earliest_traffic_bucket_start,
        ensure_daily_product_completion_bucket_guard, init_schema, input_scan_fully_processed,
        insert_bucket_coverage_rows, mark_input_bucket_status, nfcapd_logical_bucket_processed,
        optimize_all_query_planner_statistics, provision_daily_product_completion_bucket_guards,
        query_bucket_coverage, query_input_evidence, query_input_evidence_range,
        query_processed_nfcapd_range, replace_input_evidence, set_dataset_default_start_date,
        upsert_daily_product_completion, upsert_dataset_metadata, upsert_input_bucket,
        validate_database_path_separation,
    },
};

const FIVE_MINUTES: i64 = 300;
const NFCAPD_DECODE_BATCH_SIZE: usize = 12;
const NFCAPD_REVISION_HASH_MAX_WORKERS: usize = NFCAPD_DECODE_BATCH_SIZE * 2;
const MAX_MISSING_DAY_WARNING_DETAILS: usize = 8;
const DEFAULT_TIMEZONE: &str = "America/Los_Angeles";

static NFCAPD_DENSE_TRAFFIC_SCOPE_COUNT: OnceLock<i64> = OnceLock::new();

fn nfcapd_dense_traffic_scope_count() -> i64 {
    *NFCAPD_DENSE_TRAFFIC_SCOPE_COUNT.get_or_init(|| {
        let key = BucketKey::new("", Granularity::FiveMinutes, 0, FIVE_MINUTES);
        i64::try_from(StatisticalBucket::dense(key).finish_owned().traffic.len())
            .expect("dense traffic scope count fits SQLite INTEGER")
    })
}

#[cfg(test)]
type MissingDayAbsenceHook = Box<dyn FnMut(&Path, &[(String, i64)], &str)>;

#[cfg(test)]
type CoordinatedCommitGuardHook = Box<dyn FnMut()>;

#[cfg(test)]
type CoordinatedPlanHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
type SinglePlanHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
type SingleCommitGuardHook = Box<dyn FnMut()>;

#[cfg(test)]
thread_local! {
    static PREPARE_NFCAPD_TREE_TIMESTAMP_CALLS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_LOGICAL_BUCKET_TOPOLOGY_CALLS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_DAY_TOPOLOGY_AUDIT_CALLS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_CAPTURE_IDENTITY_CALLS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_REVISION_POOL_BUILDS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_DECODE_POOL_BUILDS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static NFCAPD_ACTIVITY_POOL_BUILDS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static DATASET_REGISTRY_LOAD_CALLS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static COORDINATED_POSTFLIGHT_SNAPSHOT_VERIFICATIONS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static MISSING_DAY_ABSENCE_HOOK: std::cell::RefCell<Option<MissingDayAbsenceHook>> =
        const { std::cell::RefCell::new(None) };
    static COORDINATED_COMMIT_GUARD_HOOK: std::cell::RefCell<Option<CoordinatedCommitGuardHook>> =
        const { std::cell::RefCell::new(None) };
    static COORDINATED_PLAN_HOOK: std::cell::RefCell<Option<CoordinatedPlanHook>> =
        const { std::cell::RefCell::new(None) };
    static SINGLE_COMMIT_GUARD_HOOK: std::cell::RefCell<Option<SingleCommitGuardHook>> =
        const { std::cell::RefCell::new(None) };
    static SINGLE_PLAN_HOOK: std::cell::RefCell<Option<SinglePlanHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn reset_prepare_nfcapd_tree_timestamp_calls() {
    PREPARE_NFCAPD_TREE_TIMESTAMP_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn prepare_nfcapd_tree_timestamp_calls() -> usize {
    PREPARE_NFCAPD_TREE_TIMESTAMP_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_nfcapd_logical_bucket_topology_calls() {
    NFCAPD_LOGICAL_BUCKET_TOPOLOGY_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn nfcapd_logical_bucket_topology_calls() -> usize {
    NFCAPD_LOGICAL_BUCKET_TOPOLOGY_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_nfcapd_day_topology_audit_calls() {
    NFCAPD_DAY_TOPOLOGY_AUDIT_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn nfcapd_day_topology_audit_calls() -> usize {
    NFCAPD_DAY_TOPOLOGY_AUDIT_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_nfcapd_capture_identity_calls() {
    NFCAPD_CAPTURE_IDENTITY_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn nfcapd_capture_identity_calls() -> usize {
    NFCAPD_CAPTURE_IDENTITY_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_nfcapd_pool_builds() {
    NFCAPD_REVISION_POOL_BUILDS.with(|builds| builds.set(0));
    NFCAPD_DECODE_POOL_BUILDS.with(|builds| builds.set(0));
    NFCAPD_ACTIVITY_POOL_BUILDS.with(|builds| builds.set(0));
}

#[cfg(test)]
fn nfcapd_pool_builds() -> (usize, usize, usize) {
    (
        NFCAPD_REVISION_POOL_BUILDS.with(std::cell::Cell::get),
        NFCAPD_DECODE_POOL_BUILDS.with(std::cell::Cell::get),
        NFCAPD_ACTIVITY_POOL_BUILDS.with(std::cell::Cell::get),
    )
}

#[cfg(test)]
fn reset_dataset_registry_load_calls() {
    DATASET_REGISTRY_LOAD_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn dataset_registry_load_calls() -> usize {
    DATASET_REGISTRY_LOAD_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_coordinated_postflight_snapshot_verifications() {
    COORDINATED_POSTFLIGHT_SNAPSHOT_VERIFICATIONS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn coordinated_postflight_snapshot_verifications() -> usize {
    COORDINATED_POSTFLIGHT_SNAPSHOT_VERIFICATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn set_missing_day_absence_hook(hook: impl FnMut(&Path, &[(String, i64)], &str) + 'static) {
    MISSING_DAY_ABSENCE_HOOK.with(|current| {
        *current.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_missing_day_absence_hook() {
    MISSING_DAY_ABSENCE_HOOK.with(|current| {
        *current.borrow_mut() = None;
    });
}

#[cfg(test)]
fn set_coordinated_commit_guard_hook(hook: impl FnMut() + 'static) {
    COORDINATED_COMMIT_GUARD_HOOK.with(|current| {
        *current.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_coordinated_commit_guard_hook() {
    COORDINATED_COMMIT_GUARD_HOOK.with(|current| {
        *current.borrow_mut() = None;
    });
}

#[cfg(test)]
fn invoke_coordinated_commit_guard_hook() {
    COORDINATED_COMMIT_GUARD_HOOK.with(|current| {
        if let Some(hook) = current.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
fn set_coordinated_plan_hook(hook: impl FnMut(&Path) + 'static) {
    COORDINATED_PLAN_HOOK.with(|current| {
        *current.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_coordinated_plan_hook() {
    COORDINATED_PLAN_HOOK.with(|current| {
        *current.borrow_mut() = None;
    });
}

#[cfg(test)]
fn invoke_coordinated_plan_hook(root: &Path) {
    COORDINATED_PLAN_HOOK.with(|current| {
        if let Some(hook) = current.borrow_mut().as_mut() {
            hook(root);
        }
    });
}

#[cfg(test)]
fn set_single_commit_guard_hook(hook: impl FnMut() + 'static) {
    SINGLE_COMMIT_GUARD_HOOK.with(|current| {
        *current.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_single_commit_guard_hook() {
    SINGLE_COMMIT_GUARD_HOOK.with(|current| {
        *current.borrow_mut() = None;
    });
}

#[cfg(test)]
fn invoke_single_commit_guard_hook() {
    SINGLE_COMMIT_GUARD_HOOK.with(|current| {
        if let Some(hook) = current.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
fn set_single_plan_hook(hook: impl FnMut(&Path) + 'static) {
    SINGLE_PLAN_HOOK.with(|current| {
        *current.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_single_plan_hook() {
    SINGLE_PLAN_HOOK.with(|current| {
        *current.borrow_mut() = None;
    });
}

#[cfg(test)]
fn invoke_single_plan_hook(root: &Path) {
    SINGLE_PLAN_HOOK.with(|current| {
        if let Some(hook) = current.borrow_mut().as_mut() {
            hook(root);
        }
    });
}

#[cfg(not(test))]
fn invoke_coordinated_commit_guard_hook() {}

#[cfg(not(test))]
fn invoke_coordinated_plan_hook(_root: &Path) {}

#[cfg(not(test))]
fn invoke_single_commit_guard_hook() {}

#[cfg(not(test))]
fn invoke_single_plan_hook(_root: &Path) {}

#[cfg(test)]
fn invoke_missing_day_absence_hook(root: &Path, missing: &[(String, i64)], timezone: &str) {
    MISSING_DAY_ABSENCE_HOOK.with(|current| {
        if let Some(hook) = current.borrow_mut().as_mut() {
            hook(root, missing, timezone);
        }
    });
}

#[cfg(not(test))]
fn invoke_missing_day_absence_hook(_root: &Path, _missing: &[(String, i64)], _timezone: &str) {}

fn build_revision_hash_pool() -> Result<rayon::ThreadPool, PipelineError> {
    #[cfg(test)]
    NFCAPD_REVISION_POOL_BUILDS.with(|builds| builds.set(builds.get() + 1));
    let revision_hash_workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(NFCAPD_REVISION_HASH_MAX_WORKERS);
    rayon::ThreadPoolBuilder::new()
        .num_threads(revision_hash_workers)
        .thread_name(|index| format!("nfcapd-revision-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build revision hash pool: {error}"))
        })
}

fn build_nfcapd_snapshot_pool() -> Result<rayon::ThreadPool, PipelineError> {
    let snapshot_workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(NFCAPD_REVISION_HASH_MAX_WORKERS);
    rayon::ThreadPoolBuilder::new()
        .num_threads(snapshot_workers)
        .thread_name(|index| format!("nfcapd-snapshot-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build nfcapd snapshot pool: {error}"))
        })
}

fn build_nfcapd_decode_pool() -> Result<rayon::ThreadPool, PipelineError> {
    #[cfg(test)]
    NFCAPD_DECODE_POOL_BUILDS.with(|builds| builds.set(builds.get() + 1));
    rayon::ThreadPoolBuilder::new()
        .num_threads(NFCAPD_DECODE_BATCH_SIZE)
        .thread_name(|index| format!("nfcapd-decode-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build nfcapd decode pool: {error}"))
        })
}

fn build_nfcapd_activity_pool() -> Result<rayon::ThreadPool, PipelineError> {
    #[cfg(test)]
    NFCAPD_ACTIVITY_POOL_BUILDS.with(|builds| builds.set(builds.get() + 1));
    rayon::ThreadPoolBuilder::new()
        .num_threads(NFCAPD_DECODE_BATCH_SIZE)
        .thread_name(|index| format!("nfcapd-activity-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build nfcapd activity pool: {error}"))
        })
}

#[derive(Clone, Debug)]
pub struct PipelineRequest {
    pub config_path: Option<PathBuf>,
    pub dataset_id: Option<String>,
    pub datasets_path: Option<PathBuf>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub database_path: Option<PathBuf>,
    pub selection: Value,
    pub nfdump: String,
    pub force: bool,
    pub run_maad: bool,
    pub require_complete: bool,
}

impl PipelineRequest {
    #[must_use]
    pub fn config(path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: Some(path.into()),
            dataset_id: None,
            datasets_path: None,
            start_date: None,
            end_date: None,
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: "nfdump".into(),
            force: false,
            run_maad: true,
            require_complete: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineReport {
    pub input_scans: usize,
    pub skipped_inputs: usize,
    pub five_minute_buckets: usize,
    pub rollup_buckets: usize,
    pub complete_five_minute_buckets: usize,
    pub partial_five_minute_buckets: usize,
    pub unknown_five_minute_buckets: usize,
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("pipeline request must specify either --config or --dataset")]
    MissingMode,
    #[error("pipeline request cannot combine --config and --dataset")]
    ConflictingModes,
    #[error("invalid pipeline configuration: {0}")]
    InvalidConfig(String),
    #[error("unable to read pipeline configuration: {0}")]
    Io(#[from] std::io::Error),
    #[error("unable to parse pipeline configuration: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Ingest(#[from] IngestError),
    #[error(transparent)]
    Provenance(#[from] ProvenanceError),
    #[error(transparent)]
    Publish(#[from] PublishError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("invalid pipeline time: {0}")]
    Time(String),
    #[error("requested scope contains {0} incomplete five-minute coverage buckets")]
    IncompleteCoverage(i64),
}

#[derive(Clone, Debug, Deserialize)]
struct PipelineConfigFile {
    database_path: PathBuf,
    #[serde(default = "default_timezone")]
    timezone: String,
    #[serde(default)]
    run_maad: Option<bool>,
    #[serde(default)]
    nfdump: Option<String>,
    #[serde(default)]
    selection: Value,
    inputs: Vec<InputSpec>,
    #[serde(default)]
    datasets: Vec<Dataset>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "input_kind", rename_all = "snake_case")]
enum InputSpec {
    Csv {
        path: PathBuf,
        mapping_path: PathBuf,
    },
    CsvTree {
        root_path: PathBuf,
        mapping_path: PathBuf,
    },
    Nfcapd {
        path: PathBuf,
        source_id: String,
        #[serde(default)]
        bucket_start: Option<i64>,
        #[serde(default)]
        gap: bool,
        #[serde(default)]
        expected_path: Option<PathBuf>,
    },
    NfcapdTree {
        root_path: PathBuf,
        #[serde(default)]
        source_ids: Vec<String>,
        #[serde(default)]
        sources: Vec<DatasetSource>,
        start_date: String,
        #[serde(default)]
        end_date: Option<String>,
        #[serde(default)]
        start_time: Option<String>,
        #[serde(default)]
        end_time: Option<String>,
        #[serde(default)]
        force: bool,
    },
}

#[derive(Clone, Debug)]
struct ResolvedPipeline {
    database_path: PathBuf,
    /// Files that configure or execute the pipeline and must remain read-only during output setup.
    control_paths: Vec<PathBuf>,
    timezone: String,
    run_maad: bool,
    nfdump: PathBuf,
    nfdump_revision: Option<ExecutableRevision>,
    selection: FlowSelection,
    inputs: Vec<InputSpec>,
    datasets: Vec<Dataset>,
    require_complete: bool,
}

fn nfdump_control_path(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    (path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        || value.contains(['/', '\\']))
    .then(|| path.to_owned())
}

#[cfg(unix)]
fn has_effective_execute_access(path: &Path) -> bool {
    #[cfg(all(not(target_os = "android"), not(target_os = "redox")))]
    {
        nix::unistd::faccessat(
            None,
            path,
            nix::unistd::AccessFlags::X_OK,
            nix::fcntl::AtFlags::AT_EACCESS,
        )
        .is_ok()
    }

    #[cfg(any(target_os = "android", target_os = "redox"))]
    {
        // These targets do not expose AT_EACCESS. Their normal process lookup uses the same
        // credentials as access(2) for this non-set-id pipeline.
        nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
    }
}

#[cfg(not(unix))]
fn has_effective_execute_access(_path: &Path) -> bool {
    true
}

/// Resolve the executable that a nfdump command will select.
///
/// Resolve explicit paths before output setup as well as bare names. The canonical path is stored
/// in the resolved pipeline so later command invocations use the same executable that preflight
/// checked, and output alias checks see the actual control file.
fn resolved_nfdump_control_path(value: &str) -> Result<PathBuf, PipelineError> {
    if let Some(path) = nfdump_control_path(value) {
        let resolved = canonical_path(&path)?;
        let metadata = fs::metadata(&resolved).map_err(|error| {
            PipelineError::InvalidConfig(format!(
                "cannot resolve explicit nfdump executable {value:?} at {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(PipelineError::InvalidConfig(format!(
                "explicit nfdump executable {value:?} at {} is not a regular file",
                path.display()
            )));
        }
        if !has_effective_execute_access(&resolved) {
            return Err(PipelineError::InvalidConfig(format!(
                "explicit nfdump executable {value:?} at {} is not executable by this process",
                path.display()
            )));
        }
        return Ok(resolved);
    }
    if value.is_empty() {
        return Err(PipelineError::InvalidConfig(
            "nfdump executable name is empty".into(),
        ));
    }

    let path_variable = std::env::var_os("PATH").ok_or_else(|| {
        PipelineError::InvalidConfig(format!(
            "cannot resolve bare nfdump executable {value:?}: PATH is not set"
        ))
    })?;
    for directory in std::env::split_paths(&path_variable) {
        // An empty PATH component means the current working directory for process lookup.
        let directory = if directory.as_os_str().is_empty() {
            std::env::current_dir()?
        } else {
            directory
        };
        let candidate = directory.join(value);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if !has_effective_execute_access(&candidate) {
            continue;
        }
        return Ok(canonical_path(candidate)?);
    }

    Err(PipelineError::InvalidConfig(format!(
        "cannot resolve bare nfdump executable {value:?} through PATH"
    )))
}

/// Capture and validate the exact native decoder before output setup.  The digest is paid once;
/// every later boundary uses only the stored file snapshot.
fn resolve_nfdump_revision(value: &str) -> Result<(PathBuf, ExecutableRevision), PipelineError> {
    let path = resolved_nfdump_control_path(value)?;
    let revision = ExecutableRevision::capture(&path)?;
    Ok((path, revision))
}

fn verify_nfdump_revision(pipeline: &ResolvedPipeline) -> Result<(), PipelineError> {
    if let Some(revision) = &pipeline.nfdump_revision {
        verify_nfdump_revision_snapshot(revision)?;
    }
    Ok(())
}

fn verify_nfdump_revision_snapshot(revision: &ExecutableRevision) -> Result<(), PipelineError> {
    verify_file_snapshot(Path::new(&revision.locator), &revision.snapshot).map_err(|error| {
        PipelineError::InvalidConfig(format!(
            "nfdump executable changed during pipeline execution at {}: {error}",
            revision.locator
        ))
    })
}

fn nfdump_decoder_fingerprint_for_pipeline(
    pipeline: &ResolvedPipeline,
) -> Result<String, PipelineError> {
    if let Some(revision) = &pipeline.nfdump_revision {
        return Ok(revision.decoder_fingerprint.clone());
    }
    // Manually assembled test pipelines can exercise native helpers without going through
    // request resolution. Production native requests always carry a revision.
    Ok(nfcapd_decoder_fingerprint()?)
}

fn inputs_require_nfdump(inputs: &[InputSpec]) -> bool {
    inputs.iter().any(|input| {
        matches!(
            input,
            InputSpec::Nfcapd { .. } | InputSpec::NfcapdTree { .. }
        )
    })
}

fn default_timezone() -> String {
    DEFAULT_TIMEZONE.into()
}

pub fn run(
    request: impl std::borrow::Borrow<PipelineRequest>,
) -> Result<PipelineReport, PipelineError> {
    let pipeline = resolve_request(request.borrow())?;
    execute(pipeline)
}

/// Run several registry datasets as coordinated daily-active-source products.
///
/// The datasets share discovery and nfdump work, while each output retains its own product
/// identity, provenance, transactions, and resume state. This deliberately stays separate from
/// [`run`] so the established single-dataset path remains unchanged.
pub fn run_many(
    request: impl std::borrow::Borrow<PipelineRequest>,
    dataset_ids: Vec<String>,
) -> Result<PipelineReport, PipelineError> {
    let request = request.borrow();
    if dataset_ids.len() < 2 {
        return Err(PipelineError::InvalidConfig(
            "coordinated pipeline mode requires at least two --dataset values".into(),
        ));
    }
    if request.config_path.is_some() {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode cannot combine --config with repeated --dataset".into(),
        ));
    }
    if request.database_path.is_some() {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode cannot override --database-path".into(),
        ));
    }
    if selection_override_requested(&request.selection) {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode cannot override registry selections from the CLI".into(),
        ));
    }
    if request.start_time.is_some() || request.end_time.is_some() {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode requires a whole-day date window; --start-time and --end-time are unsupported".into(),
        ));
    }

    let mut seen = BTreeSet::new();
    if let Some(duplicate) = dataset_ids.iter().find(|id| !seen.insert(id.as_str())) {
        return Err(PipelineError::InvalidConfig(format!(
            "coordinated dataset mode cannot repeat dataset {duplicate:?}"
        )));
    }

    let repository_root = std::env::current_dir()?;
    let registry_path = request
        .datasets_path
        .clone()
        .unwrap_or_else(|| DatasetRegistry::default_path(&repository_root));
    let registry = load_dataset_registry(&registry_path, &repository_root)?;
    let shared_nfdump = resolve_nfdump_revision(&request.nfdump)?;
    let mut pipelines = Vec::with_capacity(dataset_ids.len());
    for dataset_id in &dataset_ids {
        let mut single = request.clone();
        single.dataset_id = Some(dataset_id.clone());
        pipelines.push(resolve_dataset_request(
            &single,
            &registry_path,
            &registry,
            Some((&shared_nfdump.0, &shared_nfdump.1)),
        )?);
    }
    validate_compatible_pipelines(&pipelines)?;
    execute_many(pipelines)
}

fn selection_override_requested(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.values().any(|entry| !entry.is_null()))
}

fn validate_compatible_pipelines(pipelines: &[ResolvedPipeline]) -> Result<(), PipelineError> {
    let Some(first) = pipelines.first() else {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode requires at least two datasets".into(),
        ));
    };
    if !first.selection.selects_daily_active_sources() {
        return Err(PipelineError::InvalidConfig(
            "coordinated dataset mode requires every registry selection to be daily_active_sources"
                .into(),
        ));
    }
    let first_input = only_nfcapd_tree(first)?;
    let first_config = nfcapd_tree_config(first_input)?;
    let first_root = canonical_path(first_config.root_path)?;
    for pipeline in pipelines.iter().skip(1) {
        if !pipeline.selection.selects_daily_active_sources() {
            return Err(PipelineError::InvalidConfig(format!(
                "dataset {:?} does not use a daily_active_sources selection",
                pipeline
                    .datasets
                    .first()
                    .map(|dataset| dataset.dataset_id.as_str())
                    .unwrap_or("<unknown>")
            )));
        }
        let input = only_nfcapd_tree(pipeline)?;
        let config = nfcapd_tree_config(input)?;
        if first_root != canonical_path(config.root_path)? {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same nfcapd root".into(),
            ));
        }
        if first_config.start_date != config.start_date
            || first_config.end_date != config.end_date
            || first_config.start_time != config.start_time
            || first_config.end_time != config.end_time
            || first_config.force != config.force
        {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same whole-day window and force settings".into(),
            ));
        }
        if first.timezone != pipeline.timezone {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same timezone".into(),
            ));
        }
        if first.run_maad != pipeline.run_maad {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same MAAD setting".into(),
            ));
        }
        if first.nfdump != pipeline.nfdump {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same nfdump executable/configuration".into(),
            ));
        }
        let same_executable_revision = match (&first.nfdump_revision, &pipeline.nfdump_revision) {
            (Some(left), Some(right)) => {
                left.locator == right.locator
                    && left.content_fingerprint == right.content_fingerprint
                    && left.decoder_fingerprint == right.decoder_fingerprint
            }
            (None, None) => true,
            _ => false,
        };
        if !same_executable_revision {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same nfdump executable revision".into(),
            ));
        }
        if first.require_complete != pipeline.require_complete {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same coverage settings".into(),
            ));
        }
    }
    let output_paths = pipelines
        .iter()
        .map(|pipeline| pipeline.database_path.as_path())
        .collect::<Vec<_>>();
    validate_database_path_separation(&output_paths)?;
    Ok(())
}

fn only_nfcapd_tree(pipeline: &ResolvedPipeline) -> Result<&InputSpec, PipelineError> {
    if pipeline.inputs.len() != 1 {
        return Err(PipelineError::InvalidConfig(
            "coordinated datasets require exactly one nfcapd_tree input".into(),
        ));
    }
    match pipeline.inputs.first() {
        Some(input @ InputSpec::NfcapdTree { .. }) => Ok(input),
        _ => Err(PipelineError::InvalidConfig(
            "coordinated datasets require an nfcapd_tree input".into(),
        )),
    }
}

struct NfcapdTreeConfig<'a> {
    root_path: &'a Path,
    start_date: &'a str,
    end_date: Option<&'a str>,
    start_time: Option<&'a str>,
    end_time: Option<&'a str>,
    force: bool,
}

fn nfcapd_tree_config(input: &InputSpec) -> Result<NfcapdTreeConfig<'_>, PipelineError> {
    let InputSpec::NfcapdTree {
        root_path,
        start_date,
        end_date,
        start_time,
        end_time,
        force,
        ..
    } = input
    else {
        return Err(PipelineError::InvalidConfig(
            "coordinated datasets require an nfcapd_tree input".into(),
        ));
    };
    Ok(NfcapdTreeConfig {
        root_path,
        start_date,
        end_date: end_date.as_deref(),
        start_time: start_time.as_deref(),
        end_time: end_time.as_deref(),
        force: *force,
    })
}

fn canonical_logical_sources(input: &InputSpec) -> Result<Vec<DatasetSource>, PipelineError> {
    let InputSpec::NfcapdTree {
        root_path,
        source_ids,
        sources,
        ..
    } = input
    else {
        return Err(PipelineError::InvalidConfig(
            "coordinated datasets require an nfcapd_tree input".into(),
        ));
    };
    let mut sources = normalize_sources(root_path, source_ids, sources)?;
    for source in &mut sources {
        source.members.sort_unstable();
    }
    Ok(sources)
}

fn normalized_path_key(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}

fn absolute_lexical_path(path: &Path) -> Result<PathBuf, PipelineError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalized_path_key(&absolute))
}

fn sqlite_related_path(path: &Path, suffix: &str) -> Result<PathBuf, PipelineError> {
    let parent = path.parent().ok_or_else(|| {
        PipelineError::InvalidConfig(format!(
            "database path has no parent directory: {}",
            path.display()
        ))
    })?;
    let name = path.file_name().ok_or_else(|| {
        PipelineError::InvalidConfig(format!(
            "database path has no file name: {}",
            path.display()
        ))
    })?;
    Ok(parent.join(format!("{}{}", name.to_string_lossy(), suffix)))
}

/// Return every path SQLite or the pipeline lock can touch for an output path.
///
/// Keep both the caller spelling and the resolved spelling here. SQLite receives the caller
/// spelling, while the operation lock resolves the database first; a symlink can therefore make
/// those two sets differ even when the database itself is absent.
fn output_related_paths(path: &Path) -> Result<Vec<PathBuf>, PipelineError> {
    let raw = absolute_lexical_path(path)?;
    let resolved = canonical_path(path)?;
    let mut candidates = BTreeSet::new();
    let mut add = |candidate: PathBuf| -> Result<(), PipelineError> {
        let candidate = absolute_lexical_path(&candidate)?;
        candidates.insert(candidate.clone());
        candidates.insert(canonical_path(candidate)?);
        Ok(())
    };

    for database in [&raw, &resolved] {
        add(database.to_owned())?;
        for suffix in ["-journal", "-wal", "-shm"] {
            add(sqlite_related_path(database, suffix)?)?;
        }
    }
    add(database_operation_lock_path(&resolved)?)?;
    add(raw.with_file_name(format!(
        ".{}.operation.lock",
        raw.file_name()
            .ok_or_else(|| {
                PipelineError::InvalidConfig(format!(
                    "database path has no file name: {}",
                    raw.display()
                ))
            })?
            .to_string_lossy()
    )))?;
    Ok(candidates.into_iter().collect())
}

#[cfg(unix)]
fn existing_path_identity(path: &Path) -> Result<Option<(u64, u64)>, PipelineError> {
    use std::os::unix::fs::MetadataExt;

    match fs::metadata(path) {
        Ok(metadata) => Ok(Some((metadata.dev(), metadata.ino()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
#[cfg(test)]
fn nfcapd_capture_identity(path: &Path) -> Result<Option<(u64, u64)>, PipelineError> {
    #[cfg(test)]
    NFCAPD_CAPTURE_IDENTITY_CALLS.with(|calls| calls.set(calls.get() + 1));
    existing_path_identity(path)
}

fn capture_nfcapd_snapshot(path: &Path) -> Result<FileSnapshot, PipelineError> {
    FileSnapshot::capture(path).map_err(PipelineError::from)
}

/// Capture the cheap identities for a discovered capture set with bounded parallelism.
///
/// The caller keeps the resulting metadata alongside the already-discovered paths. Hashing and
/// decode work can then reuse the same observation instead of doing a serial alias pass followed
/// by another metadata walk.
fn capture_nfcapd_snapshots(
    paths: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, FileSnapshot>, PipelineError> {
    capture_nfcapd_snapshots_with(paths, capture_nfcapd_snapshot)
}

fn capture_nfcapd_snapshots_with<F>(
    paths: &BTreeSet<PathBuf>,
    capture: F,
) -> Result<BTreeMap<PathBuf, FileSnapshot>, PipelineError>
where
    F: Fn(&Path) -> Result<FileSnapshot, PipelineError> + Sync,
{
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let pool = build_nfcapd_snapshot_pool()?;
    pool.install(|| {
        paths
            .par_iter()
            .map(|path| capture(path).map(|snapshot| (path.clone(), snapshot)))
            .collect::<Result<BTreeMap<_, _>, _>>()
    })
}

#[cfg(test)]
fn capture_nfcapd_snapshots_counted(
    paths: &BTreeSet<PathBuf>,
    calls: &AtomicUsize,
) -> Result<BTreeMap<PathBuf, FileSnapshot>, PipelineError> {
    capture_nfcapd_snapshots_with(paths, |path| {
        calls.fetch_add(1, Ordering::Relaxed);
        capture_nfcapd_snapshot(path)
    })
}

#[cfg(unix)]
fn output_has_existing_identity(output_paths: &[&Path]) -> Result<bool, PipelineError> {
    for output in output_paths {
        for related in output_related_paths(output)? {
            if existing_path_identity(&related)?.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(not(unix))]
fn output_has_existing_identity(_output_paths: &[&Path]) -> Result<bool, PipelineError> {
    Ok(false)
}

/// Reject an output database, its SQLite sidecars, or its operation lock when any aliases an
/// input file. This must run after input discovery and before output setup.
fn validate_output_input_separation<'a, I>(
    output_paths: &[&Path],
    input_paths: I,
    input_label: &str,
) -> Result<(), PipelineError>
where
    I: IntoIterator<Item = &'a Path>,
{
    // Outputs are few, while a discovered capture tree can contain hundreds of thousands of
    // paths. Index the small output side and stream the input side so validation does not retain a
    // second copy of the discovered corpus.
    let mut outputs_by_path = BTreeMap::<PathBuf, (usize, PathBuf)>::new();
    #[cfg(unix)]
    let mut outputs_by_identity = BTreeMap::<(u64, u64), (usize, PathBuf)>::new();
    for (output_index, output) in output_paths.iter().enumerate() {
        for related in output_related_paths(output)? {
            let resolved = canonical_path(&related)?;
            outputs_by_path
                .entry(resolved)
                .or_insert_with(|| (output_index, related.clone()));
            #[cfg(unix)]
            if let Some(identity) = existing_path_identity(&related)? {
                outputs_by_identity
                    .entry(identity)
                    .or_insert_with(|| (output_index, related));
            }
        }
    }

    for input in input_paths {
        let resolved = canonical_path(input)?;
        if let Some((output_index, related)) = outputs_by_path.get(&resolved) {
            return Err(PipelineError::InvalidConfig(format!(
                "output database {} aliases {input_label} {} through {}",
                output_paths[*output_index].display(),
                input.display(),
                related.display()
            )));
        }
        #[cfg(unix)]
        if let Some(identity) = existing_path_identity(input)?
            && let Some((output_index, related)) = outputs_by_identity.get(&identity)
        {
            return Err(PipelineError::InvalidConfig(format!(
                "output database {} aliases {input_label} {} through device/inode {:?} at {}",
                output_paths[*output_index].display(),
                input.display(),
                identity,
                related.display()
            )));
        }
    }
    Ok(())
}

/// Reject an output database, its SQLite sidecars, or its operation lock when any aliases a
/// discovered nfcapd capture. This must run after capture discovery and before output setup.
fn validate_output_capture_separation<'a, I>(
    output_paths: &[&Path],
    capture_paths: I,
) -> Result<(), PipelineError>
where
    I: IntoIterator<Item = &'a Path>,
{
    validate_output_input_separation(output_paths, capture_paths, "discovered nfcapd capture")
}

/// Protect a discovered nfcapd tree without resolving every capture path.
///
/// Capture discovery already walks the configured member namespaces, so a capture's lexical
/// locator is known. Output aliases are checked against the configured and resolved namespace
/// spellings, including locators that do not exist yet. Only an existing output-side inode needs
/// a physical capture scan for hard-link aliases; the common new-output case performs no metadata
/// call per capture.
#[cfg(test)]
fn validate_output_nfcapd_capture_separation<'a, I>(
    output_paths: &[&Path],
    namespaces: &[PathBuf],
    _timezone: &str,
    capture_paths: I,
) -> Result<(), PipelineError>
where
    I: IntoIterator<Item = &'a Path>,
{
    validate_output_nfcapd_locator_separation(output_paths, namespaces)?;

    #[cfg(unix)]
    let mut outputs_by_identity = BTreeMap::<(u64, u64), (usize, PathBuf)>::new();
    #[cfg(unix)]
    for (output_index, output) in output_paths.iter().enumerate() {
        for related in output_related_paths(output)? {
            if let Some(identity) = existing_path_identity(&related)? {
                outputs_by_identity
                    .entry(identity)
                    .or_insert_with(|| (output_index, related));
            }
        }
    }

    #[cfg(unix)]
    if outputs_by_identity.is_empty() {
        return Ok(());
    }

    #[cfg(unix)]
    for input in capture_paths {
        if let Some(identity) = nfcapd_capture_identity(input)?
            && let Some((output_index, related)) = outputs_by_identity.get(&identity)
        {
            return Err(PipelineError::InvalidConfig(format!(
                "output database {} aliases discovered nfcapd capture {} through device/inode {:?} at {}",
                output_paths[*output_index].display(),
                input.display(),
                identity,
                related.display()
            )));
        }
    }

    #[cfg(not(unix))]
    let _ = capture_paths;
    Ok(())
}

/// Validate discovered nfcapd captures using identities captured by the bounded snapshot pass.
///
/// This is the existing-output path: the snapshot map is also consumed by revision preparation,
/// so hard-link protection does not require a second serial metadata walk.
fn validate_output_nfcapd_capture_separation_with_snapshots<'a, I>(
    output_paths: &[&Path],
    namespaces: &[PathBuf],
    capture_snapshots: I,
) -> Result<(), PipelineError>
where
    I: IntoIterator<Item = (&'a Path, &'a FileSnapshot)>,
{
    validate_output_nfcapd_locator_separation(output_paths, namespaces)?;

    #[cfg(unix)]
    let mut outputs_by_identity = BTreeMap::<(u64, u64), (usize, PathBuf)>::new();
    #[cfg(unix)]
    for (output_index, output) in output_paths.iter().enumerate() {
        for related in output_related_paths(output)? {
            if let Some(identity) = existing_path_identity(&related)? {
                outputs_by_identity
                    .entry(identity)
                    .or_insert_with(|| (output_index, related));
            }
        }
    }

    #[cfg(unix)]
    if outputs_by_identity.is_empty() {
        return Ok(());
    }

    #[cfg(unix)]
    for (input, snapshot) in capture_snapshots {
        let identity = (snapshot.device, snapshot.inode);
        if let Some((output_index, related)) = outputs_by_identity.get(&identity) {
            return Err(PipelineError::InvalidConfig(format!(
                "output database {} aliases discovered nfcapd capture {} through device/inode {:?} at {}",
                output_paths[*output_index].display(),
                input.display(),
                identity,
                related.display()
            )));
        }
    }

    #[cfg(not(unix))]
    let _ = capture_snapshots;
    Ok(())
}

/// Return the canonical and configured spellings of each member's locator namespace.
///
/// The output preflight uses these prefixes instead of materializing every possible capture
/// path in the selected window. Missing future captures are still protected because the output
/// path itself is checked against the namespace shape.
fn nfcapd_locator_namespaces(
    root: &Path,
    physical_ids: &[String],
) -> Result<Vec<PathBuf>, PipelineError> {
    let mut namespaces = BTreeSet::new();
    for member in physical_ids {
        let configured = absolute_lexical_path(&root.join(member))?;
        namespaces.insert(configured.clone());
        namespaces.insert(canonical_path(configured)?);
    }
    Ok(namespaces.into_iter().collect())
}

/// Return whether two paths overlap as a namespace and a path.
///
/// The equality and ancestor cases matter when the output or one of its sidecars is the
/// namespace itself, or when it would replace a root/ancestor needed to discover captures.
fn paths_overlap_namespace(path: &Path, namespace: &Path) -> bool {
    path == namespace || path.starts_with(namespace) || namespace.starts_with(path)
}

/// Reject output databases, sidecars, and operation locks anywhere in a configured member
/// namespace, even when the output is not itself a valid nfcapd capture locator yet.
fn validate_output_nfcapd_locator_separation(
    output_paths: &[&Path],
    namespaces: &[PathBuf],
) -> Result<(), PipelineError> {
    for output in output_paths {
        for related in output_related_paths(output)? {
            for namespace in namespaces {
                if paths_overlap_namespace(&related, namespace) {
                    return Err(PipelineError::InvalidConfig(format!(
                        "output database {} aliases discovered nfcapd capture locator in configured nfcapd member namespace {} through {}",
                        output.display(),
                        namespace.display(),
                        related.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Reject output databases, sidecars, and operation locks anywhere below an auto-discovered
/// nfcapd root. The member directory may not exist during preflight, so checking only discovered
/// captures would leave a future member namespace writable by output setup.
fn validate_output_nfcapd_auto_namespace_separation(
    output_paths: &[&Path],
    roots: &[PathBuf],
) -> Result<(), PipelineError> {
    for output in output_paths {
        for related in output_related_paths(output)? {
            for root in roots {
                if paths_overlap_namespace(&related, root) {
                    return Err(PipelineError::InvalidConfig(format!(
                        "output database {} aliases discovered nfcapd capture locator in the auto-discovered member namespace under nfcapd root {} (including direct-child directory paths) through {}",
                        output.display(),
                        root.display(),
                        related.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Reject output paths that a CSV tree would discover after SQLite creates them. Discovery is
/// intentionally flat, so only a direct child of the configured tree root can change its input
/// set.
fn validate_output_csv_tree_separation(
    output_paths: &[&Path],
    trees: &[(PathBuf, CsvSourceConfig)],
) -> Result<(), PipelineError> {
    for output in output_paths {
        for related in output_related_paths(output)? {
            for (root, mapping) in trees {
                if related.parent() != Some(root.as_path()) {
                    continue;
                }
                let Some(name) = related.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let lowercase_name = name.to_ascii_lowercase();
                let excluded = mapping
                    .discovery_exclude_suffixes
                    .iter()
                    .any(|suffix| lowercase_name.ends_with(&suffix.to_ascii_lowercase()));
                if !excluded && ingest::matches_csv_discovery(&lowercase_name, mapping) {
                    return Err(PipelineError::InvalidConfig(format!(
                        "output database {} would be discovered as a CSV tree input at {} under {}",
                        output.display(),
                        related.display(),
                        root.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_daily_active_source_layout(
    sources: &[DatasetSource],
    physical_ids: &[String],
) -> Result<(), PipelineError> {
    if sources.is_empty() {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources requires at least one logical source".into(),
        ));
    }
    if physical_ids.is_empty() {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources requires at least one physical source member".into(),
        ));
    }
    Ok(())
}

/// Identity of a planned physical member directory.
///
/// The canonical path catches a retargeted root/member symlink or a renamed directory. On Unix,
/// the device/inode pair also catches a replacement at the same configured path. Directory
/// timestamps are intentionally not part of this identity: normal capture creation changes them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MemberDirectoryIdentity {
    canonical_path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn capture_member_directory_identity(
    root: &Path,
    member: &str,
) -> Result<MemberDirectoryIdentity, PipelineError> {
    let member_path = root.join(member);
    let canonical_member_path = canonical_path(&member_path)?;
    #[cfg(unix)]
    {
        let (device, inode) = existing_path_identity(&member_path)?.ok_or_else(|| {
            PipelineError::InvalidConfig(format!(
                "source member directory {:?} disappeared while its identity was being captured",
                member
            ))
        })?;
        Ok(MemberDirectoryIdentity {
            canonical_path: canonical_member_path,
            device,
            inode,
        })
    }
    #[cfg(not(unix))]
    {
        Ok(MemberDirectoryIdentity {
            canonical_path: canonical_member_path,
        })
    }
}

fn capture_member_directory_identities(
    root: &Path,
    physical_ids: &[String],
) -> Result<BTreeMap<String, MemberDirectoryIdentity>, PipelineError> {
    physical_ids
        .iter()
        .map(|member| {
            capture_member_directory_identity(root, member)
                .map(|identity| (member.clone(), identity))
        })
        .collect()
}

fn verify_member_directory_identities(
    root: &Path,
    expected: &BTreeMap<String, MemberDirectoryIdentity>,
) -> Result<(), PipelineError> {
    for (member, expected_identity) in expected {
        let current = capture_member_directory_identity(root, member)?;
        if current != *expected_identity {
            return Err(PipelineError::InvalidConfig(format!(
                "nfcapd member directory {:?} changed during the pipeline: planned {} but found {}",
                member,
                expected_identity.canonical_path.display(),
                current.canonical_path.display()
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct FrozenNfcapdTreeLayout {
    root_path: PathBuf,
    sources: Vec<DatasetSource>,
    physical_ids: Vec<String>,
    member_identities: BTreeMap<String, MemberDirectoryIdentity>,
    auto_discovered: bool,
}

#[derive(Clone, Debug, Default)]
struct SingleOutputPlan {
    trees: BTreeMap<usize, FrozenNfcapdTreeLayout>,
    dataset_sources: BTreeMap<String, Vec<DatasetSource>>,
    capture_snapshots: BTreeMap<PathBuf, FileSnapshot>,
}

impl SingleOutputPlan {
    fn first_root(&self) -> Option<&Path> {
        self.trees
            .values()
            .next()
            .map(|tree| tree.root_path.as_path())
    }
}

/// Perform the read-only nfcapd discovery needed to protect a single output before opening it.
///
/// The returned layout is the only source membership snapshot used by output initialization,
/// processing, and strict coverage checks. Auto-discovered layouts are revalidated by
/// [`verify_single_auto_source_layouts`] after all read-only planning and immediately before
/// output setup.
fn plan_single_output(pipeline: &ResolvedPipeline) -> Result<SingleOutputPlan, PipelineError> {
    verify_nfdump_revision(pipeline)?;
    let mut locator_namespaces = BTreeSet::new();
    let mut auto_discovery_roots = BTreeSet::new();
    let mut csv_tree_configs = Vec::new();
    let mut nfcapd_windows = Vec::new();
    let mut nfcapd_capture_paths = BTreeSet::new();
    let mut trees = BTreeMap::new();
    let output_path = pipeline.database_path.as_path();
    let output_paths = std::slice::from_ref(&output_path);
    validate_output_input_separation(
        output_paths,
        pipeline.control_paths.iter().map(PathBuf::as_path),
        "pipeline control path",
    )?;
    for (input_index, input) in pipeline.inputs.iter().enumerate() {
        match input {
            InputSpec::NfcapdTree {
                root_path,
                source_ids,
                sources,
                start_date,
                end_date,
                start_time,
                end_time,
                ..
            } => {
                let root = canonical_path(root_path)?;
                let auto_discovered = source_ids.is_empty() && sources.is_empty();
                if auto_discovered {
                    auto_discovery_roots.insert(root.clone());
                }
                let sources = normalize_sources(&root, source_ids, sources)?;
                let physical_ids = sources
                    .iter()
                    .flat_map(|source| source.members.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let member_identities = capture_member_directory_identities(&root, &physical_ids)?;
                trees.insert(
                    input_index,
                    FrozenNfcapdTreeLayout {
                        root_path: root.clone(),
                        sources: sources.clone(),
                        physical_ids: physical_ids.clone(),
                        member_identities,
                        auto_discovered,
                    },
                );
                locator_namespaces.extend(nfcapd_locator_namespaces(&root, &physical_ids)?);
                if pipeline.selection.selects_daily_active_sources() {
                    validate_daily_active_source_layout(&sources, &physical_ids)?;
                    if start_time.is_some() || end_time.is_some() {
                        return Err(PipelineError::InvalidConfig(
                            "daily_active_sources selection requires whole local calendar days; start_time and end_time are unsupported".into(),
                        ));
                    }
                }
                let discovered =
                    ingest::discover_nfcapd_source_paths(&root, &physical_ids, &pipeline.timezone)?;
                nfcapd_capture_paths.extend(discovered.iter().map(|input| input.path.clone()));
                nfcapd_windows.push((
                    start_date.clone(),
                    end_date.clone(),
                    start_time.clone(),
                    end_time.clone(),
                    discovered.iter().map(|input| input.bucket_start).max(),
                ));
            }
            InputSpec::Nfcapd {
                path,
                gap,
                expected_path,
                ..
            } => {
                if *gap {
                    if let Some(expected_path) = expected_path {
                        validate_output_capture_separation(
                            output_paths,
                            std::iter::once(expected_path.as_path()),
                        )?;
                    }
                } else {
                    validate_output_capture_separation(
                        output_paths,
                        std::iter::once(path.as_path()),
                    )?;
                }
            }
            InputSpec::Csv { path, mapping_path } => {
                validate_output_input_separation(
                    output_paths,
                    [path.as_path(), mapping_path.as_path()],
                    "discovered CSV input",
                )?;
            }
            InputSpec::CsvTree {
                root_path,
                mapping_path,
            } => {
                let mapping = CsvSourceConfig::load(mapping_path)?;
                let discovered = ingest::discover_csv_inputs(root_path, mapping_path, &mapping)?;
                validate_output_input_separation(
                    output_paths,
                    std::iter::once(mapping_path.as_path())
                        .chain(discovered.iter().map(|input| input.path.as_path())),
                    "discovered CSV input",
                )?;
                csv_tree_configs.push((canonical_path(root_path)?, mapping));
            }
        }
    }
    let locator_namespaces = locator_namespaces.into_iter().collect::<Vec<_>>();
    validate_output_nfcapd_locator_separation(output_paths, &locator_namespaces)?;
    validate_output_nfcapd_auto_namespace_separation(
        output_paths,
        &auto_discovery_roots.into_iter().collect::<Vec<_>>(),
    )?;
    validate_output_csv_tree_separation(output_paths, &csv_tree_configs)?;
    let capture_snapshots = if output_has_existing_identity(output_paths)? {
        let capture_snapshots = capture_nfcapd_snapshots(&nfcapd_capture_paths)?;
        validate_output_nfcapd_capture_separation_with_snapshots(
            output_paths,
            &locator_namespaces,
            capture_snapshots
                .iter()
                .map(|(path, snapshot)| (path.as_path(), snapshot)),
        )?;
        capture_snapshots
    } else {
        BTreeMap::new()
    };
    for (start_date, end_date, start_time, end_time, discovered_end) in nfcapd_windows {
        resolve_nfcapd_tree_window(
            &start_date,
            end_date.as_deref(),
            start_time.as_deref(),
            end_time.as_deref(),
            discovered_end,
            &pipeline.timezone,
        )?;
    }
    if let Some(revision) = &pipeline.nfdump_revision {
        ingest::probe_nfdump_compatibility(&pipeline.nfdump)?;
        verify_file_snapshot(&pipeline.nfdump, &revision.snapshot)?;
    }
    verify_nfdump_revision(pipeline)?;
    let mut dataset_sources = BTreeMap::new();
    for dataset in &pipeline.datasets {
        let dataset_root = canonical_path(&dataset.root_path)?;
        let sources = trees
            .values()
            .find(|tree| tree.root_path == dataset_root)
            .map(|tree| tree.sources.clone())
            .unwrap_or(dataset.logical_sources()?);
        dataset_sources.insert(dataset.dataset_id.clone(), sources);
    }
    Ok(SingleOutputPlan {
        trees,
        dataset_sources,
        capture_snapshots,
    })
}

fn verify_single_auto_source_layouts(
    pipeline: &ResolvedPipeline,
    plan: &SingleOutputPlan,
) -> Result<(), PipelineError> {
    for tree in plan.trees.values() {
        verify_member_directory_identities(&tree.root_path, &tree.member_identities)?;
        if tree.auto_discovered {
            let current = normalize_sources(&tree.root_path, &[], &[])?;
            if current != tree.sources {
                return Err(PipelineError::InvalidConfig(
                    "auto-discovered source layout changed during single-output planning".into(),
                ));
            }
        }
    }
    // Keep this parameter in the validation seam so a future pipeline with multiple roots can
    // report the owning dataset without rediscovering its metadata.
    let _ = pipeline;
    Ok(())
}

#[cfg(test)]
fn preflight_single_output(pipeline: &ResolvedPipeline) -> Result<(), PipelineError> {
    plan_single_output(pipeline).map(|_| ())
}

fn resolve_request(request: &PipelineRequest) -> Result<ResolvedPipeline, PipelineError> {
    match (&request.config_path, &request.dataset_id) {
        (Some(_), Some(_)) => return Err(PipelineError::ConflictingModes),
        (None, None) => return Err(PipelineError::MissingMode),
        _ => {}
    }
    if let Some(path) = &request.config_path {
        let mut config: PipelineConfigFile = serde_json::from_slice(&fs::read(path)?)?;
        if let Some(path) = &request.database_path {
            config.database_path = path.clone();
        }
        let configured_selection = selection_from_value(&config.selection)?;
        let requested_selection = selection_from_value(&request.selection)?;
        let selection = if requested_selection.is_unrestricted() {
            configured_selection
        } else {
            requested_selection
        };
        validate_selection_inputs(&selection, &config.inputs)?;
        if request.force {
            let tree_count = config
                .inputs
                .iter()
                .filter(|input| matches!(input, InputSpec::NfcapdTree { .. }))
                .count();
            if tree_count != 1 {
                return Err(PipelineError::InvalidConfig(
                    "--force in config mode requires exactly one nfcapd_tree input".into(),
                ));
            }
            for input in &mut config.inputs {
                if let InputSpec::NfcapdTree { force, .. } = input {
                    *force = true;
                }
            }
        }
        let configured_nfdump = config.nfdump.unwrap_or_else(|| request.nfdump.clone());
        let requires_nfdump = inputs_require_nfdump(&config.inputs);
        let (nfdump, nfdump_revision) = if requires_nfdump {
            let (path, revision) = resolve_nfdump_revision(&configured_nfdump)?;
            (path, Some(revision))
        } else {
            (PathBuf::from(configured_nfdump), None)
        };
        let mut control_paths = vec![path.clone()];
        if requires_nfdump {
            control_paths.push(nfdump.clone());
        }
        return Ok(ResolvedPipeline {
            database_path: config.database_path,
            control_paths,
            timezone: config.timezone,
            run_maad: config.run_maad.unwrap_or(true) && request.run_maad,
            nfdump,
            nfdump_revision,
            selection,
            inputs: config.inputs,
            datasets: config.datasets,
            require_complete: request.require_complete,
        });
    }

    let repository_root = std::env::current_dir()?;
    let registry_path = request
        .datasets_path
        .clone()
        .unwrap_or_else(|| DatasetRegistry::default_path(&repository_root));
    let registry = load_dataset_registry(&registry_path, &repository_root)?;
    resolve_dataset_request(request, &registry_path, &registry, None)
}

fn load_dataset_registry(
    registry_path: &Path,
    repository_root: &Path,
) -> Result<DatasetRegistry, PipelineError> {
    #[cfg(test)]
    DATASET_REGISTRY_LOAD_CALLS.with(|calls| calls.set(calls.get() + 1));
    Ok(DatasetRegistry::load(registry_path, repository_root)?)
}

fn resolve_dataset_request(
    request: &PipelineRequest,
    registry_path: &Path,
    registry: &DatasetRegistry,
    shared_nfdump: Option<(&Path, &ExecutableRevision)>,
) -> Result<ResolvedPipeline, PipelineError> {
    let dataset_id = request
        .dataset_id
        .as_deref()
        .ok_or(PipelineError::MissingMode)?;
    let dataset = registry.get(dataset_id)?.clone();
    let start_date = request.start_date.clone().ok_or_else(|| {
        PipelineError::InvalidConfig("--start-date is required with --dataset".into())
    })?;
    let configured_selection = selection_from_value(&dataset.selection)?;
    let requested_selection = selection_from_value(&request.selection)?;
    let selection = if requested_selection.is_unrestricted() {
        configured_selection.clone()
    } else {
        requested_selection
    };
    if selection != configured_selection && request.database_path.is_none() {
        return Err(PipelineError::InvalidConfig(
            "overriding a dataset's flow selection requires an explicit --database-path".into(),
        ));
    }
    let (nfdump, nfdump_revision) = match shared_nfdump {
        Some((path, revision)) => (path.to_owned(), Some(revision.clone())),
        None => {
            let (path, revision) = resolve_nfdump_revision(&request.nfdump)?;
            (path, Some(revision))
        }
    };
    let mut control_paths = vec![registry_path.to_owned()];
    control_paths.push(nfdump.clone());
    Ok(ResolvedPipeline {
        database_path: request
            .database_path
            .clone()
            .unwrap_or_else(|| dataset.db_path.clone()),
        control_paths,
        timezone: DEFAULT_TIMEZONE.into(),
        run_maad: request.run_maad,
        nfdump,
        nfdump_revision,
        selection,
        inputs: vec![InputSpec::NfcapdTree {
            root_path: dataset.root_path.clone(),
            source_ids: dataset.source_ids.clone(),
            sources: dataset.sources.clone(),
            start_date,
            end_date: request.end_date.clone(),
            start_time: request.start_time.clone(),
            end_time: request.end_time.clone(),
            force: request.force,
        }],
        datasets: vec![dataset],
        require_complete: request.require_complete,
    })
}

fn selection_from_value(value: &Value) -> Result<FlowSelection, DomainError> {
    FlowSelection::from_payload((!value.is_null()).then_some(value))
}

fn validate_selection_inputs(
    selection: &FlowSelection,
    inputs: &[InputSpec],
) -> Result<(), PipelineError> {
    if selection.selects_daily_active_sources()
        && (inputs.len() != 1 || !matches!(inputs.first(), Some(InputSpec::NfcapdTree { .. })))
    {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources selection requires exactly one nfcapd_tree input".into(),
        ));
    }
    Ok(())
}

fn execute(pipeline: ResolvedPipeline) -> Result<PipelineReport, PipelineError> {
    let plan = plan_single_output(&pipeline)?;
    if let Some(root) = plan.first_root() {
        invoke_single_plan_hook(root);
    }
    verify_single_auto_source_layouts(&pipeline, &plan)?;
    verify_nfdump_revision(&pipeline)?;
    if let Some(parent) = pipeline.database_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = DatabaseOperationLock::acquire(&pipeline.database_path, "pipeline build")?;
    let connection = connect_pipeline_writer(&pipeline.database_path)?;
    init_schema(&connection)?;
    initialize_metadata_with_plan(&connection, &pipeline, &plan)?;

    let mut report = PipelineReport::default();
    let mut csv_inputs = pipeline
        .inputs
        .iter()
        .filter_map(|input| match input {
            InputSpec::Csv { path, mapping_path } => Some(ingest::CsvInputSpec {
                path: path.clone(),
                mapping_path: mapping_path.clone(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let explicit_nfcapd = pipeline
        .inputs
        .iter()
        .filter(|input| matches!(input, InputSpec::Nfcapd { .. }))
        .cloned()
        .collect::<Vec<_>>();
    for (input_index, input) in pipeline.inputs.iter().enumerate() {
        match input {
            InputSpec::Csv { .. } | InputSpec::Nfcapd { .. } => {}
            InputSpec::CsvTree {
                root_path,
                mapping_path,
            } => {
                let mapping = CsvSourceConfig::load(mapping_path)?;
                csv_inputs.extend(ingest::discover_csv_inputs(
                    root_path,
                    mapping_path,
                    &mapping,
                )?);
            }
            InputSpec::NfcapdTree {
                start_date,
                end_date,
                start_time,
                end_time,
                force,
                ..
            } => process_nfcapd_tree(
                &connection,
                plan.trees
                    .get(&input_index)
                    .expect("every nfcapd_tree input has a frozen layout"),
                start_date,
                end_date.as_deref(),
                start_time.as_deref(),
                end_time.as_deref(),
                *force,
                &pipeline,
                &plan.capture_snapshots,
                &mut report,
            )?,
        }
    }
    merge_report(
        &mut report,
        process_csv_inputs(&connection, &csv_inputs, &pipeline)?,
    );
    merge_report(
        &mut report,
        process_explicit_nfcapd_inputs(&connection, &explicit_nfcapd, &pipeline)?,
    );
    infer_default_start_dates(&connection, &pipeline)?;
    populate_coverage_summary(&connection, &mut report)?;
    // This is the publication seam for the in-place pipeline product. Keep it before the strict
    // coverage error so an incomplete-but-inspectable database also gets useful planner stats.
    if let Err(error) = optimize_all_query_planner_statistics(&connection) {
        tracing::warn!(%error, "could not refresh SQLite planner statistics");
    }
    if pipeline.require_complete {
        let incomplete =
            count_incomplete_requested_coverage_with_plan(&connection, &pipeline, &plan)?;
        if incomplete != 0 {
            return Err(PipelineError::IncompleteCoverage(incomplete));
        }
    }
    Ok(report)
}

struct CoordinatedOutput {
    pipeline: ResolvedPipeline,
    sources: Vec<DatasetSource>,
    connection: Connection,
    _lock: DatabaseOperationLock,
}

type NfcapdFingerprintKey = (String, u64, u64, u64, i64, i64);

/// Resume state for one output and one local day.
///
/// Coordinated preparation revisits every logical source/bucket, but the state needed for those
/// decisions is limited to this day. Keeping the maps here avoids retaining prior days or loading
/// a product's complete provenance history into memory.
#[derive(Clone, Debug, Default)]
struct NfcapdDayResumeCache {
    evidence: BTreeMap<(String, i64), Vec<InputEvidenceRow>>,
    processed: BTreeMap<(String, i64), BTreeSet<(String, String)>>,
    fingerprints: BTreeMap<NfcapdFingerprintKey, String>,
}

impl NfcapdDayResumeCache {
    fn load(
        connection: &Connection,
        sources: &[DatasetSource],
        start: i64,
        end: i64,
    ) -> Result<Self, PipelineError> {
        let source_ids = sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut cache = Self::default();
        for row in query_input_evidence_range(connection, &source_ids, start, end)? {
            cache
                .evidence
                .entry((row.source_id.clone(), row.bucket_start))
                .or_default()
                .push(row);
        }
        for row in query_processed_nfcapd_range(connection, &source_ids, start, end)? {
            cache
                .processed
                .entry((row.source_id.clone(), row.bucket_start))
                .or_default()
                .insert((row.input_locator.clone(), row.revision_fingerprint));
            if let Some(snapshot) = row.file_snapshot {
                cache
                    .fingerprints
                    .entry(Self::fingerprint_key(&row.input_locator, &snapshot))
                    .or_insert(row.content_fingerprint);
            }
        }
        Ok(cache)
    }

    fn fingerprint_key(locator: &str, snapshot: &FileSnapshot) -> NfcapdFingerprintKey {
        (
            locator.to_owned(),
            snapshot.device,
            snapshot.inode,
            snapshot.size,
            snapshot.mtime_ns,
            snapshot.ctime_ns,
        )
    }

    fn evidence(&self, source_id: &str, bucket_start: i64) -> &[InputEvidenceRow] {
        self.evidence
            .get(&(source_id.to_owned(), bucket_start))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn processed(
        &self,
        source_id: &str,
        bucket_start: i64,
        revisions: &[InputRevision],
    ) -> Result<bool, PipelineError> {
        if revisions.is_empty() {
            return Ok(false);
        }
        let requested = revisions
            .iter()
            .map(|revision| (revision.locator.clone(), revision.fingerprint.clone()))
            .collect::<BTreeSet<_>>();
        let stored = self
            .processed
            .get(&(source_id.to_owned(), bucket_start))
            .cloned()
            .unwrap_or_default();
        let stored_locators = stored
            .iter()
            .map(|(locator, _)| locator)
            .collect::<BTreeSet<_>>();
        let requested_locators = requested
            .iter()
            .map(|(locator, _)| locator)
            .collect::<BTreeSet<_>>();
        if stored_locators == requested_locators && stored != requested {
            return Err(PipelineError::Storage(
                StorageError::InputRevisionConflict {
                    locator: format!("{source_id}:{bucket_start}"),
                    components: "nfcapd content or decoder; rerun with force to rewrite it"
                        .to_owned(),
                },
            ));
        }
        Ok(stored == requested)
    }

    fn cached_content_fingerprint(&self, path: &Path, snapshot: &FileSnapshot) -> Option<String> {
        self.fingerprints
            .get(&Self::fingerprint_key(&path.to_string_lossy(), snapshot))
            .cloned()
    }
}

/// Worker pools shared by every day in a coordinated run.
///
/// Revision hashing is needed while deciding whether a day has pending work, so it is built when
/// the run starts. Decode and activity work are both lazy: a complete no-op run never allocates
/// either pool, and a multi-day run reuses each pool after its first pending day.
struct CoordinatedPools {
    revision: rayon::ThreadPool,
    decode: Option<rayon::ThreadPool>,
    activity: Option<rayon::ThreadPool>,
}

impl CoordinatedPools {
    fn new() -> Result<Self, PipelineError> {
        Ok(Self {
            revision: build_revision_hash_pool()?,
            decode: None,
            activity: None,
        })
    }

    fn decode(&mut self) -> Result<&rayon::ThreadPool, PipelineError> {
        if self.decode.is_none() {
            self.decode = Some(build_nfcapd_decode_pool()?);
        }
        Ok(self
            .decode
            .as_ref()
            .expect("decode pool was just initialized"))
    }

    fn activity(&mut self) -> Result<&rayon::ThreadPool, PipelineError> {
        if self.activity.is_none() {
            self.activity = Some(build_nfcapd_activity_pool()?);
        }
        Ok(self
            .activity
            .as_ref()
            .expect("activity pool was just initialized"))
    }
}

/// Prepared coordinated work retained between the day preflight and publication pass.
///
/// Each entry is bounded by the existing nfcapd decode batch; the coordinated day retains one
/// such entry per batch. Publication takes ownership of each batch before decoding so prepared
/// evidence does not remain live alongside decoded buckets.
struct PreparedCoordinatedBatch {
    prepared: BTreeMap<usize, Vec<PreparedTreeTimestamp>>,
}

#[derive(Debug, Default)]
struct CoordinatedDaySharedProfile {
    revision_elapsed: Duration,
    eligibility_elapsed: Duration,
    activity_elapsed: Duration,
    decode_elapsed: Duration,
    revision_paths: u64,
    activity_members: u64,
    activity_inputs: u64,
    active_set_counts: Vec<u64>,
}

impl CoordinatedDaySharedProfile {
    fn log(&self, day_start: i64, day_end: i64) {
        tracing::info!(
            target: "netflow_db::profile",
            phase = "coordinated_day_shared",
            day_start,
            day_end,
            revision_seconds = self.revision_elapsed.as_secs_f64(),
            eligibility_seconds = self.eligibility_elapsed.as_secs_f64(),
            activity_seconds = self.activity_elapsed.as_secs_f64(),
            decode_seconds = self.decode_elapsed.as_secs_f64(),
            revision_paths = self.revision_paths,
            activity_members = self.activity_members,
            activity_inputs = self.activity_inputs,
            active_set_counts = ?self.active_set_counts,
        );
    }
}

struct CoordinatedPlan {
    root_path: PathBuf,
    sources: Vec<DatasetSource>,
    dataset_sources: BTreeMap<String, Vec<DatasetSource>>,
    physical_ids: Vec<String>,
    member_identities: BTreeMap<String, MemberDirectoryIdentity>,
    auto_discovered_datasets: BTreeSet<String>,
    by_member_and_start: BTreeMap<(String, i64), PathBuf>,
    capture_snapshots: BTreeMap<PathBuf, FileSnapshot>,
    member_bounds: BTreeMap<String, (i64, i64)>,
    start: i64,
    end: i64,
    extend_gaps_to_window: bool,
    force: bool,
    timezone: String,
}

/// Resolve all read-only coordinated inputs before creating an output directory, lock, or
/// SQLite schema. The canonical root is also the shared locator root for every dataset output.
fn plan_coordinated(pipelines: &[ResolvedPipeline]) -> Result<CoordinatedPlan, PipelineError> {
    let first = pipelines
        .first()
        .ok_or_else(|| PipelineError::InvalidConfig("coordinated mode has no pipelines".into()))?;
    let first_input = only_nfcapd_tree(first)?;
    let first_config = nfcapd_tree_config(first_input)?;
    let root_path = canonical_path(first_config.root_path)?;
    let mut sources = None;
    let mut dataset_sources = BTreeMap::new();
    let mut auto_discovered_datasets = BTreeSet::new();
    for pipeline in pipelines {
        let input = only_nfcapd_tree(pipeline)?;
        let config = nfcapd_tree_config(input)?;
        if root_path != canonical_path(config.root_path)? {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same nfcapd root".into(),
            ));
        }
        let resolved_sources = canonical_logical_sources(input)?;
        if let Some(expected) = &sources {
            if expected != &resolved_sources {
                return Err(PipelineError::InvalidConfig(
                    "coordinated datasets must use the same logical source layout and membership"
                        .into(),
                ));
            }
        } else {
            sources = Some(resolved_sources.clone());
        }
        let dataset_id = pipeline
            .datasets
            .first()
            .map(|dataset| dataset.dataset_id.clone())
            .ok_or_else(|| {
                PipelineError::InvalidConfig(
                    "coordinated datasets require registry-backed dataset metadata".into(),
                )
            })?;
        dataset_sources.insert(dataset_id.clone(), resolved_sources);
        if matches!(
            input,
            InputSpec::NfcapdTree {
                source_ids,
                sources,
                ..
            } if source_ids.is_empty() && sources.is_empty()
        ) {
            auto_discovered_datasets.insert(dataset_id);
        }
    }
    let sources = sources.expect("coordinated plan has at least one pipeline");
    let selected_start = parse_date_start(first_config.start_date, first.timezone.as_str())?;
    let explicit_end = first_config
        .end_date
        .map(|date| next_date_start(date, first.timezone.as_str()))
        .transpose()?;
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    validate_daily_active_source_layout(&sources, &physical_ids)?;
    let member_identities = capture_member_directory_identities(&root_path, &physical_ids)?;

    // Parse explicit dates before discovery as well as before output setup. This keeps malformed
    // finite windows side-effect free even when the source tree is large.
    let discovery_started = Instant::now();
    let discovered =
        ingest::discover_nfcapd_source_paths(&root_path, &physical_ids, first.timezone.as_str())?;
    tracing::info!(
        target: "netflow_db::profile",
        phase = "coordinated_discovery",
        elapsed_seconds = discovery_started.elapsed().as_secs_f64(),
        physical_sources = physical_ids.len(),
        discovered_inputs = discovered.len(),
    );
    let mut by_member_and_start = BTreeMap::new();
    let mut member_bounds = BTreeMap::new();
    for input in discovered {
        member_bounds
            .entry(input.source_id.clone())
            .and_modify(|(first, last): &mut (i64, i64)| {
                *first = (*first).min(input.bucket_start);
                *last = (*last).max(input.bucket_start);
            })
            .or_insert((input.bucket_start, input.bucket_start));
        by_member_and_start.insert((input.source_id, input.bucket_start), input.path);
    }
    let discovered_end = by_member_and_start
        .keys()
        .map(|(_, bucket_start)| *bucket_start)
        .max()
        .map(|start| aggregate_bounds(start, Granularity::OneDay, first.timezone.as_str()))
        .transpose()?
        .map(|(_, end)| end)
        .unwrap_or(selected_start);
    let selected_end = explicit_end.unwrap_or(discovered_end);
    let start = match first_config.start_time {
        Some(value) => parse_local_datetime(value, first.timezone.as_str())?,
        None => selected_start,
    };
    let end = match first_config.end_time {
        Some(value) => parse_local_datetime(value, first.timezone.as_str())?,
        None => selected_end,
    };
    validate_window(
        selected_start,
        selected_end,
        start,
        end,
        first.timezone.as_str(),
    )?;

    Ok(CoordinatedPlan {
        root_path,
        sources,
        dataset_sources,
        physical_ids,
        member_identities,
        auto_discovered_datasets,
        by_member_and_start,
        capture_snapshots: BTreeMap::new(),
        member_bounds,
        start,
        end,
        extend_gaps_to_window: first_config.end_date.is_some(),
        force: first_config.force,
        timezone: first.timezone.clone(),
    })
}

/// Re-check every auto-discovered dataset after all coordinated read-only planning and before
/// creating any output parent, lock, or database. Explicit layouts were already identity-checked
/// by [`normalize_sources`] while the plan was built.
fn verify_coordinated_auto_source_layouts(
    pipelines: &[ResolvedPipeline],
    plan: &CoordinatedPlan,
) -> Result<(), PipelineError> {
    for pipeline in pipelines {
        let dataset = pipeline.datasets.first().ok_or_else(|| {
            PipelineError::InvalidConfig(
                "coordinated datasets require registry-backed dataset metadata".into(),
            )
        })?;
        if !plan.auto_discovered_datasets.contains(&dataset.dataset_id) {
            continue;
        }
        let input = only_nfcapd_tree(pipeline)?;
        let current = canonical_logical_sources(input)?;
        let expected = plan
            .dataset_sources
            .get(&dataset.dataset_id)
            .expect("every coordinated dataset has a frozen source layout");
        if current != *expected {
            return Err(PipelineError::InvalidConfig(format!(
                "auto-discovered source layout changed for dataset {:?} during coordinated planning",
                dataset.dataset_id
            )));
        }
    }
    Ok(())
}

/// Execute the shared physical nfcapd scan while keeping each logical product independent.
///
/// Preparation is performed against every output so resume decisions remain output-local. Once a
/// batch is known to be needed, its physical files are decoded once and the canonical buckets are
/// fanned out to the outputs whose pending jobs reference them.
fn execute_many(pipelines: Vec<ResolvedPipeline>) -> Result<PipelineReport, PipelineError> {
    let output_paths = pipelines
        .iter()
        .map(|pipeline| pipeline.database_path.as_path())
        .collect::<Vec<_>>();
    validate_output_input_separation(
        &output_paths,
        pipelines
            .iter()
            .flat_map(|pipeline| pipeline.control_paths.iter().map(PathBuf::as_path)),
        "pipeline control path",
    )?;
    validate_database_path_separation(&output_paths)?;
    let mut plan = plan_coordinated(&pipelines)?;
    invoke_coordinated_plan_hook(&plan.root_path);
    verify_member_directory_identities(&plan.root_path, &plan.member_identities)?;
    for pipeline in &pipelines {
        verify_nfdump_revision(pipeline)?;
    }
    let locator_namespaces = nfcapd_locator_namespaces(&plan.root_path, &plan.physical_ids)?;
    if output_has_existing_identity(&output_paths)? {
        let capture_paths = plan
            .by_member_and_start
            .values()
            .cloned()
            .collect::<BTreeSet<_>>();
        plan.capture_snapshots = capture_nfcapd_snapshots(&capture_paths)?;
        validate_output_nfcapd_capture_separation_with_snapshots(
            &output_paths,
            &locator_namespaces,
            plan.capture_snapshots
                .iter()
                .map(|(path, snapshot)| (path.as_path(), snapshot)),
        )?;
    } else {
        validate_output_nfcapd_locator_separation(&output_paths, &locator_namespaces)?;
    }
    if !plan.auto_discovered_datasets.is_empty() {
        validate_output_nfcapd_auto_namespace_separation(
            &output_paths,
            std::slice::from_ref(&plan.root_path),
        )?;
    }
    if pipelines
        .first()
        .is_some_and(|pipeline| pipeline.nfdump_revision.is_some())
    {
        ingest::probe_nfdump_compatibility(&pipelines[0].nfdump)?;
        for pipeline in &pipelines {
            verify_nfdump_revision(pipeline)?;
        }
    }
    verify_coordinated_auto_source_layouts(&pipelines, &plan)?;
    let mut lock_order = (0..pipelines.len()).collect::<Vec<_>>();
    lock_order.sort_unstable_by_key(|index| normalized_path_key(&pipelines[*index].database_path));
    let mut locks: Vec<Option<DatabaseOperationLock>> =
        (0..pipelines.len()).map(|_| None).collect();
    for index in lock_order {
        let pipeline = &pipelines[index];
        if let Some(parent) = pipeline.database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        locks[index] = Some(DatabaseOperationLock::acquire(
            &pipeline.database_path,
            "coordinated pipeline build",
        )?);
    }

    let mut outputs = Vec::with_capacity(pipelines.len());
    let mut initialization_transactions = vec![false; pipelines.len()];
    for (index, pipeline) in pipelines.into_iter().enumerate() {
        let connection = match connect_pipeline_writer(&pipeline.database_path) {
            Ok(connection) => connection,
            Err(error) => {
                rollback_coordinated_transactions(&outputs, &initialization_transactions);
                return Err(coordinated_output_error(&pipeline, error.into()));
            }
        };
        outputs.push(CoordinatedOutput {
            pipeline,
            sources: plan.sources.clone(),
            connection,
            _lock: locks[index]
                .take()
                .expect("every coordinated output has a lock"),
        });
        let initialization = (|| {
            outputs[index]
                .connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(StorageError::from)?;
            initialization_transactions[index] = true;
            init_schema(&outputs[index].connection)?;
            initialize_coordinated_metadata_in_transaction(
                &outputs[index].connection,
                &outputs[index].pipeline,
                &plan.sources,
                &plan.dataset_sources,
            )
        })();
        if let Err(error) = initialization {
            rollback_coordinated_transactions(&outputs, &initialization_transactions);
            return Err(coordinated_output_error(&outputs[index].pipeline, error));
        }
    }
    if let Err(error) = verify_coordinated_nfdump_revisions(&outputs) {
        rollback_coordinated_transactions(&outputs, &initialization_transactions);
        let pipeline = &outputs[0].pipeline;
        return Err(coordinated_output_error(pipeline, error));
    }
    for (index, output) in outputs.iter().enumerate() {
        if let Err(error) = output.connection.execute_batch("COMMIT") {
            rollback_coordinated_transactions(&outputs, &initialization_transactions);
            return Err(coordinated_output_error(
                &output.pipeline,
                PipelineError::Storage(StorageError::from(error)),
            ));
        }
        initialization_transactions[index] = false;
    }

    let mut pools = CoordinatedPools::new()?;
    let mut report = PipelineReport::default();
    let mut day_start = plan.start;
    while day_start < plan.end {
        verify_member_directory_identities(&plan.root_path, &plan.member_identities)?;
        let day_end = aggregate_bounds(day_start, Granularity::OneDay, &plan.timezone)?.1;
        let capture_complete = day_capture_is_complete(
            &plan.sources,
            &plan.by_member_and_start,
            day_start,
            day_end,
            &plan.timezone,
        )?;
        let mut stale_outputs = Vec::new();
        let mut canonical_day_verified = vec![false; outputs.len()];
        let mut marker_needs_backfill = vec![false; outputs.len()];
        for (index, output) in outputs.iter().enumerate() {
            let published_day =
                day_was_published(&output.connection, &output.sources, day_start, day_end)?;
            if !capture_complete {
                if published_day {
                    stale_outputs.push(index);
                }
            } else if published_day && !plan.force {
                match nfcapd_day_completion_state(
                    &output.connection,
                    &output.sources,
                    day_start,
                    day_end,
                    output.pipeline.run_maad,
                )? {
                    DailyProductCompletionState::Clean => {
                        canonical_day_verified[index] = true;
                    }
                    DailyProductCompletionState::Dirty => {
                        return Err(PipelineError::InvalidConfig(format!(
                            "published local day {day_start}..{day_end} was mutated after completion for database {}; rerun that whole day with --force",
                            output.pipeline.database_path.display()
                        )));
                    }
                    DailyProductCompletionState::Missing => {
                        if !nfcapd_day_has_canonical_topology(
                            &output.connection,
                            &output.sources,
                            day_start,
                            day_end,
                            &plan.timezone,
                            output.pipeline.run_maad,
                        )? {
                            return Err(PipelineError::InvalidConfig(format!(
                                "published local day {day_start}..{day_end} has damaged canonical topology for database {}; rerun that whole day with --force",
                                output.pipeline.database_path.display()
                            )));
                        }
                        canonical_day_verified[index] = true;
                        marker_needs_backfill[index] = true;
                    }
                }
            }
        }
        let reset_outputs = if plan.force {
            (0..outputs.len()).collect::<Vec<_>>()
        } else {
            stale_outputs
        };
        if !reset_outputs.is_empty() && !plan.force {
            return Err(PipelineError::InvalidConfig(format!(
                "published local day {day_start}..{day_end} no longer has complete nfcapd capture coverage; rerun that day with --force"
            )));
        }
        let missing = missing_physical_day_inputs(
            &plan.physical_ids,
            &plan.by_member_and_start,
            day_start,
            day_end,
            &plan.timezone,
        )?;
        let missing_absences = build_missing_day_absences(
            &plan.root_path,
            &missing,
            day_start,
            day_end,
            &plan.timezone,
        )?;
        invoke_missing_day_absence_hook(&plan.root_path, &missing, &plan.timezone);
        if !missing.is_empty() {
            let missing_details =
                missing_day_warning_details(&plan.root_path, &missing, &plan.timezone)?;
            tracing::warn!(
                day_start,
                day_end,
                missing_inputs = missing.len(),
                missing_details = %missing_details,
                "skipping incomplete physical day for coordinated selections"
            );
            report.skipped_inputs += missing.len();
            if reset_outputs.is_empty() {
                day_start = day_end;
                continue;
            }
        }

        let day_reports = process_coordinated_day(
            &mut outputs,
            &plan.root_path,
            &plan.physical_ids,
            &plan.member_identities,
            &plan.by_member_and_start,
            &plan.member_bounds,
            day_start,
            day_end,
            plan.extend_gaps_to_window,
            plan.force,
            &reset_outputs,
            !missing.is_empty(),
            &missing_absences,
            &canonical_day_verified,
            &marker_needs_backfill,
            &plan.capture_snapshots,
            &mut pools,
        )?;
        for day_report in day_reports {
            merge_report(&mut report, day_report);
        }
        day_start = day_end;
    }

    for output in &outputs {
        infer_default_start_dates(&output.connection, &output.pipeline)?;
        let mut coverage_report = PipelineReport::default();
        populate_coverage_summary(&output.connection, &mut coverage_report)?;
        report.complete_five_minute_buckets += coverage_report.complete_five_minute_buckets;
        report.partial_five_minute_buckets += coverage_report.partial_five_minute_buckets;
        report.unknown_five_minute_buckets += coverage_report.unknown_five_minute_buckets;
        if let Err(error) = optimize_all_query_planner_statistics(&output.connection) {
            tracing::warn!(%error, "could not refresh SQLite planner statistics");
        }
        if output.pipeline.require_complete {
            let incomplete = count_incomplete_coverage_for_layout(
                &output.connection,
                &plan.sources,
                plan.start,
                plan.end,
                &plan.timezone,
            )?;
            if incomplete != 0 {
                let dataset_id = output
                    .pipeline
                    .datasets
                    .first()
                    .map_or("<unknown>", |dataset| dataset.dataset_id.as_str());
                return Err(PipelineError::InvalidConfig(format!(
                    "dataset {dataset_id:?} database {} has {incomplete} incomplete five-minute coverage buckets",
                    output.pipeline.database_path.display()
                )));
            }
        }
    }
    Ok(report)
}

fn verify_coordinated_postflight_snapshot(
    path: &Path,
    snapshot: &FileSnapshot,
) -> Result<(), ProvenanceError> {
    #[cfg(test)]
    COORDINATED_POSTFLIGHT_SNAPSHOT_VERIFICATIONS.with(|calls| calls.set(calls.get() + 1));
    verify_file_snapshot(path, snapshot)
}

fn verify_coordinated_nfdump_revisions(outputs: &[CoordinatedOutput]) -> Result<(), PipelineError> {
    for output in outputs {
        verify_nfdump_revision(&output.pipeline)?;
    }
    Ok(())
}

/// Check every external input guard while all coordinated output transactions are still open.
///
/// This is deliberately the last fallible phase before the commit loop. Keeping the loop itself
/// to COMMIT and transaction bookkeeping prevents a late capture or decoder replacement from
/// making one output commit while another rolls back.
#[allow(clippy::too_many_arguments)]
fn verify_coordinated_precommit_guards<'a, 'b>(
    outputs: &[CoordinatedOutput],
    root: &Path,
    member_identities: &BTreeMap<String, MemberDirectoryIdentity>,
    revisions: impl IntoIterator<Item = (&'a Path, &'a FileSnapshot)>,
    activity_snapshots: impl IntoIterator<Item = (&'b Path, &'b FileSnapshot)>,
    missing_absences: &[ExpectedAbsence],
    start: i64,
    end: i64,
) -> Result<(), PipelineError> {
    invoke_coordinated_commit_guard_hook();
    verify_member_directory_identities(root, member_identities)?;
    for (path, snapshot) in revisions {
        verify_coordinated_postflight_snapshot(path, snapshot)?;
    }
    for (path, snapshot) in activity_snapshots {
        verify_coordinated_postflight_snapshot(path, snapshot)?;
    }
    verify_coordinated_nfdump_revisions(outputs)?;
    verify_missing_day_absences(missing_absences, start, end)
}

#[allow(clippy::too_many_arguments)]
fn process_coordinated_day(
    outputs: &mut [CoordinatedOutput],
    root: &Path,
    physical_ids: &[String],
    member_identities: &BTreeMap<String, MemberDirectoryIdentity>,
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    start: i64,
    end: i64,
    extend_gaps_to_window: bool,
    force: bool,
    reset_outputs: &[usize],
    skip_incomplete_day: bool,
    missing_absences: &[ExpectedAbsence],
    canonical_day_verified: &[bool],
    marker_needs_backfill: &[bool],
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
    pools: &mut CoordinatedPools,
) -> Result<Vec<PipelineReport>, PipelineError> {
    verify_member_directory_identities(root, member_identities)?;
    verify_coordinated_nfdump_revisions(outputs)?;
    let reset_set = reset_outputs.iter().copied().collect::<BTreeSet<_>>();
    if skip_incomplete_day {
        if reset_outputs.is_empty() {
            return Ok((0..outputs.len())
                .map(|_| PipelineReport::default())
                .collect());
        }
        let mut transactions = (0..outputs.len()).map(|_| false).collect::<Vec<_>>();
        for &index in reset_outputs {
            if let Err(error) = outputs[index].connection.execute_batch("BEGIN IMMEDIATE") {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(PipelineError::Storage(StorageError::from(error)));
            }
            transactions[index] = true;
            let source_ids = outputs[index]
                .sources
                .iter()
                .map(|source| source.source_id.clone())
                .collect::<Vec<_>>();
            if let Err(error) =
                delete_stats_time_range(&outputs[index].connection, &source_ids, start, end)
            {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(PipelineError::Storage(error));
            }
        }
        if let Err(error) = verify_coordinated_precommit_guards(
            outputs,
            root,
            member_identities,
            std::iter::empty(),
            std::iter::empty(),
            missing_absences,
            start,
            end,
        ) {
            rollback_coordinated_transactions(outputs, &transactions);
            return Err(error);
        }
        for &index in reset_outputs {
            if let Err(error) = outputs[index].connection.execute_batch("COMMIT") {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(PipelineError::Storage(StorageError::from(error)));
            }
            transactions[index] = false;
        }
        return Ok((0..outputs.len())
            .map(|_| PipelineReport::default())
            .collect());
    }

    let resume_caches = outputs
        .iter()
        .map(|output| NfcapdDayResumeCache::load(&output.connection, &output.sources, start, end))
        .collect::<Result<Vec<_>, _>>()?;

    let mut owned_keys = BTreeSet::new();
    let mut bucket_start = start;
    while bucket_start < end {
        for source in &outputs[0].sources {
            if (force || !reset_set.is_empty())
                && source_has_candidate(
                    source,
                    bucket_start,
                    by_member_and_start,
                    member_bounds,
                    extend_gaps_to_window,
                )
            {
                owned_keys.insert((source.source_id.clone(), bucket_start));
            }
        }
        bucket_start = next_local_five_minute_start(bucket_start, &outputs[0].pipeline.timezone)?;
    }

    let mut reports = (0..outputs.len())
        .map(|_| PipelineReport::default())
        .collect::<Vec<_>>();
    let mut profiles = (0..outputs.len())
        .map(|_| NfcapdDayPublishProfile::default())
        .collect::<Vec<_>>();
    let mut shared_profile = CoordinatedDaySharedProfile {
        active_set_counts: vec![0; outputs.len()],
        ..CoordinatedDaySharedProfile::default()
    };
    let mut pending_set = BTreeSet::new();
    let mut has_repair = false;
    let mut batches = Vec::new();
    let mut all_revisions = BTreeMap::new();
    let mut next = start;
    while next < end {
        let batch_starts = nfcapd_batch_starts(
            next,
            end,
            &outputs[0].pipeline.timezone,
            &outputs[0].sources,
            by_member_and_start,
            member_bounds,
            extend_gaps_to_window,
        )?;
        next = batch_starts
            .last()
            .copied()
            .map(|last| next_local_five_minute_start(last, &outputs[0].pipeline.timezone))
            .transpose()?
            .expect("non-empty coordinated nfcapd batch while processing a non-empty window");
        let revision_started = Instant::now();
        let revisions = resolve_coordinated_batch_revisions_with_cache(
            outputs,
            &outputs[0].sources,
            by_member_and_start,
            member_bounds,
            extend_gaps_to_window,
            force,
            &pools.revision,
            &batch_starts,
            &resume_caches,
            capture_snapshots,
        )?;
        shared_profile.revision_elapsed += revision_started.elapsed();
        shared_profile.revision_paths += profile_count(revisions.len());
        all_revisions.extend(
            revisions
                .iter()
                .map(|(path, revision)| (path.clone(), revision.clone())),
        );
        let mut prepared = BTreeMap::new();
        for (index, output) in outputs.iter().enumerate() {
            let output_prepare_started = Instant::now();
            let mut output_batch = Vec::with_capacity(batch_starts.len());
            for &bucket_start in &batch_starts {
                let mut preflight_report = PipelineReport::default();
                let timestamp = prepare_nfcapd_tree_timestamp_with_cache(
                    &output.connection,
                    root,
                    &output.sources,
                    by_member_and_start,
                    member_bounds,
                    bucket_start,
                    extend_gaps_to_window,
                    force || reset_set.contains(&index),
                    &output.pipeline,
                    &mut preflight_report,
                    &revisions,
                    canonical_day_verified[index],
                    Some(&resume_caches[index]),
                )?;
                reports[index].skipped_inputs += preflight_report.skipped_inputs;
                if !timestamp.jobs.is_empty() {
                    pending_set.insert(index);
                }
                has_repair |= timestamp.jobs.iter().any(|job| job.is_repair);
                output_batch.push(timestamp);
            }
            let output_prepare_elapsed = output_prepare_started.elapsed();
            profiles[index].prepare_elapsed += output_prepare_elapsed;
            shared_profile.eligibility_elapsed += output_prepare_elapsed;
            if output_batch
                .iter()
                .any(|timestamp| !timestamp.jobs.is_empty())
            {
                prepared.insert(index, output_batch);
            }
        }
        batches.push(PreparedCoordinatedBatch { prepared });
    }
    if !force && has_repair {
        return Err(PipelineError::InvalidConfig(format!(
            "daily_active_sources input changed for local day {start}..{end}; rerun that whole day with --force"
        )));
    }

    let pending = pending_set.into_iter().collect::<Vec<_>>();
    let mut activity_snapshots = Vec::new();
    let mut active_sources = (0..outputs.len())
        .map(|_| None)
        .collect::<Vec<Option<Arc<AddressSet>>>>();
    if !pending.is_empty() {
        verify_coordinated_nfdump_revisions(outputs)?;
        let activity_started = Instant::now();
        let selections = pending
            .iter()
            .map(|index| outputs[*index].pipeline.selection.clone())
            .collect::<Vec<_>>();
        let activity_pool = pools.activity()?;
        let (resolved_active_sources, snapshots) = resolve_coordinated_daily_active_sources(
            physical_ids,
            by_member_and_start,
            start,
            end,
            &outputs[0].pipeline.timezone,
            &selections,
            outputs[0].pipeline.nfdump.as_path(),
            activity_pool,
            capture_snapshots,
            &all_revisions,
        )?;
        shared_profile.activity_elapsed += activity_started.elapsed();
        shared_profile.activity_members = profile_count(physical_ids.len());
        activity_snapshots = snapshots;
        verify_coordinated_nfdump_revisions(outputs)?;
        shared_profile.activity_inputs = profile_count(activity_snapshots.len());
        for (path, snapshot) in &activity_snapshots {
            verify_file_snapshot(path, snapshot)?;
        }
        for (pending_index, active) in pending.iter().zip(resolved_active_sources) {
            let active_count = profile_count(active.len());
            profiles[*pending_index].active_set_count = active_count;
            shared_profile.active_set_counts[*pending_index] = active_count;
            active_sources[*pending_index] = Some(active);
        }
    }

    if pending.is_empty()
        && reset_outputs.is_empty()
        && !marker_needs_backfill.iter().copied().any(|needed| needed)
    {
        shared_profile.log(start, end);
        return Ok(reports);
    }

    let transaction_indices = (0..outputs.len())
        .filter(|index| {
            reset_set.contains(index) || pending.contains(index) || marker_needs_backfill[*index]
        })
        .collect::<Vec<_>>();
    let mut transactions = (0..outputs.len()).map(|_| false).collect::<Vec<_>>();
    let transaction_started = Instant::now();
    let mut aggregates = (0..outputs.len())
        .map(|index| {
            pending
                .contains(&index)
                .then(|| AggregateBuckets::with_owned_keys(owned_keys.clone()))
        })
        .collect::<Vec<_>>();
    for &index in &transaction_indices {
        if let Err(error) = outputs[index].connection.execute_batch("BEGIN IMMEDIATE") {
            rollback_coordinated_transactions(outputs, &transactions);
            return Err(PipelineError::Storage(StorageError::from(error)));
        }
        transactions[index] = true;
        if reset_set.contains(&index) {
            let source_ids = outputs[index]
                .sources
                .iter()
                .map(|source| source.source_id.clone())
                .collect::<Vec<_>>();
            if let Err(error) =
                delete_stats_time_range(&outputs[index].connection, &source_ids, start, end)
            {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(PipelineError::Storage(error));
            }
        }
        let source_ids = outputs[index]
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        if let Err(error) = provision_daily_product_completion_bucket_guards(
            &outputs[index].connection,
            &source_ids,
            start,
            end,
        ) {
            rollback_coordinated_transactions(outputs, &transactions);
            return Err(PipelineError::Storage(error));
        }
    }

    let result = if pending.is_empty() {
        Ok(())
    } else {
        verify_coordinated_nfdump_revisions(outputs)?;
        let decode_pool = pools.decode()?;
        process_coordinated_batches(
            outputs,
            &pending,
            &mut reports,
            &mut profiles,
            &mut shared_profile,
            &mut aggregates,
            by_member_and_start,
            force,
            &mut batches,
            &all_revisions,
            &active_sources,
            decode_pool,
        )
    };
    if let Err(error) = result {
        rollback_coordinated_transactions(outputs, &transactions);
        return Err(error);
    }
    let day_publish_elapsed = transaction_started.elapsed();
    for &index in &pending {
        let aggregates = aggregates[index]
            .take()
            .expect("pending output has aggregate state");
        let final_profile = match publish_rollups_profiled(
            &outputs[index].connection,
            aggregates,
            &outputs[index].pipeline,
            &mut reports[index],
        ) {
            Ok(profile) => profile,
            Err(error) => {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(error);
            }
        };
        profiles[index].final_rollups = final_profile;
    }
    let revision_snapshots = all_revisions.values().filter_map(|revision| {
        revision
            .snapshot
            .as_ref()
            .map(|snapshot| (Path::new(&revision.revision.locator), snapshot))
    });
    let activity_snapshot_refs = activity_snapshots
        .iter()
        .map(|(path, snapshot)| (path.as_path(), snapshot));
    if let Err(error) = verify_coordinated_precommit_guards(
        outputs,
        root,
        member_identities,
        revision_snapshots,
        activity_snapshot_refs,
        missing_absences,
        start,
        end,
    ) {
        rollback_coordinated_transactions(outputs, &transactions);
        return Err(error);
    }
    if !skip_incomplete_day {
        for &index in &transaction_indices {
            if let Err(error) = mark_nfcapd_day_complete(
                &outputs[index].connection,
                &outputs[index].sources,
                start,
                end,
                outputs[index].pipeline.run_maad,
            ) {
                rollback_coordinated_transactions(outputs, &transactions);
                return Err(error);
            }
        }
    }
    for &index in &transaction_indices {
        if let Err(error) = outputs[index].connection.execute_batch("COMMIT") {
            rollback_coordinated_transactions(outputs, &transactions);
            return Err(PipelineError::Storage(StorageError::from(error)));
        }
        transactions[index] = false;
    }
    let transaction_elapsed = transaction_started.elapsed();
    shared_profile.log(start, end);
    for &index in &pending {
        profiles[index].day_elapsed = day_publish_elapsed;
        profiles[index].log_coordinated(
            start,
            end,
            transaction_elapsed,
            index,
            &outputs[index].pipeline.database_path,
        );
    }
    Ok(reports)
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn resolve_coordinated_batch_revisions(
    outputs: &[CoordinatedOutput],
    sources: &[DatasetSource],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    extend_gaps_to_window: bool,
    force: bool,
    revision_pool: &rayon::ThreadPool,
    batch_starts: &[i64],
) -> Result<BTreeMap<PathBuf, PreparedRevision>, PipelineError> {
    resolve_coordinated_batch_revisions_with_cache(
        outputs,
        sources,
        by_member_and_start,
        member_bounds,
        extend_gaps_to_window,
        force,
        revision_pool,
        batch_starts,
        &[],
        &BTreeMap::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_coordinated_batch_revisions_with_cache(
    outputs: &[CoordinatedOutput],
    sources: &[DatasetSource],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    extend_gaps_to_window: bool,
    force: bool,
    revision_pool: &rayon::ThreadPool,
    batch_starts: &[i64],
    resume_caches: &[NfcapdDayResumeCache],
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
) -> Result<BTreeMap<PathBuf, PreparedRevision>, PipelineError> {
    let mut paths = BTreeSet::new();
    for &bucket_start in batch_starts {
        for source in sources {
            if !source_has_candidate(
                source,
                bucket_start,
                by_member_and_start,
                member_bounds,
                extend_gaps_to_window,
            ) {
                continue;
            }
            paths.extend(source.members.iter().filter_map(|member| {
                by_member_and_start
                    .get(&(member.clone(), bucket_start))
                    .cloned()
            }));
        }
    }
    let decoder_fingerprint = nfdump_decoder_fingerprint_for_pipeline(&outputs[0].pipeline)?;
    let probes = paths
        .into_iter()
        .map(|path| {
            let observed = capture_snapshots
                .get(&path)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| capture_nfcapd_snapshot(&path))?;
            let cached = if force {
                None
            } else {
                let mut shared = None;
                let mut conflict = false;
                for (index, output) in outputs.iter().enumerate() {
                    let fingerprint = match resume_caches.get(index) {
                        Some(cache) => cache.cached_content_fingerprint(&path, &observed),
                        None => cached_content_fingerprint(
                            &output.connection,
                            InputKind::Nfcapd,
                            &path.to_string_lossy(),
                            &observed,
                        )?,
                    };
                    match (&shared, fingerprint) {
                        (None, Some(value)) => shared = Some(value),
                        (Some(previous), Some(value)) if previous == &value => {}
                        (Some(_), Some(_)) => conflict = true,
                        (None, None) => {}
                        (Some(_), None) => {}
                    }
                }
                (!conflict).then_some(shared).flatten()
            };
            Ok::<_, PipelineError>((path, observed, cached))
        })
        .collect::<Result<Vec<_>, _>>()?;
    revision_pool.install(|| {
        probes
            .par_iter()
            .map(|(path, observed, cached)| {
                let (content_fingerprint, snapshot) = match cached {
                    Some(content_fingerprint) => (content_fingerprint.clone(), observed.clone()),
                    None => capture_file_revision_with_snapshot(path, observed)?,
                };
                let revision = InputRevision::create(
                    "nfcapd",
                    path.to_string_lossy().into_owned(),
                    content_fingerprint,
                    &decoder_fingerprint,
                )?;
                Ok::<_, PipelineError>((
                    path.clone(),
                    PreparedRevision {
                        revision,
                        snapshot: Some(snapshot),
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
    })
}

type CoordinatedActiveResolution = (Vec<Arc<AddressSet>>, Vec<(PathBuf, FileSnapshot)>);
type DailyActiveResolution = (Arc<AddressSet>, Vec<(PathBuf, FileSnapshot)>);

/// Return only the capture paths on the publication grid for one physical local day.
///
/// Discovery intentionally accepts every valid nfcapd timestamp, but daily eligibility must use
/// the same five-minute keys that publication reads. An off-grid capture can therefore never
/// contribute activity merely because it falls inside the day's lexical path range.
fn nfcapd_day_activity_paths(
    paths: &BTreeMap<(String, i64), PathBuf>,
    member: &str,
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<Vec<PathBuf>, PipelineError> {
    let mut member_paths = Vec::new();
    let mut bucket_start = start;
    while bucket_start < end {
        if let Some(path) = paths.get(&(member.to_owned(), bucket_start)) {
            member_paths.push(path.clone());
        }
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    Ok(member_paths)
}

fn daily_activity_scan_error(
    member: &str,
    start: i64,
    end: i64,
    paths: &[PathBuf],
    error: impl std::fmt::Display,
) -> PipelineError {
    let paths = if paths.is_empty() {
        "<none>".to_owned()
    } else {
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    PipelineError::InvalidConfig(format!(
        "daily activity scan failed for member {member:?}, day {start}..{end}, paths [{paths}]: {error}"
    ))
}

fn nfcapd_decode_error(
    member: &str,
    bucket_start: i64,
    path: &Path,
    error: impl std::fmt::Display,
) -> PipelineError {
    PipelineError::InvalidConfig(format!(
        "nfcapd decode failed for member {member:?}, bucket {bucket_start}, path {}: {error}",
        path.display()
    ))
}

#[allow(clippy::too_many_arguments)]
fn resolve_coordinated_daily_active_sources(
    physical_ids: &[String],
    paths: &BTreeMap<(String, i64), PathBuf>,
    start: i64,
    end: i64,
    timezone: &str,
    selections: &[FlowSelection],
    executable: &Path,
    activity_pool: &rayon::ThreadPool,
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
    revision_snapshots: &BTreeMap<PathBuf, PreparedRevision>,
) -> Result<CoordinatedActiveResolution, PipelineError> {
    let mut combined = (0..selections.len())
        .map(|_| HashMap::<IpAddr, nfdump::SourceActivity>::new())
        .collect::<Vec<_>>();
    let mut snapshots = Vec::new();
    for member_chunk in physical_ids.chunks(NFCAPD_DECODE_BATCH_SIZE) {
        let requests = member_chunk
            .iter()
            .map(|member| {
                nfcapd_day_activity_paths(paths, member, start, end, timezone)
                    .map(|member_paths| (member.clone(), member_paths))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let member_results = activity_pool.install(|| {
            requests
                .par_iter()
                .map(|(member, member_paths)| {
                    let snapshots = member_paths
                        .iter()
                        .map(|path| {
                            let snapshot = capture_snapshots
                                .get(path)
                                .cloned()
                                .or_else(|| {
                                    revision_snapshots
                                        .get(path)
                                        .and_then(|revision| revision.snapshot.clone())
                                })
                                .map(Ok)
                                .unwrap_or_else(|| capture_nfcapd_snapshot(path));
                            snapshot
                                .map(|snapshot| (path.clone(), snapshot))
                                .map_err(|error| {
                                    daily_activity_scan_error(
                                        member,
                                        start,
                                        end,
                                        member_paths,
                                        error,
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>, PipelineError>>()?;
                    let activities = ingest::read_nfcapd_daily_source_activities(
                        member_paths,
                        selections,
                        executable,
                    )
                    .map_err(|error| {
                        daily_activity_scan_error(member, start, end, member_paths, error)
                    })?;
                    if activities.len() != selections.len() {
                        return Err(daily_activity_scan_error(
                            member,
                            start,
                            end,
                            member_paths,
                            format!(
                                "daily activity decoder returned {} results for {} selections",
                                activities.len(),
                                selections.len()
                            ),
                        ));
                    }
                    Ok::<_, PipelineError>((activities, snapshots))
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        for (activities, member_snapshots) in member_results {
            snapshots.extend(member_snapshots);
            for (selection_index, activity) in activities.into_iter().enumerate() {
                for (address, metrics) in activity {
                    combined[selection_index]
                        .entry(address)
                        .or_default()
                        .include(metrics);
                }
            }
        }
    }
    let active_sources = combined
        .into_iter()
        .map(|activity| {
            Arc::new(
                activity
                    .into_iter()
                    .filter_map(|(address, metrics)| {
                        FlowSelection::daily_activity_threshold_met(
                            metrics.flows,
                            metrics.packets,
                            metrics.bytes,
                        )
                        .then_some(address)
                    })
                    .collect(),
            )
        })
        .collect();
    Ok((active_sources, snapshots))
}

#[allow(clippy::too_many_arguments)]
fn process_coordinated_batches(
    outputs: &[CoordinatedOutput],
    pending: &[usize],
    reports: &mut [PipelineReport],
    profiles: &mut [NfcapdDayPublishProfile],
    shared_profile: &mut CoordinatedDaySharedProfile,
    aggregates: &mut [Option<AggregateBuckets>],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    force: bool,
    batches: &mut [PreparedCoordinatedBatch],
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
    active_sources: &[Option<Arc<AddressSet>>],
    decode_pool: &rayon::ThreadPool,
) -> Result<(), PipelineError> {
    if pending.is_empty() {
        return Ok(());
    }
    let executable = outputs[pending[0]].pipeline.nfdump.clone();
    let timezone = outputs[pending[0]].pipeline.timezone.clone();
    for batch in batches {
        verify_coordinated_nfdump_revisions(outputs)?;
        let prepared = std::mem::take(&mut batch.prepared);

        let mut needed = BTreeMap::<(String, i64), BTreeSet<usize>>::new();
        for (&output_index, batch) in &prepared {
            for timestamp in batch {
                for job in &timestamp.jobs {
                    for (member, _) in &job.present {
                        needed
                            .entry((member.clone(), timestamp.bucket_start))
                            .or_default()
                            .insert(output_index);
                    }
                }
            }
        }

        let decode_requests = needed
            .iter()
            .map(|((member, bucket_start), output_indices)| {
                let path = by_member_and_start
                    .get(&(member.clone(), *bucket_start))
                    .cloned()
                    .ok_or_else(|| {
                        PipelineError::InvalidConfig(format!(
                            "coordinated decoder could not locate physical input {member}:{bucket_start}"
                        ))
                    })?;
                let output_indices = output_indices.iter().copied().collect::<Vec<_>>();
                let pairs = output_indices
                    .iter()
                    .map(|index| {
                        (
                            outputs[*index].pipeline.selection.clone(),
                            active_sources[*index]
                                .clone()
                                .expect("pending daily selection has active sources"),
                        )
                    })
                    .collect::<Vec<_>>();
                let snapshot = revisions
                    .get(&path)
                    .and_then(|owner| owner.snapshot.as_ref())
                    .ok_or_else(|| {
                        PipelineError::InvalidConfig(format!(
                            "coordinated decoder has no revision snapshot for {member}:{bucket_start}"
                        ))
                    })?;
                Ok::<_, PipelineError>((
                    member.clone(),
                    *bucket_start,
                    path,
                    output_indices,
                    pairs,
                    snapshot,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Keep at most twelve child processes active, even when one logical timestamp fans out to
        // many physical members. The decoded map is keyed by physical request and retains the
        // single fanout vector returned by nfdump rather than an output-expanded result list.
        let decode_started = Instant::now();
        let mut decoded = BTreeMap::<(String, i64), Vec<(usize, CanonicalBucket)>>::new();
        for request_chunk in nfcapd_decode_request_chunks(&decode_requests) {
            verify_coordinated_nfdump_revisions(outputs)?;
            let decoded_results = decode_pool.install(|| {
                request_chunk
                    .par_iter()
                    .map(
                        |(member, bucket_start, path, output_indices, pairs, snapshot)| {
                            let buckets = ingest::read_nfcapd_buckets_with_active_sources(
                                path,
                                member,
                                pairs,
                                &executable,
                                &timezone,
                            )
                            .map_err(|error| {
                                nfcapd_decode_error(member, *bucket_start, path, error)
                            })?;
                            if buckets.len() != output_indices.len() {
                                return Err(nfcapd_decode_error(
                                    member,
                                    *bucket_start,
                                    path,
                                    format!(
                                        "bucket decoder returned {} results for {} selections",
                                        buckets.len(),
                                        output_indices.len()
                                    ),
                                ));
                            }
                            verify_file_snapshot(path, snapshot).map_err(|error| {
                                nfcapd_decode_error(member, *bucket_start, path, error)
                            })?;
                            Ok::<_, PipelineError>((
                                member.clone(),
                                *bucket_start,
                                output_indices.clone(),
                                buckets,
                            ))
                        },
                    )
                    .collect::<Result<Vec<_>, _>>()
            })?;
            verify_coordinated_nfdump_revisions(outputs)?;
            for (member, bucket_start, output_indices, buckets) in decoded_results {
                decoded.insert(
                    (member, bucket_start),
                    output_indices.into_iter().zip(buckets).collect(),
                );
            }
        }
        let decode_elapsed = decode_started.elapsed();
        shared_profile.decode_elapsed += decode_elapsed;
        for &output_index in prepared.keys() {
            profiles[output_index].decode_elapsed += decode_elapsed;
        }

        for (&output_index, batch) in &prepared {
            let aggregate = aggregates[output_index]
                .as_mut()
                .expect("pending output has aggregate state");
            let publish_started = Instant::now();
            for timestamp in batch {
                for job in &timestamp.jobs {
                    let member_buckets = job
                        .present
                        .iter()
                        .map(|(member, _)| {
                            decoded
                                .get(&(member.clone(), timestamp.bucket_start))
                                .and_then(|buckets| {
                                    buckets.iter().find_map(|(index, bucket)| {
                                        (*index == output_index).then_some(bucket)
                                    })
                                })
                                .expect("requested physical member was decoded")
                        })
                        .collect::<Vec<_>>();
                    let logical_started = Instant::now();
                    let logical = logical_source_bucket(
                        &job.source_id,
                        timestamp.bucket_start,
                        job.expected_units,
                        &member_buckets,
                    )?;
                    profiles[output_index].logical_source_elapsed += logical_started.elapsed();
                    let sibling_started = Instant::now();
                    if !job.is_repair {
                        aggregate.reject_persisted_siblings(
                            &outputs[output_index].connection,
                            &logical,
                            &outputs[output_index].pipeline.timezone,
                        )?;
                    }
                    profiles[output_index].persisted_sibling_elapsed += sibling_started.elapsed();
                    let bucket_profile = publish_nfcapd_bucket_profiled(
                        &outputs[output_index].connection,
                        &logical,
                        &job.owners,
                        &job.absences,
                        &job.evidence,
                        true,
                        force,
                        outputs[output_index].pipeline.run_maad,
                    )?;
                    profiles[output_index]
                        .bucket_publish
                        .include(bucket_profile);
                    let flushed = if job.is_repair {
                        refresh_rollups_after_five_minute_repair(
                            &outputs[output_index].connection,
                            &logical,
                            &outputs[output_index].pipeline.timezone,
                        )?;
                        0
                    } else {
                        let aggregate_profile = aggregate
                            .include_profiled(&logical, &outputs[output_index].pipeline.timezone)?;
                        profiles[output_index]
                            .aggregate_include
                            .include(aggregate_profile);
                        let flush_started = Instant::now();
                        let (flushed, rollup_write) = aggregate.flush_complete_profiled(
                            &outputs[output_index].connection,
                            outputs[output_index].pipeline.run_maad,
                        )?;
                        profiles[output_index].completed_rollup_flush_elapsed +=
                            flush_started.elapsed();
                        profiles[output_index]
                            .completed_rollup_write
                            .include(rollup_write);
                        profiles[output_index].completed_rollup_flushes += 1;
                        if flushed > 0 {
                            profiles[output_index].nonempty_rollup_flushes += 1;
                        }
                        flushed
                    };
                    profiles[output_index].logical_buckets += 1;
                    reports[output_index].rollup_buckets += flushed;
                    reports[output_index].five_minute_buckets += 1;
                }
            }
            profiles[output_index].batch_publish_elapsed += publish_started.elapsed();
        }
    }
    Ok(())
}

fn rollback_coordinated_transactions(outputs: &[CoordinatedOutput], transactions: &[bool]) {
    for (output, active) in outputs.iter().zip(transactions) {
        if *active {
            let _ = output.connection.execute_batch("ROLLBACK");
        }
    }
}

/// Give every dataset without a configured `default_start_date` the earliest ingested local day.
///
/// This runs after ingestion so that newly ingested earlier days move the stored date back. Until
/// the database holds traffic, the row keeps the fallback that [`upsert_dataset_metadata`] wrote.
fn infer_default_start_dates(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<(), PipelineError> {
    let inferred = pipeline
        .datasets
        .iter()
        .filter(|dataset| dataset.default_start_date.trim().is_empty())
        .collect::<Vec<_>>();
    if inferred.is_empty() {
        return Ok(());
    }
    let Some(bucket_start) = earliest_traffic_bucket_start(connection)? else {
        return Ok(());
    };
    let date = local_date(bucket_start, &pipeline.timezone)?;
    with_transaction(connection, || {
        for dataset in inferred {
            set_dataset_default_start_date(connection, &dataset.dataset_id, &date)?;
        }
        Ok(())
    })
}

/// Local calendar day that contains `timestamp`, formatted as `YYYY-MM-DD`.
fn local_date(timestamp: i64, timezone: &str) -> Result<String, PipelineError> {
    Ok(Timestamp::from_second(timestamp)
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .in_tz(timezone)
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .date()
        .to_string())
}

fn with_transaction<T>(
    connection: &Connection,
    operation: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    with_transaction_precommit(connection, operation, || Ok(()))
}

/// Run a transaction with a final read-only guard immediately before COMMIT.
///
/// The guard runs after all writes have completed while the transaction is still open. Any guard
/// failure therefore rolls back the writes instead of leaving a partially repaired product behind.
fn with_transaction_precommit<T>(
    connection: &Connection,
    operation: impl FnOnce() -> Result<T, PipelineError>,
    precommit: impl FnOnce() -> Result<(), PipelineError>,
) -> Result<T, PipelineError> {
    with_transaction_precommit_value(
        connection,
        || operation().map(|value| (value, ())),
        |_| precommit(),
    )
}

fn with_transaction_precommit_value<T, G>(
    connection: &Connection,
    operation: impl FnOnce() -> Result<(T, G), PipelineError>,
    precommit: impl FnOnce(&G) -> Result<(), PipelineError>,
) -> Result<T, PipelineError> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(StorageError::from)?;
    let result = operation().and_then(|(value, guards)| {
        precommit(&guards)?;
        connection
            .execute_batch("COMMIT")
            .map_err(StorageError::from)?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn coordinated_output_error(pipeline: &ResolvedPipeline, error: PipelineError) -> PipelineError {
    let dataset_id = pipeline
        .datasets
        .first()
        .map(|dataset| dataset.dataset_id.as_str())
        .unwrap_or("<unknown>");
    PipelineError::InvalidConfig(format!(
        "coordinated output initialization failed for dataset {dataset_id:?} at database {}: {error}",
        pipeline.database_path.display()
    ))
}

#[cfg(test)]
fn initialize_metadata(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<(), PipelineError> {
    let plan = plan_single_output(pipeline)?;
    initialize_metadata_with_plan(connection, pipeline, &plan)
}

fn initialize_metadata_with_plan(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    plan: &SingleOutputPlan,
) -> Result<(), PipelineError> {
    let layouts = plan
        .trees
        .values()
        .flat_map(|tree| tree.sources.iter().cloned())
        .collect::<Vec<_>>();
    let mut source_ids = BTreeSet::new();
    if let Some(duplicate) = layouts
        .iter()
        .find(|source| !source_ids.insert(source.source_id.clone()))
    {
        return Err(PipelineError::InvalidConfig(format!(
            "nfcapd_tree inputs define duplicate logical source ID {:?}",
            duplicate.source_id
        )));
    }
    with_transaction(connection, || {
        initialize_metadata_in_transaction_with_layouts(
            connection,
            pipeline,
            &layouts,
            &plan.dataset_sources,
        )
    })
}

fn initialize_metadata_in_transaction_with_layouts(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    layouts: &[DatasetSource],
    dataset_sources: &BTreeMap<String, Vec<DatasetSource>>,
) -> Result<(), PipelineError> {
    bind_identity(connection, pipeline)?;
    for dataset in &pipeline.datasets {
        let sources = dataset_sources.get(&dataset.dataset_id).ok_or_else(|| {
            PipelineError::InvalidConfig(format!(
                "single-output plan has no frozen source layout for dataset {:?}",
                dataset.dataset_id
            ))
        })?;
        upsert_dataset_with_sources(connection, dataset, sources)?;
    }
    if !layouts.is_empty() {
        let layout = layouts
            .iter()
            .map(|source| SourceDefinition::new(&source.source_id, source.members.clone()))
            .collect::<Vec<_>>();
        bind_nfcapd_source_layout(connection, &layout)?;
    }
    Ok(())
}

fn initialize_coordinated_metadata_in_transaction(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    layout: &[DatasetSource],
    dataset_layouts: &BTreeMap<String, Vec<DatasetSource>>,
) -> Result<(), PipelineError> {
    bind_identity(connection, pipeline)?;
    for dataset in &pipeline.datasets {
        let sources = dataset_layouts.get(&dataset.dataset_id).ok_or_else(|| {
            PipelineError::InvalidConfig(format!(
                "coordinated plan has no frozen source layout for dataset {:?}",
                dataset.dataset_id
            ))
        })?;
        upsert_dataset_with_sources(connection, dataset, sources)?;
    }
    if !layout.is_empty() {
        let layout = layout
            .iter()
            .map(|source| SourceDefinition::new(&source.source_id, source.members.clone()))
            .collect::<Vec<_>>();
        bind_nfcapd_source_layout(connection, &layout)?;
    }
    Ok(())
}

fn process_atomic(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    operation: impl FnOnce(&mut AggregateBuckets, &mut PipelineReport) -> Result<(), PipelineError>,
) -> Result<PipelineReport, PipelineError> {
    let mut aggregates = AggregateBuckets::default();
    let mut report = PipelineReport::default();
    with_transaction(connection, || {
        operation(&mut aggregates, &mut report)?;
        publish_rollups(connection, aggregates, pipeline, &mut report)?;
        verify_nfdump_revision(pipeline)
    })?;
    Ok(report)
}

fn merge_report(total: &mut PipelineReport, addition: PipelineReport) {
    total.input_scans += addition.input_scans;
    total.skipped_inputs += addition.skipped_inputs;
    total.five_minute_buckets += addition.five_minute_buckets;
    total.rollup_buckets += addition.rollup_buckets;
}

fn populate_coverage_summary(
    connection: &Connection,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let mut statement = connection
        .prepare(
            "SELECT coverage_state, COUNT(*)
             FROM bucket_coverage
             WHERE granularity = '5m'
             GROUP BY coverage_state",
        )
        .map_err(StorageError::from)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(StorageError::from)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    for (state, count) in rows {
        let count = usize::try_from(count)
            .map_err(|_| PipelineError::InvalidConfig("coverage summary count overflow".into()))?;
        match state.as_str() {
            "complete" => report.complete_five_minute_buckets = count,
            "partial" => report.partial_five_minute_buckets = count,
            "unknown" => report.unknown_five_minute_buckets = count,
            _ => {
                return Err(PipelineError::InvalidConfig(format!(
                    "invalid five-minute coverage state in database: {state:?}"
                )));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoverageScope {
    source_ids: Vec<String>,
    start: i64,
    end: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CoverageRange {
    source_id: String,
    start: i64,
    end: i64,
}

fn merged_requested_coverage_ranges(scopes: Vec<CoverageScope>) -> Vec<CoverageRange> {
    let mut ranges = scopes
        .into_iter()
        .flat_map(|scope| {
            scope
                .source_ids
                .into_iter()
                .map(move |source_id| CoverageRange {
                    source_id,
                    start: scope.start,
                    end: scope.end,
                })
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable_by(|left, right| {
        (&left.source_id, left.start, left.end).cmp(&(&right.source_id, right.start, right.end))
    });
    let mut merged: Vec<CoverageRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && previous.source_id == range.source_id
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// A finite native request can be checked independently of incomplete data
/// already stored outside that request. CSV and literal-input configurations
/// have no separately declared time window, so their configured product is
/// the strict scope.
fn discovered_nfcapd_tree_end(
    root_path: &Path,
    source_ids: &[String],
    configured_sources: &[DatasetSource],
    selected_start: i64,
    timezone: &str,
) -> Result<i64, PipelineError> {
    let root = canonical_path(root_path)?;
    let sources = normalize_sources(&root, source_ids, configured_sources)?;
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let discovered = ingest::discover_nfcapd_source_paths(&root, &physical_ids, timezone)?;
    discovered
        .iter()
        .map(|input| input.bucket_start)
        .max()
        .map(|start| aggregate_bounds(start, Granularity::OneDay, timezone))
        .transpose()?
        .map_or(Ok(selected_start), |(_, end)| Ok(end))
}

fn requested_coverage_scopes_with_plan(
    pipeline: &ResolvedPipeline,
    plan: Option<&SingleOutputPlan>,
) -> Result<Option<Vec<CoverageScope>>, PipelineError> {
    let mut scopes = Vec::new();
    for (input_index, input) in pipeline.inputs.iter().enumerate() {
        let InputSpec::NfcapdTree {
            root_path,
            source_ids,
            sources,
            start_date,
            end_date,
            start_time,
            end_time,
            ..
        } = input
        else {
            return Ok(None);
        };
        let selected_start = parse_date_start(start_date, &pipeline.timezone)?;
        let start = match start_time {
            Some(value) => parse_local_datetime(value, &pipeline.timezone)?,
            None => selected_start,
        };
        let frozen = plan.and_then(|plan| plan.trees.get(&input_index));
        let end = match (end_time, end_date) {
            (Some(value), _) => parse_local_datetime(value, &pipeline.timezone)?,
            (None, Some(value)) => next_date_start(value, &pipeline.timezone)?,
            (None, None) => match frozen {
                Some(tree) => discovered_nfcapd_tree_end_with_sources(
                    &tree.root_path,
                    &tree.sources,
                    selected_start,
                    &pipeline.timezone,
                )?,
                None => discovered_nfcapd_tree_end(
                    root_path,
                    source_ids,
                    sources,
                    selected_start,
                    &pipeline.timezone,
                )?,
            },
        };
        let source_ids = match frozen {
            Some(tree) => tree.sources.clone(),
            None => normalize_sources(root_path, source_ids, sources)?,
        }
        .into_iter()
        .map(|source| source.source_id)
        .collect();
        scopes.push(CoverageScope {
            source_ids,
            start,
            end,
        });
    }
    Ok(Some(scopes))
}

fn discovered_nfcapd_tree_end_with_sources(
    root_path: &Path,
    sources: &[DatasetSource],
    selected_start: i64,
    timezone: &str,
) -> Result<i64, PipelineError> {
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let discovered = ingest::discover_nfcapd_source_paths(root_path, &physical_ids, timezone)?;
    discovered
        .iter()
        .map(|input| input.bucket_start)
        .max()
        .map(|start| aggregate_bounds(start, Granularity::OneDay, timezone))
        .transpose()?
        .map_or(Ok(selected_start), |(_, end)| Ok(end))
}

#[cfg(test)]
fn count_incomplete_requested_coverage(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<i64, PipelineError> {
    count_incomplete_requested_coverage_with_plan(
        connection,
        pipeline,
        &SingleOutputPlan::default(),
    )
}

fn count_incomplete_requested_coverage_with_plan(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    plan: &SingleOutputPlan,
) -> Result<i64, PipelineError> {
    let plan = (!plan.trees.is_empty()).then_some(plan);
    let Some(scopes) = requested_coverage_scopes_with_plan(pipeline, plan)? else {
        return connection
            .query_row(
                "SELECT COUNT(*) FROM bucket_coverage
                 WHERE granularity = '5m' AND coverage_state <> 'complete'",
                [],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
            .map_err(PipelineError::from);
    };

    count_incomplete_coverage_ranges(
        connection,
        merged_requested_coverage_ranges(scopes),
        &pipeline.timezone,
    )
}

fn count_incomplete_coverage_for_layout(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<i64, PipelineError> {
    let source_ids = sources
        .iter()
        .map(|source| source.source_id.clone())
        .collect::<Vec<_>>();
    let ranges = merged_requested_coverage_ranges(vec![CoverageScope {
        source_ids,
        start,
        end,
    }]);
    count_incomplete_coverage_ranges(connection, ranges, timezone)
}

fn count_incomplete_coverage_ranges(
    connection: &Connection,
    ranges: Vec<CoverageRange>,
    timezone: &str,
) -> Result<i64, PipelineError> {
    let mut incomplete = 0_i64;
    for range in ranges {
        let complete = connection
            .prepare(
                "SELECT bucket_start
                 FROM bucket_coverage
                 WHERE source_id = ?1
                   AND granularity = '5m'
                   AND bucket_start >= ?2
                   AND bucket_start < ?3
                   AND coverage_state = 'complete'
                 ORDER BY bucket_start",
            )
            .map_err(StorageError::from)?
            .query_map(params![&range.source_id, range.start, range.end], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(StorageError::from)?
            .collect::<rusqlite::Result<BTreeSet<_>>>()
            .map_err(StorageError::from)?;
        let mut bucket_start = range.start;
        while bucket_start < range.end {
            if !complete.contains(&bucket_start) {
                incomplete = incomplete.checked_add(1).ok_or_else(|| {
                    PipelineError::InvalidConfig(
                        "requested coverage count exceeds SQLite INTEGER range".into(),
                    )
                })?;
            }
            bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
        }
    }
    Ok(incomplete)
}

fn bind_identity(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<(), PipelineError> {
    verify_nfdump_revision(pipeline)?;
    let maad_config = serde_json::to_value(crate::maad::MaadConfig::default())?;
    let schema = json!({
        "version": 3,
        "tables": [
            {"name":"traffic_stats","version":2},
            {"name":"protocol_stats","version":1},
            {"name":"address_count_stats","version":1},
            {"name":"port_count_stats","version":1},
            {"name":"address_structure_stats","version":1},
            {"name":"bucket_coverage","version":1}
        ]
    });
    let nfdump_executable = pipeline.nfdump_revision.as_ref().map(|revision| {
        json!({
            "locator": revision.locator,
            "content_fingerprint": revision.content_fingerprint,
        })
    });
    let result_config = json!({
        "version": 4,
        "timezone": pipeline.timezone,
        "nfcapd_decoder": {
            "protocol_version": nfdump::CONTRACT_VERSION,
            "input_contract": nfdump::INPUT_CONTRACT,
            "output_contract": nfdump::OUTPUT_CONTRACT,
            "contract_id": nfcapd_decoder_fingerprint()?,
            "decoder_fingerprint": pipeline.nfdump_revision.as_ref().map(|revision| revision.decoder_fingerprint.clone()),
            "executable": nfdump_executable,
        },
        "maad": {
            "enabled": pipeline.run_maad,
            "backend": "in-process",
            "contract_version": 2,
            "config": maad_config
        }
    });
    let identity = ProductIdentity::create(
        &schema,
        &pipeline.selection.normalized_payload(),
        &result_config,
    )?;
    bind_product_identity(connection, &identity, &crate::storage::STATS_TABLE_NAMES)?;
    Ok(())
}

fn upsert_dataset_with_sources(
    connection: &Connection,
    dataset: &Dataset,
    logical_sources: &[DatasetSource],
) -> Result<(), PipelineError> {
    let sources = logical_sources
        .iter()
        .map(|source| SourceDefinition::new(&source.source_id, source.members.clone()))
        .collect::<Vec<_>>();
    let mut metadata = DatasetMetadata::new(&dataset.dataset_id);
    metadata.label = dataset.label.clone();
    metadata.default_start_date = dataset.default_start_date.clone();
    metadata.source_mode = dataset.source_mode.clone();
    metadata.discovery_mode = dataset.discovery_mode.clone();
    metadata.sort_order = dataset.sort_order;
    metadata.sources = sources;
    upsert_dataset_metadata(connection, &metadata)?;
    Ok(())
}

struct PreparedCsvInput {
    path: PathBuf,
    mapping: CsvSourceConfig,
    revision: InputRevision,
    snapshot: FileSnapshot,
}

fn prepare_file_revision(
    connection: &Connection,
    path: &Path,
    input_kind: InputKind,
    decoder_fingerprint: String,
) -> Result<(InputRevision, FileSnapshot), PipelineError> {
    prepare_file_revision_with(connection, path, input_kind, decoder_fingerprint, || {
        capture_file_revision(path)
    })
}

fn prepare_file_revision_with(
    connection: &Connection,
    path: &Path,
    input_kind: InputKind,
    decoder_fingerprint: String,
    hash_file: impl FnOnce() -> Result<(String, FileSnapshot), ProvenanceError>,
) -> Result<(InputRevision, FileSnapshot), PipelineError> {
    let locator = path.to_string_lossy().into_owned();
    let observed = FileSnapshot::capture(path)?;
    let (content_fingerprint, snapshot) =
        match cached_content_fingerprint(connection, input_kind, &locator, &observed)? {
            Some(content_fingerprint) => (content_fingerprint, observed),
            None => hash_file()?,
        };
    let revision = InputRevision::create(
        input_kind.as_str(),
        locator,
        content_fingerprint,
        decoder_fingerprint,
    )?;
    Ok((revision, snapshot))
}

fn process_csv_inputs(
    connection: &Connection,
    inputs: &[ingest::CsvInputSpec],
    pipeline: &ResolvedPipeline,
) -> Result<PipelineReport, PipelineError> {
    let mut prepared = Vec::new();
    let mut skipped_inputs = 0_usize;
    let mut needs_rescan = false;
    for input in inputs {
        let mapping = CsvSourceConfig::load(&input.mapping_path)?;
        let (revision, snapshot) = prepare_file_revision(
            connection,
            &input.path,
            InputKind::Csv,
            csv_decoder_fingerprint(&mapping)?,
        )?;
        if input_scan_fully_processed(connection, InputKind::Csv, &revision.locator, &revision)? {
            skipped_inputs += 1;
        } else {
            needs_rescan = true;
        }
        prepared.push(PreparedCsvInput {
            path: input.path.clone(),
            mapping,
            revision,
            snapshot,
        });
    }
    if !needs_rescan {
        return Ok(PipelineReport {
            skipped_inputs,
            ..PipelineReport::default()
        });
    }
    prepared.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    let mut aggregates = AggregateBuckets::default();
    let mut report = PipelineReport::default();
    with_transaction(connection, || {
        connection
            .execute_batch(
                "CREATE TEMP TABLE csv_bucket_stage (
                    source_id TEXT NOT NULL,
                    bucket_start INTEGER NOT NULL,
                    input_locator TEXT NOT NULL,
                    revision_fingerprint TEXT,
                    payload BLOB NOT NULL
                );
                CREATE INDEX csv_bucket_stage_order
                ON csv_bucket_stage(source_id, bucket_start);",
            )
            .map_err(StorageError::from)?;
        for input in &prepared {
            process_csv(
                connection,
                &input.path,
                &input.mapping,
                &input.revision,
                &input.snapshot,
                pipeline,
                &mut report,
            )?;
        }
        publish_csv_stage(connection, pipeline, &mut aggregates, &mut report)?;
        publish_rollups(connection, aggregates, pipeline, &mut report)
    })?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn process_csv(
    connection: &Connection,
    path: &Path,
    mapping: &CsvSourceConfig,
    revision: &InputRevision,
    snapshot: &FileSnapshot,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    connection
        .execute(
            "DELETE FROM processed_inputs
             WHERE input_kind = 'csv' AND scan_locator = ?1",
            params![revision.locator],
        )
        .map_err(StorageError::from)?;
    let completion = match ingest::scan_csv(path, mapping, &pipeline.selection, |event| {
        let bucket_revision = revision_for_locator(revision, &event.input_locator)?;
        let owner = InputBucket {
            input_kind: InputKind::Csv,
            input_locator: event.input_locator.clone(),
            scan_locator: event.scan_locator,
            source_id: event.bucket.key.source_id.clone(),
            bucket_start: event.bucket.key.bucket_start,
            bucket_end: event.bucket.key.bucket_end,
            revision: bucket_revision.clone(),
            file_snapshot: Some(snapshot.clone()),
        };
        upsert_input_bucket(connection, &owner, false)?;
        mark_input_bucket_status(
            connection,
            InputKind::Csv,
            &event.input_locator,
            &event.bucket.key.source_id,
            event.bucket.key.bucket_start,
            InputStatus::Processed,
            &bucket_revision,
            None,
        )?;
        let payload = serde_json::to_vec(&event.bucket)?;
        connection
            .execute(
                "INSERT INTO csv_bucket_stage (
                    source_id, bucket_start, input_locator,
                    revision_fingerprint, payload
                ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.bucket.key.source_id,
                    event.bucket.key.bucket_start,
                    event.input_locator,
                    bucket_revision.fingerprint,
                    payload,
                ],
            )
            .map_err(StorageError::from)?;
        Ok::<_, PipelineError>(())
    }) {
        Ok(completion) => completion,
        Err(ProducerError::Input(error)) => return Err(error.into()),
        Err(ProducerError::Sink(error)) => return Err(error),
    };
    verify_file_snapshot(path, snapshot)?;
    complete_input_scan(
        connection,
        InputKind::Csv,
        &completion.scan_locator,
        i64::try_from(completion.rejected_rows)
            .map_err(|_| PipelineError::InvalidConfig("rejected row count overflow".into()))?,
        i64::try_from(completion.skipped_bad_column_count).map_err(|_| {
            PipelineError::InvalidConfig("skipped bad-column count overflow".into())
        })?,
        revision,
        Some(snapshot),
    )?;
    verify_file_snapshot(path, snapshot)?;
    report.input_scans += 1;
    Ok(())
}

struct CsvStageMember {
    bucket: CanonicalBucket,
    input_locator: String,
    revision_fingerprint: Option<String>,
}

/// Merge all staged CSV buckets in source/time order. The stage is indexed on
/// disk, so only one overlapping bucket group is held in memory at a time.
fn publish_csv_stage(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let mut statement = connection
        .prepare(
            "SELECT source_id, bucket_start, input_locator,
                    revision_fingerprint, payload
             FROM csv_bucket_stage
             ORDER BY source_id, bucket_start, input_locator",
        )
        .map_err(StorageError::from)?;
    let mut rows = statement.query([]).map_err(StorageError::from)?;
    let mut group: Option<(String, i64, Vec<CsvStageMember>)> = None;
    let mut current_source = None;
    let mut next_expected = None;
    loop {
        let Some((source_id, bucket_start, input_locator, revision_fingerprint, payload)) = rows
            .next()
            .map_err(StorageError::from)?
            .map(|row| {
                Ok::<_, rusqlite::Error>((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .transpose()
            .map_err(StorageError::from)?
        else {
            break;
        };
        let member = CsvStageMember {
            bucket: serde_json::from_slice(&payload)?,
            input_locator,
            revision_fingerprint,
        };
        match group.as_mut() {
            Some((group_source, group_start, members))
                if group_source == &source_id && *group_start == bucket_start =>
            {
                members.push(member);
            }
            _ => {
                if let Some((group_source, group_start, members)) = group.take() {
                    publish_csv_stage_group(
                        connection,
                        pipeline,
                        aggregates,
                        report,
                        &group_source,
                        group_start,
                        &members,
                        &mut current_source,
                        &mut next_expected,
                    )?;
                }
                group = Some((source_id, bucket_start, vec![member]));
            }
        }
    }
    drop(rows);
    drop(statement);
    if let Some((group_source, group_start, members)) = group {
        publish_csv_stage_group(
            connection,
            pipeline,
            aggregates,
            report,
            &group_source,
            group_start,
            &members,
            &mut current_source,
            &mut next_expected,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_csv_stage_group(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
    source_id: &str,
    bucket_start: i64,
    members: &[CsvStageMember],
    current_source: &mut Option<String>,
    next_expected: &mut Option<i64>,
) -> Result<(), PipelineError> {
    if current_source.as_deref() != Some(source_id) {
        *current_source = Some(source_id.to_owned());
        *next_expected = None;
    } else {
        let mut expected = next_expected.ok_or_else(|| {
            PipelineError::InvalidConfig("CSV stage lost its source envelope".into())
        })?;
        while expected < bucket_start {
            let (bucket, evidence) = merged_csv_bucket(source_id, expected, &[])?;
            publish_csv_bucket(connection, &bucket, &evidence, pipeline, aggregates, report)?;
            expected = expected.checked_add(FIVE_MINUTES).ok_or_else(|| {
                PipelineError::InvalidConfig("CSV source envelope exceeds time range".into())
            })?;
        }
    }
    let (bucket, evidence) = merged_csv_bucket(source_id, bucket_start, members)?;
    publish_csv_bucket(connection, &bucket, &evidence, pipeline, aggregates, report)?;
    *next_expected = Some(bucket_start.checked_add(FIVE_MINUTES).ok_or_else(|| {
        PipelineError::InvalidConfig("CSV source envelope exceeds time range".into())
    })?);
    Ok(())
}

fn merged_csv_bucket(
    source_id: &str,
    bucket_start: i64,
    members: &[CsvStageMember],
) -> Result<(CanonicalBucket, InputEvidenceRow), PipelineError> {
    let key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    );
    let any_observed = members
        .iter()
        .any(|member| member.bucket.coverage.observed_units() != 0);
    let any_rejected = members
        .iter()
        .any(|member| member.bucket.coverage.rejected_units() != 0);
    let mut builder = if any_observed {
        StatisticalBucket::dense(key)
    } else {
        StatisticalBucket::new(key)
    }
    .with_coverage(BucketCoverage::empty());
    for member in members {
        builder.include(&member.bucket)?;
    }
    let coverage = BucketCoverage::new(1, u64::from(any_observed), u64::from(any_rejected))
        .map_err(DomainError::from)?;
    let bucket = builder.with_coverage(coverage).finish_owned();
    let evidence_state = if any_rejected {
        InputEvidenceState::Rejected
    } else if any_observed {
        InputEvidenceState::Observed
    } else {
        InputEvidenceState::Missing
    };
    let (input_locator, revision_fingerprint) = match members {
        [member] => (
            member.input_locator.clone(),
            member.revision_fingerprint.clone(),
        ),
        [] => (format!("csv://{source_id}"), None),
        _ => (format!("csv://{source_id}"), None),
    };
    let evidence = InputEvidenceRow::new(
        source_id,
        source_id,
        bucket_start,
        bucket_start + FIVE_MINUTES,
        input_locator,
        evidence_state,
        revision_fingerprint,
    );
    Ok((bucket, evidence))
}

fn publish_csv_bucket(
    connection: &Connection,
    bucket: &CanonicalBucket,
    evidence: &InputEvidenceRow,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    reject_cross_kind_overlap(connection, bucket, InputKind::Csv)?;
    aggregates.reject_persisted_csv_siblings(connection, bucket, &pipeline.timezone)?;
    let (day_start, day_end) = aggregate_bounds(
        bucket.key.bucket_start,
        Granularity::OneDay,
        &pipeline.timezone,
    )?;
    ensure_daily_product_completion_bucket_guard(
        connection,
        &bucket.key.source_id,
        bucket.key.bucket_start,
        day_start,
        day_end,
    )?;
    write_buckets(connection, std::slice::from_ref(bucket), pipeline.run_maad)?;
    replace_input_evidence(
        connection,
        &bucket.key.source_id,
        bucket.key.bucket_start,
        std::slice::from_ref(evidence),
    )?;
    aggregates.include(bucket, &pipeline.timezone)?;
    report.rollup_buckets += aggregates.flush_complete(connection, pipeline.run_maad)?;
    report.five_minute_buckets += 1;
    Ok(())
}

fn reject_cross_kind_overlap(
    connection: &Connection,
    bucket: &CanonicalBucket,
    input_kind: InputKind,
) -> Result<(), PipelineError> {
    let conflict = connection
        .query_row(
            "SELECT input_kind, input_locator FROM processed_inputs
             WHERE source_id = ?1 AND bucket_start = ?2 AND input_kind <> ?3
             ORDER BY input_kind, input_locator LIMIT 1",
            params![
                bucket.key.source_id,
                bucket.key.bucket_start,
                input_kind.as_str(),
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(StorageError::from)?;
    if let Some((kind, locator)) = conflict {
        return Err(PipelineError::InvalidConfig(format!(
            "overlapping canonical five-minute input for source {:?} at {} conflicts with {kind}:{locator}",
            bucket.key.source_id, bucket.key.bucket_start
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_nfcapd_tree(
    connection: &Connection,
    tree: &FrozenNfcapdTreeLayout,
    start_date: &str,
    end_date: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    force: bool,
    pipeline: &ResolvedPipeline,
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    if pipeline.selection.selects_daily_active_sources()
        && (start_time.is_some() || end_time.is_some())
    {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources selection requires whole local calendar days; start_time and end_time are unsupported".into(),
        ));
    }
    let root = &tree.root_path;
    let sources = &tree.sources;
    let physical_ids = &tree.physical_ids;
    if pipeline.selection.selects_daily_active_sources() {
        validate_daily_active_source_layout(sources, physical_ids)?;
    }
    let discovery_started = Instant::now();
    let discovered = ingest::discover_nfcapd_source_paths(root, physical_ids, &pipeline.timezone)?;
    tracing::info!(
        target: "netflow_db::profile",
        phase = "discovery",
        elapsed_seconds = discovery_started.elapsed().as_secs_f64(),
        physical_sources = physical_ids.len(),
        discovered_inputs = discovered.len(),
    );
    let mut by_member_and_start = BTreeMap::new();
    let mut member_bounds = BTreeMap::new();
    for input in discovered {
        member_bounds
            .entry(input.source_id.clone())
            .and_modify(|(first, last): &mut (i64, i64)| {
                *first = (*first).min(input.bucket_start);
                *last = (*last).max(input.bucket_start);
            })
            .or_insert((input.bucket_start, input.bucket_start));
        by_member_and_start.insert((input.source_id, input.bucket_start), input.path);
    }
    let window = resolve_nfcapd_tree_window(
        start_date,
        end_date,
        start_time,
        end_time,
        by_member_and_start
            .keys()
            .map(|(_, bucket_start)| *bucket_start),
        &pipeline.timezone,
    )?;

    let mut day_start = window.start;
    while day_start < window.end {
        verify_member_directory_identities(root, &tree.member_identities)?;
        let day_end = aggregate_bounds(day_start, Granularity::OneDay, &pipeline.timezone)?.1;
        let capture_complete = day_capture_is_complete(
            sources,
            &by_member_and_start,
            day_start,
            day_end,
            &pipeline.timezone,
        )?;
        let published_day = if pipeline.selection.selects_daily_active_sources() {
            day_was_published(connection, sources, day_start, day_end)?
        } else {
            day_has_complete_coverage(connection, sources, day_start, day_end, &pipeline.timezone)?
        };
        let stale_published_day = !capture_complete && published_day;
        let force_replaces_day = force && pipeline.selection.selects_daily_active_sources();
        if stale_published_day && !force {
            return Err(PipelineError::InvalidConfig(format!(
                "published local day {day_start}..{day_end} no longer has complete nfcapd capture coverage; rerun that day with --force"
            )));
        }
        let mut marker_needs_backfill = false;
        let canonical_day_verified = if pipeline.selection.selects_daily_active_sources()
            && capture_complete
            && published_day
            && !force
        {
            match nfcapd_day_completion_state(
                connection,
                sources,
                day_start,
                day_end,
                pipeline.run_maad,
            )? {
                DailyProductCompletionState::Clean => true,
                DailyProductCompletionState::Dirty => {
                    return Err(PipelineError::InvalidConfig(format!(
                        "published local day {day_start}..{day_end} was mutated after completion; rerun that whole day with --force"
                    )));
                }
                DailyProductCompletionState::Missing => {
                    if !nfcapd_day_has_canonical_topology(
                        connection,
                        sources,
                        day_start,
                        day_end,
                        &pipeline.timezone,
                        pipeline.run_maad,
                    )? {
                        return Err(PipelineError::InvalidConfig(format!(
                            "published local day {day_start}..{day_end} has damaged canonical topology; rerun that whole day with --force"
                        )));
                    }
                    marker_needs_backfill = true;
                    true
                }
            }
        } else {
            false
        };
        let missing = if pipeline.selection.selects_daily_active_sources() || stale_published_day {
            missing_physical_day_inputs(
                physical_ids,
                &by_member_and_start,
                day_start,
                day_end,
                &pipeline.timezone,
            )?
        } else {
            Vec::new()
        };
        let missing_absences =
            build_missing_day_absences(root, &missing, day_start, day_end, &pipeline.timezone)?;
        invoke_missing_day_absence_hook(root, &missing, &pipeline.timezone);
        verify_member_directory_identities(root, &tree.member_identities)?;
        if pipeline.selection.selects_daily_active_sources() && !missing.is_empty() {
            let missing_details = missing_day_warning_details(root, &missing, &pipeline.timezone)?;
            tracing::warn!(
                day_start,
                day_end,
                missing_inputs = missing.len(),
                missing_details = %missing_details,
                "skipping incomplete physical day for daily_active_sources selection"
            );
            report.skipped_inputs += missing.len();
            if stale_published_day || force_replaces_day {
                let source_ids = sources
                    .iter()
                    .map(|source| source.source_id.clone())
                    .collect::<Vec<_>>();
                let guards = NfcapdDayGuards {
                    nfdump_revision: pipeline.nfdump_revision.clone(),
                    ..NfcapdDayGuards::default()
                };
                with_transaction_precommit_value(
                    connection,
                    || {
                        verify_missing_day_absences(&missing_absences, day_start, day_end)?;
                        delete_stats_time_range(connection, &source_ids, day_start, day_end)?;
                        Ok(((), guards))
                    },
                    |guards| {
                        verify_single_day_guards(
                            guards,
                            root,
                            &tree.member_identities,
                            &missing_absences,
                            day_start,
                            day_end,
                        )
                    },
                )?;
            }
            day_start = day_end;
            continue;
        }
        let mut owned_keys = BTreeSet::new();
        let mut bucket_start = day_start;
        while bucket_start < day_end {
            for source in sources {
                if force
                    && source_has_candidate(
                        source,
                        bucket_start,
                        &by_member_and_start,
                        &member_bounds,
                        end_date.is_some(),
                    )
                {
                    owned_keys.insert((source.source_id.clone(), bucket_start));
                }
            }
            bucket_start = next_local_five_minute_start(bucket_start, &pipeline.timezone)?;
        }
        let transaction_started = Instant::now();
        let (day_report, day_profile) = with_transaction_precommit_value(
            connection,
            || {
                if stale_published_day || force_replaces_day {
                    verify_missing_day_absences(&missing_absences, day_start, day_end)?;
                    let source_ids = sources
                        .iter()
                        .map(|source| source.source_id.clone())
                        .collect::<Vec<_>>();
                    delete_stats_time_range(connection, &source_ids, day_start, day_end)?;
                }
                let source_ids = sources
                    .iter()
                    .map(|source| source.source_id.clone())
                    .collect::<Vec<_>>();
                provision_daily_product_completion_bucket_guards(
                    connection,
                    &source_ids,
                    day_start,
                    day_end,
                )?;
                let mut aggregates = AggregateBuckets::with_owned_keys(owned_keys);
                let mut day_report = PipelineReport::default();
                let mut day_result = process_nfcapd_tree_day(
                    connection,
                    root,
                    sources,
                    &by_member_and_start,
                    &member_bounds,
                    day_start,
                    day_end,
                    end_date.is_some(),
                    force,
                    pipeline,
                    capture_snapshots,
                    &mut aggregates,
                    &mut day_report,
                    canonical_day_verified,
                )?;
                day_result.profile.final_rollups =
                    publish_rollups_profiled(connection, aggregates, pipeline, &mut day_report)?;
                if !pipeline.selection.selects_daily_active_sources() {
                    verify_nfdump_revision(pipeline)?;
                }
                if pipeline.selection.selects_daily_active_sources()
                    && capture_complete
                    && missing.is_empty()
                    && (!published_day || force_replaces_day || marker_needs_backfill)
                {
                    mark_nfcapd_day_complete(
                        connection,
                        sources,
                        day_start,
                        day_end,
                        pipeline.run_maad,
                    )?;
                }
                Ok(((day_report, day_result.profile), day_result.guards))
            },
            |guards| {
                if pipeline.selection.selects_daily_active_sources() {
                    verify_single_day_guards(
                        guards,
                        root,
                        &tree.member_identities,
                        &missing_absences,
                        day_start,
                        day_end,
                    )
                } else {
                    invoke_single_commit_guard_hook();
                    verify_member_directory_identities(root, &tree.member_identities)?;
                    verify_missing_day_absences(&missing_absences, day_start, day_end)
                }
            },
        )?;
        day_profile.log(day_start, day_end, transaction_started.elapsed());
        merge_report(report, day_report);
        day_start = day_end;
    }
    Ok(())
}

fn missing_physical_day_inputs(
    physical_ids: &[String],
    paths: &BTreeMap<(String, i64), PathBuf>,
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<Vec<(String, i64)>, PipelineError> {
    let mut missing = Vec::new();
    let mut bucket_start = start;
    while bucket_start < end {
        for member in physical_ids {
            if !paths.contains_key(&(member.clone(), bucket_start)) {
                missing.push((member.clone(), bucket_start));
            }
        }
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    Ok(missing)
}

fn missing_day_absence_error(
    start: i64,
    end: i64,
    path: &Path,
    error: impl std::fmt::Display,
) -> PipelineError {
    PipelineError::InvalidConfig(format!(
        "nfcapd capture appeared while protecting missing inputs for local day {start}..{end} at {}; refusing to delete the existing product: {error}",
        path.display()
    ))
}

fn build_missing_day_absences(
    root: &Path,
    missing: &[(String, i64)],
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<Vec<ExpectedAbsence>, PipelineError> {
    missing
        .iter()
        .map(|(member, bucket_start)| {
            let path = expected_nfcapd_path(root, member, *bucket_start, timezone)?;
            ExpectedAbsence::capture(&path)
                .map_err(|error| missing_day_absence_error(start, end, &path, error))
        })
        .collect()
}

fn verify_missing_day_absences(
    absences: &[ExpectedAbsence],
    start: i64,
    end: i64,
) -> Result<(), PipelineError> {
    for absence in absences {
        absence
            .verify()
            .map_err(|error| missing_day_absence_error(start, end, absence.path(), error))?;
    }
    Ok(())
}

fn verify_single_day_guards(
    guards: &NfcapdDayGuards,
    root: &Path,
    member_identities: &BTreeMap<String, MemberDirectoryIdentity>,
    missing_absences: &[ExpectedAbsence],
    start: i64,
    end: i64,
) -> Result<(), PipelineError> {
    invoke_single_commit_guard_hook();
    verify_member_directory_identities(root, member_identities)?;
    for (path, revision) in &guards.capture_revisions {
        if let Some(snapshot) = &revision.snapshot {
            verify_file_snapshot(path, snapshot)?;
        }
    }
    for (path, snapshot) in &guards.activity_snapshots {
        if !guards.capture_revisions.contains_key(path) {
            verify_file_snapshot(path, snapshot)?;
        }
    }
    if let Some(revision) = &guards.nfdump_revision {
        verify_nfdump_revision_snapshot(revision)?;
    }
    verify_missing_day_absences(missing_absences, start, end)
}

fn missing_day_warning_details(
    root: &Path,
    missing: &[(String, i64)],
    timezone: &str,
) -> Result<String, PipelineError> {
    let mut details = missing
        .iter()
        .take(MAX_MISSING_DAY_WARNING_DETAILS)
        .map(|(member, bucket_start)| {
            expected_nfcapd_path(root, member, *bucket_start, timezone).map(|expected_path| {
                format!(
                    "member={member} timestamp={bucket_start} expected_path={}",
                    expected_path.display()
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let omitted = missing.len().saturating_sub(details.len());
    if omitted != 0 {
        details.push(format!("… {omitted} more missing inputs"));
    }
    Ok(details.join("; "))
}

/// Whether every member of every logical source has a discovered capture in this local day.
fn day_capture_is_complete(
    sources: &[DatasetSource],
    paths: &BTreeMap<(String, i64), PathBuf>,
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<bool, PipelineError> {
    let mut bucket_start = start;
    while bucket_start < end {
        if sources.iter().any(|source| {
            source
                .members
                .iter()
                .any(|member| !paths.contains_key(&(member.clone(), bucket_start)))
        }) {
            return Ok(false);
        }
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    Ok(true)
}

/// Non-daily selections may repair a partially published physical day as new members arrive.
/// Only treat that day as stale when its complete coverage envelope had already been committed;
/// daily-active selections use [`day_was_published`] below because their day cohort is atomic.
fn day_has_complete_coverage(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<bool, PipelineError> {
    let mut expected_bucket_count = 0_i64;
    let mut bucket_start = start;
    while bucket_start < end {
        expected_bucket_count = expected_bucket_count.checked_add(1).ok_or_else(|| {
            PipelineError::InvalidConfig("local day contains too many five-minute buckets".into())
        })?;
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    if sources.is_empty() {
        return Ok(false);
    }
    for source in sources {
        let complete = connection
            .query_row(
                "SELECT COUNT(*) FROM bucket_coverage
                 WHERE source_id = ?1 AND granularity = '5m'
                   AND bucket_start >= ?2 AND bucket_start < ?3
                   AND coverage_state = 'complete'",
                params![source.source_id, start, end],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::from)?;
        if complete != expected_bucket_count {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Any committed product, evidence, or processed-input provenance makes a day a prior
/// publication. Coverage is one part of that product, not the publication marker: if a coverage
/// row is damaged or missing while a capture also disappears, force must still remove the stale
/// day instead of treating it as a first run.
fn day_was_published(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
) -> Result<bool, PipelineError> {
    for source in sources {
        let completion = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM daily_product_completion
                     WHERE source_id = ?1 AND day_start < ?3 AND day_end > ?2
                 ) OR EXISTS(
                     SELECT 1 FROM daily_product_completion_dirty
                     WHERE source_id = ?1 AND day_start < ?3 AND day_end > ?2
                 )",
                params![source.source_id, start, end],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::from)?;
        if completion != 0 {
            return Ok(true);
        }
        for table in STATS_TABLE_NAMES {
            let published = connection
                .query_row(
                    &format!(
                        "SELECT EXISTS(
                             SELECT 1 FROM {table}
                             WHERE source_id = ?1 AND {CANONICAL_GRANULARITY_PREDICATE}
                               AND bucket_start >= ?2 AND bucket_start < ?3
                         )"
                    ),
                    params![source.source_id, start, end],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(StorageError::from)?;
            if published != 0 {
                return Ok(true);
            }
        }
        let evidence = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM input_evidence
                     WHERE source_id = ?1 AND bucket_start >= ?2 AND bucket_start < ?3
                 )",
                params![source.source_id, start, end],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::from)?;
        if evidence != 0 {
            return Ok(true);
        }
        let provenance = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM processed_inputs
                     WHERE input_kind = 'nfcapd' AND status = 'processed'
                       AND source_id = ?1 AND bucket_start >= ?2 AND bucket_start < ?3
                 )",
                params![source.source_id, start, end],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StorageError::from)?;
        if provenance != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

const CANONICAL_GRANULARITY_PREDICATE: &str = "granularity IN ('5m', '30m', '1h', '1d')";

const CANONICAL_SCOPE_PREDICATE: &str = "(
    (src_visibility = 'all' AND dst_visibility = 'all') OR
    (src_visibility = 'anonymized' AND dst_visibility = 'anonymized') OR
    (src_visibility = 'anonymized' AND dst_visibility = 'literal') OR
    (src_visibility = 'literal' AND dst_visibility = 'anonymized') OR
    (src_visibility = 'literal' AND dst_visibility = 'literal')
)";

fn canonical_row_family_predicate(table: &str) -> &'static str {
    match table {
        "traffic_stats" | "protocol_stats" => CANONICAL_SCOPE_PREDICATE,
        "address_count_stats" => {
            "(
            address_side IN ('source', 'destination') AND
            ((src_visibility = 'all' AND dst_visibility = 'all') OR
             (src_visibility = 'anonymized' AND dst_visibility = 'anonymized') OR
             (src_visibility = 'anonymized' AND dst_visibility = 'literal') OR
             (src_visibility = 'literal' AND dst_visibility = 'anonymized') OR
             (src_visibility = 'literal' AND dst_visibility = 'literal'))
        )"
        }
        "port_count_stats" => {
            "(
                port_side IN ('source', 'destination') AND
                port_range IN ('low', 'high') AND
                ((src_visibility = 'all' AND dst_visibility = 'all') OR
                 (src_visibility = 'anonymized' AND dst_visibility = 'anonymized') OR
                 (src_visibility = 'anonymized' AND dst_visibility = 'literal') OR
                 (src_visibility = 'literal' AND dst_visibility = 'anonymized') OR
                 (src_visibility = 'literal' AND dst_visibility = 'literal'))
            )"
        }
        "address_structure_stats" => {
            "(
            ip_version = 4 AND
            address_side IN ('source', 'destination') AND
            structure_kind IN ('structure', 'spectrum', 'dimension') AND
            ((src_visibility = 'all' AND dst_visibility = 'all') OR
             (src_visibility = 'anonymized' AND dst_visibility = 'anonymized') OR
             (src_visibility = 'anonymized' AND dst_visibility = 'literal') OR
             (src_visibility = 'literal' AND dst_visibility = 'anonymized') OR
             (src_visibility = 'literal' AND dst_visibility = 'literal'))
        )"
        }
        _ => unreachable!("unknown canonical product table {table}"),
    }
}

fn canonical_row_family_count(table: &str, dense: bool, run_maad: bool) -> i64 {
    if !dense || (table == "address_structure_stats" && !run_maad) {
        return 0;
    }
    let scopes = nfcapd_dense_traffic_scope_count();
    match table {
        "traffic_stats" | "protocol_stats" => scopes,
        "address_count_stats" => scopes * 2,
        "port_count_stats" => scopes * 4,
        // MAAD is emitted only for IPv4 address sets. Dense traffic has one IPv4 and one IPv6
        // row for each visibility scope, while each IPv4 side gets three MAAD structures.
        "address_structure_stats" => scopes * 3,
        _ => unreachable!("unknown canonical product table {table}"),
    }
}

fn canonical_coverage_state(
    observed_units: i64,
    expected_units: i64,
    rejected_units: i64,
) -> Option<&'static str> {
    if expected_units <= 0
        || observed_units < 0
        || rejected_units < 0
        || observed_units > expected_units
        || rejected_units > expected_units
    {
        return None;
    }
    Some(if observed_units == expected_units && rejected_units == 0 {
        "complete"
    } else if observed_units == 0 && rejected_units == 0 {
        "unknown"
    } else {
        "partial"
    })
}

fn canonical_bucket_coverage_matches(
    connection: &Connection,
    source_id: &str,
    bucket_start: i64,
    expected_end: i64,
    observed_units: usize,
    expected_units: usize,
) -> Result<bool, PipelineError> {
    let expected_units = i64::try_from(expected_units)
        .map_err(|_| PipelineError::InvalidConfig("nfcapd coverage unit count overflow".into()))?;
    let observed_units = i64::try_from(observed_units)
        .map_err(|_| PipelineError::InvalidConfig("nfcapd observed unit count overflow".into()))?;
    let Some(expected_state) = canonical_coverage_state(observed_units, expected_units, 0) else {
        return Ok(false);
    };
    let row = connection
        .query_row(
            "SELECT bucket_end, coverage_state, observed_units, expected_units, rejected_units
             FROM bucket_coverage
             WHERE source_id = ?1 AND granularity = '5m' AND bucket_start = ?2",
            params![source_id, bucket_start],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::from)?;
    Ok(
        row.is_some_and(|(bucket_end, state, observed, expected, rejected)| {
            bucket_end == expected_end
                && state == expected_state
                && observed == observed_units
                && expected == expected_units
                && rejected == 0
        }),
    )
}

fn canonical_bucket_rows_match(
    connection: &Connection,
    source_id: &str,
    granularity: Granularity,
    bucket_start: i64,
    bucket_end: i64,
    dense: bool,
    run_maad: bool,
) -> Result<bool, PipelineError> {
    for table in [
        "traffic_stats",
        "protocol_stats",
        "address_count_stats",
        "port_count_stats",
        "address_structure_stats",
    ] {
        let expected = canonical_row_family_count(table, dense, run_maad);
        let query = format!(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN bucket_end = ?4 AND ip_version IN (4, 6) AND ({predicate}) THEN 1 ELSE 0 END), 0)
             FROM {table}
             WHERE source_id = ?1 AND granularity = ?2 AND bucket_start = ?3",
            predicate = canonical_row_family_predicate(table),
        );
        let (total, canonical) = connection
            .query_row(
                &query,
                params![source_id, granularity.as_str(), bucket_start, bucket_end],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(StorageError::from)?;
        if total != expected || canonical != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A matching evidence/provenance pair is resumable only when the committed canonical bucket
/// topology is still present. Observed logical buckets are dense; all-missing buckets are valid
/// sparse coverage-only rows.
fn nfcapd_logical_bucket_has_canonical_topology(
    connection: &Connection,
    source_id: &str,
    bucket_start: i64,
    observed_units: usize,
    expected_units: usize,
    run_maad: bool,
) -> Result<bool, PipelineError> {
    #[cfg(test)]
    NFCAPD_LOGICAL_BUCKET_TOPOLOGY_CALLS.with(|calls| calls.set(calls.get() + 1));
    let bucket_end = bucket_start
        .checked_add(FIVE_MINUTES)
        .ok_or_else(|| PipelineError::InvalidConfig("nfcapd bucket end overflow".into()))?;
    if !canonical_bucket_coverage_matches(
        connection,
        source_id,
        bucket_start,
        bucket_end,
        observed_units,
        expected_units,
    )? {
        return Ok(false);
    }
    canonical_bucket_rows_match(
        connection,
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_end,
        observed_units != 0,
        run_maad,
    )
}

#[derive(Clone, Copy, Debug)]
struct ExpectedNfcapdTopologyBucket {
    bucket_end: i64,
    child_count: i64,
}

type ExpectedNfcapdTopology = BTreeMap<Granularity, BTreeMap<i64, ExpectedNfcapdTopologyBucket>>;

fn expected_nfcapd_day_topology(
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<ExpectedNfcapdTopology, PipelineError> {
    let mut topology = BTreeMap::new();
    let mut bucket_start = start;
    while bucket_start < end {
        let five_minute_end = bucket_start
            .checked_add(FIVE_MINUTES)
            .ok_or_else(|| PipelineError::InvalidConfig("nfcapd bucket end overflow".into()))?;
        topology
            .entry(Granularity::FiveMinutes)
            .or_insert_with(BTreeMap::new)
            .insert(
                bucket_start,
                ExpectedNfcapdTopologyBucket {
                    bucket_end: five_minute_end,
                    child_count: 1,
                },
            );
        for granularity in [
            Granularity::ThirtyMinutes,
            Granularity::OneHour,
            Granularity::OneDay,
        ] {
            let (rollup_start, rollup_end) = aggregate_bounds(bucket_start, granularity, timezone)?;
            topology
                .entry(granularity)
                .or_insert_with(BTreeMap::new)
                .entry(rollup_start)
                .and_modify(|bucket: &mut ExpectedNfcapdTopologyBucket| {
                    bucket.child_count += 1;
                })
                .or_insert(ExpectedNfcapdTopologyBucket {
                    bucket_end: rollup_end,
                    child_count: 1,
                });
        }
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    Ok(topology)
}

fn nfcapd_day_coverage_is_canonical(
    connection: &Connection,
    source_id: &str,
    source_units: usize,
    topology: &ExpectedNfcapdTopology,
    day_start: i64,
    day_end: i64,
) -> Result<bool, PipelineError> {
    let rows = connection
        .prepare(&format!(
            "SELECT granularity, bucket_start, bucket_end, coverage_state,
                    observed_units, expected_units, rejected_units
             FROM bucket_coverage
             WHERE source_id = ?1 AND {CANONICAL_GRANULARITY_PREDICATE}
               AND bucket_start >= ?2 AND bucket_start < ?3
             ORDER BY granularity, bucket_start"
        ))
        .map_err(StorageError::from)?
        .query_map(params![source_id, day_start, day_end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(StorageError::from)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)?;
    let actual = rows
        .into_iter()
        .map(
            |(granularity, start, end, state, observed, expected, rejected)| {
                (
                    (granularity, start, end),
                    (state, observed, expected, rejected),
                )
            },
        )
        .collect::<BTreeMap<_, _>>();
    let source_units = i64::try_from(source_units)
        .map_err(|_| PipelineError::InvalidConfig("nfcapd source unit count overflow".into()))?;
    let mut expected_keys = BTreeSet::new();
    for (granularity, buckets) in topology {
        for (bucket_start, bucket) in buckets {
            expected_keys.insert((
                granularity.as_str().to_owned(),
                *bucket_start,
                bucket.bucket_end,
            ));
            let expected_units = source_units
                .checked_mul(bucket.child_count)
                .ok_or_else(|| {
                    PipelineError::InvalidConfig("nfcapd coverage unit count overflow".into())
                })?;
            let Some((state, observed, actual_expected, rejected)) = actual.get(&(
                granularity.as_str().to_owned(),
                *bucket_start,
                bucket.bucket_end,
            )) else {
                return Ok(false);
            };
            if *state != "complete"
                || *observed != expected_units
                || *actual_expected != expected_units
                || *rejected != 0
            {
                return Ok(false);
            }
        }
    }
    Ok(actual.keys().all(|key| expected_keys.contains(key)) && actual.len() == expected_keys.len())
}

fn nfcapd_day_rows_are_canonical(
    connection: &Connection,
    source_id: &str,
    topology: &ExpectedNfcapdTopology,
    run_maad: bool,
    day_start: i64,
    day_end: i64,
) -> Result<bool, PipelineError> {
    for table in [
        "traffic_stats",
        "protocol_stats",
        "address_count_stats",
        "port_count_stats",
        "address_structure_stats",
    ] {
        let query = format!(
            "SELECT granularity, bucket_start, MIN(bucket_end), MAX(bucket_end), COUNT(*),
                    COALESCE(SUM(CASE WHEN ip_version IN (4, 6) AND ({predicate}) THEN 1 ELSE 0 END), 0)
             FROM {table}
             WHERE source_id = ?1 AND {CANONICAL_GRANULARITY_PREDICATE}
               AND bucket_start >= ?2 AND bucket_start < ?3
             GROUP BY granularity, bucket_start",
            predicate = canonical_row_family_predicate(table),
        );
        let rows = connection
            .prepare(&query)
            .map_err(StorageError::from)?
            .query_map(params![source_id, day_start, day_end], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(StorageError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?;
        let actual = rows
            .into_iter()
            .map(
                |(granularity, start, minimum_end, maximum_end, total, canonical)| {
                    (
                        (granularity, start),
                        (minimum_end, maximum_end, total, canonical),
                    )
                },
            )
            .collect::<BTreeMap<_, _>>();
        let mut expected_keys = BTreeSet::new();
        for (granularity, buckets) in topology {
            let expected = canonical_row_family_count(table, true, run_maad);
            for (bucket_start, bucket) in buckets {
                let key = (granularity.as_str().to_owned(), *bucket_start);
                if expected == 0 {
                    if actual.contains_key(&key) {
                        return Ok(false);
                    }
                    continue;
                }
                expected_keys.insert(key.clone());
                let Some((minimum_end, maximum_end, total, canonical)) = actual.get(&key) else {
                    return Ok(false);
                };
                if minimum_end != maximum_end
                    || *minimum_end != bucket.bucket_end
                    || *total != expected
                    || *canonical != expected
                {
                    return Ok(false);
                }
            }
        }
        if actual.len() != expected_keys.len()
            || actual.keys().any(|key| !expected_keys.contains(key))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Validate the complete topology once before a daily-active day is treated as resumable. The
/// grouped queries inspect all row families and all four granularities in the day, so a single
/// damaged bucket or rollup cannot be hidden by a matching total elsewhere.
fn nfcapd_day_has_canonical_topology(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
    timezone: &str,
    run_maad: bool,
) -> Result<bool, PipelineError> {
    #[cfg(test)]
    NFCAPD_DAY_TOPOLOGY_AUDIT_CALLS.with(|calls| calls.set(calls.get() + 1));
    let topology = expected_nfcapd_day_topology(start, end, timezone)?;
    if topology
        .get(&Granularity::FiveMinutes)
        .is_none_or(BTreeMap::is_empty)
    {
        return Ok(false);
    }
    let range_end = topology
        .get(&Granularity::OneDay)
        .and_then(|buckets| buckets.values().map(|bucket| bucket.bucket_end).max())
        .unwrap_or(end);
    for source in sources {
        if !nfcapd_day_coverage_is_canonical(
            connection,
            &source.source_id,
            source.members.len(),
            &topology,
            start,
            range_end,
        )? {
            return Ok(false);
        }
        if !nfcapd_day_rows_are_canonical(
            connection,
            &source.source_id,
            &topology,
            run_maad,
            start,
            range_end,
        )? {
            return Ok(false);
        }
    }
    Ok(!sources.is_empty())
}

/// Classify completion evidence for every logical source in a daily-active product.
///
/// A dirty tombstone is intentionally distinct from a missing marker. Missing markers (including
/// databases created before completion markers existed) may use the legacy topology audit once;
/// dirty days must be rebuilt with `--force` before any normal resume mutation is attempted.
fn nfcapd_day_completion_state(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
    run_maad: bool,
) -> Result<DailyProductCompletionState, PipelineError> {
    let Some(product_fingerprint) = current_product_fingerprint(connection)? else {
        return Ok(DailyProductCompletionState::Missing);
    };
    if sources.is_empty() {
        return Ok(DailyProductCompletionState::Missing);
    }
    let mut missing = false;
    for source in sources {
        match daily_product_completion_state(
            connection,
            &source.source_id,
            start,
            end,
            &product_fingerprint,
            run_maad,
        )? {
            DailyProductCompletionState::Clean => {}
            DailyProductCompletionState::Dirty => return Ok(DailyProductCompletionState::Dirty),
            DailyProductCompletionState::Missing => missing = true,
        }
    }
    Ok(if missing {
        DailyProductCompletionState::Missing
    } else {
        DailyProductCompletionState::Clean
    })
}

/// Publish completion markers after a complete daily-active transaction has written every
/// canonical family, rollup, and evidence/provenance row.
fn mark_nfcapd_day_complete(
    connection: &Connection,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
    run_maad: bool,
) -> Result<(), PipelineError> {
    let product_fingerprint = current_product_fingerprint(connection)?.ok_or_else(|| {
        PipelineError::InvalidConfig(
            "cannot publish a daily product completion marker before product identity binding"
                .into(),
        )
    })?;
    for source in sources {
        upsert_daily_product_completion(
            connection,
            &source.source_id,
            start,
            end,
            &product_fingerprint,
            run_maad,
        )?;
    }
    Ok(())
}

fn source_has_candidate(
    source: &DatasetSource,
    bucket_start: i64,
    paths: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    extend_gaps_to_window: bool,
) -> bool {
    let has_file = source
        .members
        .iter()
        .any(|member| paths.contains_key(&(member.clone(), bucket_start)));
    has_file
        || extend_gaps_to_window
        || source.members.iter().any(|member| {
            member_bounds
                .get(member)
                .is_some_and(|(first, last)| *first <= bucket_start && bucket_start <= *last)
        })
}

/// Group local five-minute starts so each decode batch has at most twelve physical requests when
/// possible. A timestamp with more than twelve members is kept as one batch and drained in
/// physical-request chunks by the decode caller.
fn nfcapd_batch_starts(
    start: i64,
    end: i64,
    timezone: &str,
    sources: &[DatasetSource],
    paths: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    extend_gaps_to_window: bool,
) -> Result<Vec<i64>, PipelineError> {
    let mut starts = Vec::with_capacity(NFCAPD_DECODE_BATCH_SIZE);
    let mut physical_requests = BTreeSet::new();
    let mut next = start;
    while next < end {
        let timestamp_requests = sources
            .iter()
            .filter(|source| {
                source_has_candidate(source, next, paths, member_bounds, extend_gaps_to_window)
            })
            .flat_map(|source| {
                source.members.iter().filter_map(|member| {
                    paths
                        .contains_key(&(member.clone(), next))
                        .then_some((member.clone(), next))
                })
            })
            .collect::<BTreeSet<_>>();
        let would_exceed_physical_limit = !starts.is_empty()
            && physical_requests.len() + timestamp_requests.len() > NFCAPD_DECODE_BATCH_SIZE;
        if would_exceed_physical_limit || starts.len() == NFCAPD_DECODE_BATCH_SIZE {
            break;
        }
        starts.push(next);
        physical_requests.extend(timestamp_requests);
        next = next_local_five_minute_start(next, timezone)?;
    }
    Ok(starts)
}

fn nfcapd_decode_request_chunks<T>(requests: &[T]) -> impl Iterator<Item = &[T]> {
    requests.chunks(NFCAPD_DECODE_BATCH_SIZE)
}

/// External inputs observed while preparing one local day. The maps stay bounded to that day and
/// let the transaction's final guard verify the exact files used by the day before COMMIT.
#[derive(Clone, Debug, Default)]
struct NfcapdDayGuards {
    capture_revisions: BTreeMap<PathBuf, PreparedRevision>,
    activity_snapshots: BTreeMap<PathBuf, FileSnapshot>,
    nfdump_revision: Option<ExecutableRevision>,
}

struct NfcapdDayResult {
    profile: NfcapdDayPublishProfile,
    guards: NfcapdDayGuards,
}

#[allow(clippy::too_many_arguments)]
fn process_nfcapd_tree_day(
    connection: &Connection,
    root: &Path,
    sources: &[DatasetSource],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    start: i64,
    end: i64,
    extend_gaps_to_window: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
    canonical_day_verified: bool,
) -> Result<NfcapdDayResult, PipelineError> {
    let day_started = Instant::now();
    let mut prepare_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut publish_elapsed = Duration::ZERO;
    let mut publish_profile = NfcapdDayPublishProfile::default();
    let mut guards = NfcapdDayGuards {
        nfdump_revision: pipeline
            .selection
            .selects_daily_active_sources()
            .then(|| pipeline.nfdump_revision.clone())
            .flatten(),
        ..NfcapdDayGuards::default()
    };
    let revision_hash_workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(NFCAPD_REVISION_HASH_MAX_WORKERS);
    let revision_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(revision_hash_workers)
        .thread_name(|index| format!("nfcapd-revision-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build revision hash pool: {error}"))
        })?;
    let decoder_fingerprint = nfdump_decoder_fingerprint_for_pipeline(pipeline)?;
    let revision_context = NfcapdRevisionContext {
        connection,
        sources,
        by_member_and_start,
        member_bounds,
        extend_gaps_to_window,
        force,
        decoder_fingerprint,
        capture_snapshots,
        revision_pool: &revision_pool,
    };
    let mut bucket_start = start;
    let mut batches = Vec::new();
    while bucket_start < end {
        let prepare_started = Instant::now();
        let batch_starts = nfcapd_batch_starts(
            bucket_start,
            end,
            &pipeline.timezone,
            sources,
            by_member_and_start,
            member_bounds,
            extend_gaps_to_window,
        )?;
        bucket_start = batch_starts
            .last()
            .copied()
            .map(|last| next_local_five_minute_start(last, &pipeline.timezone))
            .transpose()?
            .expect("non-empty nfcapd batch while processing a non-empty window");
        let revisions = resolve_nfcapd_batch_revisions(&revision_context, &batch_starts)?;
        if pipeline.selection.selects_daily_active_sources() {
            guards.capture_revisions.extend(
                revisions
                    .iter()
                    .map(|(path, revision)| (path.clone(), revision.clone())),
            );
        }
        let mut batch = Vec::with_capacity(batch_starts.len());
        for bucket_start in batch_starts {
            batch.push(prepare_nfcapd_tree_timestamp(
                connection,
                root,
                sources,
                by_member_and_start,
                member_bounds,
                bucket_start,
                extend_gaps_to_window,
                force,
                pipeline,
                report,
                &revisions,
                canonical_day_verified,
            )?);
        }
        prepare_elapsed += prepare_started.elapsed();
        batches.push(batch);
    }

    if pipeline.selection.selects_daily_active_sources()
        && !force
        && batches
            .iter()
            .flatten()
            .flat_map(|timestamp| &timestamp.jobs)
            .any(|job| job.is_repair)
    {
        return Err(PipelineError::InvalidConfig(format!(
            "daily_active_sources input changed for local day {start}..{end}; rerun that whole day with --force"
        )));
    }
    let has_jobs = batches
        .iter()
        .flatten()
        .any(|timestamp| !timestamp.jobs.is_empty());
    let active_resolution = if pipeline.selection.selects_daily_active_sources() && has_jobs {
        verify_nfdump_revision(pipeline)?;
        Some(resolve_daily_active_sources(
            sources,
            by_member_and_start,
            start,
            end,
            pipeline,
            capture_snapshots,
            &guards.capture_revisions,
        )?)
    } else {
        None
    };
    if active_resolution.is_some() {
        verify_nfdump_revision(pipeline)?;
    }
    if let Some((active_sources, _)) = &active_resolution {
        publish_profile.active_set_count = profile_count(active_sources.len());
    }
    let decode_pool = if has_jobs {
        Some(build_nfcapd_decode_pool()?)
    } else {
        None
    };

    for batch in batches {
        verify_nfdump_revision(pipeline)?;
        let decode_started = Instant::now();
        let needed = batch
            .iter()
            .flat_map(|timestamp| {
                timestamp.jobs.iter().flat_map(|job| {
                    job.present.iter().map(|(member, path)| {
                        let snapshot = timestamp
                            .revision_cache
                            .get(member)
                            .and_then(|owner| owner.snapshot.clone())
                            .expect("present member has a snapshot");
                        (
                            (member.clone(), timestamp.bucket_start),
                            (path.clone(), snapshot),
                        )
                    })
                })
            })
            .collect::<BTreeMap<_, _>>();
        let requests = needed.into_iter().collect::<Vec<_>>();
        let mut decoded_cache = BTreeMap::new();
        for request_chunk in nfcapd_decode_request_chunks(&requests) {
            let decoded = decode_pool
                .as_ref()
                .expect("pending nfcapd work has a decode pool")
                .install(|| {
                    request_chunk
                        .par_iter()
                        .map(|((member, bucket_start), (path, snapshot))| {
                            let bucket = (|| -> Result<CanonicalBucket, PipelineError> {
                                let bucket = match &active_resolution {
                                    Some((active_sources, _)) => {
                                        ingest::read_nfcapd_bucket_with_active_sources(
                                            path,
                                            member,
                                            &pipeline.selection,
                                            active_sources.clone(),
                                            &pipeline.nfdump,
                                            &pipeline.timezone,
                                        )?
                                    }
                                    None => ingest::read_nfcapd_bucket(
                                        path,
                                        member,
                                        &pipeline.selection,
                                        &pipeline.nfdump,
                                        &pipeline.timezone,
                                    )?,
                                };
                                verify_file_snapshot(path, snapshot)?;
                                Ok(bucket)
                            })()
                            .map_err(|error| {
                                nfcapd_decode_error(member, *bucket_start, path, error)
                            })?;
                            Ok::<_, PipelineError>(((member.clone(), *bucket_start), bucket))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })?;
            decoded_cache.extend(decoded);
            verify_nfdump_revision(pipeline)?;
        }
        decode_elapsed += decode_started.elapsed();

        let publish_started = Instant::now();
        for timestamp in batch {
            for job in timestamp.jobs {
                let member_buckets = job
                    .present
                    .iter()
                    .map(|(member, _)| {
                        decoded_cache
                            .get(&(member.clone(), timestamp.bucket_start))
                            .expect("requested physical member was decoded")
                    })
                    .collect::<Vec<_>>();
                let logical_started = Instant::now();
                let logical = logical_source_bucket(
                    &job.source_id,
                    timestamp.bucket_start,
                    job.expected_units,
                    &member_buckets,
                )?;
                publish_profile.logical_source_elapsed += logical_started.elapsed();
                let sibling_started = Instant::now();
                if !job.is_repair {
                    aggregates.reject_persisted_siblings(
                        connection,
                        &logical,
                        &pipeline.timezone,
                    )?;
                }
                publish_profile.persisted_sibling_elapsed += sibling_started.elapsed();
                let bucket_profile = publish_nfcapd_bucket_profiled(
                    connection,
                    &logical,
                    &job.owners,
                    &job.absences,
                    &job.evidence,
                    true,
                    force,
                    pipeline.run_maad,
                )?;
                publish_profile.bucket_publish.include(bucket_profile);
                let flushed = if job.is_repair {
                    refresh_rollups_after_five_minute_repair(
                        connection,
                        &logical,
                        &pipeline.timezone,
                    )?;
                    0
                } else {
                    let aggregate_profile =
                        aggregates.include_profiled(&logical, &pipeline.timezone)?;
                    publish_profile.aggregate_include.include(aggregate_profile);
                    let flush_started = Instant::now();
                    let (flushed, rollup_write) =
                        aggregates.flush_complete_profiled(connection, pipeline.run_maad)?;
                    publish_profile.completed_rollup_flush_elapsed += flush_started.elapsed();
                    publish_profile.completed_rollup_write.include(rollup_write);
                    publish_profile.completed_rollup_flushes += 1;
                    if flushed > 0 {
                        publish_profile.nonempty_rollup_flushes += 1;
                    }
                    flushed
                };
                publish_profile.logical_buckets += 1;
                report.rollup_buckets += flushed;
                report.five_minute_buckets += 1;
            }
            decoded_cache.retain(|(_, start), _| *start != timestamp.bucket_start);
        }
        publish_elapsed += publish_started.elapsed();
    }
    if let Some((_, snapshots)) = &active_resolution {
        for (path, snapshot) in snapshots {
            guards
                .activity_snapshots
                .insert(path.clone(), snapshot.clone());
            verify_file_snapshot(path, snapshot)?;
        }
    }
    verify_nfdump_revision(pipeline)?;
    publish_profile.day_elapsed = day_started.elapsed();
    publish_profile.prepare_elapsed = prepare_elapsed;
    publish_profile.decode_elapsed = decode_elapsed;
    publish_profile.batch_publish_elapsed = publish_elapsed;
    tracing::info!(
        target: "netflow_db::profile",
        phase = "nfcapd_tree_day",
        day_start = start,
        day_end = end,
        elapsed_seconds = publish_profile.day_elapsed.as_secs_f64(),
        prepare_seconds = prepare_elapsed.as_secs_f64(),
        decode_seconds = decode_elapsed.as_secs_f64(),
        publish_seconds = publish_elapsed.as_secs_f64(),
    );
    Ok(NfcapdDayResult {
        profile: publish_profile,
        guards,
    })
}

fn resolve_daily_active_sources(
    sources: &[DatasetSource],
    paths: &BTreeMap<(String, i64), PathBuf>,
    start: i64,
    end: i64,
    pipeline: &ResolvedPipeline,
    capture_snapshots: &BTreeMap<PathBuf, FileSnapshot>,
    revision_snapshots: &BTreeMap<PathBuf, PreparedRevision>,
) -> Result<DailyActiveResolution, PipelineError> {
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>();
    let physical_ids = physical_ids.into_iter().collect::<Vec<_>>();
    let activity_pool = build_nfcapd_activity_pool()?;
    let mut combined = HashMap::<IpAddr, nfdump::SourceActivity>::new();
    let mut snapshots = Vec::new();
    for member_chunk in physical_ids.chunks(NFCAPD_DECODE_BATCH_SIZE) {
        let requests = member_chunk
            .iter()
            .map(|member| {
                nfcapd_day_activity_paths(paths, member, start, end, &pipeline.timezone)
                    .map(|member_paths| (member.clone(), member_paths))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let member_results = activity_pool.install(|| {
            requests
                .par_iter()
                .map(|(member, member_paths)| {
                    let member_snapshots = member_paths
                        .iter()
                        .map(|path| {
                            let snapshot = capture_snapshots
                                .get(path)
                                .cloned()
                                .or_else(|| {
                                    revision_snapshots
                                        .get(path)
                                        .and_then(|revision| revision.snapshot.clone())
                                })
                                .map(Ok)
                                .unwrap_or_else(|| capture_nfcapd_snapshot(path));
                            snapshot
                                .map(|snapshot| (path.clone(), snapshot))
                                .map_err(|error| {
                                    daily_activity_scan_error(
                                        member,
                                        start,
                                        end,
                                        member_paths,
                                        error,
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>, PipelineError>>()?;
                    let activity = ingest::read_nfcapd_daily_source_activity(
                        member_paths,
                        &pipeline.selection,
                        &pipeline.nfdump,
                    )
                    .map_err(|error| {
                        daily_activity_scan_error(member, start, end, member_paths, error)
                    })?;
                    Ok::<_, PipelineError>((activity, member_snapshots))
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        for (activity, member_snapshots) in member_results {
            snapshots.extend(member_snapshots);
            for (address, metrics) in activity {
                combined.entry(address).or_default().include(metrics);
            }
        }
    }
    for (path, snapshot) in &snapshots {
        verify_file_snapshot(path, snapshot)?;
    }

    let mut active = AddressSet::default();
    active.extend(combined.into_iter().filter_map(|(address, metrics)| {
        FlowSelection::daily_activity_threshold_met(metrics.flows, metrics.packets, metrics.bytes)
            .then_some(address)
    }));
    tracing::info!(
        day_start = start,
        day_end = end,
        active_sources = active.len(),
        "resolved daily active sources"
    );
    Ok((Arc::new(active), snapshots))
}

struct NfcapdRevisionProbe {
    path: PathBuf,
    observed: FileSnapshot,
    cached_content_fingerprint: Option<String>,
}

/// Hash a capture after an already-captured observation, retaining the usual before/after
/// stability check without taking a redundant pre-hash snapshot.
fn capture_file_revision_with_snapshot(
    path: &Path,
    observed: &FileSnapshot,
) -> Result<(String, FileSnapshot), ProvenanceError> {
    let content_fingerprint = file_sha256(path)?;
    let after = FileSnapshot::capture(path)?;
    if &after != observed {
        return Err(ProvenanceError::InputContentChanged(format!(
            "Input changed while its revision was being captured: {:?}",
            path
        )));
    }
    Ok((content_fingerprint, after))
}

struct NfcapdRevisionContext<'a> {
    connection: &'a Connection,
    sources: &'a [DatasetSource],
    by_member_and_start: &'a BTreeMap<(String, i64), PathBuf>,
    member_bounds: &'a BTreeMap<String, (i64, i64)>,
    extend_gaps_to_window: bool,
    force: bool,
    decoder_fingerprint: String,
    capture_snapshots: &'a BTreeMap<PathBuf, FileSnapshot>,
    revision_pool: &'a rayon::ThreadPool,
}

/// Resolve the physical files needed by a decode batch before making any job decisions.
/// SQLite access stays on the pipeline thread; only exact hashes run in parallel.
fn resolve_nfcapd_batch_revisions(
    context: &NfcapdRevisionContext<'_>,
    batch_starts: &[i64],
) -> Result<BTreeMap<PathBuf, PreparedRevision>, PipelineError> {
    let mut paths = BTreeSet::new();
    for &bucket_start in batch_starts {
        for source in context.sources {
            if !source_has_candidate(
                source,
                bucket_start,
                context.by_member_and_start,
                context.member_bounds,
                context.extend_gaps_to_window,
            ) {
                continue;
            }
            paths.extend(source.members.iter().filter_map(|member| {
                context
                    .by_member_and_start
                    .get(&(member.clone(), bucket_start))
                    .cloned()
            }));
        }
    }

    let probes = paths
        .into_iter()
        .map(|path| {
            let locator = path.to_string_lossy().into_owned();
            let observed = context
                .capture_snapshots
                .get(&path)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| capture_nfcapd_snapshot(&path))?;
            let cached_fingerprint = if context.force {
                None
            } else {
                cached_content_fingerprint(
                    context.connection,
                    InputKind::Nfcapd,
                    &locator,
                    &observed,
                )?
            };
            Ok::<_, PipelineError>(NfcapdRevisionProbe {
                path,
                observed,
                cached_content_fingerprint: cached_fingerprint,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let decoder_fingerprint = context.decoder_fingerprint.clone();

    let resolved = context.revision_pool.install(|| {
        probes
            .par_iter()
            .map(|probe| {
                let captured = match &probe.cached_content_fingerprint {
                    Some(content_fingerprint) => {
                        Ok((content_fingerprint.clone(), probe.observed.clone()))
                    }
                    None => capture_file_revision_with_snapshot(&probe.path, &probe.observed),
                };
                captured
                    .map_err(PipelineError::from)
                    .and_then(|(content_fingerprint, snapshot)| {
                        let revision = InputRevision::create(
                            "nfcapd",
                            probe.path.to_string_lossy().into_owned(),
                            content_fingerprint,
                            &decoder_fingerprint,
                        )?;
                        Ok(PreparedRevision {
                            revision,
                            snapshot: Some(snapshot),
                        })
                    })
            })
            .collect::<Vec<_>>()
    });

    probes
        .into_iter()
        .zip(resolved)
        .map(|(probe, result)| result.map(|revision| (probe.path, revision)))
        .collect::<Result<BTreeMap<_, _>, _>>()
}

#[allow(clippy::too_many_arguments)]
fn prepare_nfcapd_tree_timestamp(
    connection: &Connection,
    root: &Path,
    sources: &[DatasetSource],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    bucket_start: i64,
    extend_gaps_to_window: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
    canonical_day_verified: bool,
) -> Result<PreparedTreeTimestamp, PipelineError> {
    prepare_nfcapd_tree_timestamp_with_cache(
        connection,
        root,
        sources,
        by_member_and_start,
        member_bounds,
        bucket_start,
        extend_gaps_to_window,
        force,
        pipeline,
        report,
        revisions,
        canonical_day_verified,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_nfcapd_tree_timestamp_with_cache(
    connection: &Connection,
    root: &Path,
    sources: &[DatasetSource],
    by_member_and_start: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    bucket_start: i64,
    extend_gaps_to_window: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
    canonical_day_verified: bool,
    resume_cache: Option<&NfcapdDayResumeCache>,
) -> Result<PreparedTreeTimestamp, PipelineError> {
    #[cfg(test)]
    PREPARE_NFCAPD_TREE_TIMESTAMP_CALLS.with(|calls| calls.set(calls.get() + 1));

    let mut revision_cache: BTreeMap<String, PreparedRevision> = BTreeMap::new();
    let mut jobs = Vec::new();
    for source in sources {
        if !source_has_candidate(
            source,
            bucket_start,
            by_member_and_start,
            member_bounds,
            extend_gaps_to_window,
        ) {
            continue;
        }
        let present = source
            .members
            .iter()
            .filter_map(|member| {
                by_member_and_start
                    .get(&(member.clone(), bucket_start))
                    .map(|path| (member.clone(), path.clone()))
            })
            .collect::<Vec<_>>();
        let mut owners = Vec::new();
        for (member, path) in &present {
            let owner = match revision_cache.get(member) {
                Some(owner) => owner.clone(),
                None => {
                    let owner = revisions
                        .get(path)
                        .cloned()
                        .expect("present member has a resolved revision");
                    revision_cache.insert(member.clone(), owner.clone());
                    owner
                }
            };
            owners.push(owner);
        }
        let mut absences = Vec::new();
        let mut evidence = Vec::with_capacity(source.members.len());
        for ((member, path), owner) in present.iter().zip(&owners) {
            evidence.push(InputEvidenceRow::new(
                &source.source_id,
                member,
                bucket_start,
                bucket_start + FIVE_MINUTES,
                path.to_string_lossy(),
                InputEvidenceState::Observed,
                Some(owner.revision.fingerprint.clone()),
            ));
        }
        for member in &source.members {
            if !present.iter().any(|(present, _)| present == member) {
                let expected =
                    expected_nfcapd_path(root, member, bucket_start, &pipeline.timezone)?;
                absences.push(ExpectedAbsence::capture(&expected)?);
                evidence.push(InputEvidenceRow::new(
                    &source.source_id,
                    member,
                    bucket_start,
                    bucket_start + FIVE_MINUTES,
                    expected.to_string_lossy(),
                    InputEvidenceState::Missing,
                    None,
                ));
            }
        }
        evidence.sort_unstable_by(|left, right| left.unit_id.cmp(&right.unit_id));
        let previous_evidence = match resume_cache {
            Some(cache) => Cow::Borrowed(cache.evidence(&source.source_id, bucket_start)),
            None => Cow::Owned(query_input_evidence(
                connection,
                &source.source_id,
                bucket_start,
            )?),
        };
        let observed_input_disappeared = previous_evidence.iter().any(|previous| {
            previous.evidence_state == InputEvidenceState::Observed
                && evidence.iter().any(|current| {
                    current.unit_id == previous.unit_id
                        && current.evidence_state == InputEvidenceState::Missing
                })
        });
        if observed_input_disappeared {
            tracing::warn!(
                source_id = source.source_id,
                bucket_start,
                "preserving prior bucket because an observed input is now missing"
            );
            report.skipped_inputs += 1;
            continue;
        }
        let revisions = owners
            .iter()
            .map(|owner| owner.revision.clone())
            .collect::<Vec<_>>();
        let persisted_processed = if force || revisions.is_empty() {
            false
        } else {
            match resume_cache {
                Some(cache) => cache.processed(&source.source_id, bucket_start, &revisions)?,
                None => nfcapd_logical_bucket_processed(
                    connection,
                    &source.source_id,
                    bucket_start,
                    &revisions,
                )?,
            }
        };
        let mut topology_corruption = false;
        if !force && previous_evidence == evidence {
            let provenance_complete = revisions.is_empty() || persisted_processed;
            if provenance_complete {
                let topology_matches = canonical_day_verified
                    || nfcapd_logical_bucket_has_canonical_topology(
                        connection,
                        &source.source_id,
                        bucket_start,
                        present.len(),
                        source.members.len(),
                        pipeline.run_maad,
                    )?;
                if topology_matches {
                    report.skipped_inputs += 1;
                    continue;
                }
                topology_corruption = true;
            }
        }
        let orphaned_provenance = !force
            && previous_evidence.is_empty()
            && persisted_processed
            && (canonical_day_verified
                || nfcapd_logical_bucket_has_canonical_topology(
                    connection,
                    &source.source_id,
                    bucket_start,
                    present.len(),
                    source.members.len(),
                    pipeline.run_maad,
                )?);
        let is_repair = !force
            && (orphaned_provenance
                || (!previous_evidence.is_empty()
                    && (previous_evidence != evidence || topology_corruption)));
        jobs.push(PreparedTreeJob {
            source_id: source.source_id.clone(),
            expected_units: source.members.len(),
            present,
            owners,
            absences,
            evidence,
            is_repair,
        });
    }
    Ok(PreparedTreeTimestamp {
        bucket_start,
        revision_cache,
        jobs,
    })
}

#[allow(clippy::too_many_arguments)]
enum PreparedExplicitNfcapdKind {
    File(PreparedRevision),
    Gap { expected_path: Option<PathBuf> },
}

struct PreparedExplicitNfcapd {
    path: PathBuf,
    source_id: String,
    bucket_start: i64,
    kind: PreparedExplicitNfcapdKind,
}

fn process_explicit_nfcapd_inputs(
    connection: &Connection,
    inputs: &[InputSpec],
    pipeline: &ResolvedPipeline,
) -> Result<PipelineReport, PipelineError> {
    let mut prepared = Vec::new();
    for input in inputs {
        let InputSpec::Nfcapd {
            path,
            source_id,
            bucket_start,
            gap,
            expected_path,
        } = input
        else {
            continue;
        };
        let bucket_start = match bucket_start {
            Some(start) => *start,
            None if !gap => ingest::parse_nfcapd_bucket_start(path, &pipeline.timezone)?,
            None => {
                return Err(PipelineError::InvalidConfig(
                    "explicit nfcapd gap requires bucket_start".into(),
                ));
            }
        };
        let kind = if *gap {
            PreparedExplicitNfcapdKind::Gap {
                expected_path: expected_path.clone(),
            }
        } else {
            let (revision, snapshot) = prepare_file_revision(
                connection,
                path,
                InputKind::Nfcapd,
                nfdump_decoder_fingerprint_for_pipeline(pipeline)?,
            )?;
            PreparedExplicitNfcapdKind::File(PreparedRevision {
                revision,
                snapshot: Some(snapshot),
            })
        };
        prepared.push(PreparedExplicitNfcapd {
            path: path.clone(),
            source_id: source_id.clone(),
            bucket_start,
            kind,
        });
    }
    prepared.sort_unstable_by(|left, right| {
        (left.bucket_start, &left.source_id, &left.path).cmp(&(
            right.bucket_start,
            &right.source_id,
            &right.path,
        ))
    });
    if prepared.is_empty() {
        return Ok(PipelineReport::default());
    }
    process_atomic(connection, pipeline, |aggregates, report| {
        for input in &prepared {
            match &input.kind {
                PreparedExplicitNfcapdKind::File(owner) => process_nfcapd(
                    connection,
                    &input.path,
                    &input.source_id,
                    input.bucket_start,
                    owner,
                    pipeline,
                    aggregates,
                    report,
                )?,
                PreparedExplicitNfcapdKind::Gap { expected_path } => process_nfcapd_gap(
                    connection,
                    &input.path,
                    expected_path.as_deref(),
                    &input.source_id,
                    input.bucket_start,
                    pipeline,
                    aggregates,
                    report,
                )?,
            }
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
fn process_nfcapd(
    connection: &Connection,
    path: &Path,
    source_id: &str,
    bucket_start: i64,
    owner: &PreparedRevision,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    if nfcapd_logical_bucket_processed(
        connection,
        source_id,
        bucket_start,
        std::slice::from_ref(&owner.revision),
    )? {
        report.skipped_inputs += 1;
        return Ok(());
    }
    verify_nfdump_revision(pipeline)?;
    let bucket = ingest::read_nfcapd_bucket(
        path,
        source_id,
        &pipeline.selection,
        &pipeline.nfdump,
        &pipeline.timezone,
    )
    .map_err(|error| nfcapd_decode_error(source_id, bucket_start, path, error))?;
    verify_nfdump_revision(pipeline)?;
    let snapshot = owner
        .snapshot
        .as_ref()
        .expect("explicit file input has a snapshot");
    verify_file_snapshot(path, snapshot)
        .map_err(|error| nfcapd_decode_error(source_id, bucket_start, path, error))?;
    aggregates.reject_persisted_siblings(connection, &bucket, &pipeline.timezone)?;
    publish_nfcapd_bucket(
        connection,
        &bucket,
        std::slice::from_ref(owner),
        &[],
        &[InputEvidenceRow::new(
            source_id,
            source_id,
            bucket_start,
            bucket_start + FIVE_MINUTES,
            &owner.revision.locator,
            InputEvidenceState::Observed,
            Some(owner.revision.fingerprint.clone()),
        )],
        false,
        false,
        pipeline.run_maad,
    )?;
    aggregates.include(&bucket, &pipeline.timezone)?;
    report.rollup_buckets += aggregates.flush_complete(connection, pipeline.run_maad)?;
    report.five_minute_buckets += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_nfcapd_gap(
    connection: &Connection,
    _locator_path: &Path,
    expected_path: Option<&Path>,
    source_id: &str,
    bucket_start: i64,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let expected_path = expected_path.ok_or_else(|| {
        PipelineError::InvalidConfig(
            "explicit nfcapd gap requires expected_path for absence verification".into(),
        )
    })?;
    let absence = ExpectedAbsence::capture(expected_path)?;
    let evidence = [InputEvidenceRow::new(
        source_id,
        source_id,
        bucket_start,
        bucket_start + FIVE_MINUTES,
        expected_path.to_string_lossy(),
        InputEvidenceState::Missing,
        None,
    )];
    if query_input_evidence(connection, source_id, bucket_start)? == evidence {
        report.skipped_inputs += 1;
        return Ok(());
    }
    let bucket = StatisticalBucket::new(BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    ))
    .with_coverage(BucketCoverage::new(1, 0, 0).map_err(DomainError::from)?)
    .finish_owned();
    aggregates.reject_persisted_siblings(connection, &bucket, &pipeline.timezone)?;
    publish_nfcapd_bucket(
        connection,
        &bucket,
        &[],
        &[absence],
        &evidence,
        false,
        false,
        pipeline.run_maad,
    )?;
    aggregates.include(&bucket, &pipeline.timezone)?;
    report.rollup_buckets += aggregates.flush_complete(connection, pipeline.run_maad)?;
    report.five_minute_buckets += 1;
    Ok(())
}

fn normalize_sources(
    root: &Path,
    source_ids: &[String],
    sources: &[DatasetSource],
) -> Result<Vec<DatasetSource>, PipelineError> {
    if !source_ids.is_empty() && !sources.is_empty() {
        return Err(PipelineError::InvalidConfig(
            "nfcapd_tree cannot define both source_ids and sources".into(),
        ));
    }
    let mut normalized = if !sources.is_empty() {
        sources.to_vec()
    } else if !source_ids.is_empty() {
        source_ids
            .iter()
            .map(|source_id| DatasetSource {
                source_id: source_id.clone(),
                members: vec![source_id.clone()],
            })
            .collect()
    } else {
        let entries = fs::read_dir(root)?;
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry.file_type().ok()?.is_dir().then(|| {
                    let source_id = entry.file_name().to_string_lossy().into_owned();
                    DatasetSource {
                        source_id: source_id.clone(),
                        members: vec![source_id],
                    }
                })
            })
            .collect()
    };
    normalized.sort_unstable_by(|left, right| left.source_id.cmp(&right.source_id));
    let mut ids = BTreeSet::new();
    for source in &normalized {
        if !is_safe_path_component(&source.source_id)
            || source.members.is_empty()
            || !ids.insert(source.source_id.clone())
        {
            return Err(PipelineError::InvalidConfig(
                "logical sources require unique non-empty IDs and members".into(),
            ));
        }
        let mut members = BTreeSet::new();
        for member in &source.members {
            if !is_safe_path_component(member) || !members.insert(member) {
                return Err(PipelineError::InvalidConfig(format!(
                    "source {:?} has an unsafe or duplicate member path component",
                    source.source_id
                )));
            }
            if !root.join(member).is_dir() {
                return Err(PipelineError::InvalidConfig(format!(
                    "source {:?} references missing member directory {:?}",
                    source.source_id, member
                )));
            }
        }
    }

    let member_ids = normalized
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut member_paths = BTreeMap::<PathBuf, String>::new();
    #[cfg(unix)]
    let mut member_identities = BTreeMap::<(u64, u64), (String, PathBuf)>::new();
    for member in member_ids {
        let member_path = root.join(&member);
        let canonical_member_path = canonical_path(&member_path)?;
        if let Some(previous_member) = member_paths.get(&canonical_member_path) {
            return Err(PipelineError::InvalidConfig(format!(
                "nfcapd_tree member IDs {:?} and {:?} resolve to the same directory {}",
                previous_member,
                member,
                canonical_member_path.display()
            )));
        }
        member_paths.insert(canonical_member_path.clone(), member.clone());

        #[cfg(unix)]
        {
            let identity = existing_path_identity(&member_path)?.ok_or_else(|| {
                PipelineError::InvalidConfig(format!(
                    "source member directory {:?} disappeared during normalization",
                    member
                ))
            })?;
            if let Some((previous_member, previous_path)) = member_identities.get(&identity) {
                return Err(PipelineError::InvalidConfig(format!(
                    "nfcapd_tree member IDs {:?} and {:?} resolve to the same physical directory via device/inode {:?} ({} and {})",
                    previous_member,
                    member,
                    identity,
                    previous_path.display(),
                    canonical_member_path.display()
                )));
            }
            member_identities.insert(identity, (member, canonical_member_path));
        }
    }
    Ok(normalized)
}

fn merge_source_bucket(
    source_id: &str,
    bucket_start: i64,
    expected_units: usize,
    members: &[&CanonicalBucket],
) -> Result<CanonicalBucket, PipelineError> {
    let key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    );
    let mut builder = if members.is_empty() {
        StatisticalBucket::new(key)
    } else {
        StatisticalBucket::dense(key)
    }
    .with_coverage(BucketCoverage::empty());
    for member in members {
        builder.include(member)?;
    }
    let coverage = BucketCoverage::new(
        u64::try_from(expected_units).unwrap_or(u64::MAX),
        u64::try_from(members.len()).unwrap_or(u64::MAX),
        0,
    )
    .map_err(DomainError::from)?;
    Ok(builder.with_coverage(coverage).finish_owned())
}

fn logical_source_bucket<'a>(
    source_id: &str,
    bucket_start: i64,
    expected_units: usize,
    members: &[&'a CanonicalBucket],
) -> Result<Cow<'a, CanonicalBucket>, PipelineError> {
    let expected_key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    );
    if expected_units == 1
        && let [member] = members
        && member.key == expected_key
    {
        return Ok(Cow::Borrowed(member));
    }
    Ok(Cow::Owned(merge_source_bucket(
        source_id,
        bucket_start,
        expected_units,
        members,
    )?))
}

#[derive(Debug, Default)]
struct NfcapdBucketPublishProfile {
    total_elapsed: Duration,
    preflight_elapsed: Duration,
    overlap_elapsed: Duration,
    force_delete_elapsed: Duration,
    owner_upsert_elapsed: Duration,
    write: WriteBucketsProfile,
    owner_status_elapsed: Duration,
    postflight_elapsed: Duration,
    owners: u64,
    absences: u64,
}

impl NfcapdBucketPublishProfile {
    fn include(&mut self, profile: Self) {
        self.total_elapsed += profile.total_elapsed;
        self.preflight_elapsed += profile.preflight_elapsed;
        self.overlap_elapsed += profile.overlap_elapsed;
        self.force_delete_elapsed += profile.force_delete_elapsed;
        self.owner_upsert_elapsed += profile.owner_upsert_elapsed;
        self.write.include(profile.write);
        self.owner_status_elapsed += profile.owner_status_elapsed;
        self.postflight_elapsed += profile.postflight_elapsed;
        self.owners += profile.owners;
        self.absences += profile.absences;
    }

    fn other_elapsed(&self) -> Duration {
        self.total_elapsed.saturating_sub(
            self.preflight_elapsed
                + self.overlap_elapsed
                + self.force_delete_elapsed
                + self.owner_upsert_elapsed
                + self.write.total_elapsed
                + self.owner_status_elapsed
                + self.postflight_elapsed,
        )
    }
}

#[derive(Debug, Default)]
struct FinalRollupProfile {
    total_elapsed: Duration,
    finish_elapsed: Duration,
    delete_elapsed: Duration,
    write: WriteBucketsProfile,
    incomplete_keys: u64,
    rollup_buckets: u64,
}

impl FinalRollupProfile {
    fn other_elapsed(&self) -> Duration {
        self.total_elapsed
            .saturating_sub(self.finish_elapsed + self.delete_elapsed + self.write.total_elapsed)
    }
}

#[derive(Debug, Default)]
struct AggregateGranularityProfile {
    total_elapsed: Duration,
    bounds_elapsed: Duration,
    builder_elapsed: Duration,
    bucket: StatisticalBucketIncludeProfile,
}

impl AggregateGranularityProfile {
    fn include(
        &mut self,
        total_elapsed: Duration,
        bounds_elapsed: Duration,
        builder_elapsed: Duration,
        bucket: StatisticalBucketIncludeProfile,
    ) {
        self.total_elapsed += total_elapsed;
        self.bounds_elapsed += bounds_elapsed;
        self.builder_elapsed += builder_elapsed;
        self.bucket.include(bucket);
    }

    fn other_elapsed(&self) -> Duration {
        self.total_elapsed
            .saturating_sub(self.bounds_elapsed + self.builder_elapsed + self.bucket.total_elapsed)
    }
}

#[derive(Debug, Default)]
struct AggregateIncludeProfile {
    total_elapsed: Duration,
    thirty_minutes: AggregateGranularityProfile,
    one_hour: AggregateGranularityProfile,
    one_day: AggregateGranularityProfile,
}

impl AggregateIncludeProfile {
    fn include(&mut self, profile: Self) {
        self.total_elapsed += profile.total_elapsed;
        self.thirty_minutes.include(
            profile.thirty_minutes.total_elapsed,
            profile.thirty_minutes.bounds_elapsed,
            profile.thirty_minutes.builder_elapsed,
            profile.thirty_minutes.bucket,
        );
        self.one_hour.include(
            profile.one_hour.total_elapsed,
            profile.one_hour.bounds_elapsed,
            profile.one_hour.builder_elapsed,
            profile.one_hour.bucket,
        );
        self.one_day.include(
            profile.one_day.total_elapsed,
            profile.one_day.bounds_elapsed,
            profile.one_day.builder_elapsed,
            profile.one_day.bucket,
        );
    }

    fn granularity_mut(&mut self, granularity: Granularity) -> &mut AggregateGranularityProfile {
        match granularity {
            Granularity::ThirtyMinutes => &mut self.thirty_minutes,
            Granularity::OneHour => &mut self.one_hour,
            Granularity::OneDay => &mut self.one_day,
            Granularity::FiveMinutes => unreachable!("five-minute buckets are not rollups"),
        }
    }

    fn other_elapsed(&self) -> Duration {
        self.total_elapsed.saturating_sub(
            self.thirty_minutes.total_elapsed
                + self.one_hour.total_elapsed
                + self.one_day.total_elapsed,
        )
    }
}

#[derive(Debug, Default)]
struct NfcapdDayPublishProfile {
    day_elapsed: Duration,
    prepare_elapsed: Duration,
    decode_elapsed: Duration,
    batch_publish_elapsed: Duration,
    logical_source_elapsed: Duration,
    persisted_sibling_elapsed: Duration,
    bucket_publish: NfcapdBucketPublishProfile,
    aggregate_include: AggregateIncludeProfile,
    completed_rollup_flush_elapsed: Duration,
    completed_rollup_write: WriteBucketsProfile,
    final_rollups: FinalRollupProfile,
    logical_buckets: u64,
    completed_rollup_flushes: u64,
    nonempty_rollup_flushes: u64,
    active_set_count: u64,
}

impl NfcapdDayPublishProfile {
    fn log(&self, day_start: i64, day_end: i64, transaction_elapsed: Duration) {
        self.log_with_context(
            day_start,
            day_end,
            transaction_elapsed,
            "single",
            None,
            None,
        );
    }

    fn log_coordinated(
        &self,
        day_start: i64,
        day_end: i64,
        transaction_elapsed: Duration,
        output_index: usize,
        output_path: &Path,
    ) {
        self.log_with_context(
            day_start,
            day_end,
            transaction_elapsed,
            "coordinated",
            Some(output_index),
            Some(output_path),
        );
    }

    fn log_with_context(
        &self,
        day_start: i64,
        day_end: i64,
        transaction_elapsed: Duration,
        mode: &'static str,
        output_index: Option<usize>,
        output_path: Option<&Path>,
    ) {
        let mut rollup_write = self.completed_rollup_write.clone();
        rollup_write.include(self.final_rollups.write.clone());
        let publish_other = self.batch_publish_elapsed.saturating_sub(
            self.logical_source_elapsed
                + self.persisted_sibling_elapsed
                + self.bucket_publish.total_elapsed
                + self.aggregate_include.total_elapsed
                + self.completed_rollup_flush_elapsed,
        );
        let transaction_other =
            transaction_elapsed.saturating_sub(self.day_elapsed + self.final_rollups.total_elapsed);
        let completed_rollup_housekeeping = self
            .completed_rollup_flush_elapsed
            .saturating_sub(self.completed_rollup_write.total_elapsed);
        tracing::info!(
            target: "netflow_db::profile",
            phase = "nfcapd_tree_day_publish_detail",
            mode,
            output_index = ?output_index,
            output_path = ?output_path,
            day_start,
            day_end,
            transaction_seconds = transaction_elapsed.as_secs_f64(),
            transaction_other_seconds = transaction_other.as_secs_f64(),
            day_seconds = self.day_elapsed.as_secs_f64(),
            prepare_seconds = self.prepare_elapsed.as_secs_f64(),
            decode_seconds = self.decode_elapsed.as_secs_f64(),
            batch_publish_seconds = self.batch_publish_elapsed.as_secs_f64(),
            publish_other_seconds = publish_other.as_secs_f64(),
            logical_source_seconds = self.logical_source_elapsed.as_secs_f64(),
            persisted_sibling_seconds = self.persisted_sibling_elapsed.as_secs_f64(),
            bucket_publish_seconds = self.bucket_publish.total_elapsed.as_secs_f64(),
            bucket_preflight_seconds = self.bucket_publish.preflight_elapsed.as_secs_f64(),
            bucket_overlap_seconds = self.bucket_publish.overlap_elapsed.as_secs_f64(),
            bucket_force_delete_seconds = self.bucket_publish.force_delete_elapsed.as_secs_f64(),
            owner_upsert_seconds = self.bucket_publish.owner_upsert_elapsed.as_secs_f64(),
            owner_status_seconds = self.bucket_publish.owner_status_elapsed.as_secs_f64(),
            bucket_postflight_seconds = self.bucket_publish.postflight_elapsed.as_secs_f64(),
            bucket_other_seconds = self.bucket_publish.other_elapsed().as_secs_f64(),
            aggregate_include_seconds = self.aggregate_include.total_elapsed.as_secs_f64(),
            aggregate_include_other_seconds = self.aggregate_include.other_elapsed().as_secs_f64(),
            aggregate_30m_seconds = self.aggregate_include.thirty_minutes.total_elapsed.as_secs_f64(),
            aggregate_30m_bounds_seconds = self.aggregate_include.thirty_minutes.bounds_elapsed.as_secs_f64(),
            aggregate_30m_builder_seconds = self.aggregate_include.thirty_minutes.builder_elapsed.as_secs_f64(),
            aggregate_30m_traffic_seconds = self.aggregate_include.thirty_minutes.bucket.traffic_elapsed.as_secs_f64(),
            aggregate_30m_protocols_seconds = self.aggregate_include.thirty_minutes.bucket.protocols_elapsed.as_secs_f64(),
            aggregate_30m_addresses_seconds = self.aggregate_include.thirty_minutes.bucket.addresses_elapsed.as_secs_f64(),
            aggregate_30m_ports_seconds = self.aggregate_include.thirty_minutes.bucket.ports_elapsed.as_secs_f64(),
            aggregate_30m_coverage_seconds = self.aggregate_include.thirty_minutes.bucket.coverage_elapsed.as_secs_f64(),
            aggregate_30m_bucket_other_seconds = self.aggregate_include.thirty_minutes.bucket.other_elapsed().as_secs_f64(),
            aggregate_30m_other_seconds = self.aggregate_include.thirty_minutes.other_elapsed().as_secs_f64(),
            aggregate_1h_seconds = self.aggregate_include.one_hour.total_elapsed.as_secs_f64(),
            aggregate_1h_bounds_seconds = self.aggregate_include.one_hour.bounds_elapsed.as_secs_f64(),
            aggregate_1h_builder_seconds = self.aggregate_include.one_hour.builder_elapsed.as_secs_f64(),
            aggregate_1h_traffic_seconds = self.aggregate_include.one_hour.bucket.traffic_elapsed.as_secs_f64(),
            aggregate_1h_protocols_seconds = self.aggregate_include.one_hour.bucket.protocols_elapsed.as_secs_f64(),
            aggregate_1h_addresses_seconds = self.aggregate_include.one_hour.bucket.addresses_elapsed.as_secs_f64(),
            aggregate_1h_ports_seconds = self.aggregate_include.one_hour.bucket.ports_elapsed.as_secs_f64(),
            aggregate_1h_coverage_seconds = self.aggregate_include.one_hour.bucket.coverage_elapsed.as_secs_f64(),
            aggregate_1h_bucket_other_seconds = self.aggregate_include.one_hour.bucket.other_elapsed().as_secs_f64(),
            aggregate_1h_other_seconds = self.aggregate_include.one_hour.other_elapsed().as_secs_f64(),
            aggregate_1d_seconds = self.aggregate_include.one_day.total_elapsed.as_secs_f64(),
            aggregate_1d_bounds_seconds = self.aggregate_include.one_day.bounds_elapsed.as_secs_f64(),
            aggregate_1d_builder_seconds = self.aggregate_include.one_day.builder_elapsed.as_secs_f64(),
            aggregate_1d_traffic_seconds = self.aggregate_include.one_day.bucket.traffic_elapsed.as_secs_f64(),
            aggregate_1d_protocols_seconds = self.aggregate_include.one_day.bucket.protocols_elapsed.as_secs_f64(),
            aggregate_1d_addresses_seconds = self.aggregate_include.one_day.bucket.addresses_elapsed.as_secs_f64(),
            aggregate_1d_ports_seconds = self.aggregate_include.one_day.bucket.ports_elapsed.as_secs_f64(),
            aggregate_1d_coverage_seconds = self.aggregate_include.one_day.bucket.coverage_elapsed.as_secs_f64(),
            aggregate_1d_bucket_other_seconds = self.aggregate_include.one_day.bucket.other_elapsed().as_secs_f64(),
            aggregate_1d_other_seconds = self.aggregate_include.one_day.other_elapsed().as_secs_f64(),
            completed_rollup_flush_seconds = self.completed_rollup_flush_elapsed.as_secs_f64(),
            completed_rollup_housekeeping_seconds = completed_rollup_housekeeping.as_secs_f64(),
            final_rollup_seconds = self.final_rollups.total_elapsed.as_secs_f64(),
            final_rollup_finish_seconds = self.final_rollups.finish_elapsed.as_secs_f64(),
            final_rollup_delete_seconds = self.final_rollups.delete_elapsed.as_secs_f64(),
            final_rollup_other_seconds = self.final_rollups.other_elapsed().as_secs_f64(),
            five_minute_write_seconds = self.bucket_publish.write.total_elapsed.as_secs_f64(),
            five_minute_delete_seconds = self.bucket_publish.write.delete_elapsed.as_secs_f64(),
            five_minute_canonical_rows_seconds = self.bucket_publish.write.canonical_rows_elapsed.as_secs_f64(),
            five_minute_scalar_rows_seconds = self.bucket_publish.write.scalar_rows_elapsed.as_secs_f64(),
            five_minute_scalar_insert_seconds = scalar_insert_elapsed(&self.bucket_publish.write).as_secs_f64(),
            five_minute_maad_seconds = self.bucket_publish.write.maad_elapsed.as_secs_f64(),
            five_minute_address_structure_insert_seconds = self.bucket_publish.write.address_structure_insert_elapsed.as_secs_f64(),
            five_minute_write_other_seconds = self.bucket_publish.write.other_elapsed().as_secs_f64(),
            rollup_write_seconds = rollup_write.total_elapsed.as_secs_f64(),
            rollup_delete_seconds = rollup_write.delete_elapsed.as_secs_f64(),
            rollup_canonical_rows_seconds = rollup_write.canonical_rows_elapsed.as_secs_f64(),
            rollup_scalar_rows_seconds = rollup_write.scalar_rows_elapsed.as_secs_f64(),
            rollup_scalar_insert_seconds = scalar_insert_elapsed(&rollup_write).as_secs_f64(),
            rollup_maad_seconds = rollup_write.maad_elapsed.as_secs_f64(),
            rollup_address_structure_insert_seconds = rollup_write.address_structure_insert_elapsed.as_secs_f64(),
            rollup_write_other_seconds = rollup_write.other_elapsed().as_secs_f64(),
            logical_buckets = self.logical_buckets,
            owners = self.bucket_publish.owners,
            absences = self.bucket_publish.absences,
            completed_rollup_flushes = self.completed_rollup_flushes,
            nonempty_rollup_flushes = self.nonempty_rollup_flushes,
            active_set_count = self.active_set_count,
            final_incomplete_keys = self.final_rollups.incomplete_keys,
            final_rollup_buckets = self.final_rollups.rollup_buckets,
            five_minute_write_calls = self.bucket_publish.write.write_calls,
            rollup_write_calls = rollup_write.write_calls,
            five_minute_bucket_keys = self.bucket_publish.write.bucket_keys,
            rollup_bucket_keys = rollup_write.bucket_keys,
            traffic_rows = self.bucket_publish.write.traffic_rows + rollup_write.traffic_rows,
            protocol_rows = self.bucket_publish.write.protocol_rows + rollup_write.protocol_rows,
            address_count_rows = self.bucket_publish.write.address_count_rows + rollup_write.address_count_rows,
            port_count_rows = self.bucket_publish.write.port_count_rows + rollup_write.port_count_rows,
            address_structure_rows = self.bucket_publish.write.address_structure_rows + rollup_write.address_structure_rows,
            maad_address_sets = self.bucket_publish.write.maad_address_sets + rollup_write.maad_address_sets,
            maad_addresses = self.bucket_publish.write.maad_addresses + rollup_write.maad_addresses,
            address_structure_json_bytes = self.bucket_publish.write.address_structure_json_bytes + rollup_write.address_structure_json_bytes,
        );
    }
}

fn scalar_insert_elapsed(profile: &WriteBucketsProfile) -> Duration {
    profile.traffic_insert_elapsed
        + profile.protocol_insert_elapsed
        + profile.address_count_insert_elapsed
        + profile.port_count_insert_elapsed
}

fn profile_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[derive(Clone, Debug)]
struct PreparedRevision {
    revision: InputRevision,
    snapshot: Option<FileSnapshot>,
}

struct PreparedTreeJob {
    source_id: String,
    expected_units: usize,
    present: Vec<(String, PathBuf)>,
    owners: Vec<PreparedRevision>,
    absences: Vec<ExpectedAbsence>,
    evidence: Vec<InputEvidenceRow>,
    is_repair: bool,
}

struct PreparedTreeTimestamp {
    bucket_start: i64,
    revision_cache: BTreeMap<String, PreparedRevision>,
    jobs: Vec<PreparedTreeJob>,
}

#[allow(clippy::too_many_arguments)]
fn publish_nfcapd_bucket(
    connection: &Connection,
    bucket: &CanonicalBucket,
    owners: &[PreparedRevision],
    absences: &[ExpectedAbsence],
    evidence: &[InputEvidenceRow],
    allow_coverage_repair: bool,
    force: bool,
    run_maad: bool,
) -> Result<(), PipelineError> {
    publish_nfcapd_bucket_profiled(
        connection,
        bucket,
        owners,
        absences,
        evidence,
        allow_coverage_repair,
        force,
        run_maad,
    )
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn publish_nfcapd_bucket_profiled(
    connection: &Connection,
    bucket: &CanonicalBucket,
    owners: &[PreparedRevision],
    absences: &[ExpectedAbsence],
    evidence: &[InputEvidenceRow],
    allow_coverage_repair: bool,
    force: bool,
    run_maad: bool,
) -> Result<NfcapdBucketPublishProfile, PipelineError> {
    let total_started = Instant::now();
    let mut profile = NfcapdBucketPublishProfile {
        owners: profile_count(owners.len()),
        absences: profile_count(absences.len()),
        ..NfcapdBucketPublishProfile::default()
    };
    let preflight_started = Instant::now();
    for absence in absences {
        absence.verify()?;
    }
    for owner in owners {
        if let Some(snapshot) = &owner.snapshot {
            verify_file_snapshot(&owner.revision.locator, snapshot)?;
        }
    }
    profile.preflight_elapsed += preflight_started.elapsed();
    let overlap_started = Instant::now();
    reject_overlapping_bucket(
        connection,
        bucket,
        InputKind::Nfcapd,
        "",
        force || allow_coverage_repair,
    )?;
    profile.overlap_elapsed += overlap_started.elapsed();
    if force {
        let force_delete_started = Instant::now();
        connection.execute(
            "DELETE FROM processed_inputs WHERE input_kind = 'nfcapd' AND source_id = ?1 AND bucket_start = ?2",
            params![bucket.key.source_id, bucket.key.bucket_start],
        ).map_err(StorageError::from)?;
        profile.force_delete_elapsed += force_delete_started.elapsed();
    }
    let publication = (|| -> Result<(), PipelineError> {
        let owner_upsert_started = Instant::now();
        for prepared in owners {
            let revision = &prepared.revision;
            let owner = InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: revision.locator.clone(),
                scan_locator: revision.locator.clone(),
                source_id: bucket.key.source_id.clone(),
                bucket_start: bucket.key.bucket_start,
                bucket_end: bucket.key.bucket_end,
                revision: revision.clone(),
                file_snapshot: prepared.snapshot.clone(),
            };
            upsert_input_bucket(connection, &owner, force)?;
        }
        profile.owner_upsert_elapsed += owner_upsert_started.elapsed();
        profile.write = write_buckets_profiled(connection, std::slice::from_ref(bucket), run_maad)?;
        replace_input_evidence(
            connection,
            &bucket.key.source_id,
            bucket.key.bucket_start,
            evidence,
        )?;
        let owner_status_started = Instant::now();
        for prepared in owners {
            let revision = &prepared.revision;
            mark_input_bucket_status(
                connection,
                InputKind::Nfcapd,
                &revision.locator,
                &bucket.key.source_id,
                bucket.key.bucket_start,
                InputStatus::Processed,
                revision,
                None,
            )?;
        }
        profile.owner_status_elapsed += owner_status_started.elapsed();
        let postflight_started = Instant::now();
        for absence in absences {
            absence.verify()?;
        }
        profile.postflight_elapsed += postflight_started.elapsed();
        Ok(())
    })();
    publication?;
    profile.total_elapsed = total_started.elapsed();
    Ok(profile)
}

fn reject_overlapping_bucket(
    connection: &Connection,
    bucket: &CanonicalBucket,
    input_kind: InputKind,
    allowed_scan: &str,
    replace_nfcapd: bool,
) -> Result<(), PipelineError> {
    let conflict = connection
        .query_row(
            "SELECT input_kind, input_locator, scan_locator FROM processed_inputs
             WHERE source_id = ?1 AND bucket_start = ?2
               AND NOT (input_kind = ?3 AND scan_locator = ?4)
             ORDER BY input_kind, input_locator LIMIT 1",
            params![
                bucket.key.source_id,
                bucket.key.bucket_start,
                input_kind.as_str(),
                allowed_scan,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::from)?;
    if let Some((kind, locator, _)) = conflict
        && !(replace_nfcapd && kind == InputKind::Nfcapd.as_str())
    {
        return Err(PipelineError::InvalidConfig(format!(
            "overlapping canonical five-minute input for source {:?} at {} conflicts with {kind}:{locator}",
            bucket.key.source_id, bucket.key.bucket_start
        )));
    }
    Ok(())
}

#[derive(Default)]
struct AggregateBuckets {
    builders: BTreeMap<(String, Granularity, i64, i64), StatisticalBucket>,
    published_through: BTreeMap<String, i64>,
    owned_keys: BTreeSet<(String, i64)>,
    current_run_keys: BTreeSet<(String, i64)>,
    persisted_sibling_validations: BTreeSet<(String, i64, bool)>,
    #[cfg(test)]
    persisted_sibling_queries: usize,
}

impl AggregateBuckets {
    fn with_owned_keys(owned_keys: BTreeSet<(String, i64)>) -> Self {
        Self {
            owned_keys,
            ..Self::default()
        }
    }

    fn reject_persisted_siblings(
        &mut self,
        connection: &Connection,
        child: &CanonicalBucket,
        timezone: &str,
    ) -> Result<(), PipelineError> {
        self.reject_persisted_siblings_inner(connection, child, timezone, false)
    }

    fn reject_persisted_csv_siblings(
        &mut self,
        connection: &Connection,
        child: &CanonicalBucket,
        timezone: &str,
    ) -> Result<(), PipelineError> {
        self.reject_persisted_siblings_inner(connection, child, timezone, true)
    }

    fn reject_persisted_siblings_inner(
        &mut self,
        connection: &Connection,
        child: &CanonicalBucket,
        timezone: &str,
        allow_staged_csv_keys: bool,
    ) -> Result<(), PipelineError> {
        let (day_start, day_end) =
            aggregate_bounds(child.key.bucket_start, Granularity::OneDay, timezone)?;
        let validation_key = (
            child.key.source_id.clone(),
            day_start,
            allow_staged_csv_keys,
        );
        if self.persisted_sibling_validations.contains(&validation_key) {
            return Ok(());
        }
        #[cfg(test)]
        {
            self.persisted_sibling_queries += 1;
        }
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT bucket_start FROM traffic_stats
                 WHERE source_id = ?1 AND granularity = '5m'
                   AND bucket_start >= ?2 AND bucket_start < ?3
                 ORDER BY bucket_start",
            )
            .map_err(StorageError::from)?;
        let persisted = statement
            .query_map(params![child.key.source_id, day_start, day_end], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(StorageError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?;
        for bucket_start in persisted {
            if bucket_start == child.key.bucket_start
                || self
                    .owned_keys
                    .contains(&(child.key.source_id.clone(), bucket_start))
                || self
                    .current_run_keys
                    .contains(&(child.key.source_id.clone(), bucket_start))
            {
                continue;
            }
            if allow_staged_csv_keys
                && connection
                    .query_row(
                        "SELECT 1 FROM csv_bucket_stage
                         WHERE source_id = ?1 AND bucket_start = ?2 LIMIT 1",
                        params![child.key.source_id, bucket_start],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(StorageError::from)?
                    .is_some()
            {
                continue;
            }
            return Err(PipelineError::InvalidConfig(format!(
                "cannot reopen a persisted aggregate interval exactly: source={:?} bucket_start={} shares its local day with persisted five-minute bucket {bucket_start} from another transaction",
                child.key.source_id, child.key.bucket_start
            )));
        }
        // This validation is intentionally local to one aggregate transaction/output. Persisted
        // siblings cannot change except through keys owned by this run, which are already excluded
        // above, so later children in the same source/day can reuse the successful result.
        self.persisted_sibling_validations.insert(validation_key);
        Ok(())
    }

    fn include(&mut self, child: &CanonicalBucket, timezone: &str) -> Result<(), PipelineError> {
        self.include_profiled(child, timezone).map(|_| ())
    }

    fn include_profiled(
        &mut self,
        child: &CanonicalBucket,
        timezone: &str,
    ) -> Result<AggregateIncludeProfile, PipelineError> {
        let total_started = Instant::now();
        let mut profile = AggregateIncludeProfile::default();
        if self
            .published_through
            .get(&child.key.source_id)
            .is_some_and(|previous| child.key.bucket_start <= *previous)
        {
            return Err(PipelineError::InvalidConfig(format!(
                "five-minute buckets must be unique and chronological for source {:?}: {} followed {}",
                child.key.source_id,
                self.published_through[&child.key.source_id],
                child.key.bucket_start
            )));
        }
        for granularity in [
            Granularity::ThirtyMinutes,
            Granularity::OneHour,
            Granularity::OneDay,
        ] {
            let granularity_started = Instant::now();
            let bounds_started = Instant::now();
            let (start, end) = aggregate_bounds(child.key.bucket_start, granularity, timezone)?;
            let bounds_elapsed = bounds_started.elapsed();
            let key = (child.key.source_id.clone(), granularity, start, end);
            let builder_started = Instant::now();
            let builder = self.builders.entry(key.clone()).or_insert_with(|| {
                StatisticalBucket::new(BucketKey::new(&key.0, key.1, key.2, key.3))
            });
            let builder_elapsed = builder_started.elapsed();
            let bucket = builder.include_profiled(child)?;
            profile.granularity_mut(granularity).include(
                granularity_started.elapsed(),
                bounds_elapsed,
                builder_elapsed,
                bucket,
            );
        }
        self.published_through
            .insert(child.key.source_id.clone(), child.key.bucket_start);
        self.current_run_keys
            .insert((child.key.source_id.clone(), child.key.bucket_start));
        profile.total_elapsed = total_started.elapsed();
        Ok(profile)
    }

    fn flush_complete(
        &mut self,
        connection: &Connection,
        run_maad: bool,
    ) -> Result<usize, PipelineError> {
        self.flush_complete_profiled(connection, run_maad)
            .map(|(count, _)| count)
    }

    fn flush_complete_profiled(
        &mut self,
        connection: &Connection,
        run_maad: bool,
    ) -> Result<(usize, WriteBucketsProfile), PipelineError> {
        let complete_keys = self
            .builders
            .iter()
            .filter(|(_, builder)| builder.has_complete_five_minute_coverage())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let buckets = complete_keys
            .into_iter()
            .filter_map(|key| self.builders.remove(&key))
            .map(StatisticalBucket::finish_owned)
            .collect::<Vec<_>>();
        let count = buckets.len();
        let profile = write_buckets_profiled(connection, &buckets, run_maad)?;
        Ok((count, profile))
    }

    fn finish(self) -> (Vec<CanonicalBucket>, Vec<StatsBucketKey>) {
        (
            self.builders
                .into_values()
                .map(StatisticalBucket::finish_owned)
                .collect(),
            Vec::new(),
        )
    }
}

fn publish_rollups(
    connection: &Connection,
    aggregates: AggregateBuckets,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    publish_rollups_profiled(connection, aggregates, pipeline, report).map(|_| ())
}

fn publish_rollups_profiled(
    connection: &Connection,
    aggregates: AggregateBuckets,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
) -> Result<FinalRollupProfile, PipelineError> {
    let total_started = Instant::now();
    let finish_started = Instant::now();
    let (rollups, incomplete) = aggregates.finish();
    let finish_elapsed = finish_started.elapsed();
    let delete_started = Instant::now();
    delete_stats_bucket_keys(connection, &incomplete)?;
    let delete_elapsed = delete_started.elapsed();
    let write = write_buckets_profiled(connection, &rollups, pipeline.run_maad)?;
    report.rollup_buckets += rollups.len();
    Ok(FinalRollupProfile {
        total_elapsed: total_started.elapsed(),
        finish_elapsed,
        delete_elapsed,
        write,
        incomplete_keys: profile_count(incomplete.len()),
        rollup_buckets: profile_count(rollups.len()),
    })
}

/// A repaired five-minute bucket is exact, but persisted coarse unique-count
/// and MAAD rows cannot be patched from scalar results. Keep additive capture
/// coverage current and remove only the affected derived metric rows.
fn refresh_rollups_after_five_minute_repair(
    connection: &Connection,
    child: &CanonicalBucket,
    timezone: &str,
) -> Result<(), PipelineError> {
    const DERIVED_TABLES: [&str; 5] = [
        "traffic_stats",
        "protocol_stats",
        "address_count_stats",
        "port_count_stats",
        "address_structure_stats",
    ];

    for granularity in [
        Granularity::ThirtyMinutes,
        Granularity::OneHour,
        Granularity::OneDay,
    ] {
        let (start, end) = aggregate_bounds(child.key.bucket_start, granularity, timezone)?;
        for table in DERIVED_TABLES {
            connection
                .execute(
                    &format!(
                        "DELETE FROM {table}
                         WHERE source_id = ?1 AND granularity = ?2 AND bucket_start = ?3"
                    ),
                    params![child.key.source_id, granularity.as_str(), start],
                )
                .map_err(StorageError::from)?;
        }

        let children = query_bucket_coverage(
            connection,
            &child.key.source_id,
            Granularity::FiveMinutes.as_str(),
            start,
            end,
        )?;
        let expected_children =
            usize::try_from((end - start).div_euclid(FIVE_MINUTES)).unwrap_or(usize::MAX);
        if children.len() != expected_children {
            connection
                .execute(
                    "DELETE FROM bucket_coverage
                     WHERE source_id = ?1 AND granularity = ?2 AND bucket_start = ?3",
                    params![child.key.source_id, granularity.as_str(), start],
                )
                .map_err(StorageError::from)?;
            continue;
        }

        let mut coverage = BucketCoverage::empty();
        for row in children {
            coverage
                .include(row.coverage()?)
                .map_err(DomainError::from)?;
        }
        insert_bucket_coverage_rows(
            connection,
            &[BucketCoverageRow::new(
                &child.key.source_id,
                granularity.as_str(),
                start,
                end,
                coverage,
            )],
        )?;
    }
    Ok(())
}

fn aggregate_bounds(
    bucket_start: i64,
    granularity: Granularity,
    timezone: &str,
) -> Result<(i64, i64), PipelineError> {
    let timestamp = Timestamp::from_second(bucket_start)
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    let zoned = timestamp
        .in_tz(timezone)
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    let start = match granularity {
        Granularity::ThirtyMinutes => zoned
            .round(
                ZonedRound::new()
                    .smallest(Unit::Minute)
                    .increment(30)
                    .mode(RoundMode::Trunc),
            )
            .map_err(|error| PipelineError::Time(error.to_string()))?
            .timestamp()
            .as_second(),
        Granularity::OneHour => zoned
            .round(
                ZonedRound::new()
                    .smallest(Unit::Hour)
                    .mode(RoundMode::Trunc),
            )
            .map_err(|error| PipelineError::Time(error.to_string()))?
            .timestamp()
            .as_second(),
        Granularity::OneDay => zoned
            .date()
            .in_tz(timezone)
            .map_err(|error| PipelineError::Time(error.to_string()))?
            .timestamp()
            .as_second(),
        Granularity::FiveMinutes => {
            return Err(PipelineError::InvalidConfig(
                "five-minute input is not a rollup granularity".into(),
            ));
        }
    };
    let end = match granularity {
        Granularity::ThirtyMinutes => start + 1_800,
        Granularity::OneHour => start + 3_600,
        Granularity::OneDay => zoned
            .date()
            .tomorrow()
            .and_then(|date| date.in_tz(timezone))
            .map_err(|error| PipelineError::Time(error.to_string()))?
            .timestamp()
            .as_second(),
        Granularity::FiveMinutes => unreachable!("rejected above"),
    };
    Ok((start, end))
}

fn next_local_five_minute_start(bucket_start: i64, timezone: &str) -> Result<i64, PipelineError> {
    let current = Timestamp::from_second(bucket_start)
        .and_then(|timestamp| timestamp.in_tz(timezone))
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    let next = current
        .datetime()
        .checked_add(5.minutes())
        .and_then(|datetime| datetime.in_tz(timezone))
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .timestamp()
        .as_second();
    if next <= bucket_start {
        return Err(PipelineError::Time(format!(
            "local five-minute clock did not advance after {bucket_start} in {timezone:?}"
        )));
    }
    Ok(next)
}

#[derive(Clone, Copy, Debug)]
struct NfcapdTreeWindow {
    start: i64,
    end: i64,
}

/// Resolve the selected and requested nfcapd window using the same date, timezone, and alignment
/// rules for preflight, single-output processing, and coordinated planning.
fn resolve_nfcapd_tree_window(
    start_date: &str,
    end_date: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    discovered_bucket_starts: impl IntoIterator<Item = i64>,
    timezone: &str,
) -> Result<NfcapdTreeWindow, PipelineError> {
    let selected_start = parse_date_start(start_date, timezone)?;
    let explicit_end = end_date
        .map(|date| next_date_start(date, timezone))
        .transpose()?;
    let explicit_start_time = start_time
        .map(|value| parse_local_datetime(value, timezone))
        .transpose()?;
    let explicit_end_time = end_time
        .map(|value| parse_local_datetime(value, timezone))
        .transpose()?;
    let discovered_end = discovered_bucket_starts
        .into_iter()
        .max()
        .map(|start| aggregate_bounds(start, Granularity::OneDay, timezone))
        .transpose()?
        .map(|(_, end)| end)
        .unwrap_or(selected_start);
    let selected_end = explicit_end.unwrap_or(discovered_end);
    let start = explicit_start_time.unwrap_or(selected_start);
    let end = explicit_end_time.unwrap_or(selected_end);
    validate_window(selected_start, selected_end, start, end, timezone)?;
    Ok(NfcapdTreeWindow { start, end })
}

fn parse_date_start(raw: &str, timezone: &str) -> Result<i64, PipelineError> {
    let date: Date = raw
        .parse()
        .map_err(|error: jiff::Error| PipelineError::Time(error.to_string()))?;
    Ok(date
        .in_tz(timezone)
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .timestamp()
        .as_second())
}

fn next_date_start(raw: &str, timezone: &str) -> Result<i64, PipelineError> {
    let date: Date = raw
        .parse()
        .map_err(|error: jiff::Error| PipelineError::Time(error.to_string()))?;
    Ok(date
        .tomorrow()
        .and_then(|date| date.in_tz(timezone))
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .timestamp()
        .as_second())
}

fn parse_local_datetime(raw: &str, timezone: &str) -> Result<i64, PipelineError> {
    let normalized = if raw.len() == 16 {
        format!("{raw}:00")
    } else {
        raw.to_owned()
    };
    let datetime = normalized
        .parse::<jiff::civil::DateTime>()
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    Ok(datetime
        .in_tz(timezone)
        .map_err(|error| PipelineError::Time(error.to_string()))?
        .timestamp()
        .as_second())
}

fn validate_window(
    selected_start: i64,
    selected_end: i64,
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<(), PipelineError> {
    if start < selected_start {
        return Err(PipelineError::InvalidConfig(
            "start_time must be on or after the selected start_date".into(),
        ));
    }
    if end > selected_end {
        return Err(PipelineError::InvalidConfig(
            "end_time must be on or before the selected end_date window".into(),
        ));
    }
    if start >= end {
        return Err(PipelineError::InvalidConfig(
            "input time window must be non-empty".into(),
        ));
    }
    for (label, value) in [("start_time", start), ("end_time", end)] {
        if aggregate_bounds(value, Granularity::OneDay, timezone)?.0 != value {
            return Err(PipelineError::InvalidConfig(format!(
                "{label} must align to a local-day boundary so aggregate rows stay complete"
            )));
        }
    }
    Ok(())
}

fn expected_nfcapd_path(
    root: &Path,
    member: &str,
    bucket_start: i64,
    timezone: &str,
) -> Result<PathBuf, PipelineError> {
    let timestamp = Timestamp::from_second(bucket_start)
        .and_then(|timestamp| timestamp.in_tz(timezone))
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    Ok(root
        .join(member)
        .join(timestamp.strftime("%Y").to_string())
        .join(timestamp.strftime("%m").to_string())
        .join(timestamp.strftime("%d").to_string())
        .join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr},
    };

    use rusqlite::{Connection, types::ValueRef};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        coverage::CoverageState,
        domain::{AddressSide, FlowObservation, IpVersion, Scope, Visibility},
        storage::database_operation_lock_path,
    };

    fn write_fake_nfdump(executable: &Path, setup: &str) {
        let stream = executable.with_extension("stream");
        let empty_stream = executable.with_extension("empty.stream");
        fs::write(&stream, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            &empty_stream,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then\ncat '{}'\nexit 0\nfi\n{setup}\ncat '{}'\n",
                empty_stream.display(),
                stream.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[cfg(unix)]
    fn write_nfcapd_day(root: &Path) {
        write_nfcapd_day_for_date(root, "2025-06-01");
    }

    #[cfg(unix)]
    fn write_nfcapd_day_for_date(root: &Path, date: &str) {
        let date_path = date.replace('-', "/");
        let day = root.join(format!("edge/{date_path}"));
        fs::create_dir_all(&day).unwrap();
        let day_start = parse_date_start(date, DEFAULT_TIMEZONE).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(day_start + bucket * FIVE_MINUTES)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
    }

    #[cfg(unix)]
    fn replace_member_directory(root: &Path) -> PathBuf {
        let member = root.join("edge");
        let previous = root.join("edge-before-replacement");
        fs::rename(&member, &previous).unwrap();
        fs::create_dir(&member).unwrap();
        previous
    }

    #[cfg(unix)]
    fn repeated_dataset_request(
        temporary: &tempfile::TempDir,
        nfdump: PathBuf,
    ) -> (PipelineRequest, PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output_directory = temporary.path().join("outputs");
        let first_database = output_directory.join("first.sqlite");
        let second_database = output_directory.join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        let sentinel = temporary.path().join("sentinel.txt");
        fs::write(&sentinel, b"leave this alone").unwrap();
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": temporary.path().join("captures"),
                    "db_path": second_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        (
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: nfdump.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            output_directory,
            first_database,
            second_database,
            sentinel,
        )
    }

    #[test]
    fn daily_active_sources_rejects_inputs_without_a_tree_day_cohort() {
        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "0.220.0.0/16"
        })))
        .unwrap();
        let csv = InputSpec::Csv {
            path: "flows.csv".into(),
            mapping_path: "mapping.json".into(),
        };
        let explicit = InputSpec::Nfcapd {
            path: "nfcapd.202501010000".into(),
            source_id: "edge".into(),
            bucket_start: None,
            gap: false,
            expected_path: None,
        };

        assert!(validate_selection_inputs(&selection, &[csv]).is_err());
        assert!(validate_selection_inputs(&selection, &[explicit]).is_err());
        assert!(
            validate_selection_inputs(
                &selection,
                &[
                    InputSpec::NfcapdTree {
                        root_path: "captures".into(),
                        source_ids: vec!["edge".into()],
                        sources: Vec::new(),
                        start_date: "2025-01-01".into(),
                        end_date: Some("2025-01-01".into()),
                        start_time: None,
                        end_time: None,
                        force: false,
                    },
                    InputSpec::CsvTree {
                        root_path: "csv".into(),
                        mapping_path: "mapping.json".into(),
                    },
                ],
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn auto_discovered_nfcapd_root_rejects_output_that_would_create_a_member_directory() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir(&root).unwrap();
        let database = root.join("new-member/netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": DEFAULT_TIMEZONE,
                "inputs": [{
                    "input_kind": "nfcapd_tree",
                    "root_path": root,
                    "start_date": "2025-06-01",
                    "end_date": "2025-06-01"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let error = run(PipelineRequest::config(&config)).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("direct-child directory"), "{message}");
        assert!(!database.parent().unwrap().exists());
    }

    #[test]
    fn dataset_mode_applies_its_persisted_selection() {
        let temporary = tempdir().unwrap();
        let registry = temporary.path().join("datasets.json");
        let database = temporary.path().join("active.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": temporary.path(),
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {
                    "kind": "daily_active_sources",
                    "ip_prefix": "72.5.0.0/16"
                }
            }]))
            .unwrap(),
        )
        .unwrap();
        let resolved = resolve_request(&PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: "nfdump".into(),
            force: false,
            run_maad: true,
            require_complete: false,
        })
        .unwrap();

        assert!(resolved.selection.selects_daily_active_sources());
        assert_eq!(resolved.database_path, database);
    }

    #[cfg(unix)]
    #[test]
    fn nfcapd_member_aliases_are_rejected_before_output_mutation() {
        use std::os::unix::fs::symlink;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let real_member = root.join("real-member");
        fs::create_dir_all(&real_member).unwrap();
        let capture = real_member.join("2025/06/01/nfcapd.202506010000");
        fs::create_dir_all(capture.parent().unwrap()).unwrap();
        fs::write(&capture, b"capture bytes").unwrap();
        symlink("real-member", root.join("edge-a")).unwrap();
        symlink("real-member", root.join("edge-b")).unwrap();
        let output = temporary.path().join("output.sqlite");

        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: vec!["edge-a".into(), "edge-b".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let before = fs::read(&capture).unwrap();
        let error = execute(pipeline).unwrap_err();

        assert!(error.to_string().contains("same directory"));
        assert_eq!(fs::read(capture).unwrap(), before);
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn repeated_dataset_missing_explicit_nfdump_is_side_effect_free() {
        let temporary = tempdir().unwrap();
        let missing = temporary.path().join("tools/missing-nfdump");
        let (request, output_directory, first_database, second_database, sentinel) =
            repeated_dataset_request(&temporary, missing);
        let sentinel_before = fs::read(&sentinel).unwrap();

        let error = run_many(request, vec!["first".into(), "second".into()]).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("explicit nfdump executable"), "{message}");
        assert!(message.contains("missing-nfdump"), "{message}");
        assert!(!output_directory.exists());
        for database in [&first_database, &second_database] {
            assert!(!database.exists());
            assert!(!database_operation_lock_path(database).unwrap().exists());
        }
        assert_eq!(fs::read(sentinel).unwrap(), sentinel_before);
    }

    #[cfg(unix)]
    #[test]
    fn repeated_dataset_non_executable_explicit_nfdump_is_side_effect_free() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let non_executable = temporary.path().join("nfdump-not-executable");
        fs::write(&non_executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o644)).unwrap();
        let (request, output_directory, first_database, second_database, sentinel) =
            repeated_dataset_request(&temporary, non_executable);
        let sentinel_before = fs::read(&sentinel).unwrap();

        let error = run_many(request, vec!["first".into(), "second".into()]).unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("not executable by this process"),
            "{message}"
        );
        assert!(!output_directory.exists());
        for database in [&first_database, &second_database] {
            assert!(!database.exists());
            assert!(!database_operation_lock_path(database).unwrap().exists());
        }
        assert_eq!(fs::read(sentinel).unwrap(), sentinel_before);
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_mode_loads_one_registry_snapshot_for_all_datasets() {
        let temporary = tempdir().unwrap();
        let first_root = temporary.path().join("first-captures");
        let second_root = temporary.path().join("second-captures");
        fs::create_dir_all(first_root.join("edge")).unwrap();
        fs::create_dir_all(second_root.join("edge")).unwrap();
        let first_database = temporary.path().join("first.sqlite");
        let second_database = temporary.path().join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": first_root,
                    "db_path": first_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": second_root,
                    "db_path": second_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        let nfdump = temporary.path().join("nfdump");
        write_fake_nfdump(&nfdump, "");
        let request = PipelineRequest {
            config_path: None,
            dataset_id: None,
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        };

        reset_dataset_registry_load_calls();
        let error = run_many(request, vec!["first".into(), "second".into()]).unwrap_err();

        assert!(error.to_string().contains("same nfcapd root"), "{error}");
        assert_eq!(dataset_registry_load_calls(), 1);
    }

    #[test]
    fn coordinated_mode_rejects_duplicate_and_incompatible_datasets() {
        let temporary = tempdir().unwrap();
        let first_root = temporary.path().join("first");
        let second_root = temporary.path().join("second");
        fs::create_dir_all(first_root.join("edge")).unwrap();
        fs::create_dir_all(second_root.join("edge")).unwrap();
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": first_root,
                    "db_path": temporary.path().join("first.sqlite"),
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "72.5.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": second_root,
                    "db_path": temporary.path().join("second.sqlite"),
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "72.6.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        let request = PipelineRequest {
            config_path: None,
            dataset_id: None,
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: "nfdump".into(),
            force: false,
            run_maad: true,
            require_complete: false,
        };

        assert!(run_many(request.clone(), vec!["first".into(), "first".into()]).is_err());
        assert!(run_many(request, vec!["first".into(), "second".into()]).is_err());
    }

    fn empty_coordinated_pipeline(database_path: PathBuf) -> ResolvedPipeline {
        ResolvedPipeline {
            database_path,
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: Vec::new(),
            datasets: Vec::new(),
            require_complete: false,
        }
    }

    fn semantic_table_rows(connection: &Connection, table: &str) -> Vec<Vec<String>> {
        let mut columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        columns.sort_unstable_by_key(|(index, _)| *index);
        let semantic = columns
            .into_iter()
            .filter(|(_, column)| {
                !matches!(
                    column.as_str(),
                    "bound_at" | "discovered_at" | "processed_at"
                )
            })
            .collect::<Vec<_>>();
        let selected = semantic
            .iter()
            .map(|(_, column)| column.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let order = semantic
            .iter()
            .map(|(_, column)| column.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection
            .prepare(&format!("SELECT {selected} FROM {table} ORDER BY {order}"))
            .unwrap();
        statement
            .query_map([], |row| {
                (0..semantic.len())
                    .map(|index| match row.get_ref(index)? {
                        ValueRef::Null => Ok("NULL".into()),
                        ValueRef::Integer(value) => Ok(value.to_string()),
                        ValueRef::Real(value) => Ok(value.to_string()),
                        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
                        ValueRef::Blob(value) => Ok(value
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()),
                    })
                    .collect()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn coordinated_semantic_snapshot(connection: &Connection) -> Vec<Vec<Vec<String>>> {
        [
            "pipeline_product",
            "nfcapd_source_layout",
            "datasets",
            "source_members",
            "bucket_coverage",
            "traffic_stats",
            "protocol_stats",
            "address_count_stats",
            "port_count_stats",
            "address_structure_stats",
            "processed_inputs",
            "input_evidence",
        ]
        .into_iter()
        .map(|table| semantic_table_rows(connection, table))
        .collect()
    }

    #[test]
    fn coordinated_output_aliases_are_rejected_before_filesystem_mutation() {
        let temporary = tempdir().unwrap();
        let target = temporary.path().join("target.sqlite");
        let operation_lock = database_operation_lock_path(&target).unwrap();
        for alias in [
            target.with_file_name("target.sqlite-wal"),
            operation_lock.clone(),
        ] {
            let error = execute_many(vec![
                empty_coordinated_pipeline(target.clone()),
                empty_coordinated_pipeline(alias),
            ])
            .unwrap_err();
            assert!(error.to_string().contains("must be distinct"));
            assert!(!target.exists());
            assert!(!operation_lock.exists());
        }

        let normalized_parent = temporary.path().join("normalized");
        let normalized = normalized_parent.join("..").join("normalized.sqlite");
        let normalized_target = temporary.path().join("normalized.sqlite");
        let error = execute_many(vec![
            empty_coordinated_pipeline(normalized_target.clone()),
            empty_coordinated_pipeline(normalized),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("must be distinct"));
        assert!(!normalized_parent.exists());
        assert!(!normalized_target.exists());

        let target = temporary.path().join("nested-target.sqlite");
        let target_related = [
            target.clone(),
            target.with_file_name("nested-target.sqlite-wal"),
            database_operation_lock_path(&target).unwrap(),
        ];
        for ancestor in target_related {
            let descendant = ancestor.join("second.sqlite");
            let error = execute_many(vec![
                empty_coordinated_pipeline(target.clone()),
                empty_coordinated_pipeline(descendant.clone()),
            ])
            .unwrap_err();
            assert!(error.to_string().contains("must be distinct"), "{error}");
            assert!(!target.exists());
            assert!(!descendant.exists());
            assert!(!ancestor.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_output_symlink_and_hard_link_aliases_are_rejected_before_mutation() {
        use std::{fs::hard_link, os::unix::fs::symlink};

        let temporary = tempdir().unwrap();
        let target = temporary.path().join("target.sqlite");
        fs::write(&target, b"existing database bytes").unwrap();
        let target_lock = database_operation_lock_path(&target).unwrap();

        let symlink_alias = temporary.path().join("symlink.sqlite");
        symlink(&target, &symlink_alias).unwrap();
        let error = execute_many(vec![
            empty_coordinated_pipeline(target.clone()),
            empty_coordinated_pipeline(symlink_alias.clone()),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("must be distinct"));
        assert_eq!(fs::read(&target).unwrap(), b"existing database bytes");
        assert!(!target_lock.exists());
        assert!(symlink_alias.is_symlink());

        let hard_link_alias = temporary.path().join("hard-link.sqlite");
        hard_link(&target, &hard_link_alias).unwrap();
        let error = execute_many(vec![
            empty_coordinated_pipeline(target.clone()),
            empty_coordinated_pipeline(hard_link_alias.clone()),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("must be distinct"));
        assert_eq!(fs::read(&target).unwrap(), b"existing database bytes");
        assert_eq!(
            fs::read(&hard_link_alias).unwrap(),
            b"existing database bytes"
        );
        assert!(!target_lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn output_capture_aliases_are_rejected_without_mutating_capture_bytes() {
        use std::{fs::hard_link, os::unix::fs::symlink};

        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202506010000");
        fs::write(&capture, b"capture bytes").unwrap();
        let output = temporary.path().join("output.sqlite");
        let sidecar = output.with_file_name("output.sqlite-wal");
        symlink(&capture, &sidecar).unwrap();

        for alias in [capture.clone(), sidecar.clone()] {
            let error =
                validate_output_capture_separation(&[&alias], std::iter::once(capture.as_path()))
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("aliases discovered nfcapd capture")
            );
            assert_eq!(fs::read(&capture).unwrap(), b"capture bytes");
        }

        let hard_link_path = temporary.path().join("hard-link.sqlite");
        hard_link(&capture, &hard_link_path).unwrap();
        let error = validate_output_capture_separation(
            &[&hard_link_path],
            std::iter::once(capture.as_path()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("device/inode"));
        assert_eq!(fs::read(&capture).unwrap(), b"capture bytes");
    }

    #[test]
    fn output_separation_streams_input_paths_until_a_conflict() {
        struct OneInputThenPanic<'a> {
            input: Option<&'a Path>,
        }

        impl<'a> Iterator for OneInputThenPanic<'a> {
            type Item = &'a Path;

            fn next(&mut self) -> Option<Self::Item> {
                self.input
                    .take()
                    .or_else(|| panic!("input separation collected the entire iterator"))
            }
        }

        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202506010000");
        fs::write(&capture, b"capture bytes").unwrap();
        let error = validate_output_capture_separation(
            &[&capture],
            OneInputThenPanic {
                input: Some(capture.as_path()),
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("aliases discovered nfcapd capture")
        );
    }

    #[test]
    fn nfcapd_tree_output_validation_skips_capture_metadata_for_new_outputs() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let namespaces = nfcapd_locator_namespaces(&root, &["edge".into()]).unwrap();
        let output = temporary.path().join("pipeline.sqlite");
        let captures = (0..4_096)
            .map(|index| {
                root.join(format!(
                    "edge/2025/06/01/nfcapd.202506{:06}",
                    index % 100_000
                ))
            })
            .collect::<Vec<_>>();

        reset_nfcapd_capture_identity_calls();
        validate_output_nfcapd_capture_separation(
            &[&output],
            &namespaces,
            "UTC",
            captures.iter().map(PathBuf::as_path),
        )
        .unwrap();
        assert_eq!(
            nfcapd_capture_identity_calls(),
            0,
            "a new output has no inode that can require a physical capture scan"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nfcapd_tree_output_validation_scans_capture_inodes_only_for_existing_output() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let namespaces = nfcapd_locator_namespaces(&root, &["edge".into()]).unwrap();
        let output = temporary.path().join("pipeline.sqlite");
        fs::write(&output, b"existing output").unwrap();
        let captures = (0..64)
            .map(|index| {
                let path = temporary.path().join(format!("capture-{index}.nfcapd"));
                fs::write(&path, format!("capture-{index}")).unwrap();
                path
            })
            .collect::<Vec<_>>();

        reset_nfcapd_capture_identity_calls();
        validate_output_nfcapd_capture_separation(
            &[&output],
            &namespaces,
            "UTC",
            captures.iter().map(PathBuf::as_path),
        )
        .unwrap();
        assert_eq!(nfcapd_capture_identity_calls(), captures.len());

        let hard_link_output = temporary.path().join("hard-link.sqlite");
        fs::hard_link(&captures[0], &hard_link_output).unwrap();
        reset_nfcapd_capture_identity_calls();
        let error = validate_output_nfcapd_capture_separation(
            &[&hard_link_output],
            &namespaces,
            "UTC",
            captures.iter().map(PathBuf::as_path),
        )
        .unwrap_err();
        assert!(error.to_string().contains("device/inode"));
        assert_eq!(nfcapd_capture_identity_calls(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn existing_output_reuses_one_bounded_capture_snapshot_for_alias_and_revision_work() {
        let temporary = tempdir().unwrap();
        let output = temporary.path().join("pipeline.sqlite");
        fs::write(&output, b"existing output").unwrap();
        let captures = (0..4_096)
            .map(|index| {
                let path = temporary.path().join(format!("capture-{index}.nfcapd"));
                fs::write(&path, format!("capture-{index}")).unwrap();
                path
            })
            .collect::<BTreeSet<_>>();

        reset_nfcapd_capture_identity_calls();
        let snapshot_calls = AtomicUsize::new(0);
        let snapshots = capture_nfcapd_snapshots_counted(&captures, &snapshot_calls).unwrap();
        assert_eq!(snapshot_calls.load(Ordering::Relaxed), captures.len());
        validate_output_nfcapd_capture_separation_with_snapshots(
            &[&output],
            &[],
            snapshots
                .iter()
                .map(|(path, snapshot)| (path.as_path(), snapshot)),
        )
        .unwrap();
        assert_eq!(
            nfcapd_capture_identity_calls(),
            0,
            "snapshot-backed alias validation must not run a second serial metadata pass"
        );

        let capture = captures.iter().next().unwrap().clone();
        let sources = [DatasetSource {
            source_id: "r1".into(),
            members: vec!["r1".into()],
        }];
        let paths = BTreeMap::from([(("r1".into(), 0), capture.clone())]);
        let bounds = BTreeMap::from([("r1".into(), (0, 0))]);
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let context = NfcapdRevisionContext {
            connection: &connection,
            sources: &sources,
            by_member_and_start: &paths,
            member_bounds: &bounds,
            extend_gaps_to_window: false,
            force: false,
            decoder_fingerprint: "decoder".into(),
            capture_snapshots: &snapshots,
            revision_pool: &pool,
        };
        resolve_nfcapd_batch_revisions(&context, &[0]).unwrap();

        let hard_link_output = temporary.path().join("hard-link.sqlite");
        fs::hard_link(&capture, &hard_link_output).unwrap();
        let error = validate_output_nfcapd_capture_separation_with_snapshots(
            &[&hard_link_output],
            &[],
            snapshots
                .iter()
                .map(|(path, snapshot)| (path.as_path(), snapshot)),
        )
        .unwrap_err();
        assert!(error.to_string().contains("device/inode"), "{error}");
        fs::remove_file(hard_link_output).unwrap();

        fs::write(&capture, b"capture changed").unwrap();
        let error = resolve_nfcapd_batch_revisions(&context, &[0]).unwrap_err();
        assert!(
            error.to_string().contains("changed while"),
            "revision preparation must reuse the original snapshot: {error}"
        );
    }

    #[test]
    fn auto_discovered_tree_rejects_a_new_member_locator_before_mutation() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(&root).unwrap();
        let output = root.join("new-member/2025/06/01/nfcapd.202506010000");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: Vec::new(),
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = execute(pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("auto-discovered member namespace")
        );
        assert!(!output.exists());
        assert!(!root.join("new-member").exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn configured_nfcapd_member_rejects_output_in_a_year_directory() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output = root.join("edge/2025");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = execute(pipeline).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("configured nfcapd member namespace"),
            "{message}"
        );
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn auto_discovered_tree_rejects_a_future_member_directory() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(&root).unwrap();
        let output = root.join("future-member");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: Vec::new(),
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = execute(pipeline).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("auto-discovered member namespace"),
            "{message}"
        );
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn nfcapd_namespace_aliases_are_rejected_before_output_mutation() {
        use std::os::unix::fs::symlink;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let configured_target = root.join("edge/2025");
        let configured_alias = temporary.path().join("configured-alias.sqlite");
        symlink(&configured_target, &configured_alias).unwrap();

        let configured_pipeline = ResolvedPipeline {
            database_path: configured_alias.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };
        let error = execute(configured_pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("configured nfcapd member namespace")
        );
        assert!(configured_alias.is_symlink());
        assert!(
            !database_operation_lock_path(&configured_alias)
                .unwrap()
                .exists()
        );

        let auto_target = root.join("future-member");
        let auto_alias = temporary.path().join("auto-alias.sqlite");
        symlink(&auto_target, &auto_alias).unwrap();
        let auto_pipeline = ResolvedPipeline {
            database_path: auto_alias.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: Vec::new(),
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };
        let error = execute(auto_pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("auto-discovered member namespace")
        );
        assert!(auto_alias.is_symlink());
        assert!(!database_operation_lock_path(&auto_alias).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn csv_and_mapping_aliases_are_rejected_before_mutation() {
        use std::{fs::hard_link, os::unix::fs::symlink};

        let temporary = tempdir().unwrap();
        let input = temporary.path().join("flows.csv");
        let mapping = temporary.path().join("mapping.json");
        fs::write(&input, b"CSV input bytes").unwrap();
        fs::write(&mapping, b"mapping bytes").unwrap();

        let mut aliases = vec![input.clone(), mapping.clone()];
        for (index, target) in [(&input, "input"), (&mapping, "mapping")] {
            let symlink_alias = temporary.path().join(format!("{target}-symlink.sqlite"));
            symlink(index, &symlink_alias).unwrap();
            aliases.push(symlink_alias);
            let hard_link_alias = temporary.path().join(format!("{target}-hard-link.sqlite"));
            hard_link(index, &hard_link_alias).unwrap();
            aliases.push(hard_link_alias);
        }

        for output in aliases {
            let pipeline = ResolvedPipeline {
                database_path: output.clone(),
                control_paths: Vec::new(),
                timezone: "UTC".into(),
                run_maad: false,
                nfdump: "nfdump".into(),
                nfdump_revision: None,
                selection: FlowSelection::default(),
                inputs: vec![InputSpec::Csv {
                    path: input.clone(),
                    mapping_path: mapping.clone(),
                }],
                datasets: Vec::new(),
                require_complete: false,
            };
            let error = execute(pipeline).unwrap_err();
            assert!(error.to_string().contains("aliases discovered CSV input"));
            assert_eq!(fs::read(&input).unwrap(), b"CSV input bytes");
            assert_eq!(fs::read(&mapping).unwrap(), b"mapping bytes");
            for suffix in ["-journal", "-wal", "-shm"] {
                assert!(
                    !output
                        .with_file_name(format!(
                            "{}{}",
                            output.file_name().unwrap().to_string_lossy(),
                            suffix
                        ))
                        .exists()
                );
            }
            assert!(!database_operation_lock_path(&output).unwrap().exists());
        }
    }

    #[test]
    fn csv_tree_rejects_an_output_that_would_be_discovered_after_creation() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("csv-tree");
        fs::create_dir_all(&root).unwrap();
        let mapping = temporary.path().join("mapping.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {
                    "time_end": "time",
                    "src_ip": "src",
                    "dst_ip": "dst"
                },
                "source_id": {"value": "r1"},
                "discovery": {"include_suffixes": [".csv"]}
            }))
            .unwrap(),
        )
        .unwrap();
        let output = root.join("new-output.csv");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::CsvTree {
                root_path: root.clone(),
                mapping_path: mapping.clone(),
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = execute(pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("would be discovered as a CSV tree input")
        );
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
        assert_eq!(
            fs::read(&mapping).unwrap(),
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {
                    "time_end": "time",
                    "src_ip": "src",
                    "dst_ip": "dst"
                },
                "source_id": {"value": "r1"},
                "discovery": {"include_suffixes": [".csv"]}
            }))
            .unwrap()
        );
    }

    #[test]
    fn single_daily_active_preflight_rejects_an_absent_expected_capture_alias() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output = root.join("edge/2025/06/01/nfcapd.202506010000");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "192.0.0.0/16"
            })))
            .unwrap(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = preflight_single_output(&pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("aliases discovered nfcapd capture")
        );
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn single_tree_preflight_rejects_missing_locator_in_finite_window() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output = root.join("edge/2025/06/01/nfcapd.202506010000");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = preflight_single_output(&pipeline).unwrap_err();
        assert!(error.to_string().contains("nfcapd capture locator"));
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn single_tree_preflight_rejects_open_ended_future_successor_locator() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output = root.join("edge/2025/06/02/nfcapd.202506020000");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: None,
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = preflight_single_output(&pipeline).unwrap_err();
        assert!(error.to_string().contains("nfcapd capture locator"));
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn explicit_gap_expected_locator_is_checked_before_output_setup() {
        let temporary = tempdir().unwrap();
        let expected_path = temporary
            .path()
            .join("captures/edge/2025/06/01/nfcapd.202506010000");
        let output = expected_path.clone();
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::Nfcapd {
                path: temporary.path().join("gap-marker"),
                source_id: "edge".into(),
                bucket_start: Some(1_735_689_600),
                gap: true,
                expected_path: Some(expected_path),
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        let error = preflight_single_output(&pipeline).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("aliases discovered nfcapd capture")
        );
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn one_year_ten_member_preflight_does_not_materialize_capture_paths() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let members = (0..10).map(|index| format!("edge-{index}"));
        for member in members.clone() {
            fs::create_dir_all(root.join(member)).unwrap();
        }
        let output = temporary.path().join("pipeline.sqlite");
        let pipeline = ResolvedPipeline {
            database_path: output.clone(),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "192.0.0.0/16"
            })))
            .unwrap(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: members.collect(),
                sources: Vec::new(),
                start_date: "2025-01-01".into(),
                end_date: Some("2026-01-01".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        };

        preflight_single_output(&pipeline).unwrap();
        assert!(!output.exists());
        assert!(!database_operation_lock_path(&output).unwrap().exists());
    }

    #[test]
    fn coordinated_metadata_conflict_rolls_back_earlier_output_initialization() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let first_database = temporary.path().join("first.sqlite");
        let second_database = temporary.path().join("second.sqlite");
        let selection_a = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "192.0.0.0/16"
        })))
        .unwrap();
        let selection_b = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "198.51.0.0/16"
        })))
        .unwrap();
        let pipeline_for =
            |database_path: PathBuf, dataset_id: &str, label: &str, selection: FlowSelection| {
                let dataset = Dataset {
                    dataset_id: dataset_id.into(),
                    label: label.into(),
                    root_path: root.clone(),
                    db_path: database_path.clone(),
                    default_start_date: String::new(),
                    source_mode: "static".into(),
                    discovery_mode: "static".into(),
                    sort_order: 0,
                    source_ids: vec!["edge".into()],
                    sources: Vec::new(),
                    selection: selection.normalized_payload(),
                };
                ResolvedPipeline {
                    database_path,
                    control_paths: Vec::new(),
                    timezone: "UTC".into(),
                    run_maad: false,
                    nfdump: "nfdump".into(),
                    nfdump_revision: None,
                    selection,
                    inputs: vec![InputSpec::NfcapdTree {
                        root_path: root.clone(),
                        source_ids: vec!["edge".into()],
                        sources: Vec::new(),
                        start_date: "1970-01-01".into(),
                        end_date: Some("1970-01-01".into()),
                        start_time: None,
                        end_time: None,
                        force: false,
                    }],
                    datasets: vec![dataset],
                    require_complete: false,
                }
            };
        let first_existing = pipeline_for(
            first_database.clone(),
            "first",
            "before",
            selection_a.clone(),
        );
        let second_existing = pipeline_for(
            second_database.clone(),
            "second",
            "second",
            selection_a.clone(),
        );
        for pipeline in [&first_existing, &second_existing] {
            let lock = DatabaseOperationLock::acquire(&pipeline.database_path, "test").unwrap();
            let connection = connect_pipeline_writer(&pipeline.database_path).unwrap();
            init_schema(&connection).unwrap();
            initialize_metadata(&connection, pipeline).unwrap();
            drop(connection);
            drop(lock);
        }
        let before = coordinated_semantic_snapshot(&Connection::open(&first_database).unwrap());

        let first_requested = pipeline_for(first_database.clone(), "first", "after", selection_a);
        let second_requested =
            pipeline_for(second_database.clone(), "second", "second", selection_b);
        let error = execute_many(vec![first_requested, second_requested]).unwrap_err();
        assert!(error.to_string().contains("second"));
        assert!(
            error
                .to_string()
                .contains(second_database.to_string_lossy().as_ref())
        );
        assert_eq!(
            coordinated_semantic_snapshot(&Connection::open(first_database).unwrap()),
            before
        );
    }

    #[test]
    fn coordinated_invalid_finite_windows_leave_output_paths_uncreated() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output_directory = temporary.path().join("outputs");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": output_directory.join("first.sqlite"),
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": output_directory.join("second.sqlite"),
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        for (start_date, end_date) in [("2025-99-01", "2025-10-01"), ("2025-10-02", "2025-10-01")] {
            let error = run_many(
                PipelineRequest {
                    config_path: None,
                    dataset_id: None,
                    datasets_path: Some(registry.clone()),
                    start_date: Some(start_date.into()),
                    end_date: Some(end_date.into()),
                    start_time: None,
                    end_time: None,
                    database_path: None,
                    selection: Value::Null,
                    nfdump: "./missing-nfdump".into(),
                    force: false,
                    run_maad: false,
                    require_complete: false,
                },
                vec!["first".into(), "second".into()],
            )
            .unwrap_err();
            assert!(matches!(
                error,
                PipelineError::Time(_) | PipelineError::InvalidConfig(_)
            ));
            assert!(!output_directory.exists());
        }
    }

    #[test]
    fn coordinated_auto_discovery_protects_root_regardless_of_dataset_order() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let configured_database = temporary.path().join("configured.sqlite");
        let auto_database = root.join("new-member/netflow.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "configured",
                    "root_path": root,
                    "db_path": configured_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "auto",
                    "root_path": root,
                    "db_path": auto_database,
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        for order in [
            vec!["configured".to_owned(), "auto".to_owned()],
            vec!["auto".to_owned(), "configured".to_owned()],
        ] {
            let error = run_many(
                PipelineRequest {
                    config_path: None,
                    dataset_id: None,
                    datasets_path: Some(registry.clone()),
                    start_date: Some("2025-06-01".into()),
                    end_date: Some("2025-06-01".into()),
                    start_time: None,
                    end_time: None,
                    database_path: None,
                    selection: Value::Null,
                    nfdump: nfdump.to_string_lossy().into_owned(),
                    force: false,
                    run_maad: false,
                    require_complete: false,
                },
                order,
            )
            .unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("auto-discovered member namespace")
            );
            assert!(!configured_database.exists());
            assert!(!auto_database.exists());
            assert!(!auto_database.parent().unwrap().exists());
            assert!(
                !database_operation_lock_path(&configured_database)
                    .unwrap()
                    .exists()
            );
            assert!(
                !database_operation_lock_path(&auto_database)
                    .unwrap()
                    .exists()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_auto_layout_change_after_planning_is_side_effect_free_in_both_orders() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output_directory = temporary.path().join("outputs");
        let first_database = output_directory.join("first.sqlite");
        let second_database = output_directory.join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "configured",
                    "root_path": root,
                    "db_path": first_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "auto",
                    "root_path": temporary.path().join("captures"),
                    "db_path": second_database,
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        let request = || PipelineRequest {
            config_path: None,
            dataset_id: None,
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        };

        for order in [
            vec!["configured".to_owned(), "auto".to_owned()],
            vec!["auto".to_owned(), "configured".to_owned()],
        ] {
            let added_member = root.join("edge-b");
            set_coordinated_plan_hook(move |planned_root| {
                fs::create_dir_all(planned_root.join("edge-b")).unwrap();
            });
            let error = run_many(request(), order).unwrap_err();
            clear_coordinated_plan_hook();

            assert!(
                error
                    .to_string()
                    .contains("auto-discovered source layout changed"),
                "{error}"
            );
            assert!(!output_directory.exists());
            for database in [&first_database, &second_database] {
                assert!(!database.exists());
                assert!(!database_operation_lock_path(database).unwrap().exists());
            }
            fs::remove_dir(added_member).unwrap();
        }
    }

    #[test]
    fn coordinated_revision_resolution_reuses_a_digest_from_one_output() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let member = root.join("edge");
        fs::create_dir_all(&member).unwrap();
        let capture = member.join("nfcapd.197001010000");
        fs::write(&capture, b"capture").unwrap();
        let sources = vec![DatasetSource {
            source_id: "edge".into(),
            members: vec!["edge".into()],
        }];
        let mut paths = BTreeMap::new();
        paths.insert(("edge".into(), 0), capture.clone());
        let mut outputs = Vec::new();
        for name in ["first.sqlite", "second.sqlite"] {
            let database_path = temporary.path().join(name);
            let lock = DatabaseOperationLock::acquire(&database_path, "test").unwrap();
            let connection = connect_pipeline_writer(&database_path).unwrap();
            init_schema(&connection).unwrap();
            outputs.push(CoordinatedOutput {
                pipeline: ResolvedPipeline {
                    database_path,
                    control_paths: Vec::new(),
                    timezone: "UTC".into(),
                    run_maad: false,
                    nfdump: "nfdump".into(),
                    nfdump_revision: None,
                    selection: FlowSelection::default(),
                    inputs: vec![InputSpec::NfcapdTree {
                        root_path: root.clone(),
                        source_ids: vec!["edge".into()],
                        sources: Vec::new(),
                        start_date: "1970-01-01".into(),
                        end_date: Some("1970-01-01".into()),
                        start_time: None,
                        end_time: None,
                        force: false,
                    }],
                    datasets: Vec::new(),
                    require_complete: false,
                },
                sources: sources.clone(),
                connection,
                _lock: lock,
            });
        }

        let observed = FileSnapshot::capture(&capture).unwrap();
        let cached_revision = InputRevision::create(
            "nfcapd",
            capture.to_string_lossy(),
            "cached-content",
            "decoder",
        )
        .unwrap();
        upsert_input_bucket(
            &outputs[0].connection,
            &InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: cached_revision.locator.clone(),
                scan_locator: cached_revision.locator.clone(),
                source_id: "edge".into(),
                bucket_start: 0,
                bucket_end: FIVE_MINUTES,
                revision: cached_revision.clone(),
                file_snapshot: Some(observed),
            },
            false,
        )
        .unwrap();
        mark_input_bucket_status(
            &outputs[0].connection,
            InputKind::Nfcapd,
            &cached_revision.locator,
            "edge",
            0,
            InputStatus::Processed,
            &cached_revision,
            None,
        )
        .unwrap();

        let revision_pool = build_revision_hash_pool().unwrap();
        let revisions = resolve_coordinated_batch_revisions(
            &outputs,
            &sources,
            &paths,
            &BTreeMap::from([("edge".into(), (0, 0))]),
            false,
            false,
            &revision_pool,
            &[0],
        )
        .unwrap();
        assert_eq!(revisions.len(), 1);
        assert_eq!(
            revisions[&capture].revision.content_fingerprint,
            "cached-content"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_run_decodes_each_capture_once_and_publishes_distinct_products() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        let executable = temporary.path().join("fake-nfdump");
        let stream_path = temporary.path().join("stream.bin");
        let empty_stream_path = temporary.path().join("empty-stream.bin");
        let invocation_log = temporary.path().join("invocations.log");
        let mut stream = crate::nfdump::ONE_V4_TEST_STREAM.to_vec();
        let record = 16;
        stream[record + 32..record + 40].copy_from_slice(&20_u64.to_le_bytes());
        stream[record + 40..record + 48].copy_from_slice(&2_000_u64.to_le_bytes());
        stream[record + 48..record + 56].copy_from_slice(&3_u64.to_le_bytes());
        stream[record + 64..record + 66].copy_from_slice(&55_000_u16.to_le_bytes());
        stream[record + 69] = 0b010;
        fs::write(&stream_path, stream).unwrap();
        fs::write(
            &empty_stream_path,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then\ncat '{}'\nexit 0\nfi\nprintf 'x\\n' >> '{}'\ncat '{}'\n",
                empty_stream_path.display(),
                invocation_log.display(),
                stream_path.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let day_start = parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(day_start + bucket * FIVE_MINUTES)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            let path = day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M")));
            fs::write(path, b"capture").unwrap();
        }
        let registry = temporary.path().join("datasets.json");
        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        reset_prepare_nfcapd_tree_timestamp_calls();
        reset_nfcapd_pool_builds();
        reset_dataset_registry_load_calls();
        reset_coordinated_postflight_snapshot_verifications();
        let report = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry.clone()),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap();

        assert_eq!(report.five_minute_buckets, 576);
        assert_eq!(
            dataset_registry_load_calls(),
            1,
            "coordinated resolution must use one registry snapshot"
        );
        assert_eq!(nfcapd_pool_builds(), (1, 1, 1));
        assert_eq!(
            coordinated_postflight_snapshot_verifications(),
            288 + 288,
            "cold coordinated publication must verify every input snapshot once before commit"
        );
        assert_eq!(
            prepare_nfcapd_tree_timestamp_calls(),
            2 * 288,
            "coordinated publication must consume preflight preparation"
        );
        assert_eq!(
            fs::read_to_string(&invocation_log).unwrap().lines().count(),
            289
        );
        let first_identity: String = Connection::open(&first_db)
            .unwrap()
            .query_row(
                "SELECT selection_json FROM pipeline_product WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let second_identity: String = Connection::open(&second_db)
            .unwrap()
            .query_row(
                "SELECT selection_json FROM pipeline_product WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(first_identity, second_identity);

        let canonical_root = fs::canonicalize(&root).unwrap();
        let root_alias = temporary.path().join("captures-alias");
        symlink(&root, &root_alias).unwrap();
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root_alias,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root_alias,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        reset_nfcapd_logical_bucket_topology_calls();
        reset_nfcapd_day_topology_audit_calls();
        reset_nfcapd_pool_builds();
        reset_dataset_registry_load_calls();
        reset_coordinated_postflight_snapshot_verifications();
        crate::storage::reset_resume_query_counters();
        let resumed = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry.clone()),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["second".into(), "first".into()],
        )
        .unwrap();
        assert_eq!(resumed.five_minute_buckets, 0);
        assert_eq!(dataset_registry_load_calls(), 1);
        assert_eq!(nfcapd_logical_bucket_topology_calls(), 0);
        assert_eq!(
            nfcapd_day_topology_audit_calls(),
            0,
            "a healthy marker-backed no-op must not scan stats-family topology"
        );
        assert_eq!(nfcapd_pool_builds(), (1, 0, 0));
        assert_eq!(
            crate::storage::resume_query_counters(),
            crate::storage::ResumeQueryCounters {
                input_evidence: 2,
                processed_nfcapd: 2,
                content_fingerprint: 0,
            },
            "coordinated no-op resume state should load once per output"
        );
        assert_eq!(
            coordinated_postflight_snapshot_verifications(),
            0,
            "a complete coordinated no-op must not repeat postflight snapshot verification"
        );
        assert_eq!(
            fs::read_to_string(&invocation_log).unwrap().lines().count(),
            289
        );
        let locator: String = Connection::open(&first_db)
            .unwrap()
            .query_row(
                "SELECT input_locator FROM processed_inputs WHERE input_kind = 'nfcapd' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(locator.starts_with(canonical_root.to_string_lossy().as_ref()));
        assert!(!locator.contains("captures-alias"));

        let connection = Connection::open(&first_db).unwrap();
        connection
            .execute(
                "DELETE FROM daily_product_completion
                 WHERE source_id = 'edge' AND day_start = ?1",
                params![parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap()],
            )
            .unwrap();
        drop(connection);
        reset_nfcapd_day_topology_audit_calls();
        let legacy_resumed = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap();
        assert_eq!(legacy_resumed.five_minute_buckets, 0);
        assert_eq!(
            nfcapd_day_topology_audit_calls(),
            1,
            "a missing marker without a dirty tombstone must use the legacy topology audit"
        );
        let connection = Connection::open(&first_db).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "legacy recovery must backfill the completion marker"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion_dirty
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_resume_keeps_a_complete_output_unchanged_while_catching_up_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(
                parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap() + bucket * FIVE_MINUTES,
            )
            .unwrap()
            .in_tz(DEFAULT_TIMEZONE)
            .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let executable = temporary.path().join("fake-nfdump");
        let stream_path = temporary.path().join("stream.bin");
        let empty_stream_path = temporary.path().join("empty.stream");
        fs::write(&stream_path, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            &empty_stream_path,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then cat '{}'; exit 0; fi\ncat '{}'\n",
                empty_stream_path.display(),
                stream_path.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        let single_request = PipelineRequest {
            config_path: None,
            dataset_id: Some("first".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        };
        run(single_request).unwrap();
        let before = coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap());

        let report = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap();

        assert_eq!(report.five_minute_buckets, 288);
        assert_eq!(report.skipped_inputs, 288);
        assert_eq!(
            coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap()),
            before,
            "a complete coordinated output must remain semantically unchanged"
        );
        assert_eq!(
            Connection::open(&second_db)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE granularity = '5m' AND coverage_state = 'complete'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            288
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_resume_handles_an_output_needed_only_in_a_later_decode_batch() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(
                parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap() + bucket * FIVE_MINUTES,
            )
            .unwrap()
            .in_tz(DEFAULT_TIMEZONE)
            .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let executable = temporary.path().join("fake-nfdump");
        let stream_path = temporary.path().join("stream.bin");
        let empty_stream_path = temporary.path().join("empty.stream");
        fs::write(&stream_path, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            &empty_stream_path,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then cat '{}'; exit 0; fi\ncat '{}'\n",
                empty_stream_path.display(),
                stream_path.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "203.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest {
            config_path: None,
            dataset_id: Some("first".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap();

        let before = coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap());
        let first_start = parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap();
        let later_batch_start = first_start + 12 * FIVE_MINUTES;
        let later_batch_end = later_batch_start + 12 * FIVE_MINUTES;
        let first_connection = Connection::open(&first_db).unwrap();
        first_connection
            .execute(
                "DELETE FROM input_evidence
                 WHERE source_id = 'edge' AND bucket_start >= ?1 AND bucket_start < ?2",
                params![later_batch_start, later_batch_end],
            )
            .unwrap();
        drop(first_connection);

        let after_fixture = coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap());
        let error = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry.clone()),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("rerun that whole day with --force"),
            "normal resume must reject orphaned provenance: {message}"
        );
        let after_error = coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap());
        assert_eq!(
            &after_error[4..],
            &after_fixture[4..],
            "normal resume must not mutate the partially orphaned output"
        );

        let report = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: executable.to_string_lossy().into_owned(),
                force: true,
                run_maad: false,
                require_complete: false,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap();

        assert_eq!(report.five_minute_buckets, 576);
        for database in [&first_db, &second_db] {
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM bucket_coverage
                         WHERE granularity = '5m' AND coverage_state = 'complete'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                288
            );
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(DISTINCT bucket_start) FROM traffic_stats
                         WHERE source_id = 'edge' AND granularity = '5m'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                288,
                "resume must restore every distinct five-minute traffic bucket"
            );
            let five_minute_totals = connection
                .query_row(
                    "SELECT COALESCE(SUM(flows), 0), COALESCE(SUM(packets), 0), COALESCE(SUM(bytes), 0)
                     FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '5m'
                       AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .unwrap();
            let daily_totals = connection
                .query_row(
                    "SELECT COALESCE(SUM(flows), 0), COALESCE(SUM(packets), 0), COALESCE(SUM(bytes), 0)
                     FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '1d'
                       AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(
                daily_totals, five_minute_totals,
                "daily traffic must match 5m parity"
            );
        }
        let after_force = coordinated_semantic_snapshot(&Connection::open(&first_db).unwrap());
        assert_eq!(
            &after_force[4..],
            &before[4..],
            "force rebuild must restore the original first output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn daily_active_resume_accepts_complete_maad_and_rejects_missing_maad_until_force_rebuilds_the_day()
     {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        let day_start = parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(day_start + bucket * FIVE_MINUTES)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let database = temporary.path().join("pipeline.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "default_start_date": "2025-06-01",
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let request = |force: bool| PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force,
            run_maad: true,
            require_complete: false,
        };

        run(request(false)).unwrap();
        let resumed = run(request(false)).unwrap();
        assert_eq!(resumed.five_minute_buckets, 0);

        // Coverage is part of the certified daily product. A direct mutation must dirty the
        // marker so a normal resume cannot silently accept the stale canonical output; force
        // rebuilds the complete day and restores the coverage envelope.
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE bucket_coverage
                 SET coverage_state = 'partial', observed_units = 1, expected_units = 2
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                params![day_start],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion_dirty
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![day_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "coverage updates must dirty a completed daily-active marker"
        );
        drop(connection);
        let error = run(request(false)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rerun that whole day with --force"),
            "normal resume must reject post-certification coverage mutation: {error}"
        );
        run(request(true)).unwrap();

        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "DELETE FROM bucket_coverage
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                params![day_start],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion_dirty
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![day_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "coverage deletes must dirty a completed daily-active marker"
        );
        drop(connection);
        let error = run(request(false)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rerun that whole day with --force"),
            "normal resume must reject post-certification coverage deletion: {error}"
        );
        run(request(true)).unwrap();
        assert_eq!(
            Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'edge' AND granularity = '5m'
                       AND coverage_state = 'complete'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            288,
            "force rebuild must republish deleted coverage"
        );

        let before = coordinated_semantic_snapshot(&Connection::open(&database).unwrap());
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE traffic_stats SET flows = flows + 1
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1
                   AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                params![day_start],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion_dirty
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![day_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(connection);
        reset_nfcapd_day_topology_audit_calls();
        let error = run(request(false)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rerun that whole day with --force"),
            "normal resume must reject a post-certification metric mutation: {error}"
        );
        assert_eq!(
            nfcapd_day_topology_audit_calls(),
            0,
            "dirty completion evidence must not fall through to legacy topology auditing"
        );
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![day_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "normal resume must retain the prior marker while refusing the dirty day"
        );
        drop(connection);
        run(request(true)).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            coordinated_semantic_snapshot(&connection),
            before,
            "force rebuild must restore exact metrics after a direct mutation"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion_dirty
                     WHERE source_id = 'edge' AND day_start = ?1",
                    params![day_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "force rebuild must leave a clean completion marker"
        );
        drop(connection);
        let mixed_end_bucket = day_start + 8 * FIVE_MINUTES;
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE traffic_stats SET bucket_end = ?2
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1
                   AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                params![mixed_end_bucket, mixed_end_bucket + FIVE_MINUTES + 1],
            )
            .unwrap();
        drop(connection);
        let error = run(request(false)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rerun that whole day with --force"),
            "mixed bucket_end corruption must fail full-day validation: {error}"
        );
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE traffic_stats SET bucket_end = bucket_start + 300
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                params![mixed_end_bucket],
            )
            .unwrap();
        let corrupted_bucket = day_start + 12 * FIVE_MINUTES;
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM address_structure_stats
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![corrupted_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            30,
            "a healthy dense MAAD bucket has five IPv4 scopes, two sides, and three structures"
        );
        connection
            .execute(
                "DELETE FROM address_structure_stats
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1
                   AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'
                   AND address_side = 'source' AND structure_kind = 'structure'",
                params![corrupted_bucket],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM address_structure_stats
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![corrupted_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            29,
            "the corruption fixture must remove one of the 30 IPv4 MAAD rows"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM processed_inputs
                     WHERE input_kind = 'nfcapd' AND source_id = 'edge' AND bucket_start = ?1
                       AND status = 'processed'",
                    params![corrupted_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "the corruption fixture must retain provenance"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![corrupted_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "the corruption fixture must retain coverage"
        );
        drop(connection);

        let error = run(request(false)).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("rerun that whole day with --force"),
            "normal resume must reject canonical topology corruption: {message}"
        );
        assert_eq!(
            Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM address_structure_stats
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![corrupted_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            29,
            "normal resume must not partially repair the product"
        );

        run(request(true)).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(DISTINCT bucket_start) FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            288
        );
        let five_minute_totals = connection
            .query_row(
                "SELECT COALESCE(SUM(flows), 0), COALESCE(SUM(packets), 0), COALESCE(SUM(bytes), 0)
                 FROM traffic_stats
                 WHERE source_id = 'edge' AND granularity = '5m'
                   AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        let daily_totals = connection
            .query_row(
                "SELECT COALESCE(SUM(flows), 0), COALESCE(SUM(packets), 0), COALESCE(SUM(bytes), 0)
                 FROM traffic_stats
                 WHERE source_id = 'edge' AND granularity = '1d'
                   AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            daily_totals, five_minute_totals,
            "force rebuild must restore daily parity"
        );
        assert_eq!(
            coordinated_semantic_snapshot(&connection),
            before,
            "force rebuild must restore the semantic product"
        );
    }

    #[cfg(unix)]
    #[test]
    fn daily_active_force_rebuild_removes_surplus_day_rows() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        let day_start = parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(day_start + bucket * FIVE_MINUTES)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let database = temporary.path().join("pipeline.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "default_start_date": "2025-06-01",
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let request = |force: bool| PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force,
            run_maad: false,
            require_complete: false,
        };

        run(request(false)).unwrap();
        let before = coordinated_semantic_snapshot(&Connection::open(&database).unwrap());
        let surplus_start = day_start + 60;
        let surplus_end = surplus_start + FIVE_MINUTES;
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "INSERT INTO bucket_coverage (
                    source_id, granularity, bucket_start, bucket_end, coverage_state,
                    observed_units, expected_units, rejected_units
                 ) VALUES ('edge', '5m', ?1, ?2, 'complete', 1, 1, 0)",
                params![surplus_start, surplus_end],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO traffic_stats (
                    source_id, granularity, bucket_start, bucket_end, ip_version,
                    src_visibility, dst_visibility, flows, flows_tcp, flows_udp,
                    flows_icmp, flows_other, packets, packets_tcp, packets_udp,
                    packets_icmp, packets_other, bytes, bytes_tcp, bytes_udp,
                    bytes_icmp, bytes_other, duration_sum_ms, duration_count,
                    average_duration_ms, min_ttl_sum, min_ttl_count, average_min_ttl,
                    max_ttl_sum, max_ttl_count, average_max_ttl
                 )
                 SELECT source_id, granularity, ?1, ?2, ip_version,
                    src_visibility, dst_visibility, flows, flows_tcp, flows_udp,
                    flows_icmp, flows_other, packets, packets_tcp, packets_udp,
                    packets_icmp, packets_other, bytes, bytes_tcp, bytes_udp,
                    bytes_icmp, bytes_other, duration_sum_ms, duration_count,
                    average_duration_ms, min_ttl_sum, min_ttl_count, average_min_ttl,
                    max_ttl_sum, max_ttl_count, average_max_ttl
                 FROM traffic_stats
                 WHERE source_id = 'edge' AND granularity = '5m'
                 LIMIT 1",
                params![surplus_start, surplus_end],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![surplus_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![surplus_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(connection);

        let error = run(request(false)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rerun that whole day with --force"),
            "normal resume must reject a valid surplus row: {error}"
        );

        run(request(true)).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![surplus_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "force must remove surplus coverage inside the requested day"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                    params![surplus_start],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "force must remove surplus product rows inside the requested day"
        );
        assert_eq!(coordinated_semantic_snapshot(&connection), before);
        drop(connection);

        let resumed = run(request(false)).unwrap();
        assert_eq!(resumed.five_minute_buckets, 0);
        assert_eq!(
            coordinated_semantic_snapshot(&Connection::open(&database).unwrap()),
            before,
            "a clean normal resume must be a no-op after force rebuild"
        );
    }

    #[test]
    fn coordinated_run_skips_an_incomplete_physical_day_for_every_output() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("nfcapd.202506010000"), b"capture").unwrap();
        let registry = temporary.path().join("datasets.json");
        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        let error = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: nfdump.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: true,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("dataset \"first\""), "{message}");
        assert!(
            message.contains(&first_db.to_string_lossy().to_string()),
            "{message}"
        );
        assert!(
            message.contains("576 incomplete five-minute coverage buckets"),
            "{message}"
        );
        for database in [first_db, second_db] {
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn canonical_day_resume_queries_seek_bounded_source_granularity_ranges() {
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();

        let explain = |query: String| {
            connection
                .prepare(&query)
                .unwrap()
                .query_map(params!["r1", 0_i64, 86_400_i64], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let assert_bounded_seek = |label: &str, plan: &[String]| {
            let uses_range_seek = plan.iter().any(|detail| {
                let compact = detail.replace(' ', "");
                compact.contains("SEARCH")
                    && compact.contains("source_id=?")
                    && compact.contains("granularity=?")
                    && compact.contains("bucket_start>?")
                    && compact.contains("bucket_start<?")
            });
            assert!(
                uses_range_seek,
                "{label} should seek source/granularity and constrain bucket_start: {}",
                plan.join("\n")
            );
            assert!(
                plan.iter()
                    .all(|detail| !detail.contains("USE TEMP B-TREE")),
                "{label} should not materialize a temporary GROUP BY B-tree: {}",
                plan.join("\n")
            );
        };

        assert_bounded_seek(
            "bucket_coverage",
            &explain(format!(
                "EXPLAIN QUERY PLAN
                 SELECT granularity, bucket_start, bucket_end
                 FROM bucket_coverage
                 WHERE source_id = ?1 AND {CANONICAL_GRANULARITY_PREDICATE}
                   AND bucket_start >= ?2 AND bucket_start < ?3
                 ORDER BY granularity, bucket_start"
            )),
        );
        for table in [
            "traffic_stats",
            "protocol_stats",
            "address_count_stats",
            "port_count_stats",
            "address_structure_stats",
        ] {
            assert_bounded_seek(
                table,
                &explain(format!(
                    "EXPLAIN QUERY PLAN
                     SELECT granularity, bucket_start, MIN(bucket_end), MAX(bucket_end), COUNT(*)
                     FROM {table}
                     WHERE source_id = ?1 AND {CANONICAL_GRANULARITY_PREDICATE}
                       AND bucket_start >= ?2 AND bucket_start < ?3
                     GROUP BY granularity, bucket_start"
                )),
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_mixed_explicit_and_auto_layouts_publish_identical_source_metadata() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        write_nfcapd_day(&root);
        let first_database = temporary.path().join("first.sqlite");
        let second_database = temporary.path().join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "configured",
                    "root_path": root,
                    "db_path": first_database,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "auto",
                    "root_path": temporary.path().join("captures"),
                    "db_path": second_database,
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        let report = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-02".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: nfdump.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["configured".into(), "auto".into()],
        )
        .unwrap();
        assert_eq!(report.five_minute_buckets, 576);

        let source_members = |database: &Path| {
            let connection = Connection::open(database).unwrap();
            connection
                .prepare(
                    "SELECT source_id, member_id FROM source_members
                     ORDER BY dataset_id, source_id, member_id",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let source_layout = |database: &Path| {
            let connection = Connection::open(database).unwrap();
            connection
                .query_row(
                    "SELECT layout_json FROM nfcapd_source_layout WHERE singleton = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        let coverage_layout = |database: &Path| {
            let connection = Connection::open(database).unwrap();
            connection
                .prepare(
                    "SELECT source_id, granularity, bucket_start, bucket_end
                     FROM bucket_coverage ORDER BY source_id, granularity, bucket_start",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        assert_eq!(
            source_members(&first_database),
            source_members(&second_database)
        );
        assert_eq!(
            source_layout(&first_database),
            source_layout(&second_database)
        );
        assert_eq!(
            coverage_layout(&first_database),
            coverage_layout(&second_database)
        );
    }

    #[cfg(unix)]
    #[test]
    fn single_auto_layout_change_after_planning_is_side_effect_free() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let output_directory = temporary.path().join("outputs");
        let database = output_directory.join("active.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        set_single_plan_hook(move |planned_root| {
            fs::create_dir_all(planned_root.join("edge-b")).unwrap();
        });
        let error = run(PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap_err();
        clear_single_plan_hook();

        assert!(
            error
                .to_string()
                .contains("auto-discovered source layout changed"),
            "{error}"
        );
        assert!(!output_directory.exists());
        assert!(!database.exists());
        assert!(!database_operation_lock_path(&database).unwrap().exists());
        fs::remove_dir_all(root.join("edge-b")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn member_directory_replacement_after_planning_is_side_effect_free_in_single_and_coordinated_modes()
     {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let database = temporary.path().join("single.sqlite");
        let registry = temporary.path().join("single-datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let single_backup = root.join("edge-before-single-replacement");
        let single_backup_for_hook = single_backup.clone();
        set_single_plan_hook(move |planned_root| {
            let member = planned_root.join("edge");
            fs::rename(&member, &single_backup_for_hook).unwrap();
            fs::create_dir(&member).unwrap();
        });
        let single_error = run(PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap_err();
        clear_single_plan_hook();
        assert!(single_error.to_string().contains("member directory"));
        assert!(!database.exists());
        assert!(!database_operation_lock_path(&database).unwrap().exists());
        fs::remove_dir(root.join("edge")).unwrap();
        fs::rename(single_backup, root.join("edge")).unwrap();

        let (request, output_directory, first_database, second_database, sentinel) =
            repeated_dataset_request(&temporary, nfdump.clone());
        let coordinated_backup = root.join("edge-before-coordinated-replacement");
        set_coordinated_plan_hook(move |planned_root| {
            let member = planned_root.join("edge");
            fs::rename(&member, &coordinated_backup).unwrap();
            fs::create_dir(&member).unwrap();
        });
        let coordinated_error =
            run_many(request, vec!["first".into(), "second".into()]).unwrap_err();
        clear_coordinated_plan_hook();
        assert!(coordinated_error.to_string().contains("member directory"));
        assert!(!output_directory.exists());
        for database in [&first_database, &second_database] {
            assert!(!database.exists());
            assert!(!database_operation_lock_path(database).unwrap().exists());
        }
        assert_eq!(fs::read(sentinel).unwrap(), b"leave this alone");
    }

    #[cfg(unix)]
    #[test]
    fn member_directory_replacement_between_days_does_not_mix_single_product_days() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        write_nfcapd_day_for_date(&root, "2025-06-01");
        write_nfcapd_day_for_date(&root, "2025-06-02");
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let database = temporary.path().join("active.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let hook_calls = std::rc::Rc::clone(&calls);
        set_missing_day_absence_hook(move |planned_root, _, _| {
            let call = hook_calls.get();
            hook_calls.set(call + 1);
            if call == 1 {
                replace_member_directory(planned_root);
            }
        });
        let error = run(PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-03".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap_err();
        clear_missing_day_absence_hook();

        assert!(error.to_string().contains("member directory"), "{error}");
        assert_eq!(calls.get(), 2);
        let connection = Connection::open(database).unwrap();
        let day_count = |start_date: &str| {
            let start = parse_date_start(start_date, DEFAULT_TIMEZONE).unwrap();
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'edge' AND granularity = '5m'
                       AND bucket_start >= ?1 AND bucket_start < ?2",
                    params![start, start + 86_400],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(day_count("2025-06-01"), 288);
        assert_eq!(day_count("2025-06-02"), 0);
    }

    #[cfg(unix)]
    #[test]
    fn member_directory_replacement_at_precommit_rolls_back_single_and_coordinated_days() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);
        let database = temporary.path().join("single.sqlite");
        let registry = temporary.path().join("single-datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let single_backup = root.join("edge-before-single-precommit-replacement");
        let root_for_hook = root.clone();
        let single_backup_for_hook = single_backup.clone();
        set_single_commit_guard_hook(move || {
            let member = root_for_hook.join("edge");
            fs::rename(&member, &single_backup_for_hook).unwrap();
            fs::create_dir(&member).unwrap();
        });
        let single_error = run(PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-02".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap_err();
        clear_single_commit_guard_hook();
        assert!(single_error.to_string().contains("member directory"));
        let single_connection = Connection::open(&database).unwrap();
        assert_eq!(
            single_connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        fs::remove_dir(root.join("edge")).unwrap();
        fs::rename(single_backup, root.join("edge")).unwrap();

        let (mut request, _, first_database, second_database, _) =
            repeated_dataset_request(&temporary, nfdump.clone());
        request.end_date = Some("2025-06-01".into());
        write_nfcapd_day(&root);
        let coordinated_backup = root.join("edge-before-coordinated-precommit-replacement");
        set_coordinated_commit_guard_hook(move || {
            let member = root.join("edge");
            fs::rename(&member, &coordinated_backup).unwrap();
            fs::create_dir(&member).unwrap();
        });
        let coordinated_error =
            run_many(request, vec!["first".into(), "second".into()]).unwrap_err();
        clear_coordinated_commit_guard_hook();
        assert!(coordinated_error.to_string().contains("member directory"));
        for database in [first_database, second_database] {
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn single_daily_active_capture_rewrite_before_commit_rolls_back_day() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let database = temporary.path().join("active.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "default_start_date": "2025-06-01",
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let request = |force| PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: nfdump.to_string_lossy().into_owned(),
            force,
            run_maad: false,
            require_complete: false,
        };

        run(request(false)).unwrap();
        let before = coordinated_semantic_snapshot(&Connection::open(&database).unwrap());
        let marker_count_before = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM daily_product_completion
                 WHERE source_id = 'edge'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        let capture = root.join("edge/2025/06/01/nfcapd.202506010000");
        let original = fs::read(&capture).unwrap();
        let rewritten = capture.clone();
        set_single_commit_guard_hook(move || {
            fs::write(&rewritten, b"rewritten after processing").unwrap();
        });

        let error = run(request(true)).unwrap_err();
        clear_single_commit_guard_hook();

        assert!(error.to_string().contains("Input changed"), "{error}");
        assert_eq!(
            coordinated_semantic_snapshot(&Connection::open(&database).unwrap()),
            before,
            "a capture rewrite at the precommit seam must roll back all day rows"
        );
        assert_eq!(
            Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM daily_product_completion
                     WHERE source_id = 'edge'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            marker_count_before,
            "the completion marker must roll back with the day rows"
        );
        assert_ne!(fs::read(&capture).unwrap(), original);
        fs::write(capture, original).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn single_tree_rejects_stale_complete_day_and_force_removes_it_before_strict_failure() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(
                parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap() + bucket * FIVE_MINUTES,
            )
            .unwrap()
            .in_tz(DEFAULT_TIMEZONE)
            .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let registry = temporary.path().join("datasets.json");
        let database = temporary.path().join("pipeline.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let request = |force: bool, require_complete: bool| PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force,
            run_maad: false,
            require_complete,
        };

        run(request(false, false)).unwrap();
        let removed = day.join("nfcapd.202506010000");
        fs::remove_file(&removed).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "DELETE FROM bucket_coverage
                 WHERE source_id = 'edge' AND granularity = '5m' AND bucket_start = ?1",
                params![parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap()],
            )
            .unwrap();
        let before = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert!(matches!(
            run(request(false, false)),
            Err(PipelineError::InvalidConfig(_))
        ));
        assert_eq!(
            Connection::open(&database)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            before
        );

        let error = run(request(true, true)).unwrap_err();
        assert!(matches!(error, PipelineError::IncompleteCoverage(288)));
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM input_evidence", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn single_force_stale_day_rejects_a_late_capture_without_product_loss() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let registry = temporary.path().join("datasets.json");
        let database = temporary.path().join("pipeline.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        let request = |force: bool| PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry.clone()),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force,
            run_maad: false,
            require_complete: false,
        };

        run(request(false)).unwrap();
        let before = coordinated_semantic_snapshot(&Connection::open(&database).unwrap());
        let removed = root.join("edge/2025/06/01/nfcapd.202506010000");
        fs::remove_file(&removed).unwrap();
        let restored = removed.clone();
        set_missing_day_absence_hook(move |_, missing, _| {
            assert!(!missing.is_empty());
            fs::write(&restored, b"late capture").unwrap();
        });

        let error = run(request(true)).unwrap_err();
        clear_missing_day_absence_hook();

        let message = error.to_string();
        assert!(
            message.contains("refusing to delete the existing product"),
            "{message}"
        );
        assert!(message.contains("nfcapd.202506010000"), "{message}");
        assert!(removed.is_file());
        assert_eq!(
            &coordinated_semantic_snapshot(&Connection::open(database).unwrap())[4..],
            &before[4..],
            "late capture detection must roll back the stale-day deletion"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_force_stale_day_rejects_a_late_capture_without_product_loss() {
        let temporary = tempdir().unwrap();
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let (mut request, _output_directory, first_database, second_database, _sentinel) =
            repeated_dataset_request(&temporary, executable.clone());
        request.end_date = Some("2025-06-01".into());
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);

        run_many(request.clone(), vec!["first".into(), "second".into()]).unwrap();
        let before_first =
            coordinated_semantic_snapshot(&Connection::open(&first_database).unwrap());
        let before_second =
            coordinated_semantic_snapshot(&Connection::open(&second_database).unwrap());
        let removed = root.join("edge/2025/06/01/nfcapd.202506010000");
        fs::remove_file(&removed).unwrap();
        let restored = removed.clone();
        set_missing_day_absence_hook(move |_, missing, _| {
            assert!(!missing.is_empty());
            fs::write(&restored, b"late capture").unwrap();
        });

        let mut forced = request;
        forced.force = true;
        let error = run_many(forced, vec!["first".into(), "second".into()]).unwrap_err();
        clear_missing_day_absence_hook();

        let message = error.to_string();
        assert!(
            message.contains("refusing to delete the existing product"),
            "{message}"
        );
        assert!(message.contains("nfcapd.202506010000"), "{message}");
        assert!(removed.is_file());
        assert_eq!(
            &coordinated_semantic_snapshot(&Connection::open(first_database).unwrap())[4..],
            &before_first[4..],
            "first coordinated output must survive a late capture"
        );
        assert_eq!(
            &coordinated_semantic_snapshot(&Connection::open(second_database).unwrap())[4..],
            &before_second[4..],
            "second coordinated output must survive a late capture"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_force_stale_day_does_not_guard_again_after_the_first_commit() {
        let temporary = tempdir().unwrap();
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let (mut request, _output_directory, first_database, second_database, _sentinel) =
            repeated_dataset_request(&temporary, executable.clone());
        request.end_date = Some("2025-06-01".into());
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);

        run_many(request.clone(), vec!["first".into(), "second".into()]).unwrap();
        let removed = root.join("edge/2025/06/01/nfcapd.202506010000");
        fs::remove_file(&removed).unwrap();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let hook_calls = std::rc::Rc::clone(&calls);
        let late_capture = removed.clone();
        set_coordinated_commit_guard_hook(move || {
            let call = hook_calls.get();
            hook_calls.set(call + 1);
            if call == 1 {
                fs::write(&late_capture, b"late capture").unwrap();
            }
        });

        let mut forced = request;
        forced.force = true;
        let result = run_many(forced, vec!["first".into(), "second".into()]);
        clear_coordinated_commit_guard_hook();
        result.unwrap();

        assert_eq!(calls.get(), 1);
        assert!(!removed.exists());
        assert_eq!(
            &coordinated_semantic_snapshot(&Connection::open(first_database).unwrap())[4..],
            &coordinated_semantic_snapshot(&Connection::open(second_database).unwrap())[4..],
            "a late-capture hook at the former second-commit seam must not split outputs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinated_publication_does_not_guard_again_after_the_first_commit() {
        let temporary = tempdir().unwrap();
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, "");
        let (request, _output_directory, first_database, second_database, _sentinel) =
            repeated_dataset_request(&temporary, executable.clone());
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root);

        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let hook_calls = std::rc::Rc::clone(&calls);
        let changed_decoder = executable.clone();
        set_coordinated_commit_guard_hook(move || {
            let call = hook_calls.get();
            hook_calls.set(call + 1);
            if call == 1 {
                let mut contents = fs::read(&changed_decoder).unwrap();
                contents.push(b'\n');
                fs::write(&changed_decoder, contents).unwrap();
            }
        });

        let result = run_many(request, vec!["first".into(), "second".into()]);
        clear_coordinated_commit_guard_hook();
        result.unwrap();

        assert_eq!(calls.get(), 1);
        let mut states = Vec::new();
        for database in [first_database, second_database] {
            let connection = Connection::open(database).unwrap();
            let processed_inputs = connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            let bucket_coverage = connection
                .query_row("SELECT COUNT(*) FROM bucket_coverage", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            assert!(processed_inputs > 0);
            assert!(bucket_coverage > 0);
            states.push((processed_inputs, bucket_coverage));
        }
        assert_eq!(states[0], states[1]);
    }

    #[test]
    fn coordinated_empty_finite_request_is_incomplete_without_publishing_traffic() {
        let temporary = tempdir().unwrap();
        let nfdump = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&nfdump, "");
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([
                {
                    "dataset_id": "first",
                    "root_path": root,
                    "db_path": first_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {"kind": "daily_active_sources", "ip_prefix": "198.51.0.0/16"}
                }
            ]))
            .unwrap(),
        )
        .unwrap();

        let error = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(registry),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: nfdump.to_string_lossy().into_owned(),
                force: false,
                run_maad: false,
                require_complete: true,
            },
            vec!["first".into(), "second".into()],
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("dataset \"first\""), "{message}");
        assert!(
            message.contains(&first_db.to_string_lossy().to_string()),
            "{message}"
        );
        assert!(
            message.contains("288 incomplete five-minute coverage buckets"),
            "{message}"
        );
        for database in [first_db, second_db] {
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn complete_physical_day_uses_local_dst_bucket_boundaries() {
        let start = parse_date_start("2025-03-09", "America/Los_Angeles").unwrap();
        let end = next_date_start("2025-03-09", "America/Los_Angeles").unwrap();
        let members = vec!["cc".to_owned(), "oh".to_owned()];
        let mut paths = BTreeMap::new();
        let mut bucket_start = start;
        let mut count = 0;
        while bucket_start < end {
            for member in &members {
                paths.insert(
                    (member.clone(), bucket_start),
                    PathBuf::from(format!("{member}/{bucket_start}")),
                );
            }
            count += 1;
            bucket_start =
                next_local_five_minute_start(bucket_start, "America/Los_Angeles").unwrap();
        }

        assert_eq!(count, 276);
        assert!(
            missing_physical_day_inputs(&members, &paths, start, end, "America/Los_Angeles")
                .unwrap()
                .is_empty()
        );
        paths.remove(&("oh".to_owned(), start));
        assert_eq!(
            missing_physical_day_inputs(&members, &paths, start, end, "America/Los_Angeles")
                .unwrap(),
            [("oh".to_owned(), start)]
        );
    }

    #[test]
    fn nfcapd_decode_chunks_cap_a_timestamp_with_more_than_twelve_members() {
        let members = (0..13).map(|index| format!("member-{index:02}"));
        let members = members.collect::<Vec<_>>();
        let sources = [DatasetSource {
            source_id: "logical".into(),
            members: members.clone(),
        }];
        let paths = members
            .iter()
            .map(|member| {
                (
                    (member.clone(), 0),
                    PathBuf::from(format!("{member}/nfcapd.197001010000")),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            nfcapd_batch_starts(
                0,
                FIVE_MINUTES,
                "UTC",
                &sources,
                &paths,
                &BTreeMap::new(),
                false,
            )
            .unwrap(),
            [0]
        );
        let requests = members
            .iter()
            .map(|member| (member.clone(), 0_i64))
            .collect::<Vec<_>>();
        let chunk_lengths = nfcapd_decode_request_chunks(&requests)
            .map(<[_]>::len)
            .collect::<Vec<_>>();
        assert_eq!(chunk_lengths, [12, 1]);
        assert!(
            chunk_lengths
                .iter()
                .all(|length| *length <= NFCAPD_DECODE_BATCH_SIZE)
        );
    }

    #[cfg(unix)]
    #[test]
    fn daily_activity_unions_each_physical_member_once() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let executable = temporary.path().join("fake-nfdump");
        let stream_path = temporary.path().join("activity.stream");
        let empty_stream_path = temporary.path().join("empty.stream");
        let invocation_log = temporary.path().join("invocations.log");
        let mut stream = crate::nfdump::ONE_V4_TEST_STREAM.to_vec();
        let record = 16;
        stream[record + 32..record + 40].copy_from_slice(&10_u64.to_le_bytes());
        stream[record + 40..record + 48].copy_from_slice(&1_000_u64.to_le_bytes());
        stream[record + 48..record + 56].copy_from_slice(&2_u64.to_le_bytes());
        stream[record + 64..record + 66].copy_from_slice(&1_024_u16.to_le_bytes());
        stream[record + 69] = 0b010;
        fs::write(&stream_path, stream).unwrap();
        fs::write(
            &empty_stream_path,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then\ncat '{}'\nexit 0\nfi\nprintf 'x\\n' >> '{}'\ncat '{}'\n",
                empty_stream_path.display(),
                invocation_log.display(),
                stream_path.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let mut paths = BTreeMap::new();
        for member in ["cc", "oh"] {
            let directory = temporary.path().join(member);
            fs::create_dir(&directory).unwrap();
            let path = directory.join("nfcapd.197001010000");
            fs::write(&path, "capture").unwrap();
            paths.insert((member.to_owned(), 0), path);
        }
        let sources = vec![
            DatasetSource {
                source_id: "cc".into(),
                members: vec!["cc".into()],
            },
            DatasetSource {
                source_id: "oh".into(),
                members: vec!["oh".into()],
            },
            DatasetSource {
                source_id: "all".into(),
                members: vec!["cc".into(), "oh".into()],
            },
        ];
        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "192.0.0.0/16"
        })))
        .unwrap();
        let pipeline = ResolvedPipeline {
            database_path: temporary.path().join("unused.sqlite"),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: executable.to_string_lossy().into_owned().into(),
            nfdump_revision: None,
            selection,
            inputs: Vec::new(),
            datasets: Vec::new(),
            require_complete: false,
        };

        let (active, _) = resolve_daily_active_sources(
            &sources,
            &paths,
            0,
            FIVE_MINUTES,
            &pipeline,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();

        assert!(active.contains(&IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert_eq!(
            fs::read_to_string(invocation_log).unwrap().lines().count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn daily_active_eligibility_ignores_off_grid_capture_keys() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        let day_start = parse_date_start("2025-06-01", DEFAULT_TIMEZONE).unwrap();
        for bucket in 0..288 {
            let timestamp = Timestamp::from_second(day_start + bucket * FIVE_MINUTES)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            fs::write(
                day.join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))),
                b"capture",
            )
            .unwrap();
        }
        let off_grid = day.join("nfcapd.202506010001");
        fs::write(&off_grid, b"off-grid capture").unwrap();

        let executable = temporary.path().join("fake-nfdump");
        let active_stream = temporary.path().join("active.stream");
        let empty_stream = temporary.path().join("empty.stream");
        let mut active_bytes = crate::nfdump::ONE_V4_TEST_STREAM.to_vec();
        active_bytes[16 + 32..16 + 40].copy_from_slice(&20_u64.to_le_bytes());
        active_bytes[16 + 40..16 + 48].copy_from_slice(&2_000_u64.to_le_bytes());
        active_bytes[16 + 48..16 + 56].copy_from_slice(&3_u64.to_le_bytes());
        fs::write(&active_stream, active_bytes).unwrap();
        fs::write(
            &empty_stream,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh
if [ \"$1\" = \"-R\" ]; then
  if [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then
    cat '{}'
    exit 0
  fi
  for path in \"$2\"/*; do
    target=$(readlink \"$path\")
    case \"$target\" in
      *nfcapd.202506010001) cat '{}'; exit 0 ;;
    esac
  done
  cat '{}'
else
  cat '{}'
fi
",
                empty_stream.display(),
                active_stream.display(),
                empty_stream.display(),
                active_stream.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let database = temporary.path().join("pipeline.sqlite");
        let registry = temporary.path().join("datasets.json");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
                "db_path": database,
                "source_ids": ["edge"],
                "selection": {"kind": "daily_active_sources", "ip_prefix": "192.0.0.0/16"}
            }]))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest {
            config_path: None,
            dataset_id: Some("active".into()),
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        })
        .unwrap();

        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COALESCE(SUM(flows), 0) FROM traffic_stats
                     WHERE source_id = 'edge' AND granularity = '5m'
                       AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "an off-grid-only threshold hit must not qualify the active source"
        );
        for table in [
            "bucket_coverage",
            "traffic_stats",
            "processed_inputs",
            "input_evidence",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {table}
                             WHERE source_id = 'edge' AND bucket_start = ?1"
                        ),
                        params![day_start + 60],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0,
                "off-grid capture must not appear in {table}"
            );
        }
    }

    #[test]
    fn logical_sources_borrow_singletons_and_merge_overlapping_members() {
        let build = |source_id: &str, destination: [u8; 4]| {
            let mut bucket = StatisticalBucket::dense(BucketKey::new(
                source_id,
                Granularity::FiveMinutes,
                0,
                FIVE_MINUTES,
            ));
            bucket
                .add(
                    FlowObservation::new(
                        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
                        IpAddr::V4(Ipv4Addr::from(destination)),
                        6,
                        2,
                        128,
                        0,
                    )
                    .unwrap(),
                )
                .unwrap();
            bucket.finish()
        };
        let cc = build("cc_ir1_gw", [198, 51, 100, 1]);
        let oh = build("oh_ir1_gw", [198, 51, 100, 2]);

        let singleton = logical_source_bucket("cc_ir1_gw", 0, 1, &[&cc]).unwrap();
        assert!(matches!(singleton, Cow::Borrowed(_)));

        let combined = logical_source_bucket("uoregon_all", 0, 2, &[&cc, &oh]).unwrap();
        assert!(matches!(combined, Cow::Owned(_)));
        let all_v4 = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
        assert_eq!(
            combined
                .traffic
                .iter()
                .find(|entry| entry.scope == all_v4)
                .unwrap()
                .metrics
                .flows,
            2
        );
        assert_eq!(combined.coverage.state(), CoverageState::Complete);

        let partial = logical_source_bucket("uoregon_all", 0, 2, &[&cc]).unwrap();
        assert_eq!(partial.coverage.state(), CoverageState::Partial);
        assert_eq!(partial.coverage.observed_units(), 1);

        let unknown = logical_source_bucket("uoregon_all", 0, 2, &[]).unwrap();
        assert_eq!(unknown.coverage.state(), CoverageState::Unknown);
        assert!(unknown.traffic.is_empty());
        assert_eq!(
            combined
                .addresses
                .iter()
                .find(|entry| {
                    entry.scope == all_v4 && entry.address_side == AddressSide::Source
                })
                .unwrap()
                .addresses
                .len(),
            1
        );
        assert_eq!(
            combined
                .addresses
                .iter()
                .find(|entry| {
                    entry.scope == all_v4 && entry.address_side == AddressSide::Destination
                })
                .unwrap()
                .addresses
                .len(),
            2
        );
    }

    #[test]
    fn strict_coverage_checks_only_the_finite_native_request() {
        let temporary = tempdir().unwrap();
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let capture_root = temporary.path().join("captures");
        fs::create_dir_all(capture_root.join("r1")).unwrap();
        let inside = parse_date_start("2025-01-01", "UTC").unwrap();
        let outside = parse_date_start("2025-01-03", "UTC").unwrap();
        for (source_id, bucket_start) in [("r1", inside), ("r1", outside), ("unrequested", inside)]
        {
            connection
                .execute(
                    "INSERT INTO bucket_coverage (
                        source_id, granularity, bucket_start, bucket_end,
                        coverage_state, observed_units, expected_units, rejected_units
                     ) VALUES (?1, '5m', ?2, ?3, 'unknown', 0, 1, 0)",
                    params![source_id, bucket_start, bucket_start + FIVE_MINUTES],
                )
                .unwrap();
        }
        let pipeline = ResolvedPipeline {
            database_path: temporary.path().join("netflow.sqlite"),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: capture_root,
                source_ids: vec!["r1".into()],
                sources: Vec::new(),
                start_date: "2025-01-01".into(),
                end_date: Some("2025-01-01".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: true,
        };

        assert_eq!(
            count_incomplete_requested_coverage(&connection, &pipeline).unwrap(),
            288
        );
    }

    #[test]
    fn open_ended_native_strict_coverage_includes_the_discovered_latest_day() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let day = root.join("edge/2025/06/01");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("nfcapd.202506010000"), b"capture").unwrap();
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let start = parse_date_start("2025-06-01", "UTC").unwrap();
        connection
            .execute(
                "INSERT INTO bucket_coverage (
                    source_id, granularity, bucket_start, bucket_end,
                    coverage_state, observed_units, expected_units, rejected_units
                 ) VALUES ('edge', '5m', ?1, ?2, 'complete', 1, 1, 0)",
                params![start, start + FIVE_MINUTES],
            )
            .unwrap();
        let pipeline = ResolvedPipeline {
            database_path: temporary.path().join("unused.sqlite"),
            control_paths: Vec::new(),
            timezone: "UTC".into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root,
                source_ids: vec!["edge".into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: None,
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: true,
        };

        assert_eq!(
            count_incomplete_requested_coverage(&connection, &pipeline).unwrap(),
            287
        );
    }

    #[test]
    fn persisted_sibling_validation_is_cached_per_source_day_but_rejects_foreign_rows() {
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let bucket = |bucket_start| {
            StatisticalBucket::dense(BucketKey::new(
                "r1",
                Granularity::FiveMinutes,
                bucket_start,
                bucket_start + FIVE_MINUTES,
            ))
            .finish_owned()
        };

        let mut aggregates = AggregateBuckets::default();
        for index in 0..288_i64 {
            let child = bucket(index * FIVE_MINUTES);
            aggregates
                .reject_persisted_siblings(&connection, &child, "UTC")
                .unwrap();
            aggregates.include(&child, "UTC").unwrap();
        }
        assert_eq!(aggregates.persisted_sibling_queries, 1);

        let foreign = bucket(FIVE_MINUTES);
        write_buckets(&connection, std::slice::from_ref(&foreign), false).unwrap();
        let error = AggregateBuckets::default()
            .reject_persisted_siblings(&connection, &bucket(0), "UTC")
            .unwrap_err();
        assert!(error.to_string().contains("cannot reopen"));
    }

    #[test]
    fn strict_coverage_counts_missing_partial_and_dst_successor_rows() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("r1")).unwrap();
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();

        let pipeline_for = |date: &str, timezone: &str| ResolvedPipeline {
            database_path: temporary.path().join("unused.sqlite"),
            control_paths: Vec::new(),
            timezone: timezone.into(),
            run_maad: false,
            nfdump: "nfdump".into(),
            nfdump_revision: None,
            selection: FlowSelection::default(),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.clone(),
                source_ids: vec!["r1".into()],
                sources: Vec::new(),
                start_date: date.into(),
                end_date: Some(date.into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: true,
        };
        let local_starts = |date: &str, timezone: &str| {
            let start = parse_date_start(date, timezone).unwrap();
            let end = next_date_start(date, timezone).unwrap();
            let mut starts = Vec::new();
            let mut current = start;
            while current < end {
                starts.push(current);
                current = next_local_five_minute_start(current, timezone).unwrap();
            }
            starts
        };
        let insert = |start: i64, state: &str, observed: i64, expected: i64| {
            connection
                .execute(
                    "INSERT INTO bucket_coverage (
                        source_id, granularity, bucket_start, bucket_end,
                        coverage_state, observed_units, expected_units, rejected_units
                     ) VALUES ('r1', '5m', ?1, ?2, ?3, ?4, ?5, 0)",
                    params![start, start + FIVE_MINUTES, state, observed, expected],
                )
                .unwrap();
        };

        let normal = local_starts("2025-01-01", "UTC");
        assert_eq!(normal.len(), 288);
        for start in &normal {
            insert(*start, "complete", 1, 1);
        }
        assert_eq!(
            count_incomplete_requested_coverage(&connection, &pipeline_for("2025-01-01", "UTC"))
                .unwrap(),
            0
        );
        connection
            .execute(
                "DELETE FROM bucket_coverage WHERE source_id = 'r1' AND bucket_start = ?1",
                params![normal[0]],
            )
            .unwrap();
        assert_eq!(
            count_incomplete_requested_coverage(&connection, &pipeline_for("2025-01-01", "UTC"))
                .unwrap(),
            1
        );
        insert(normal[0], "partial", 1, 2);
        assert_eq!(
            count_incomplete_requested_coverage(&connection, &pipeline_for("2025-01-01", "UTC"))
                .unwrap(),
            1
        );

        connection
            .execute("DELETE FROM bucket_coverage", [])
            .unwrap();
        let spring = local_starts("2025-03-09", DEFAULT_TIMEZONE);
        assert_eq!(spring.len(), 276);
        for start in &spring {
            insert(*start, "complete", 1, 1);
        }
        assert_eq!(
            count_incomplete_requested_coverage(
                &connection,
                &pipeline_for("2025-03-09", DEFAULT_TIMEZONE)
            )
            .unwrap(),
            0
        );

        connection
            .execute("DELETE FROM bucket_coverage", [])
            .unwrap();
        let fall = local_starts("2025-11-02", DEFAULT_TIMEZONE);
        assert_eq!(
            fall.len(),
            288,
            "preserve the current fall-back successor contract"
        );
        for start in &fall {
            insert(*start, "complete", 1, 1);
        }
        assert_eq!(
            count_incomplete_requested_coverage(
                &connection,
                &pipeline_for("2025-11-02", DEFAULT_TIMEZONE)
            )
            .unwrap(),
            0
        );
        connection
            .execute(
                "UPDATE bucket_coverage SET coverage_state = 'partial', observed_units = 1, expected_units = 2
                 WHERE source_id = 'r1' AND bucket_start = ?1",
                params![fall[0]],
            )
            .unwrap();
        assert_eq!(
            count_incomplete_requested_coverage(
                &connection,
                &pipeline_for("2025-11-02", DEFAULT_TIMEZONE)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn coarse_rollups_keep_observed_zeroes_without_fabricating_unknown_metrics() {
        let unknown = StatisticalBucket::new(BucketKey::new(
            "r1",
            Granularity::FiveMinutes,
            0,
            FIVE_MINUTES,
        ))
        .with_coverage(BucketCoverage::new(1, 0, 0).unwrap())
        .finish();
        let mut unknown_aggregates = AggregateBuckets::default();
        unknown_aggregates.include(&unknown, "UTC").unwrap();
        let (unknown_rollups, _) = unknown_aggregates.finish();
        assert_eq!(unknown_rollups.len(), 3);
        assert!(
            unknown_rollups
                .iter()
                .all(|bucket| bucket.traffic.is_empty())
        );

        let observed_zero = StatisticalBucket::dense(BucketKey::new(
            "r1",
            Granularity::FiveMinutes,
            0,
            FIVE_MINUTES,
        ))
        .with_coverage(BucketCoverage::complete_unit())
        .finish();
        let mut observed_aggregates = AggregateBuckets::default();
        observed_aggregates.include(&observed_zero, "UTC").unwrap();
        let (observed_rollups, _) = observed_aggregates.finish();
        assert_eq!(observed_rollups.len(), 3);
        assert!(observed_rollups.iter().all(|bucket| {
            !bucket.traffic.is_empty()
                && bucket.traffic.iter().all(|entry| entry.metrics.flows == 0)
        }));
    }

    #[test]
    fn pipeline_persists_in_process_maad_identity() {
        let temporary = tempdir().unwrap();
        let database = temporary.path().join("pipeline.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "UTC",
                "run_maad": true,
                "inputs": []
            }))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest::config(&config)).unwrap();

        let connection = Connection::open(database).unwrap();
        let config_json: Value = connection
            .query_row(
                "SELECT config_json FROM pipeline_product WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            config_json["maad"],
            json!({
                "enabled": true,
                "backend": "in-process",
                "contract_version": 2,
                "config": {
                    "q_min": -0.5,
                    "q_max": 3.5,
                    "q_step": 0.125,
                    "min_prefix_length": 8,
                    "max_prefix_length": 24,
                    "full_threshold": 0.05
                }
            })
        );
    }

    #[test]
    fn config_run_publishes_csv_gaps_as_coverage_only_buckets() {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let input = temporary.path().join("flows.csv");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {
                    "time_end": "time",
                    "src_ip": "src",
                    "dst_ip": "dst",
                    "protocol": "protocol",
                    "packets": "packets",
                    "bytes": "bytes"
                },
                "source_id": {"value": "r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &input,
            "time,src,dst,protocol,packets,bytes\n\
             2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10\n\
             2025-01-15 00:25:00,192.0.2.2,198.51.100.2,17,2,20\n",
        )
        .unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "UTC",
                "run_maad": false,
                "inputs": [{
                    "input_kind": "csv",
                    "path": input,
                    "mapping_path": mapping
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(&config)).unwrap();

        assert_eq!(report.five_minute_buckets, 6);
        assert_eq!(report.complete_five_minute_buckets, 2);
        assert_eq!(report.partial_five_minute_buckets, 0);
        assert_eq!(report.unknown_five_minute_buckets, 4);
        assert_eq!(report.rollup_buckets, 3);
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m' AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE granularity = '5m' AND coverage_state = 'unknown'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            4
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT flows FROM traffic_stats WHERE granularity = '30m' AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT coverage_state FROM bucket_coverage
                     WHERE granularity = '30m'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "partial"
        );

        // Simulate a legacy/in-progress product without planner statistics. The strict run still
        // returns its coverage error, but first leaves that inspectable product optimized.
        connection.execute("DELETE FROM sqlite_stat1", []).unwrap();

        let mut strict = PipelineRequest::config(&config);
        strict.require_complete = true;
        assert!(matches!(
            run(strict),
            Err(PipelineError::IncompleteCoverage(4))
        ));
        assert!(
            connection
                .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
                > 0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity IN ('1h', '1d')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            20
        );
    }

    /// Run a one-dataset pipeline over optional CSV rows and read back the stored start date.
    fn stored_default_start_date(dataset: Value, csv_rows: &str) -> String {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let input = temporary.path().join("flows.csv");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {
                    "time_end": "time",
                    "src_ip": "src",
                    "dst_ip": "dst",
                    "protocol": "protocol",
                    "packets": "packets",
                    "bytes": "bytes"
                },
                "source_id": {"value": "r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        let inputs = if csv_rows.is_empty() {
            json!([])
        } else {
            fs::write(
                &input,
                format!("time,src,dst,protocol,packets,bytes\n{csv_rows}"),
            )
            .unwrap();
            json!([{"input_kind": "csv", "path": input, "mapping_path": mapping}])
        };
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "America/Los_Angeles",
                "run_maad": false,
                "inputs": inputs,
                "datasets": [dataset]
            }))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest::config(&config)).unwrap();

        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT default_start_date FROM datasets WHERE id = 'example'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    }

    #[test]
    fn unset_start_date_becomes_the_earliest_ingested_local_day() {
        // 2025-01-15 00:00 UTC is still 2025-01-14 in the pipeline timezone.
        assert_eq!(
            stored_default_start_date(
                json!({"dataset_id": "example", "root_path": "/captures"}),
                "2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10\n",
            ),
            "2025-01-14"
        );
    }

    #[test]
    fn configured_start_date_survives_ingestion() {
        assert_eq!(
            stored_default_start_date(
                json!({
                    "dataset_id": "example",
                    "root_path": "/captures",
                    "default_start_date": "2024-12-25"
                }),
                "2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10\n",
            ),
            "2024-12-25"
        );
    }

    #[test]
    fn unset_start_date_falls_back_when_nothing_is_ingested() {
        assert_eq!(
            stored_default_start_date(
                json!({"dataset_id": "example", "root_path": "/captures"}),
                ""
            ),
            crate::storage::FALLBACK_DEFAULT_START_DATE
        );
    }

    #[test]
    fn csv_tree_files_share_rollups_within_one_transaction() {
        let temporary = tempdir().unwrap();
        let inputs = temporary.path().join("inputs");
        let mapping = temporary.path().join("mapping.json");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::create_dir(&inputs).unwrap();
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header":true,
                "timestamp_format":"datetime",
                "timestamp_timezone":"UTC",
                "columns":{"time_end":"time", "src_ip":"src", "dst_ip":"dst"},
                "source_id":{"value":"r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            inputs.join("2025-01-a.csv"),
            "time,src,dst\n\
             2025-01-15 00:00:00,192.0.2.1,198.51.100.1\n\
             2025-01-15 11:55:00,192.0.2.2,198.51.100.2\n",
        )
        .unwrap();
        fs::write(
            inputs.join("2025-01-b.csv"),
            "time,src,dst\n\
             2025-01-15 12:00:00,192.0.2.3,198.51.100.3\n\
             2025-01-15 23:55:00,192.0.2.4,198.51.100.4\n",
        )
        .unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "run_maad":false,
                "inputs":[{
                    "input_kind":"csv_tree",
                    "root_path":inputs,
                    "mapping_path":mapping
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(config)).unwrap();

        assert_eq!(report.input_scans, 2);
        assert_eq!(report.five_minute_buckets, 288);
        assert_eq!(report.rollup_buckets, 73);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(DISTINCT granularity || ':' || bucket_start) FROM traffic_stats",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            13
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM bucket_coverage", [], |row| row
                    .get::<_, i64>(0),)
                .unwrap(),
            361
        );
    }

    #[test]
    fn explicit_csv_files_share_rollups_within_one_transaction() {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let first = temporary.path().join("a.csv");
        let second = temporary.path().join("b.csv");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header":true,
                "timestamp_format":"datetime",
                "timestamp_timezone":"UTC",
                "columns":{"time_end":"time", "src_ip":"src", "dst_ip":"dst"},
                "source_id":{"value":"r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &first,
            "time,src,dst\n\
             2025-01-15 00:00:00,192.0.2.1,198.51.100.1\n\
             2025-01-15 00:10:00,192.0.2.2,198.51.100.2\n",
        )
        .unwrap();
        fs::write(
            &second,
            "time,src,dst\n\
             2025-01-15 00:15:00,192.0.2.3,198.51.100.3\n\
             2025-01-15 00:25:00,192.0.2.4,198.51.100.4\n",
        )
        .unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "run_maad":false,
                "inputs":[
                    {"input_kind":"csv", "path":first, "mapping_path":mapping},
                    {"input_kind":"csv", "path":second, "mapping_path":mapping}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(config)).unwrap();

        assert_eq!(report.input_scans, 2);
        assert_eq!(report.five_minute_buckets, 6);
        assert_eq!(report.rollup_buckets, 3);
    }

    #[test]
    fn overlapping_csv_batch_merges_metrics_and_coverage() {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let first = temporary.path().join("first.csv");
        let second = temporary.path().join("second.csv");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {"time_end":"time", "src_ip":"src", "dst_ip":"dst"},
                "source_id": {"value":"r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        for path in [&first, &second] {
            fs::write(
                path,
                "time,src,dst\n2025-01-15 00:00:00,192.0.2.1,198.51.100.1\n",
            )
            .unwrap();
        }
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone":"UTC",
                "run_maad":false,
                "inputs":[
                    {"input_kind":"csv", "path":first, "mapping_path":mapping},
                    {"input_kind":"csv", "path":second, "mapping_path":mapping}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(&config)).unwrap();
        assert_eq!(report.five_minute_buckets, 1);
        assert_eq!(report.complete_five_minute_buckets, 1);
        assert_eq!(report.partial_five_minute_buckets, 0);
        assert_eq!(report.unknown_five_minute_buckets, 0);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT flows FROM traffic_stats
                     WHERE granularity = '5m' AND source_id = 'r1'
                       AND bucket_start = 1736899200 AND ip_version = 4
                       AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT expected_units, observed_units, rejected_units
                     FROM bucket_coverage
                     WHERE granularity = '5m' AND source_id = 'r1'
                       AND bucket_start = 1736899200",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .unwrap(),
            (1, 1, 0)
        );
    }

    #[test]
    fn csv_files_fill_unknown_buckets_across_global_source_envelope() {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let first = temporary.path().join("first.csv");
        let second = temporary.path().join("second.csv");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header": true,
                "timestamp_format": "datetime",
                "timestamp_timezone": "UTC",
                "columns": {"time_end":"time", "src_ip":"src", "dst_ip":"dst"},
                "source_id": {"value": "r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &first,
            "time,src,dst\n2025-01-15 00:00:00,192.0.2.1,198.51.100.1\n",
        )
        .unwrap();
        fs::write(
            &second,
            "time,src,dst\n2025-01-15 00:25:00,192.0.2.2,198.51.100.2\n",
        )
        .unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "UTC",
                "run_maad": false,
                "inputs": [
                    {"input_kind": "csv", "path": first, "mapping_path": mapping},
                    {"input_kind": "csv", "path": second, "mapping_path": mapping}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(&config)).unwrap();
        assert_eq!(report.five_minute_buckets, 6);
        assert_eq!(report.complete_five_minute_buckets, 2);
        assert_eq!(report.partial_five_minute_buckets, 0);
        assert_eq!(report.unknown_five_minute_buckets, 4);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'r1' AND granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            6
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'r1' AND granularity = '5m'
                       AND coverage_state = 'unknown'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            4
        );
    }

    #[test]
    fn fall_back_hours_have_distinct_hour_rollups() {
        let first = "2024-11-03T01:15:00-07:00[America/Los_Angeles]"
            .parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
            .as_second();
        let second = "2024-11-03T01:15:00-08:00[America/Los_Angeles]"
            .parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
            .as_second();

        let first_bounds =
            aggregate_bounds(first, Granularity::OneHour, "America/Los_Angeles").unwrap();
        let second_bounds =
            aggregate_bounds(second, Granularity::OneHour, "America/Los_Angeles").unwrap();

        assert_eq!(first_bounds.0, first - 15 * 60);
        assert_eq!(second_bounds.0, second - 15 * 60);
        assert_ne!(first_bounds, second_bounds);
    }

    #[test]
    fn tree_windows_must_cover_complete_selected_local_days() {
        let selected_start = parse_date_start("2025-02-11", "America/Los_Angeles").unwrap();
        let selected_end = next_date_start("2025-02-11", "America/Los_Angeles").unwrap();

        validate_window(
            selected_start,
            selected_end,
            selected_start,
            selected_end,
            "America/Los_Angeles",
        )
        .unwrap();
        assert!(
            validate_window(
                selected_start,
                selected_end,
                selected_start + FIVE_MINUTES,
                selected_end,
                "America/Los_Angeles",
            )
            .unwrap_err()
            .to_string()
            .contains("local-day boundary")
        );
        assert!(
            validate_window(
                selected_start,
                selected_end,
                selected_start - 86_400,
                selected_end,
                "America/Los_Angeles",
            )
            .unwrap_err()
            .to_string()
            .contains("on or after")
        );
    }

    #[test]
    fn later_partial_scan_cannot_reopen_persisted_rollups() {
        let temporary = tempdir().unwrap();
        let mapping = temporary.path().join("mapping.json");
        let first = temporary.path().join("first.csv");
        let second = temporary.path().join("second.csv");
        let database = temporary.path().join("netflow.sqlite");
        fs::write(
            &mapping,
            serde_json::to_vec(&json!({
                "has_header":true,
                "timestamp_format":"datetime",
                "timestamp_timezone":"UTC",
                "columns":{"time_end":"time", "src_ip":"src", "dst_ip":"dst"},
                "source_id":{"value":"r1"}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &first,
            "time,src,dst\n2025-01-15 00:00:00,192.0.2.1,198.51.100.1\n",
        )
        .unwrap();
        fs::write(
            &second,
            "time,src,dst\n2025-01-15 00:05:00,192.0.2.2,198.51.100.2\n",
        )
        .unwrap();
        for (index, input) in [&first, &second].into_iter().enumerate() {
            let config = temporary.path().join(format!("pipeline-{index}.json"));
            fs::write(
                &config,
                serde_json::to_vec(&json!({
                    "database_path":database,
                    "timezone":"UTC",
                    "run_maad":false,
                    "inputs":[{"input_kind":"csv", "path":input, "mapping_path":mapping}]
                }))
                .unwrap(),
            )
            .unwrap();
            if index == 0 {
                run(PipelineRequest::config(config)).unwrap();
            } else {
                let error = run(PipelineRequest::config(config)).unwrap_err();
                assert!(error.to_string().contains("cannot reopen"));
            }
        }
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn unchanged_file_revision_reuses_the_persisted_digest() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("nfcapd.202501010000");
        fs::write(&path, "original").unwrap();
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();
        let locator = path.to_string_lossy().into_owned();
        let original = InputRevision::create("nfcapd", &locator, "digest", "decoder").unwrap();
        upsert_input_bucket(
            &connection,
            &InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: locator.clone(),
                scan_locator: locator.clone(),
                source_id: "r1".into(),
                bucket_start: 0,
                bucket_end: FIVE_MINUTES,
                revision: original.clone(),
                file_snapshot: Some(snapshot),
            },
            false,
        )
        .unwrap();
        mark_input_bucket_status(
            &connection,
            InputKind::Nfcapd,
            &locator,
            "r1",
            0,
            InputStatus::Processed,
            &original,
            None,
        )
        .unwrap();

        let (cached, _) = prepare_file_revision_with(
            &connection,
            &path,
            InputKind::Nfcapd,
            "decoder".into(),
            || panic!("unchanged input must not be rehashed"),
        )
        .unwrap();
        assert_eq!(cached, original);

        fs::write(&path, "replacement with a different size").unwrap();
        let rehashed = std::cell::Cell::new(false);
        let (changed, _) = prepare_file_revision_with(
            &connection,
            &path,
            InputKind::Nfcapd,
            "decoder".into(),
            || {
                rehashed.set(true);
                capture_file_revision(&path)
            },
        )
        .unwrap();
        assert!(rehashed.get());
        assert_ne!(changed.content_fingerprint, original.content_fingerprint);
    }

    #[test]
    fn force_nfcapd_revision_resolution_rehashes_matching_snapshots() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("nfcapd.202501010000");
        fs::write(&path, "actual bytes").unwrap();
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let snapshot = FileSnapshot::capture(&path).unwrap();
        let locator = path.to_string_lossy().into_owned();
        let stale = InputRevision::create("nfcapd", &locator, "stale", "decoder").unwrap();
        upsert_input_bucket(
            &connection,
            &InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: locator.clone(),
                scan_locator: locator.clone(),
                source_id: "r1".into(),
                bucket_start: 0,
                bucket_end: FIVE_MINUTES,
                revision: stale,
                file_snapshot: Some(snapshot),
            },
            false,
        )
        .unwrap();
        mark_input_bucket_status(
            &connection,
            InputKind::Nfcapd,
            &locator,
            "r1",
            0,
            InputStatus::Processed,
            &InputRevision::create("nfcapd", &locator, "stale", "decoder").unwrap(),
            None,
        )
        .unwrap();

        let sources = [DatasetSource {
            source_id: "r1".into(),
            members: vec!["r1".into()],
        }];
        let paths = BTreeMap::from([(("r1".into(), 0), path.clone())]);
        let bounds = BTreeMap::from([("r1".into(), (0, 0))]);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let normal_context = NfcapdRevisionContext {
            connection: &connection,
            sources: &sources,
            by_member_and_start: &paths,
            member_bounds: &bounds,
            extend_gaps_to_window: false,
            force: false,
            decoder_fingerprint: "decoder".into(),
            capture_snapshots: &BTreeMap::new(),
            revision_pool: &pool,
        };
        let cached = resolve_nfcapd_batch_revisions(&normal_context, &[0])
            .unwrap()
            .remove(&path)
            .unwrap();
        assert_eq!(cached.revision.content_fingerprint, "stale");

        let forced_context = NfcapdRevisionContext {
            force: true,
            ..normal_context
        };
        let forced = resolve_nfcapd_batch_revisions(&forced_context, &[0])
            .unwrap()
            .remove(&path)
            .unwrap();
        let (actual_digest, _) = capture_file_revision(&path).unwrap();
        assert_eq!(forced.revision.content_fingerprint, actual_digest);
    }

    #[cfg(unix)]
    #[test]
    fn nfcapd_tree_commits_completed_days_before_a_later_day_fails() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let first = root.join("r1/2025/01/01/nfcapd.202501010000");
        let second = root.join("r1/2025/01/02/nfcapd.202501020000");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "case \"$*\" in *20250102*) exit 9;; esac");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[{
                    "input_kind":"nfcapd_tree",
                    "root_path":root,
                    "source_ids":["r1"],
                    "start_date":"2025-01-01",
                    "end_date":"2025-01-02"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let error = run(PipelineRequest::config(config)).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("nfcapd decode failed"), "{message}");
        assert!(message.contains("member \"r1\""), "{message}");
        assert!(
            message.contains(&second.to_string_lossy().to_string()),
            "{message}"
        );
        let connection = Connection::open(database).unwrap();
        let starts = connection
            .prepare("SELECT bucket_start FROM processed_inputs ORDER BY bucket_start")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(starts, [1_735_689_600]);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_nfcapd_files_share_one_transaction() {
        let temporary = tempdir().unwrap();
        let first = temporary.path().join("nfcapd.202501010000");
        let second = temporary.path().join("nfcapd.202501010005");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[
                    {"input_kind":"nfcapd", "path":first, "source_id":"r1"},
                    {"input_kind":"nfcapd", "path":second, "source_id":"r1"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(config)).unwrap();

        assert_eq!(report.five_minute_buckets, 2);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn fall_back_tree_uses_wall_clock_filenames_without_repeating_one_am() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let capture = root.join("r1/2025/11/02/nfcapd.202511020100");
        fs::create_dir_all(capture.parent().unwrap()).unwrap();
        fs::write(&capture, "capture").unwrap();
        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"America/Los_Angeles",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[{
                    "input_kind":"nfcapd_tree",
                    "root_path":root,
                    "source_ids":["r1"],
                    "start_date":"2025-11-02",
                    "end_date":"2025-11-02"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(config)).unwrap();

        assert_eq!(report.five_minute_buckets, 288);
        assert_eq!(report.rollup_buckets, 73);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(DISTINCT bucket_start) FROM processed_inputs",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "missing inputs are evidence, not synthetic processed revisions"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage WHERE granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            288
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE granularity = '5m' AND coverage_state = 'unknown'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            287
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            10,
            "unknown buckets must not fabricate zero-valued metric rows"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '1d'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            10
        );
    }

    #[cfg(unix)]
    #[test]
    fn newly_arrived_member_repairs_five_minute_coverage_without_erasing_disappeared_inputs() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let first = root.join("r1/2025/01/01/nfcapd.202501010000");
        let second = root.join("r2/2025/01/01/nfcapd.202501010000");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "first").unwrap();
        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[{
                    "input_kind":"nfcapd_tree",
                    "root_path":root,
                    "sources":[{"source_id":"both", "members":["r1", "r2"]}],
                    "start_date":"2025-01-01",
                    "end_date":"2025-01-01"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest::config(&config)).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT coverage_state FROM bucket_coverage
                     WHERE source_id = 'both' AND granularity = '5m'
                       AND bucket_start = 1735689600",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "partial"
        );
        drop(connection);

        fs::write(&second, "second").unwrap();
        run(PipelineRequest::config(&config)).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT coverage_state FROM bucket_coverage
                     WHERE source_id = 'both' AND granularity = '5m'
                       AND bucket_start = 1735689600",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "complete"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT flows FROM traffic_stats
                     WHERE source_id = 'both' AND granularity = '5m'
                       AND bucket_start = 1735689600 AND ip_version = 4
                       AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats
                     WHERE source_id = 'both' AND granularity <> '5m'
                       AND bucket_start = 1735689600",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "repair invalidates coarse derived rows that cannot be patched exactly"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM bucket_coverage
                     WHERE source_id = 'both' AND granularity <> '5m'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            73,
            "capture coverage remains available when derived metrics are invalidated"
        );
        drop(connection);

        fs::remove_file(&first).unwrap();
        run(PipelineRequest::config(&config)).unwrap();
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT observed_units FROM bucket_coverage
                     WHERE source_id = 'both' AND granularity = '5m'
                       AND bucket_start = 1735689600",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2,
            "a disappearing file must not erase prior observations"
        );
    }

    #[cfg(unix)]
    #[test]
    fn implicit_tree_end_models_only_each_members_observed_bounds() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let first = root.join("r1/2025/01/01/nfcapd.202501010000");
        let second = root.join("r2/2025/01/02/nfcapd.202501020000");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"UTC",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[{
                    "input_kind":"nfcapd_tree",
                    "root_path":root,
                    "source_ids":["r1", "r2"],
                    "start_date":"2025-01-01"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(PipelineRequest::config(config)).unwrap();

        assert_eq!(report.five_minute_buckets, 2);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_nfcapd_input_is_skipped_before_running_decoder_again() {
        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202504151200");
        let decoder = temporary.path().join("fake-nfdump");
        let calls = temporary.path().join("calls");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(&capture, "fixture").unwrap();
        write_fake_nfdump(&decoder, &format!("echo called >> '{}'", calls.display()));
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path":database,
                "timezone":"America/Los_Angeles",
                "nfdump":decoder,
                "run_maad":false,
                "inputs":[{"input_kind":"nfcapd", "path":capture, "source_id":"r1"}]
            }))
            .unwrap(),
        )
        .unwrap();

        run(PipelineRequest::config(&config)).unwrap();
        run(PipelineRequest::config(&config)).unwrap();

        assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_revision_conflicts_before_a_second_binary_can_mix_product_rows() {
        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202504151200");
        let first_decoder = temporary.path().join("nfdump-a");
        let second_decoder = temporary.path().join("nfdump-b");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(&capture, "fixture").unwrap();
        write_fake_nfdump(&first_decoder, "");
        write_fake_nfdump(&second_decoder, "");
        let write_config = |decoder: &Path| {
            fs::write(
                &config,
                serde_json::to_vec(&json!({
                    "database_path": database,
                    "timezone": "America/Los_Angeles",
                    "nfdump": decoder,
                    "run_maad": false,
                    "inputs": [{"input_kind": "nfcapd", "path": capture, "source_id": "r1"}]
                }))
                .unwrap(),
            )
            .unwrap();
        };

        write_config(&first_decoder);
        run(PipelineRequest::config(&config)).unwrap();
        let connection = Connection::open(&database).unwrap();
        let before_inputs: i64 = connection
            .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                row.get(0)
            })
            .unwrap();
        let before_traffic: i64 = connection
            .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| row.get(0))
            .unwrap();
        drop(connection);

        write_config(&second_decoder);
        let error = run(PipelineRequest::config(&config)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Pipeline product identity mismatch")
        );

        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            before_inputs
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            before_traffic
        );
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_replacement_after_decode_rolls_back_the_native_transaction() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202504151200");
        let decoder = temporary.path().join("nfdump");
        let replacement = temporary.path().join("nfdump-replacement");
        let stream = temporary.path().join("stream.bin");
        let empty_stream = temporary.path().join("empty.stream");
        let database = temporary.path().join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(&capture, "fixture").unwrap();
        fs::write(&stream, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            &empty_stream,
            [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(
            &replacement,
            format!("#!/bin/sh\ncat '{}'\n", stream.display()),
        )
        .unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            &decoder,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ]; then cat '{}'; exit 0; fi\ncat '{}'\ncp '{}' \"$0\"\n",
                empty_stream.display(),
                stream.display(),
                replacement.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&decoder, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "America/Los_Angeles",
                "nfdump": decoder,
                "run_maad": false,
                "inputs": [{"input_kind": "nfcapd", "path": capture, "source_id": "r1"}]
            }))
            .unwrap(),
        )
        .unwrap();

        let error = run(PipelineRequest::config(&config)).unwrap_err();
        assert!(
            error.to_string().contains("nfdump executable changed"),
            "{error}"
        );
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_nfdump_probe_fails_before_output_setup() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let capture = temporary.path().join("nfcapd.202504151200");
        let decoder = temporary.path().join("incompatible-nfdump");
        let output_directory = temporary.path().join("outputs");
        let database = output_directory.join("netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(&capture, "fixture").unwrap();
        fs::write(&decoder, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&decoder, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": "America/Los_Angeles",
                "nfdump": decoder,
                "run_maad": false,
                "inputs": [{"input_kind": "nfcapd", "path": capture, "source_id": "r1"}]
            }))
            .unwrap(),
        )
        .unwrap();

        let error = run(PipelineRequest::config(&config)).unwrap_err();
        assert!(error.to_string().contains("compatibility probe"), "{error}");
        assert!(!output_directory.exists());
        assert!(!database_operation_lock_path(&database).unwrap().exists());
    }
}
