//! End-to-end pipeline orchestration.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::{RoundMode, Timestamp, ToSpan, Unit, ZonedRound, civil::Date};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    config::{ConfigError, CsvSourceConfig},
    coverage::BucketCoverage,
    domain::{
        AddressSet, BucketKey, CanonicalBucket, DomainError, FlowSelection, Granularity,
        StatisticalBucket,
    },
    ingest::{self, IngestError, ProducerError},
    nfdump,
    provenance::{
        ExecutableRevision, ExpectedAbsence, FileSnapshot, InputRevision, ProvenanceError,
        capture_file_revision, csv_decoder_fingerprint, nfcapd_decoder_fingerprint,
        revision_for_locator, verify_file_snapshot,
    },
    publish::{PublishError, write_buckets},
    registry::{Dataset, DatasetRegistry, DatasetSource, RegistryError, is_safe_path_component},
    storage::{
        DatabaseOperationLock, DatasetMetadata, InputBucket, InputEvidenceRow, InputEvidenceState,
        InputKind, InputStatus, ProductIdentity, SourceDefinition, StorageError,
        bind_nfcapd_source_layout, bind_product_identity, cached_content_fingerprint,
        canonical_path, complete_input_scan, connect_pipeline_writer, current_product_fingerprint,
        daily_product_completion_matches, database_related_paths, delete_stats_time_range,
        earliest_traffic_bucket_start, init_schema, input_scan_fully_processed,
        mark_input_bucket_status, nfcapd_logical_bucket_processed,
        optimize_all_query_planner_statistics, query_input_evidence, replace_input_evidence,
        set_dataset_default_start_date, upsert_daily_product_completion, upsert_dataset_metadata,
        upsert_input_bucket, validate_database_path_separation,
    },
};

const FIVE_MINUTES: i64 = 300;
const NFCAPD_DECODE_BATCH_SIZE: usize = 12;
const NFCAPD_REVISION_HASH_MAX_WORKERS: usize = NFCAPD_DECODE_BATCH_SIZE * 2;
const MAX_MISSING_DAY_WARNING_DETAILS: usize = 8;
const DEFAULT_TIMEZONE: &str = "America/Los_Angeles";

fn build_revision_hash_pool() -> Result<rayon::ThreadPool, PipelineError> {
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

fn build_nfcapd_decode_pool() -> Result<rayon::ThreadPool, PipelineError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(NFCAPD_DECODE_BATCH_SIZE)
        .thread_name(|index| format!("nfcapd-decode-{index}"))
        .build()
        .map_err(|error| {
            PipelineError::InvalidConfig(format!("failed to build nfcapd decode pool: {error}"))
        })
}

fn build_nfcapd_activity_pool() -> Result<rayon::ThreadPool, PipelineError> {
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

fn has_effective_execute_access(path: &Path) -> bool {
    nix::unistd::faccessat(
        None,
        path,
        nix::unistd::AccessFlags::X_OK,
        nix::fcntl::AtFlags::AT_EACCESS,
    )
    .is_ok()
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
/// The datasets share the same frozen discovery plan, day loop, and nfdump work used by [`run`],
/// while each output retains its own product identity, transaction, and completion markers.
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
            &registry,
            Some((&shared_nfdump.0, &shared_nfdump.1)),
        )?);
    }
    execute_many(validate_compatible_pipelines(pipelines)?)
}

fn selection_override_requested(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.values().any(|entry| !entry.is_null()))
}

struct CompatiblePlan {
    pipelines: Vec<ResolvedPipeline>,
    tree: FrozenNfcapdTreeLayout,
}

