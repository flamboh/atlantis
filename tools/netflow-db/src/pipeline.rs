//! End-to-end pipeline orchestration.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use jiff::{RoundMode, Timestamp, ToSpan, Unit, ZonedRound, civil::Date};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    config::{ConfigError, CsvSourceConfig},
    domain::{
        BucketKey, CanonicalBucket, DomainError, FlowSelection, Granularity, StatisticalBucket,
        StatisticalBucketIncludeProfile,
    },
    ingest::{self, IngestError, ProducerError},
    nfdump,
    provenance::{
        ExpectedAbsence, FileSnapshot, InputRevision, ProvenanceError, capture_file_revision,
        csv_decoder_fingerprint, gap_input_revision, nfcapd_decoder_fingerprint,
        revision_for_locator, verify_file_snapshot,
    },
    publish::{PublishError, WriteBucketsProfile, write_buckets, write_buckets_profiled},
    registry::{Dataset, DatasetRegistry, DatasetSource, RegistryError, is_safe_path_component},
    storage::{
        DatabaseOperationLock, DatasetMetadata, InputBucket, InputKind, InputStatus,
        ProductIdentity, SourceDefinition, StatsBucketKey, StorageError, bind_nfcapd_source_layout,
        bind_product_identity, cached_content_fingerprint, complete_input_scan,
        connect_pipeline_writer, delete_stats_bucket_keys, init_schema, input_scan_fully_processed,
        mark_input_bucket_status, nfcapd_logical_bucket_processed, upsert_dataset_metadata,
        upsert_input_bucket,
    },
};

const FIVE_MINUTES: i64 = 300;
const NFCAPD_DECODE_BATCH_SIZE: usize = 12;
const NFCAPD_REVISION_HASH_MAX_WORKERS: usize = NFCAPD_DECODE_BATCH_SIZE * 2;
const DEFAULT_TIMEZONE: &str = "America/Los_Angeles";

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
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineReport {
    pub input_scans: usize,
    pub skipped_inputs: usize,
    pub five_minute_buckets: usize,
    pub rollup_buckets: usize,
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
        #[serde(default = "default_true")]
        zero_fill_gaps: bool,
        #[serde(default)]
        force: bool,
    },
}

#[derive(Clone, Debug)]
struct ResolvedPipeline {
    database_path: PathBuf,
    timezone: String,
    run_maad: bool,
    nfdump: String,
    selection: FlowSelection,
    inputs: Vec<InputSpec>,
    datasets: Vec<Dataset>,
}

fn default_timezone() -> String {
    DEFAULT_TIMEZONE.into()
}

const fn default_true() -> bool {
    true
}

pub fn run(
    request: impl std::borrow::Borrow<PipelineRequest>,
) -> Result<PipelineReport, PipelineError> {
    let pipeline = resolve_request(request.borrow())?;
    execute(pipeline)
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
        return Ok(ResolvedPipeline {
            database_path: config.database_path,
            timezone: config.timezone,
            run_maad: config.run_maad.unwrap_or(true) && request.run_maad,
            nfdump: config.nfdump.unwrap_or_else(|| request.nfdump.clone()),
            selection,
            inputs: config.inputs,
            datasets: config.datasets,
        });
    }

    let repository_root = std::env::current_dir()?;
    let registry = match &request.datasets_path {
        Some(path) => DatasetRegistry::load(path, &repository_root)?,
        None => DatasetRegistry::load_default(&repository_root)?,
    };
    let dataset_id = request
        .dataset_id
        .as_deref()
        .ok_or(PipelineError::MissingMode)?;
    let dataset = registry.get(dataset_id)?.clone();
    let start_date = request.start_date.clone().ok_or_else(|| {
        PipelineError::InvalidConfig("--start-date is required with --dataset".into())
    })?;
    let selection = selection_from_value(&request.selection)?;
    if !selection.is_unrestricted() && request.database_path.is_none() {
        return Err(PipelineError::InvalidConfig(
            "flow selection requires an explicit --database-path".into(),
        ));
    }
    Ok(ResolvedPipeline {
        database_path: request
            .database_path
            .clone()
            .unwrap_or_else(|| dataset.db_path.clone()),
        timezone: DEFAULT_TIMEZONE.into(),
        run_maad: request.run_maad,
        nfdump: request.nfdump.clone(),
        selection,
        inputs: vec![InputSpec::NfcapdTree {
            root_path: dataset.root_path.clone(),
            source_ids: dataset.source_ids.clone(),
            sources: dataset.sources.clone(),
            start_date,
            end_date: request.end_date.clone(),
            start_time: request.start_time.clone(),
            end_time: request.end_time.clone(),
            zero_fill_gaps: true,
            force: request.force,
        }],
        datasets: vec![dataset],
    })
}