fn validate_compatible_pipelines(
    pipelines: Vec<ResolvedPipeline>,
) -> Result<CompatiblePlan, PipelineError> {
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
    let output_paths = pipelines
        .iter()
        .map(|pipeline| pipeline.database_path.as_path())
        .collect::<Vec<_>>();
    validate_database_path_separation(&output_paths)?;
    let tree = freeze_nfcapd_tree(
        first_input,
        &first.selection,
        &first.timezone,
        &output_paths,
    )?;
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
        if tree.root_path != fs::canonicalize(config.root_path)? {
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
        if tree.sources != canonical_logical_sources(input)? {
            return Err(PipelineError::InvalidConfig(
                "coordinated datasets must use the same logical source layout and membership"
                    .into(),
            ));
        }
    }
    if first.nfdump_revision.is_some() {
        ingest::probe_nfdump_compatibility(&first.nfdump)?;
    }
    Ok(CompatiblePlan { pipelines, tree })
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_output_capture_separation(
    output_paths: &[&Path],
    capture_root: &Path,
    member_ids: &[String],
) -> Result<(), PipelineError> {
    let capture_root = fs::canonicalize(capture_root)?;
    let mut capture_paths = vec![capture_root.clone()];
    for member in member_ids {
        capture_paths.push(fs::canonicalize(capture_root.join(member))?);
    }
    for path in output_paths {
        for output in database_related_paths(path)? {
            if capture_paths
                .iter()
                .any(|capture| paths_overlap(&output, capture))
            {
                return Err(PipelineError::InvalidConfig(format!(
                    "output database {} overlaps the nfcapd capture tree {}",
                    path.display(),
                    capture_root.display()
                )));
            }
        }
    }
    Ok(())
}

fn validate_daily_active_source_layout(
    sources: &[DatasetSource],
    physical_ids: &[String],
) -> Result<(), PipelineError> {
    if sources.is_empty() || physical_ids.is_empty() {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources requires at least one logical and physical source".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct FrozenNfcapdTreeLayout {
    root_path: PathBuf,
    sources: Vec<DatasetSource>,
    physical_ids: Vec<String>,
    by_member_and_start: BTreeMap<(String, i64), PathBuf>,
    member_bounds: BTreeMap<String, (i64, i64)>,
    start: i64,
    end: i64,
    extend_gaps_to_window: bool,
    force: bool,
}

#[derive(Clone, Debug, Default)]
struct SingleOutputPlan {
    trees: BTreeMap<usize, FrozenNfcapdTreeLayout>,
    dataset_sources: BTreeMap<String, Vec<DatasetSource>>,
}

fn freeze_nfcapd_tree(
    input: &InputSpec,
    selection: &FlowSelection,
    timezone: &str,
    output_paths: &[&Path],
) -> Result<FrozenNfcapdTreeLayout, PipelineError> {
    let InputSpec::NfcapdTree {
        root_path,
        source_ids,
        sources,
        start_date,
        end_date,
        start_time,
        end_time,
        force,
    } = input
    else {
        return Err(PipelineError::InvalidConfig(
            "expected an nfcapd_tree input".into(),
        ));
    };
    if selection.selects_daily_active_sources() && (start_time.is_some() || end_time.is_some()) {
        return Err(PipelineError::InvalidConfig(
            "daily_active_sources selection requires whole local calendar days; start_time and end_time are unsupported".into(),
        ));
    }

    let root_path = fs::canonicalize(root_path)?;
    let sources = normalize_sources(&root_path, source_ids, sources)?;
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if selection.selects_daily_active_sources() {
        validate_daily_active_source_layout(&sources, &physical_ids)?;
    }
    validate_output_capture_separation(output_paths, &root_path, &physical_ids)?;

    let discovered = ingest::discover_nfcapd_source_paths(&root_path, &physical_ids, timezone)?;
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
        end_date.as_deref(),
        start_time.as_deref(),
        end_time.as_deref(),
        by_member_and_start.keys().map(|(_, start)| *start),
        timezone,
    )?;

    Ok(FrozenNfcapdTreeLayout {
        root_path,
        sources,
        physical_ids,
        by_member_and_start,
        member_bounds,
        start: window.start,
        end: window.end,
        extend_gaps_to_window: end_date.is_some(),
        force: *force,
    })
}

fn plan_single_output(pipeline: &ResolvedPipeline) -> Result<SingleOutputPlan, PipelineError> {
    let output_path = pipeline.database_path.as_path();
    let output_paths = std::slice::from_ref(&output_path);
    let mut trees = BTreeMap::new();
    for (input_index, input) in pipeline.inputs.iter().enumerate() {
        if matches!(input, InputSpec::NfcapdTree { .. }) {
            trees.insert(
                input_index,
                freeze_nfcapd_tree(input, &pipeline.selection, &pipeline.timezone, output_paths)?,
            );
        }
    }
    if pipeline.nfdump_revision.is_some() {
        ingest::probe_nfdump_compatibility(&pipeline.nfdump)?;
    }

    let mut dataset_sources = BTreeMap::new();
    for dataset in &pipeline.datasets {
        let dataset_root = fs::canonicalize(&dataset.root_path)?;
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
    })
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
        return Ok(ResolvedPipeline {
            database_path: config.database_path,
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
    resolve_dataset_request(request, &registry, None)
}

fn load_dataset_registry(
    registry_path: &Path,
    repository_root: &Path,
) -> Result<DatasetRegistry, PipelineError> {
    Ok(DatasetRegistry::load(registry_path, repository_root)?)
}

fn resolve_dataset_request(
    request: &PipelineRequest,
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
    Ok(ResolvedPipeline {
        database_path: request
            .database_path
            .clone()
            .unwrap_or_else(|| dataset.db_path.clone()),
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
            InputSpec::NfcapdTree { .. } => {
                let mut sinks = [ProductSink {
                    pipeline: &pipeline,
                    connection: &connection,
                    report: &mut report,
                }];
                process_nfcapd_tree(
                    plan.trees
                        .get(&input_index)
                        .expect("every nfcapd_tree input has a frozen layout"),
                    &mut sinks,
                )?;
            }
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
    connection: Connection,
    report: PipelineReport,
    _lock: DatabaseOperationLock,
}

struct ProductSink<'a> {
    pipeline: &'a ResolvedPipeline,
    connection: &'a Connection,
    report: &'a mut PipelineReport,
}

fn execute_many(plan: CompatiblePlan) -> Result<PipelineReport, PipelineError> {
    let CompatiblePlan { pipelines, tree } = plan;
    let mut dataset_sources = BTreeMap::new();
    for pipeline in &pipelines {
        let dataset = pipeline.datasets.first().ok_or_else(|| {
            PipelineError::InvalidConfig(
                "coordinated datasets require registry-backed dataset metadata".into(),
            )
        })?;
        dataset_sources.insert(dataset.dataset_id.clone(), tree.sources.clone());
    }

    let mut outputs = Vec::with_capacity(pipelines.len());
    for pipeline in pipelines {
        if let Some(parent) = pipeline.database_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock =
            DatabaseOperationLock::acquire(&pipeline.database_path, "coordinated pipeline build")?;
        let connection = connect_pipeline_writer(&pipeline.database_path)?;
        init_schema(&connection)?;
        with_transaction(&connection, || {
            initialize_coordinated_metadata_in_transaction(
                &connection,
                &pipeline,
                &tree.sources,
                &dataset_sources,
            )
        })?;
        outputs.push(CoordinatedOutput {
            pipeline,
            connection,
            report: PipelineReport::default(),
            _lock: lock,
        });
    }

    {
        let mut sinks = outputs
            .iter_mut()
            .map(|output| ProductSink {
                pipeline: &output.pipeline,
                connection: &output.connection,
                report: &mut output.report,
            })
            .collect::<Vec<_>>();
        process_nfcapd_tree(&tree, &mut sinks)?;
    }

    let mut report = PipelineReport::default();
    for output in &mut outputs {
        infer_default_start_dates(&output.connection, &output.pipeline)?;
        populate_coverage_summary(&output.connection, &mut output.report)?;
        if let Err(error) = optimize_all_query_planner_statistics(&output.connection) {
            tracing::warn!(%error, "could not refresh SQLite planner statistics");
        }
        if output.pipeline.require_complete {
            let incomplete = count_incomplete_coverage_for_layout(
                &output.connection,
                &tree.sources,
                tree.start,
                tree.end,
                &output.pipeline.timezone,
            )?;
            if incomplete != 0 {
                return Err(PipelineError::IncompleteCoverage(incomplete));
            }
        }
        merge_report(&mut report, std::mem::take(&mut output.report));
    }
    Ok(report)
}
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
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(StorageError::from)?;
    let result = operation().and_then(|value| {
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

/// A finite native request is checked against the same frozen window and source layout that was
/// used for publication. CSV and literal-input configurations have no separately declared window,
/// so their configured product remains the strict scope.
fn requested_coverage_scopes_with_plan(
    pipeline: &ResolvedPipeline,
    plan: &SingleOutputPlan,
) -> Result<Option<Vec<CoverageScope>>, PipelineError> {
    let mut scopes = Vec::new();
    for (input_index, input) in pipeline.inputs.iter().enumerate() {
        if !matches!(input, InputSpec::NfcapdTree { .. }) {
            return Ok(None);
        }
        let tree = plan
            .trees
            .get(&input_index)
            .expect("every nfcapd_tree input has a frozen layout");
        scopes.push(CoverageScope {
            source_ids: tree
                .sources
                .iter()
                .map(|source| source.source_id.clone())
                .collect(),
            start: tree.start,
            end: tree.end,
        });
    }
    Ok(Some(scopes))
}

fn count_incomplete_requested_coverage_with_plan(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
    plan: &SingleOutputPlan,
) -> Result<i64, PipelineError> {
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

fn nfcapd_day_is_complete(
    sink: &ProductSink<'_>,
    sources: &[DatasetSource],
    start: i64,
    end: i64,
) -> Result<bool, PipelineError> {
    let Some(product_fingerprint) = current_product_fingerprint(sink.connection)? else {
        return Ok(false);
    };
    if sources.is_empty() {
        return Ok(false);
    }
    for source in sources {
        if !daily_product_completion_matches(
            sink.connection,
            &source.source_id,
            start,
            end,
            &product_fingerprint,
            sink.pipeline.run_maad,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn rollback_sink_transactions(sinks: &[ProductSink<'_>], transactions: &[bool]) {
    for (sink, active) in sinks.iter().zip(transactions) {
        if *active {
            let _ = sink.connection.execute_batch("ROLLBACK");
        }
    }
}

fn process_nfcapd_tree(
    tree: &FrozenNfcapdTreeLayout,
    sinks: &mut [ProductSink<'_>],
) -> Result<(), PipelineError> {
    let Some(first) = sinks.first() else {
        return Ok(());
    };
    let timezone = first.pipeline.timezone.clone();
    let daily_active = first.pipeline.selection.selects_daily_active_sources();
    let source_ids = tree
        .sources
        .iter()
        .map(|source| source.source_id.clone())
        .collect::<Vec<_>>();
    let mut day_start = tree.start;

    while day_start < tree.end {
        let day_end = aggregate_bounds(day_start, Granularity::OneDay, &timezone)?
            .1
            .min(tree.end);
        let mut pending = Vec::new();
        for (index, sink) in sinks.iter().enumerate() {
            if tree.force || !nfcapd_day_is_complete(sink, &tree.sources, day_start, day_end)? {
                pending.push(index);
            }
        }
        if pending.is_empty() {
            day_start = day_end;
            continue;
        }

        let missing = daily_active
            .then(|| {
                missing_physical_day_inputs(
                    &tree.physical_ids,
                    &tree.by_member_and_start,
                    day_start,
                    day_end,
                    &timezone,
                )
            })
            .transpose()?
            .unwrap_or_default();

        let mut transactions = vec![false; sinks.len()];
        for &index in &pending {
            if let Err(error) = sinks[index].connection.execute_batch("BEGIN IMMEDIATE") {
                rollback_sink_transactions(sinks, &transactions);
                return Err(PipelineError::Storage(StorageError::from(error)));
            }
            transactions[index] = true;
            if let Err(error) =
                delete_stats_time_range(sinks[index].connection, &source_ids, day_start, day_end)
            {
                rollback_sink_transactions(sinks, &transactions);
                return Err(PipelineError::Storage(error));
            }
        }

        if daily_active && !missing.is_empty() {
            let details = missing_day_warning_details(&tree.root_path, &missing, &timezone)?;
            tracing::warn!(
                day_start,
                day_end,
                missing_inputs = missing.len(),
                missing_details = %details,
                "skipping incomplete physical day for daily_active_sources selection"
            );
            sinks[pending[0]].report.skipped_inputs += missing.len();
        } else {
            let mut owned_keys = BTreeSet::new();
            let mut bucket_start = day_start;
            while bucket_start < day_end {
                for source in &tree.sources {
                    if source_has_candidate(
                        source,
                        bucket_start,
                        &tree.by_member_and_start,
                        &tree.member_bounds,
                        tree.extend_gaps_to_window,
                    ) {
                        owned_keys.insert((source.source_id.clone(), bucket_start));
                    }
                }
                bucket_start = next_local_five_minute_start(bucket_start, &timezone)?;
            }
            let mut aggregates = (0..sinks.len())
                .map(|index| {
                    pending
                        .contains(&index)
                        .then(|| AggregateBuckets::with_owned_keys(owned_keys.clone()))
                })
                .collect::<Vec<_>>();
            let result =
                process_nfcapd_tree_day(tree, day_start, day_end, sinks, &pending, &mut aggregates);
            if let Err(error) = result {
                rollback_sink_transactions(sinks, &transactions);
                return Err(error);
            }
            for &index in &pending {
                let aggregate = aggregates[index]
                    .take()
                    .expect("pending output has aggregate state");
                if let Err(error) = publish_rollups(
                    sinks[index].connection,
                    aggregate,
                    sinks[index].pipeline,
                    sinks[index].report,
                ) {
                    rollback_sink_transactions(sinks, &transactions);
                    return Err(error);
                }
                if let Err(error) = mark_nfcapd_day_complete(
                    sinks[index].connection,
                    &tree.sources,
                    day_start,
                    day_end,
                    sinks[index].pipeline.run_maad,
                ) {
                    rollback_sink_transactions(sinks, &transactions);
                    return Err(error);
                }
            }
        }

        if let Err(error) = verify_nfdump_revision(sinks[pending[0]].pipeline) {
            rollback_sink_transactions(sinks, &transactions);
            return Err(error);
        }

        for &index in &pending {
            if let Err(error) = sinks[index].connection.execute_batch("COMMIT") {
                rollback_sink_transactions(sinks, &transactions);
                return Err(PipelineError::Storage(StorageError::from(error)));
            }
            transactions[index] = false;
        }
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

/// Publish completion markers in the same transaction as the day's product rows.
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

fn nfcapd_day_activity_paths(
    paths: &BTreeMap<(String, i64), PathBuf>,
    member: &str,
    start: i64,
    end: i64,
    timezone: &str,
) -> Result<Vec<PathBuf>, PipelineError> {
    let mut result = Vec::new();
    let mut bucket_start = start;
    while bucket_start < end {
        if let Some(path) = paths.get(&(member.to_owned(), bucket_start)) {
            result.push(path.clone());
        }
        bucket_start = next_local_five_minute_start(bucket_start, timezone)?;
    }
    Ok(result)
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

fn resolve_daily_active_sources(
    tree: &FrozenNfcapdTreeLayout,
    start: i64,
    end: i64,
    timezone: &str,
    selections: &[FlowSelection],
    executable: &Path,
) -> Result<Vec<Arc<AddressSet>>, PipelineError> {
    let pool = build_nfcapd_activity_pool()?;
    let requests = tree
        .physical_ids
        .iter()
        .map(|member| {
            nfcapd_day_activity_paths(&tree.by_member_and_start, member, start, end, timezone)
                .map(|paths| (member.clone(), paths))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let results = pool.install(|| {
        requests
            .par_iter()
            .map(|(member, paths)| {
                ingest::read_nfcapd_daily_source_activities(paths, selections, executable)
                    .map_err(|error| {
                        PipelineError::InvalidConfig(format!(
                            "daily activity scan failed for member {member:?}, day {start}..{end}: {error}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
    })?;

    let mut combined = (0..selections.len())
        .map(|_| HashMap::<IpAddr, nfdump::SourceActivity>::new())
        .collect::<Vec<_>>();
    for member_results in results {
        for (selection_index, activity) in member_results.into_iter().enumerate() {
            for (address, metrics) in activity {
                combined[selection_index]
                    .entry(address)
                    .or_default()
                    .include(metrics);
            }
        }
    }
    Ok(combined
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
        .collect())
}

fn resolve_nfcapd_batch_revisions(
    tree: &FrozenNfcapdTreeLayout,
    batch_starts: &[i64],
    decoder_fingerprint: &str,
    pool: &rayon::ThreadPool,
) -> Result<BTreeMap<PathBuf, PreparedRevision>, PipelineError> {
    let paths = batch_starts
        .iter()
        .flat_map(|bucket_start| {
            tree.sources.iter().flat_map(move |source| {
                source.members.iter().filter_map(move |member| {
                    tree.by_member_and_start
                        .get(&(member.clone(), *bucket_start))
                        .cloned()
                })
            })
        })
        .collect::<BTreeSet<_>>();
    pool.install(|| {
        paths
            .par_iter()
            .map(|path| {
                let (content_fingerprint, snapshot) = capture_file_revision(path)?;
                let revision = InputRevision::create(
                    "nfcapd",
                    path.to_string_lossy().into_owned(),
                    content_fingerprint,
                    decoder_fingerprint,
                )?;
                Ok((
                    path.clone(),
                    PreparedRevision {
                        revision,
                        snapshot: Some(snapshot),
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, PipelineError>>()
    })
}

fn verify_prepared_revision_snapshots(
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
) -> Result<(), PipelineError> {
    for (path, prepared) in revisions {
        if let Some(snapshot) = &prepared.snapshot {
            verify_file_snapshot(path, snapshot)?;
        }
    }
    Ok(())
}

fn prepare_nfcapd_tree_timestamp(
    tree: &FrozenNfcapdTreeLayout,
    bucket_start: i64,
    timezone: &str,
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
) -> Result<PreparedTreeTimestamp, PipelineError> {
    let mut jobs = Vec::new();
    for source in &tree.sources {
        if !source_has_candidate(
            source,
            bucket_start,
            &tree.by_member_and_start,
            &tree.member_bounds,
            tree.extend_gaps_to_window,
        ) {
            continue;
        }
        let present = source
            .members
            .iter()
            .filter_map(|member| {
                tree.by_member_and_start
                    .get(&(member.clone(), bucket_start))
                    .map(|path| (member.clone(), path.clone()))
            })
            .collect::<Vec<_>>();
        let owners = present
            .iter()
            .map(|(_, path)| {
                revisions
                    .get(path)
                    .cloned()
                    .expect("present capture has a prepared revision")
            })
            .collect::<Vec<_>>();
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
            if present.iter().all(|(present, _)| present != member) {
                let expected =
                    expected_nfcapd_path(&tree.root_path, member, bucket_start, timezone)?;
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
        jobs.push(PreparedTreeJob {
            source_id: source.source_id.clone(),
            expected_units: source.members.len(),
            present,
            owners,
            absences,
            evidence,
        });
    }
    Ok(PreparedTreeTimestamp { bucket_start, jobs })
}

fn process_nfcapd_tree_day(
    tree: &FrozenNfcapdTreeLayout,
    start: i64,
    end: i64,
    sinks: &mut [ProductSink<'_>],
    pending: &[usize],
    aggregates: &mut [Option<AggregateBuckets>],
) -> Result<(), PipelineError> {
    let first_pipeline = sinks[pending[0]].pipeline;
    let timezone = first_pipeline.timezone.clone();
    let executable = first_pipeline.nfdump.clone();
    let daily_active = first_pipeline.selection.selects_daily_active_sources();
    let selections = pending
        .iter()
        .map(|index| sinks[*index].pipeline.selection.clone())
        .collect::<Vec<_>>();
    let decoder_fingerprint = nfdump_decoder_fingerprint_for_pipeline(first_pipeline)?;
    let revision_pool = build_revision_hash_pool()?;
    let mut revision_starts = Vec::new();
    let mut revision_start = start;
    while revision_start < end {
        revision_starts.push(revision_start);
        revision_start = next_local_five_minute_start(revision_start, &timezone)?;
    }
    let revisions = resolve_nfcapd_batch_revisions(
        tree,
        &revision_starts,
        &decoder_fingerprint,
        &revision_pool,
    )?;
    verify_nfdump_revision(first_pipeline)?;
    let active_sources = if daily_active {
        Some(resolve_daily_active_sources(
            tree,
            start,
            end,
            &timezone,
            &selections,
            &executable,
        )?)
    } else {
        None
    };
    verify_nfdump_revision(first_pipeline)?;
    verify_prepared_revision_snapshots(&revisions)?;
    let active_pairs = active_sources.as_ref().map(|active| {
        selections
            .iter()
            .cloned()
            .zip(active.iter().cloned())
            .collect::<Vec<_>>()
    });
    let decode_pool = build_nfcapd_decode_pool()?;
    let mut next = start;

    while next < end {
        let batch_starts = nfcapd_batch_starts(
            next,
            end,
            &timezone,
            &tree.sources,
            &tree.by_member_and_start,
            &tree.member_bounds,
            tree.extend_gaps_to_window,
        )?;
        next = batch_starts
            .last()
            .copied()
            .map(|last| next_local_five_minute_start(last, &timezone))
            .transpose()?
            .expect("non-empty nfcapd batch");
        let prepared = batch_starts
            .iter()
            .map(|start| prepare_nfcapd_tree_timestamp(tree, *start, &timezone, &revisions))
            .collect::<Result<Vec<_>, _>>()?;
        let requests = prepared
            .iter()
            .flat_map(|timestamp| {
                timestamp.jobs.iter().flat_map(move |job| {
                    job.present.iter().map(move |(member, path)| {
                        ((member.clone(), timestamp.bucket_start), path.clone())
                    })
                })
            })
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect::<Vec<_>>();
        verify_nfdump_revision(first_pipeline)?;
        let decoded = decode_pool.install(|| {
            requests
                .par_iter()
                .map(|((member, bucket_start), path)| {
                    let buckets = match &active_pairs {
                        Some(pairs) => ingest::read_nfcapd_buckets_with_active_sources(
                            path,
                            member,
                            pairs,
                            &executable,
                            &timezone,
                        )?,
                        None => vec![ingest::read_nfcapd_bucket(
                            path,
                            member,
                            &selections[0],
                            &executable,
                            &timezone,
                        )?],
                    };
                    Ok::<_, PipelineError>(((member.clone(), *bucket_start), buckets))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        })?;
        verify_nfdump_revision(first_pipeline)?;

        for (selection_index, sink_index) in pending.iter().copied().enumerate() {
            let aggregate = aggregates[sink_index]
                .as_mut()
                .expect("pending output has aggregate state");
            for timestamp in &prepared {
                for job in &timestamp.jobs {
                    let member_buckets = job
                        .present
                        .iter()
                        .map(|(member, _)| {
                            &decoded[&(member.clone(), timestamp.bucket_start)][selection_index]
                        })
                        .collect::<Vec<_>>();
                    let logical = logical_source_bucket(
                        &job.source_id,
                        timestamp.bucket_start,
                        job.expected_units,
                        &member_buckets,
                    )?;
                    aggregate.reject_persisted_siblings(
                        sinks[sink_index].connection,
                        &logical,
                        &timezone,
                    )?;
                    publish_nfcapd_bucket(
                        sinks[sink_index].connection,
                        &logical,
                        &job.owners,
                        &job.absences,
                        &job.evidence,
                        true,
                        sinks[sink_index].pipeline.run_maad,
                    )?;
                    aggregate.include(&logical, &timezone)?;
                    sinks[sink_index].report.rollup_buckets += aggregate.flush_complete(
                        sinks[sink_index].connection,
                        sinks[sink_index].pipeline.run_maad,
                    )?;
                    sinks[sink_index].report.five_minute_buckets += 1;
                }
            }
        }
    }
    Ok(())
}
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
        member_paths.insert(canonical_member_path, member);
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
}

struct PreparedTreeTimestamp {
    bucket_start: i64,
    jobs: Vec<PreparedTreeJob>,
}

#[allow(clippy::too_many_arguments)]
fn publish_nfcapd_bucket(
    connection: &Connection,
    bucket: &CanonicalBucket,
    owners: &[PreparedRevision],
    absences: &[ExpectedAbsence],
    evidence: &[InputEvidenceRow],
    replace_existing: bool,
    run_maad: bool,
) -> Result<(), PipelineError> {
    for prepared in owners {
        if let Some(snapshot) = &prepared.snapshot {
            verify_file_snapshot(Path::new(&prepared.revision.locator), snapshot)?;
        }
    }
    for absence in absences {
        absence.verify()?;
    }
    reject_overlapping_bucket(connection, bucket, InputKind::Nfcapd, "", replace_existing)?;
    if replace_existing {
        connection
            .execute(
                "DELETE FROM processed_inputs
                 WHERE input_kind = 'nfcapd' AND source_id = ?1 AND bucket_start = ?2",
                params![bucket.key.source_id, bucket.key.bucket_start],
            )
            .map_err(StorageError::from)?;
    }
    for prepared in owners {
        let revision = &prepared.revision;
        upsert_input_bucket(
            connection,
            &InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: revision.locator.clone(),
                scan_locator: revision.locator.clone(),
                source_id: bucket.key.source_id.clone(),
                bucket_start: bucket.key.bucket_start,
                bucket_end: bucket.key.bucket_end,
                revision: revision.clone(),
                file_snapshot: prepared.snapshot.clone(),
            },
            replace_existing,
        )?;
    }
    write_buckets(connection, std::slice::from_ref(bucket), run_maad)?;
    replace_input_evidence(
        connection,
        &bucket.key.source_id,
        bucket.key.bucket_start,
        evidence,
    )?;
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
    Ok(())
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
            let (start, end) = aggregate_bounds(child.key.bucket_start, granularity, timezone)?;
            let key = (child.key.source_id.clone(), granularity, start, end);
            let builder = self.builders.entry(key.clone()).or_insert_with(|| {
                StatisticalBucket::new(BucketKey::new(&key.0, key.1, key.2, key.3))
            });
            builder.include(child)?;
        }
        self.published_through
            .insert(child.key.source_id.clone(), child.key.bucket_start);
        self.current_run_keys
            .insert((child.key.source_id.clone(), child.key.bucket_start));
        Ok(())
    }

    fn flush_complete(
        &mut self,
        connection: &Connection,
        run_maad: bool,
    ) -> Result<usize, PipelineError> {
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
        write_buckets(connection, &buckets, run_maad)?;
        Ok(count)
    }

    fn finish(self) -> Vec<CanonicalBucket> {
        self.builders
            .into_values()
            .map(StatisticalBucket::finish_owned)
            .collect()
    }
}

fn publish_rollups(
    connection: &Connection,
    aggregates: AggregateBuckets,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let rollups = aggregates.finish();
    write_buckets(connection, &rollups, pipeline.run_maad)?;
    report.rollup_buckets += rollups.len();
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
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use rusqlite::Connection;
    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::*;

    fn write_fake_nfdump(executable: &Path, invocation_log: &Path) {
        let stream_path = executable.with_extension("stream");
        let empty_stream_path = executable.with_extension("empty-stream");
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
            executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"-R\" ] && [ -z \"$(find \"$2\" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)\" ]; then\ncat '{}'\nexit 0\nfi\nprintf 'x\\n' >> '{}'\ncat '{}'\n",
                empty_stream_path.display(),
                invocation_log.display(),
                stream_path.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn write_nfcapd_day(root: &Path, member: &str, date: &str) {
        let mut bucket_start = parse_date_start(date, DEFAULT_TIMEZONE).unwrap();
        let end = next_date_start(date, DEFAULT_TIMEZONE).unwrap();
        while bucket_start < end {
            let timestamp = Timestamp::from_second(bucket_start)
                .unwrap()
                .in_tz(DEFAULT_TIMEZONE)
                .unwrap();
            let path = root
                .join(member)
                .join(timestamp.strftime("%Y").to_string())
                .join(timestamp.strftime("%m").to_string())
                .join(timestamp.strftime("%d").to_string())
                .join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M")));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"capture").unwrap();
            bucket_start = next_local_five_minute_start(bucket_start, DEFAULT_TIMEZONE).unwrap();
        }
    }

    fn coordinated_request(
        registry: PathBuf,
        executable: &Path,
        start_date: &str,
        end_date: &str,
    ) -> PipelineRequest {
        PipelineRequest {
            config_path: None,
            dataset_id: None,
            datasets_path: Some(registry),
            start_date: Some(start_date.into()),
            end_date: Some(end_date.into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force: false,
            run_maad: false,
            require_complete: false,
        }
    }

    fn daily_selection(prefix: &str) -> FlowSelection {
        selection_from_value(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": prefix,
        }))
        .unwrap()
    }

    fn resolved_pipeline(
        root: &Path,
        database: PathBuf,
        member: &str,
        timezone: &str,
    ) -> ResolvedPipeline {
        ResolvedPipeline {
            database_path: database,
            timezone: timezone.into(),
            run_maad: false,
            nfdump: PathBuf::from("/bin/true"),
            nfdump_revision: None,
            selection: daily_selection("192.0.0.0/16"),
            inputs: vec![InputSpec::NfcapdTree {
                root_path: root.to_owned(),
                source_ids: vec![member.into()],
                sources: Vec::new(),
                start_date: "2025-06-01".into(),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                force: false,
            }],
            datasets: Vec::new(),
            require_complete: false,
        }
    }

    fn incompatibility(pipelines: Vec<ResolvedPipeline>) -> String {
        match validate_compatible_pipelines(pipelines) {
            Ok(_) => panic!("pipelines unexpectedly compatible"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn coordinated_compatibility_rejects_duplicate_ids_roots_layouts_timezones_and_outputs() {
        let temporary = tempdir().unwrap();
        let first_root = temporary.path().join("first-root");
        let second_root = temporary.path().join("second-root");
        for path in [
            first_root.join("edge"),
            first_root.join("other"),
            second_root.join("edge"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let first_db = temporary.path().join("first.sqlite");
        let second_db = temporary.path().join("second.sqlite");

        let duplicate = run_many(
            PipelineRequest {
                config_path: None,
                dataset_id: None,
                datasets_path: Some(temporary.path().join("unused.json")),
                start_date: Some("2025-06-01".into()),
                end_date: Some("2025-06-01".into()),
                start_time: None,
                end_time: None,
                database_path: None,
                selection: Value::Null,
                nfdump: "/bin/true".into(),
                force: false,
                run_maad: false,
                require_complete: false,
            },
            vec!["same".into(), "same".into()],
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("cannot repeat dataset"));

        let roots = incompatibility(vec![
            resolved_pipeline(&first_root, first_db.clone(), "edge", DEFAULT_TIMEZONE),
            resolved_pipeline(&second_root, second_db.clone(), "edge", DEFAULT_TIMEZONE),
        ]);
        assert!(roots.contains("same nfcapd root"), "{roots}");

        let layouts = incompatibility(vec![
            resolved_pipeline(&first_root, first_db.clone(), "edge", DEFAULT_TIMEZONE),
            resolved_pipeline(&first_root, second_db.clone(), "other", DEFAULT_TIMEZONE),
        ]);
        assert!(layouts.contains("same logical source layout"), "{layouts}");

        let timezones = incompatibility(vec![
            resolved_pipeline(&first_root, first_db.clone(), "edge", DEFAULT_TIMEZONE),
            resolved_pipeline(&first_root, second_db.clone(), "edge", "UTC"),
        ]);
        assert!(timezones.contains("same timezone"), "{timezones}");

        let outputs = incompatibility(vec![
            resolved_pipeline(&first_root, first_db.clone(), "edge", DEFAULT_TIMEZONE),
            resolved_pipeline(&first_root, first_db, "edge", DEFAULT_TIMEZONE),
        ]);
        assert!(outputs.contains("must be distinct"), "{outputs}");
    }

    #[test]
    fn auto_discovered_nfcapd_root_rejects_output_that_would_create_a_member_directory() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, &temporary.path().join("invocations"));
        let database = root.join("future-member/netflow.sqlite");
        let config = temporary.path().join("pipeline.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "database_path": database,
                "timezone": DEFAULT_TIMEZONE,
                "nfdump": executable,
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
        assert!(
            error
                .to_string()
                .contains("overlaps the nfcapd capture tree")
        );
        assert!(!database.exists());
    }

    #[test]
    fn dataset_mode_applies_its_persisted_selection() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        fs::create_dir_all(root.join("edge")).unwrap();
        let executable = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&executable, &temporary.path().join("invocations"));
        let registry = temporary.path().join("datasets.json");
        let database = temporary.path().join("active.sqlite");
        fs::write(
            &registry,
            serde_json::to_vec(&json!([{
                "dataset_id": "active",
                "root_path": root,
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
            datasets_path: Some(registry),
            start_date: Some("2025-06-01".into()),
            end_date: Some("2025-06-01".into()),
            start_time: None,
            end_time: None,
            database_path: None,
            selection: Value::Null,
            nfdump: executable.to_string_lossy().into_owned(),
            force: false,
            run_maad: true,
            require_complete: false,
        })
        .unwrap();

        assert!(resolved.selection.selects_daily_active_sources());
        assert_eq!(resolved.database_path, database);
    }

    #[test]
    fn coordinated_products_share_decode_and_resume_by_whole_day() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        write_nfcapd_day(&root, "edge", "2025-06-01");
        write_nfcapd_day(&root, "edge", "2025-06-02");
        let executable = temporary.path().join("fake-nfdump");
        let invocation_log = temporary.path().join("invocations");
        write_fake_nfdump(&executable, &invocation_log);
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
                    "selection": {
                        "kind": "daily_active_sources",
                        "ip_prefix": "192.0.0.0/16"
                    }
                },
                {
                    "dataset_id": "second",
                    "root_path": root,
                    "db_path": second_db,
                    "source_ids": ["edge"],
                    "selection": {
                        "kind": "daily_active_sources",
                        "ip_prefix": "198.51.0.0/16"
                    }
                }
            ]))
            .unwrap(),
        )
        .unwrap();
        let request = coordinated_request(registry, &executable, "2025-06-01", "2025-06-02");

        let initial = run_many(request.clone(), vec!["first".into(), "second".into()]).unwrap();
        assert_eq!(initial.five_minute_buckets, 1_152);
        let initial_invocations = fs::read_to_string(&invocation_log).unwrap().lines().count();
        assert_eq!(initial_invocations, 578);

        let first = Connection::open(&first_db).unwrap();
        let second = Connection::open(&second_db).unwrap();
        let first_max = first
            .query_row(
                "SELECT MAX(flows) FROM traffic_stats WHERE granularity = '5m'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap()
            .unwrap();
        let second_max = second
            .query_row(
                "SELECT MAX(flows) FROM traffic_stats WHERE granularity = '5m'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap()
            .unwrap();
        assert!(first_max > 0);
        assert_eq!(second_max, 0);
        let first_rows = first
            .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        drop(first);
        drop(second);

        let no_op = run_many(request.clone(), vec!["second".into(), "first".into()]).unwrap();
        assert_eq!(no_op.five_minute_buckets, 0);
        assert_eq!(
            fs::read_to_string(&invocation_log).unwrap().lines().count(),
            initial_invocations,
        );

        let day_start = parse_date_start("2025-06-02", DEFAULT_TIMEZONE).unwrap();
        let day_end = next_date_start("2025-06-02", DEFAULT_TIMEZONE).unwrap();
        let second = Connection::open(&second_db).unwrap();
        second.execute_batch("BEGIN IMMEDIATE").unwrap();
        delete_stats_time_range(&second, &["edge".to_owned()], day_start, day_end).unwrap();
        second.execute_batch("COMMIT").unwrap();
        assert_eq!(
            second
                .query_row("SELECT COUNT(*) FROM daily_product_completion", [], |row| {
                    row.get::<_, i64>(0)
                },)
                .unwrap(),
            1,
        );
        drop(second);

        let resumed = run_many(request, vec!["first".into(), "second".into()]).unwrap();
        assert_eq!(resumed.five_minute_buckets, 288);
        assert_eq!(
            fs::read_to_string(&invocation_log).unwrap().lines().count(),
            initial_invocations + 289,
        );
        assert_eq!(
            Connection::open(&first_db)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            first_rows,
        );
        for database in [&first_db, &second_db] {
            assert_eq!(
                Connection::open(database)
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM daily_product_completion", [], |row| {
                        row.get::<_, i64>(0)
                    },)
                    .unwrap(),
                2,
            );
        }
    }

    #[test]
    fn local_day_iteration_handles_both_dst_transitions() {
        for (date, expected) in [("2025-03-09", 276), ("2025-11-02", 288)] {
            let start = parse_date_start(date, DEFAULT_TIMEZONE).unwrap();
            let end = next_date_start(date, DEFAULT_TIMEZONE).unwrap();
            let mut bucket_start = start;
            let mut count = 0;
            while bucket_start < end {
                count += 1;
                bucket_start =
                    next_local_five_minute_start(bucket_start, DEFAULT_TIMEZONE).unwrap();
            }
            assert_eq!(count, expected, "{date}");
            assert_eq!(bucket_start, end, "{date}");
        }
    }
}