fn selection_from_value(value: &Value) -> Result<FlowSelection, DomainError> {
    FlowSelection::from_payload((!value.is_null()).then_some(value))
}

fn execute(pipeline: ResolvedPipeline) -> Result<PipelineReport, PipelineError> {
    if let Some(parent) = pipeline.database_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = DatabaseOperationLock::acquire(&pipeline.database_path, "pipeline build")?;
    let connection = connect_pipeline_writer(&pipeline.database_path)?;
    init_schema(&connection)?;
    initialize_metadata(&connection, &pipeline)?;

    let mut report = PipelineReport::default();
    let explicit_csv = pipeline
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
    for input in &pipeline.inputs {
        match input {
            InputSpec::Csv { .. } | InputSpec::Nfcapd { .. } => {}
            InputSpec::CsvTree {
                root_path,
                mapping_path,
            } => {
                let mapping = CsvSourceConfig::load(mapping_path)?;
                let inputs = ingest::discover_csv_inputs(root_path, mapping_path, &mapping)?;
                merge_report(
                    &mut report,
                    process_csv_inputs(&connection, &inputs, &pipeline)?,
                );
            }
            InputSpec::NfcapdTree {
                root_path,
                source_ids,
                sources,
                start_date,
                end_date,
                start_time,
                end_time,
                zero_fill_gaps,
                force,
            } => process_nfcapd_tree(
                &connection,
                root_path,
                source_ids,
                sources,
                start_date,
                end_date.as_deref(),
                start_time.as_deref(),
                end_time.as_deref(),
                *zero_fill_gaps,
                *force,
                &pipeline,
                &mut report,
            )?,
        }
    }
    merge_report(
        &mut report,
        process_csv_inputs(&connection, &explicit_csv, &pipeline)?,
    );
    merge_report(
        &mut report,
        process_explicit_nfcapd_inputs(&connection, &explicit_nfcapd, &pipeline)?,
    );
    Ok(report)
}

fn with_transaction<T>(
    connection: &Connection,
    operation: impl FnOnce() -> Result<T, PipelineError>,
) -> Result<T, PipelineError> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(StorageError::from)?;
    let result = operation();
    match result {
        Ok(value) => {
            connection
                .execute_batch("COMMIT")
                .map_err(StorageError::from)?;
            Ok(value)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn initialize_metadata(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<(), PipelineError> {
    let layouts = pipeline
        .inputs
        .iter()
        .filter_map(|input| match input {
            InputSpec::NfcapdTree {
                root_path,
                source_ids,
                sources,
                ..
            } => Some(normalize_sources(root_path, source_ids, sources)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
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
        bind_identity(connection, pipeline)?;
        for dataset in &pipeline.datasets {
            upsert_dataset(connection, dataset)?;
        }
        if !layouts.is_empty() {
            let layout = layouts
                .iter()
                .map(|source| SourceDefinition::new(&source.source_id, source.members.clone()))
                .collect::<Vec<_>>();
            bind_nfcapd_source_layout(connection, &layout)?;
        }
        Ok(())
    })
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
        publish_rollups(connection, aggregates, pipeline, &mut report)
    })?;
    Ok(report)
}

fn merge_report(total: &mut PipelineReport, addition: PipelineReport) {
    total.input_scans += addition.input_scans;
    total.skipped_inputs += addition.skipped_inputs;
    total.five_minute_buckets += addition.five_minute_buckets;
    total.rollup_buckets += addition.rollup_buckets;
}

fn bind_identity(
    connection: &Connection,
    pipeline: &ResolvedPipeline,
) -> Result<(), PipelineError> {
    let maad_config = serde_json::to_value(crate::maad::MaadConfig::default())?;
    let schema = json!({
        "version": 2,
        "tables": [
            {"name":"traffic_stats","version":2},
            {"name":"protocol_stats","version":1},
            {"name":"address_count_stats","version":1},
            {"name":"port_count_stats","version":1},
            {"name":"address_structure_stats","version":1}
        ]
    });
    let result_config = json!({
        "version": 2,
        "timezone": pipeline.timezone,
        "nfcapd_decoder": {
            "protocol_version": nfdump::CONTRACT_VERSION,
            "input_contract": nfdump::INPUT_CONTRACT,
            "output_contract": nfdump::OUTPUT_CONTRACT
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

fn upsert_dataset(connection: &Connection, dataset: &Dataset) -> Result<(), PipelineError> {
    let sources = dataset
        .logical_sources()?
        .into_iter()
        .map(|source| SourceDefinition::new(source.source_id, source.members))
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
    let mut report = PipelineReport::default();
    for input in inputs {
        let mapping = CsvSourceConfig::load(&input.mapping_path)?;
        let (revision, snapshot) = prepare_file_revision(
            connection,
            &input.path,
            InputKind::Csv,
            csv_decoder_fingerprint(&mapping)?,
        )?;
        if input_scan_fully_processed(connection, InputKind::Csv, &revision.locator, &revision)? {
            report.skipped_inputs += 1;
        } else {
            prepared.push(PreparedCsvInput {
                path: input.path.clone(),
                mapping,
                revision,
                snapshot,
            });
        }
    }
    if prepared.is_empty() {
        return Ok(report);
    }
    prepared.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    let mut aggregates = AggregateBuckets::default();
    with_transaction(connection, || {
        for input in &prepared {
            process_csv(
                connection,
                &input.path,
                &input.mapping,
                &input.revision,
                &input.snapshot,
                pipeline,
                &mut aggregates,
                &mut report,
            )?;
        }
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
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let mut published = 0_usize;
    let completion = match ingest::scan_csv(path, mapping, &pipeline.selection, |event| {
        let bucket_revision = revision_for_locator(revision, &event.input_locator)?;
        aggregates.reject_persisted_siblings(connection, &event.bucket, &pipeline.timezone)?;
        reject_overlapping_bucket(
            connection,
            &event.bucket,
            InputKind::Csv,
            &event.scan_locator,
            false,
        )?;
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
        write_buckets(
            connection,
            std::slice::from_ref(&event.bucket),
            pipeline.run_maad,
        )?;
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
        aggregates.include(&event.bucket, &pipeline.timezone)?;
        report.rollup_buckets += aggregates.flush_complete(connection, pipeline.run_maad)?;
        published += 1;
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
    report.five_minute_buckets += published;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_nfcapd_tree(
    connection: &Connection,
    root: &Path,
    source_ids: &[String],
    configured_sources: &[DatasetSource],
    start_date: &str,
    end_date: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    zero_fill_gaps: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let sources = normalize_sources(root, source_ids, configured_sources)?;
    let physical_ids = sources
        .iter()
        .flat_map(|source| source.members.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let discovery_started = Instant::now();
    let discovered = ingest::discover_nfcapd_source_paths(root, &physical_ids, &pipeline.timezone)?;
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
    let selected_start = parse_date_start(start_date, &pipeline.timezone)?;
    let discovered_end = by_member_and_start
        .keys()
        .map(|(_, bucket_start)| *bucket_start)
        .max()
        .map(|start| aggregate_bounds(start, Granularity::OneDay, &pipeline.timezone))
        .transpose()?
        .map(|(_, end)| end)
        .unwrap_or(selected_start);
    let selected_end = match end_date {
        Some(date) => next_date_start(date, &pipeline.timezone)?,
        None => discovered_end,
    };
    let start = match start_time {
        Some(value) => parse_local_datetime(value, &pipeline.timezone)?,
        None => selected_start,
    };
    let end = match end_time {
        Some(value) => parse_local_datetime(value, &pipeline.timezone)?,
        None => selected_end,
    };
    validate_window(selected_start, selected_end, start, end, &pipeline.timezone)?;

    let mut day_start = start;
    while day_start < end {
        let day_end = aggregate_bounds(day_start, Granularity::OneDay, &pipeline.timezone)?.1;
        let mut owned_keys = BTreeSet::new();
        let mut bucket_start = day_start;
        while bucket_start < day_end {
            for source in &sources {
                if force
                    && source_has_candidate(
                        source,
                        bucket_start,
                        &by_member_and_start,
                        &member_bounds,
                        zero_fill_gaps,
                        end_date.is_some(),
                    )
                {
                    owned_keys.insert((source.source_id.clone(), bucket_start));
                }
            }
            bucket_start = next_local_five_minute_start(bucket_start, &pipeline.timezone)?;
        }
        let transaction_started = Instant::now();
        let (day_report, day_profile) = with_transaction(connection, || {
            let mut aggregates = AggregateBuckets::with_owned_keys(owned_keys);
            let mut day_report = PipelineReport::default();
            let mut day_profile = process_nfcapd_tree_day(
                connection,
                root,
                &sources,
                &by_member_and_start,
                &member_bounds,
                day_start,
                day_end,
                zero_fill_gaps,
                end_date.is_some(),
                force,
                pipeline,
                &mut aggregates,
                &mut day_report,
            )?;
            day_profile.final_rollups =
                publish_rollups_profiled(connection, aggregates, pipeline, &mut day_report)?;
            Ok((day_report, day_profile))
        })?;
        day_profile.log(day_start, day_end, transaction_started.elapsed());
        merge_report(report, day_report);
        day_start = day_end;
    }
    Ok(())
}

fn source_has_candidate(
    source: &DatasetSource,
    bucket_start: i64,
    paths: &BTreeMap<(String, i64), PathBuf>,
    member_bounds: &BTreeMap<String, (i64, i64)>,
    zero_fill_gaps: bool,
    extend_gaps_to_window: bool,
) -> bool {
    let has_file = source
        .members
        .iter()
        .any(|member| paths.contains_key(&(member.clone(), bucket_start)));
    has_file
        || zero_fill_gaps
            && (extend_gaps_to_window
                || source.members.iter().any(|member| {
                    member_bounds.get(member).is_some_and(|(first, last)| {
                        *first <= bucket_start && bucket_start <= *last
                    })
                }))
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
    zero_fill_gaps: bool,
    extend_gaps_to_window: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<NfcapdDayPublishProfile, PipelineError> {
    let day_started = Instant::now();
    let mut prepare_elapsed = Duration::ZERO;
    let mut decode_elapsed = Duration::ZERO;
    let mut publish_elapsed = Duration::ZERO;
    let mut publish_profile = NfcapdDayPublishProfile::default();
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
    let revision_context = NfcapdRevisionContext {
        connection,
        sources,
        by_member_and_start,
        member_bounds,
        zero_fill_gaps,
        extend_gaps_to_window,
        revision_pool: &revision_pool,
    };
    let mut bucket_start = start;
    while bucket_start < end {
        let prepare_started = Instant::now();
        let mut batch_starts = Vec::with_capacity(NFCAPD_DECODE_BATCH_SIZE);
        while bucket_start < end && batch_starts.len() < NFCAPD_DECODE_BATCH_SIZE {
            batch_starts.push(bucket_start);
            bucket_start = next_local_five_minute_start(bucket_start, &pipeline.timezone)?;
        }
        let revisions = resolve_nfcapd_batch_revisions(&revision_context, &batch_starts)?;
        let mut batch = Vec::with_capacity(batch_starts.len());
        for bucket_start in batch_starts {
            batch.push(prepare_nfcapd_tree_timestamp(
                connection,
                root,
                sources,
                by_member_and_start,
                member_bounds,
                bucket_start,
                zero_fill_gaps,
                extend_gaps_to_window,
                force,
                pipeline,
                report,
                &revisions,
            )?);
        }
        prepare_elapsed += prepare_started.elapsed();

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
        let mut decoded_cache = needed
            .par_iter()
            .map(|((member, bucket_start), (path, snapshot))| {
                let bucket = ingest::read_nfcapd_bucket(
                    path,
                    member,
                    &pipeline.selection,
                    &pipeline.nfdump,
                    &pipeline.timezone,
                )?;
                verify_file_snapshot(path, snapshot)?;
                Ok::<_, PipelineError>(((member.clone(), *bucket_start), bucket))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
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
                let logical =
                    logical_source_bucket(&job.source_id, timestamp.bucket_start, &member_buckets)?;
                publish_profile.logical_source_elapsed += logical_started.elapsed();
                let sibling_started = Instant::now();
                aggregates.reject_persisted_siblings(connection, &logical, &pipeline.timezone)?;
                publish_profile.persisted_sibling_elapsed += sibling_started.elapsed();
                let bucket_profile = publish_nfcapd_bucket_profiled(
                    connection,
                    &logical,
                    &job.owners,
                    &job.absences,
                    force,
                    pipeline.run_maad,
                )?;
                publish_profile.bucket_publish.include(bucket_profile);
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
                publish_profile.logical_buckets += 1;
                report.rollup_buckets += flushed;
                report.five_minute_buckets += 1;
            }
            decoded_cache.retain(|(_, start), _| *start != timestamp.bucket_start);
        }
        publish_elapsed += publish_started.elapsed();
    }
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
    Ok(publish_profile)
}

struct NfcapdRevisionProbe {
    path: PathBuf,
    observed: FileSnapshot,
    cached_content_fingerprint: Option<String>,
}

struct NfcapdRevisionContext<'a> {
    connection: &'a Connection,
    sources: &'a [DatasetSource],
    by_member_and_start: &'a BTreeMap<(String, i64), PathBuf>,
    member_bounds: &'a BTreeMap<String, (i64, i64)>,
    zero_fill_gaps: bool,
    extend_gaps_to_window: bool,
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
                context.zero_fill_gaps,
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

    let decoder_fingerprint = nfcapd_decoder_fingerprint()?;
    let probes = paths
        .into_iter()
        .map(|path| {
            let locator = path.to_string_lossy().into_owned();
            let observed = FileSnapshot::capture(&path)?;
            let cached_fingerprint = cached_content_fingerprint(
                context.connection,
                InputKind::Nfcapd,
                &locator,
                &observed,
            )?;
            Ok::<_, PipelineError>(NfcapdRevisionProbe {
                path,
                observed,
                cached_content_fingerprint: cached_fingerprint,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let resolved = context.revision_pool.install(|| {
        probes
            .par_iter()
            .map(|probe| {
                let captured = match &probe.cached_content_fingerprint {
                    Some(content_fingerprint) => {
                        Ok((content_fingerprint.clone(), probe.observed.clone()))
                    }
                    None => capture_file_revision(&probe.path),
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
    zero_fill_gaps: bool,
    extend_gaps_to_window: bool,
    force: bool,
    pipeline: &ResolvedPipeline,
    report: &mut PipelineReport,
    revisions: &BTreeMap<PathBuf, PreparedRevision>,
) -> Result<PreparedTreeTimestamp, PipelineError> {
    let mut revision_cache: BTreeMap<String, PreparedRevision> = BTreeMap::new();
    let mut jobs = Vec::new();
    for source in sources {
        if !source_has_candidate(
            source,
            bucket_start,
            by_member_and_start,
            member_bounds,
            zero_fill_gaps,
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
        for member in &source.members {
            if !present.iter().any(|(present, _)| present == member) {
                let expected =
                    expected_nfcapd_path(root, member, bucket_start, &pipeline.timezone)?;
                absences.push(ExpectedAbsence::capture(&expected)?);
            }
        }
        if present.is_empty() {
            owners.push(PreparedRevision {
                revision: gap_input_revision(
                    "nfcapd",
                    &nfcapd_gap_locator(&source.source_id, bucket_start, &pipeline.timezone)?,
                )?,
                snapshot: None,
            });
        }
        let revisions = owners
            .iter()
            .map(|owner| owner.revision.clone())
            .collect::<Vec<_>>();
        if !force
            && nfcapd_logical_bucket_processed(
                connection,
                &source.source_id,
                bucket_start,
                &revisions,
            )?
        {
            report.skipped_inputs += 1;
            continue;
        }
        jobs.push(PreparedTreeJob {
            source_id: source.source_id.clone(),
            present,
            owners,
            absences,
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
                nfcapd_decoder_fingerprint()?,
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
    let bucket = ingest::read_nfcapd_bucket(
        path,
        source_id,
        &pipeline.selection,
        &pipeline.nfdump,
        &pipeline.timezone,
    )?;
    let snapshot = owner
        .snapshot
        .as_ref()
        .expect("explicit file input has a snapshot");
    verify_file_snapshot(path, snapshot)?;
    aggregates.reject_persisted_siblings(connection, &bucket, &pipeline.timezone)?;
    publish_nfcapd_bucket(
        connection,
        &bucket,
        std::slice::from_ref(owner),
        &[],
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
    locator_path: &Path,
    expected_path: Option<&Path>,
    source_id: &str,
    bucket_start: i64,
    pipeline: &ResolvedPipeline,
    aggregates: &mut AggregateBuckets,
    report: &mut PipelineReport,
) -> Result<(), PipelineError> {
    let locator = locator_path.to_string_lossy().into_owned();
    let expected_path = expected_path.ok_or_else(|| {
        PipelineError::InvalidConfig(
            "explicit nfcapd gap requires expected_path for absence verification".into(),
        )
    })?;
    let absence = ExpectedAbsence::capture(expected_path)?;
    let revision = gap_input_revision("nfcapd", &locator)?;
    if nfcapd_logical_bucket_processed(
        connection,
        source_id,
        bucket_start,
        std::slice::from_ref(&revision),
    )? {
        report.skipped_inputs += 1;
        return Ok(());
    }
    let bucket = StatisticalBucket::dense(BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    ))
    .finish();
    aggregates.reject_persisted_siblings(connection, &bucket, &pipeline.timezone)?;
    publish_nfcapd_bucket(
        connection,
        &bucket,
        &[PreparedRevision {
            revision,
            snapshot: None,
        }],
        &[absence],
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
    Ok(normalized)
}

fn merge_source_bucket(
    source_id: &str,
    bucket_start: i64,
    members: &[&CanonicalBucket],
) -> Result<CanonicalBucket, PipelineError> {
    let mut builder = StatisticalBucket::dense(BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    ));
    for member in members {
        builder.include(member)?;
    }
    Ok(builder.finish())
}

fn logical_source_bucket<'a>(
    source_id: &str,
    bucket_start: i64,
    members: &[&'a CanonicalBucket],
) -> Result<Cow<'a, CanonicalBucket>, PipelineError> {
    let expected_key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + FIVE_MINUTES,
    );
    if let [member] = members
        && member.key == expected_key
    {
        return Ok(Cow::Borrowed(member));
    }
    Ok(Cow::Owned(merge_source_bucket(
        source_id,
        bucket_start,
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
}

impl NfcapdDayPublishProfile {
    fn log(&self, day_start: i64, day_end: i64, transaction_elapsed: Duration) {
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
    present: Vec<(String, PathBuf)>,
    owners: Vec<PreparedRevision>,
    absences: Vec<ExpectedAbsence>,
}

struct PreparedTreeTimestamp {
    bucket_start: i64,
    revision_cache: BTreeMap<String, PreparedRevision>,
    jobs: Vec<PreparedTreeJob>,
}

fn publish_nfcapd_bucket(
    connection: &Connection,
    bucket: &CanonicalBucket,
    owners: &[PreparedRevision],
    absences: &[ExpectedAbsence],
    force: bool,
    run_maad: bool,
) -> Result<(), PipelineError> {
    publish_nfcapd_bucket_profiled(connection, bucket, owners, absences, force, run_maad)
        .map(|_| ())
}

fn publish_nfcapd_bucket_profiled(
    connection: &Connection,
    bucket: &CanonicalBucket,
    owners: &[PreparedRevision],
    absences: &[ExpectedAbsence],
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
    reject_overlapping_bucket(connection, bucket, InputKind::Nfcapd, "", force)?;
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
}

impl AggregateBuckets {
    fn with_owned_keys(owned_keys: BTreeSet<(String, i64)>) -> Self {
        Self {
            owned_keys,
            ..Self::default()
        }
    }

    fn reject_persisted_siblings(
        &self,
        connection: &Connection,
        child: &CanonicalBucket,
        timezone: &str,
    ) -> Result<(), PipelineError> {
        let (day_start, day_end) =
            aggregate_bounds(child.key.bucket_start, Granularity::OneDay, timezone)?;
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
        if let Some(external) = persisted.into_iter().find(|bucket_start| {
            *bucket_start != child.key.bucket_start
                && !self
                    .owned_keys
                    .contains(&(child.key.source_id.clone(), *bucket_start))
                && !self
                    .current_run_keys
                    .contains(&(child.key.source_id.clone(), *bucket_start))
        }) {
            return Err(PipelineError::InvalidConfig(format!(
                "cannot reopen a persisted aggregate interval exactly: source={:?} bucket_start={} shares its local day with persisted five-minute bucket {external} from another transaction",
                child.key.source_id, child.key.bucket_start
            )));
        }
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
                StatisticalBucket::dense(BucketKey::new(&key.0, key.1, key.2, key.3))
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
            .map(|builder| builder.finish())
            .collect::<Vec<_>>();
        let count = buckets.len();
        let profile = write_buckets_profiled(connection, &buckets, run_maad)?;
        Ok((count, profile))
    }

    fn finish(self) -> (Vec<CanonicalBucket>, Vec<StatsBucketKey>) {
        let mut complete = Vec::new();
        let mut incomplete = Vec::new();
        for bucket in self.builders.into_values().map(|builder| builder.finish()) {
            if bucket.has_complete_five_minute_coverage() {
                complete.push(bucket);
            } else {
                incomplete.push(StatsBucketKey::new(
                    &bucket.key.source_id,
                    bucket.key.granularity.as_str(),
                    bucket.key.bucket_start,
                ));
            }
        }
        (complete, incomplete)
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

fn nfcapd_gap_locator(
    source_id: &str,
    bucket_start: i64,
    timezone: &str,
) -> Result<String, PipelineError> {
    let timestamp = Timestamp::from_second(bucket_start)
        .and_then(|timestamp| timestamp.in_tz(timezone))
        .map_err(|error| PipelineError::Time(error.to_string()))?;
    Ok(format!(
        "gap://nfcapd/{source_id}/{}",
        timestamp.strftime("%Y%m%d%H%M")
    ))
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

    use rusqlite::Connection;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::domain::{AddressSide, FlowObservation, IpVersion, Scope, Visibility};

    #[cfg(unix)]
    fn write_fake_nfdump(executable: &Path, setup: &str) {
        use std::os::unix::fs::PermissionsExt;

        let stream = executable.with_extension("stream");
        fs::write(&stream, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            executable,
            format!("#!/bin/sh\n{setup}\ncat '{}'\n", stream.display()),
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
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

        let singleton = logical_source_bucket("cc_ir1_gw", 0, &[&cc]).unwrap();
        assert!(matches!(singleton, Cow::Borrowed(_)));

        let combined = logical_source_bucket("uoregon_all", 0, &[&cc, &oh]).unwrap();
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
    fn config_run_publishes_dense_five_minute_buckets_and_only_complete_rollups() {
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
        assert_eq!(report.rollup_buckets, 1);
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity = '5m' AND ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            6
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
                    "SELECT COUNT(*) FROM traffic_stats WHERE granularity IN ('1h', '1d')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
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
        assert_eq!(report.rollup_buckets, 1);
    }

    #[test]
    fn overlapping_csv_batch_rolls_back_the_whole_explicit_transaction() {
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

        let error = run(PipelineRequest::config(&config)).unwrap_err();

        assert!(error.to_string().contains("overlapping canonical"));
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM processed_inputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM traffic_stats", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
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
                    "end_date":"2025-01-02",
                    "zero_fill_gaps":false
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(run(PipelineRequest::config(config)).is_err());
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
                    "end_date":"2025-11-02",
                    "zero_fill_gaps":true
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
            288
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
    fn implicit_tree_end_zero_fills_only_within_each_members_observed_bounds() {
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
                    "start_date":"2025-01-01",
                    "zero_fill_gaps":true
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
}
