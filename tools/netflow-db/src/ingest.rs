//! Streaming adapters that turn external CSV inputs into canonical five-minute buckets.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsString,
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use command_group::CommandGroup;
use csv::ReaderBuilder;
use flate2::read::MultiGzDecoder;
use jiff::civil::DateTime;
use rusqlite::{Connection, OptionalExtension, params};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{
    config::{CsvSourceConfig, InputOrder},
    coverage::BucketCoverage,
    domain::{
        AddressSet, BucketKey, CanonicalBucket, DomainError, FlowObservation, FlowSelection,
        Granularity, StatisticalBucket,
    },
    nfdump,
    normalize::{NormalizeError, field_indexes, normalize_csv_values},
};

const BUCKET_SECONDS: i64 = 300;
const NFDUMP_TIMEOUT: Duration = Duration::from_secs(300);
const NFDUMP_DAY_TIMEOUT: Duration = Duration::from_secs(3_600);
const NFDUMP_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const TIMESTAMP_KEYS: [&str; 3] = ["time_received", "time_end", "time_start"];

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("unable to read input {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{locator}:{line}: {message}")]
    CsvRow {
        locator: String,
        line: u64,
        message: String,
    },
    #[error("{0}")]
    InvalidInput(String),
    #[error("CSV staging database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("unable to start nfdump executable {executable:?}: {source}")]
    StartNfdump {
        executable: OsString,
        #[source]
        source: std::io::Error,
    },
    #[error("nfdump timed out after {seconds}s")]
    NfdumpTimeout { seconds: u64 },
    #[error("nfdump failed with exit code {exit_code:?}: {stderr}")]
    NfdumpFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

#[derive(Debug, Error)]
pub enum ProducerError<E> {
    #[error(transparent)]
    Input(IngestError),
    #[error("ingestion sink rejected an item: {0}")]
    Sink(E),
}

impl<E> From<IngestError> for ProducerError<E> {
    fn from(value: IngestError) -> Self {
        Self::Input(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvInputSpec {
    pub path: PathBuf,
    pub mapping_path: PathBuf,
}

#[derive(Debug)]
pub struct CsvBucketReady {
    pub scan_locator: String,
    pub input_locator: String,
    pub bucket: CanonicalBucket,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvScanComplete {
    pub scan_locator: String,
    pub rejected_rows: u64,
    pub skipped_bad_column_count: u64,
    pub observed_bounds: BTreeMap<String, (i64, i64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NfcapdInputSpec {
    pub path: PathBuf,
    pub source_id: String,
    pub bucket_start: i64,
}

/// One coordinated daily selection and its resolved source set.
pub type NfcapdSelectionAndActiveSources = (FlowSelection, Arc<AddressSet>);

/// Discover configured CSV inputs under one flat directory.
pub fn discover_csv_inputs(
    root: impl AsRef<Path>,
    mapping_path: impl AsRef<Path>,
    config: &CsvSourceConfig,
) -> Result<Vec<CsvInputSpec>, IngestError> {
    let root = root.as_ref();
    let mut specs = Vec::new();
    let entries = fs::read_dir(root).map_err(|source| IngestError::Io {
        path: root.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| IngestError::Io {
            path: root.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| IngestError::Io {
            path: path.clone(),
            source,
        })?;
        if !file_type.is_file() || has_incomplete_download_sidecar(&path) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if config
            .discovery_exclude_suffixes
            .iter()
            .any(|suffix| name.ends_with(&suffix.to_ascii_lowercase()))
            || !matches_csv_discovery(&name, config)
        {
            continue;
        }
        specs.push(CsvInputSpec {
            path,
            mapping_path: mapping_path.as_ref().to_owned(),
        });
    }
    specs.sort_by(|left, right| {
        csv_discovery_sort_key(&left.path).cmp(&csv_discovery_sort_key(&right.path))
    });
    Ok(specs)
}

#[must_use]
pub fn matches_csv_discovery(name: &str, config: &CsvSourceConfig) -> bool {
    config
        .discovery_include_suffixes
        .iter()
        .any(|suffix| name.ends_with(&suffix.to_ascii_lowercase()))
        || config
            .discovery_include_contains
            .iter()
            .any(|fragment| name.contains(&fragment.to_ascii_lowercase()))
}

fn has_incomplete_download_sidecar(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let mut sidecar = name.to_os_string();
    sidecar.push(".aria2");
    path.with_file_name(sidecar).exists()
}

fn csv_discovery_sort_key(path: &Path) -> (u8, u32, String) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let normalized = name.replace(['.', '-'], "_");
    let parts = normalized.split('_').collect::<Vec<_>>();
    let month = parts
        .iter()
        .find_map(|part| match *part {
            "january" => Some(1),
            "february" => Some(2),
            "march" => Some(3),
            "april" => Some(4),
            "may" => Some(5),
            "june" => Some(6),
            "july" => Some(7),
            "august" => Some(8),
            "september" => Some(9),
            "october" => Some(10),
            "november" => Some(11),
            "december" => Some(12),
            _ => None,
        })
        .unwrap_or(99);
    let week = parts
        .iter()
        .find_map(|part| part.strip_prefix("week")?.parse().ok())
        .unwrap_or(99);
    (month, week, name)
}

/// Scan one CSV file or gzip-compressed tar archive and emit dense buckets in order.
pub fn scan_csv<E>(
    path: impl AsRef<Path>,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    mut emit: impl FnMut(CsvBucketReady) -> Result<(), E>,
) -> Result<CsvScanComplete, ProducerError<E>> {
    let path = path.as_ref();
    let scan_locator = path.to_string_lossy().into_owned();
    match &config.input_order {
        InputOrder::TimestampAscending => {
            let mut state = CsvScanState::new(scan_locator.clone());
            scan_csv_inputs(
                path,
                &scan_locator,
                config,
                selection,
                &mut state,
                &mut emit,
            )?;
            state.finish(emit)
        }
        InputOrder::Unsorted => {
            let mut state = UnsortedCsvScanState::new(scan_locator.clone())?;
            scan_csv_inputs(
                path,
                &scan_locator,
                config,
                selection,
                &mut state,
                &mut emit,
            )?;
            state.finish(emit)
        }
    }
}

fn scan_csv_inputs<E, S: CsvScanAccumulator>(
    path: &Path,
    scan_locator: &str,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    state: &mut S,
    emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
) -> Result<(), ProducerError<E>> {
    if is_tar_archive(path) {
        scan_tar(path, config, selection, state, emit)
    } else {
        let file = fs::File::open(path).map_err(|source| IngestError::Io {
            path: path.to_owned(),
            source,
        })?;
        scan_csv_reader(file, scan_locator, config, selection, state, emit)
    }
}

fn is_tar_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    name.ends_with(".tar.gz") || name.ends_with(".tgz")
}

fn scan_tar<E, S: CsvScanAccumulator>(
    path: &Path,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    state: &mut S,
    emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
) -> Result<(), ProducerError<E>> {
    let file = fs::File::open(path).map_err(|source| IngestError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut archive = tar::Archive::new(MultiGzDecoder::new(file));
    let entries = archive.entries().map_err(|source| IngestError::Io {
        path: path.to_owned(),
        source,
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|source| IngestError::Io {
            path: path.to_owned(),
            source,
        })?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let member = entry
            .path()
            .map_err(|source| IngestError::Io {
                path: path.to_owned(),
                source,
            })?
            .to_string_lossy()
            .into_owned();
        if config
            .archive_member_contains
            .as_ref()
            .is_some_and(|fragment| !member.contains(fragment))
        {
            continue;
        }
        let locator = format!("{}:{member}", path.display());
        scan_csv_reader(&mut entry, &locator, config, selection, state, emit)?;
    }
    Ok(())
}

fn scan_csv_reader<E, S: CsvScanAccumulator>(
    reader: impl Read,
    locator: &str,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    state: &mut S,
    emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
) -> Result<(), ProducerError<E>> {
    let mut reader = ReaderBuilder::new()
        .delimiter(config.delimiter)
        .has_headers(config.has_header)
        .flexible(true)
        .from_reader(reader);
    let (indexes, expected_columns) = if config.has_header {
        let headers = reader
            .headers()
            .map_err(|error| csv_error(locator, error))?
            .iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let indexes =
            field_indexes(&headers, config).map_err(|error| row_error(locator, 1, error))?;
        (indexes, headers.len())
    } else {
        let fields = config.fieldnames.as_ref().ok_or_else(|| {
            IngestError::InvalidInput(
                "CSV config fieldnames are required for headerless input".into(),
            )
        })?;
        let indexes =
            field_indexes(fields, config).map_err(|error| row_error(locator, 1, error))?;
        (indexes, fields.len())
    };

    let starting_line = if config.has_header { 2 } else { 1 };
    for (offset, result) in reader.records().enumerate() {
        let line = u64::try_from(offset).unwrap_or(u64::MAX)
            + u64::try_from(starting_line).unwrap_or(u64::MAX);
        let record = result.map_err(|error| csv_error(locator, error))?;
        if record.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        if record.len() != expected_columns {
            if config.skip_bad_column_count {
                state.note_skipped_bad_column_count();
                continue;
            }
            return Err(IngestError::CsvRow {
                locator: locator.into(),
                line,
                message: format!(
                    "CSV row must contain {expected_columns} values, got {}",
                    record.len()
                ),
            }
            .into());
        }

        let values = record.iter().map(str::to_owned).collect::<Vec<_>>();
        let observed = coverage_from_values(&values, config, &indexes);
        if let Some((source_id, bucket_start)) = &observed {
            state.observe(source_id, *bucket_start)?;
        }
        match normalize_csv_values(&values, config, &indexes) {
            Ok(row) => {
                state.mark_valid(&row.source_id, row.bucket_start)?;
                if selection.matches_qualifying_flow(&row.observation) {
                    state.accept(row)?;
                }
            }
            Err(error) => {
                state.note_rejected_row();
                if let Some((source_id, bucket_start)) = &observed {
                    state.mark_rejected(source_id, *bucket_start)?;
                }
                tracing::warn!(locator, line, error = %error, "rejected CSV row");
            }
        }
        if let (InputOrder::TimestampAscending, Some((source_id, bucket_start))) =
            (&config.input_order, observed)
        {
            state.emit_ordered(
                &source_id,
                bucket_start,
                config.out_of_order_lag_buckets,
                emit,
            )?;
        }
    }
    Ok(())
}

fn csv_error(locator: &str, error: csv::Error) -> IngestError {
    IngestError::CsvRow {
        locator: locator.into(),
        line: error.position().map_or(0, csv::Position::line),
        message: error.to_string(),
    }
}

fn row_error(locator: &str, line: u64, error: NormalizeError) -> IngestError {
    IngestError::CsvRow {
        locator: locator.into(),
        line,
        message: error.to_string(),
    }
}

fn coverage_from_values(
    values: &[String],
    config: &CsvSourceConfig,
    indexes: &BTreeMap<String, usize>,
) -> Option<(String, i64)> {
    let raw = |column: &str| values.get(*indexes.get(column)?).map(String::as_str);
    let source_id = match (&config.source_id_value, &config.source_id_column) {
        (Some(source_id), _) => source_id.clone(),
        (None, Some(column)) => raw(column)?.trim().to_owned(),
        (None, None) => return None,
    };
    if source_id.is_empty() {
        return None;
    }
    let mut timestamp_ms = None;
    for key in TIMESTAMP_KEYS {
        let Some(column) = config.columns.get(key) else {
            continue;
        };
        let value = raw(column)?.trim();
        if !value.is_empty() {
            timestamp_ms = config.parse_timestamp_ms(value).ok();
            break;
        }
    }
    let timestamp_ms = timestamp_ms?;
    Some((
        source_id,
        timestamp_ms.div_euclid(BUCKET_SECONDS * 1_000) * BUCKET_SECONDS,
    ))
}

trait CsvScanAccumulator {
    fn note_skipped_bad_column_count(&mut self);

    fn note_rejected_row(&mut self);

    fn observe(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError>;

    fn mark_valid(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError>;

    fn mark_rejected(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError>;

    fn accept(&mut self, row: crate::normalize::NormalizedRow) -> Result<(), IngestError>;

    fn emit_ordered<E>(
        &mut self,
        source_id: &str,
        bucket_start: i64,
        lag_buckets: u64,
        emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>>;
}

struct CsvScanState {
    scan_locator: String,
    buckets: BTreeMap<(String, i64), StatisticalBucket>,
    bounds: BTreeMap<String, (i64, i64)>,
    next_emit: BTreeMap<String, i64>,
    has_emitted: BTreeSet<String>,
    valid_buckets: BTreeSet<(String, i64)>,
    rejected_buckets: BTreeSet<(String, i64)>,
    rejected_rows: u64,
    skipped_bad_column_count: u64,
}

impl CsvScanState {
    fn new(scan_locator: String) -> Self {
        Self {
            scan_locator,
            buckets: BTreeMap::new(),
            bounds: BTreeMap::new(),
            next_emit: BTreeMap::new(),
            has_emitted: BTreeSet::new(),
            valid_buckets: BTreeSet::new(),
            rejected_buckets: BTreeSet::new(),
            rejected_rows: 0,
            skipped_bad_column_count: 0,
        }
    }

    fn mark_valid(&mut self, source_id: &str, bucket_start: i64) {
        self.valid_buckets
            .insert((source_id.to_owned(), bucket_start));
    }

    fn mark_rejected(&mut self, source_id: &str, bucket_start: i64) {
        self.rejected_buckets
            .insert((source_id.to_owned(), bucket_start));
    }

    fn observe(&mut self, source_id: &str, bucket_start: i64) {
        self.bounds
            .entry(source_id.to_owned())
            .and_modify(|bounds| {
                bounds.0 = bounds.0.min(bucket_start);
                bounds.1 = bounds.1.max(bucket_start);
            })
            .or_insert((bucket_start, bucket_start));
        if !self.has_emitted.contains(source_id) {
            self.next_emit
                .entry(source_id.to_owned())
                .and_modify(|next| *next = (*next).min(bucket_start))
                .or_insert(bucket_start);
        }
    }

    fn accept(&mut self, row: crate::normalize::NormalizedRow) -> Result<(), IngestError> {
        self.buckets
            .entry((row.source_id.clone(), row.bucket_start))
            .or_insert_with(|| {
                StatisticalBucket::dense(BucketKey::new(
                    row.source_id,
                    Granularity::FiveMinutes,
                    row.bucket_start,
                    row.bucket_end,
                ))
            })
            .add(row.observation)?;
        Ok(())
    }

    fn emit_ordered<E>(
        &mut self,
        source_id: &str,
        bucket_start: i64,
        lag_buckets: u64,
        emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>> {
        let next = self.next_emit[source_id];
        if bucket_start < next {
            return Err(IngestError::InvalidInput(format!(
                "CSV input is not ordered enough for streaming: {} row bucket {bucket_start} arrived after flush cutoff; set input_order to unsorted",
                self.scan_locator
            ))
            .into());
        }
        let upper = self.bounds[source_id].1;
        let lag_seconds = i64::try_from(lag_buckets)
            .unwrap_or(i64::MAX)
            .saturating_mul(BUCKET_SECONDS);
        let last_start = upper
            .saturating_sub(lag_seconds)
            .saturating_sub(BUCKET_SECONDS);
        self.emit_through(source_id, last_start, emit)
    }

    fn emit_through<E>(
        &mut self,
        source_id: &str,
        last_start: i64,
        emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>> {
        let Some(mut next) = self.next_emit.get(source_id).copied() else {
            return Ok(());
        };
        while next <= last_start {
            let unit = (source_id.to_owned(), next);
            let observed = self.valid_buckets.remove(&unit);
            let rejected = self.rejected_buckets.remove(&unit);
            let coverage = BucketCoverage::new(1, u64::from(observed), u64::from(rejected))
                .map_err(DomainError::from)
                .map_err(IngestError::from)?;
            let key = BucketKey::new(
                source_id,
                Granularity::FiveMinutes,
                next,
                next + BUCKET_SECONDS,
            );
            let bucket = match self.buckets.remove(&unit) {
                Some(bucket) => bucket.with_coverage(coverage).finish(),
                None if observed => StatisticalBucket::dense(key).finish(),
                None => StatisticalBucket::new(key).with_coverage(coverage).finish(),
            };
            emit(CsvBucketReady {
                scan_locator: self.scan_locator.clone(),
                input_locator: self.scan_locator.clone(),
                bucket,
            })
            .map_err(ProducerError::Sink)?;
            next += BUCKET_SECONDS;
            self.has_emitted.insert(source_id.to_owned());
        }
        self.next_emit.insert(source_id.to_owned(), next);
        Ok(())
    }

    fn finish<E>(
        mut self,
        mut emit: impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<CsvScanComplete, ProducerError<E>> {
        for (source_id, (_, upper)) in self.bounds.clone() {
            self.emit_through(&source_id, upper, &mut emit)?;
        }
        Ok(CsvScanComplete {
            scan_locator: self.scan_locator,
            rejected_rows: self.rejected_rows,
            skipped_bad_column_count: self.skipped_bad_column_count,
            observed_bounds: self.bounds,
        })
    }
}

impl CsvScanAccumulator for CsvScanState {
    fn note_skipped_bad_column_count(&mut self) {
        self.skipped_bad_column_count += 1;
    }

    fn note_rejected_row(&mut self) {
        self.rejected_rows += 1;
    }

    fn observe(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        Self::observe(self, source_id, bucket_start);
        Ok(())
    }

    fn mark_valid(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        Self::mark_valid(self, source_id, bucket_start);
        Ok(())
    }

    fn mark_rejected(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        Self::mark_rejected(self, source_id, bucket_start);
        Ok(())
    }

    fn accept(&mut self, row: crate::normalize::NormalizedRow) -> Result<(), IngestError> {
        Self::accept(self, row)
    }

    fn emit_ordered<E>(
        &mut self,
        source_id: &str,
        bucket_start: i64,
        lag_buckets: u64,
        emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>> {
        Self::emit_ordered(self, source_id, bucket_start, lag_buckets, emit)
    }
}

const UNSORTED_STAGE_SCHEMA: &str = r#"
PRAGMA cache_size = -8192;
PRAGMA temp_store = FILE;
CREATE TABLE bounds (
    source_id TEXT PRIMARY KEY NOT NULL,
    lower_bound INTEGER NOT NULL,
    upper_bound INTEGER NOT NULL
);
CREATE TABLE coverage (
    source_id TEXT NOT NULL,
    bucket_start INTEGER NOT NULL,
    valid INTEGER NOT NULL DEFAULT 0,
    rejected INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_id, bucket_start)
);
CREATE TABLE accepted_rows (
    id INTEGER PRIMARY KEY,
    source_id TEXT NOT NULL,
    bucket_start INTEGER NOT NULL,
    src_ip TEXT NOT NULL,
    dst_ip TEXT NOT NULL,
    protocol INTEGER NOT NULL,
    packets INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    src_tos INTEGER NOT NULL,
    time_received_ms INTEGER,
    time_end_ms INTEGER,
    time_start_ms INTEGER,
    src_port INTEGER,
    dst_port INTEGER,
    dst_tos INTEGER NOT NULL,
    duration_ms INTEGER,
    min_ttl INTEGER,
    max_ttl INTEGER,
    flow_count INTEGER NOT NULL
);
CREATE INDEX accepted_rows_bucket
    ON accepted_rows (source_id, bucket_start, id);
"#;

struct UnsortedCsvScanState {
    scan_locator: String,
    connection: Connection,
    _temporary: NamedTempFile,
    rejected_rows: u64,
    skipped_bad_column_count: u64,
}

impl UnsortedCsvScanState {
    fn new(scan_locator: String) -> Result<Self, IngestError> {
        let temporary = NamedTempFile::new().map_err(|source| IngestError::Io {
            path: PathBuf::from("<temporary CSV staging database>"),
            source,
        })?;
        let connection = Connection::open(temporary.path())?;
        connection.execute_batch(UNSORTED_STAGE_SCHEMA)?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            scan_locator,
            connection,
            _temporary: temporary,
            rejected_rows: 0,
            skipped_bad_column_count: 0,
        })
    }

    fn emit_bucket<E>(
        &mut self,
        source_id: &str,
        bucket_start: i64,
        emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>> {
        let key = BucketKey::new(
            source_id,
            Granularity::FiveMinutes,
            bucket_start,
            bucket_start + BUCKET_SECONDS,
        );
        let mut bucket = None;
        {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT src_ip, dst_ip, protocol, packets, bytes, src_tos, \
                 time_received_ms, time_end_ms, time_start_ms, src_port, dst_port, \
                 dst_tos, duration_ms, min_ttl, max_ttl, flow_count \
                 FROM accepted_rows \
                 WHERE source_id = ?1 AND bucket_start = ?2 \
                 ORDER BY id",
                )
                .map_err(IngestError::from)?;
            let mut rows = statement
                .query(params![source_id, bucket_start])
                .map_err(IngestError::from)?;
            while let Some(row) = rows.next().map_err(IngestError::from)? {
                let observation = staged_observation(row)?;
                let bucket = bucket.get_or_insert_with(|| StatisticalBucket::dense(key.clone()));
                bucket.add(observation).map_err(IngestError::from)?;
            }
        }

        let (valid, rejected) = self
            .connection
            .query_row(
                "SELECT valid, rejected FROM coverage \
                 WHERE source_id = ?1 AND bucket_start = ?2",
                params![source_id, bucket_start],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(IngestError::from)?
            .unwrap_or((false, false));
        let coverage = BucketCoverage::new(1, u64::from(valid), u64::from(rejected))
            .map_err(DomainError::from)
            .map_err(IngestError::from)?;
        let bucket = match bucket {
            Some(bucket) => bucket.with_coverage(coverage).finish(),
            None if valid => StatisticalBucket::dense(key).finish(),
            None => StatisticalBucket::new(key).with_coverage(coverage).finish(),
        };
        emit(CsvBucketReady {
            scan_locator: self.scan_locator.clone(),
            input_locator: self.scan_locator.clone(),
            bucket,
        })
        .map_err(ProducerError::Sink)?;
        Ok(())
    }

    fn bounds(&self) -> Result<BTreeMap<String, (i64, i64)>, IngestError> {
        let mut statement = self
            .connection
            .prepare("SELECT source_id, lower_bound, upper_bound FROM bounds ORDER BY source_id")?;
        let mut rows = statement.query([])?;
        let mut bounds = BTreeMap::new();
        while let Some(row) = rows.next()? {
            bounds.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
        }
        Ok(bounds)
    }

    fn finish<E>(
        mut self,
        mut emit: impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<CsvScanComplete, ProducerError<E>> {
        self.connection
            .execute_batch("COMMIT")
            .map_err(IngestError::from)?;
        let observed_bounds = self.bounds()?;

        // Replay one source bucket at a time. The source rows, bounds, and
        // coverage flags stay on disk; only the current StatisticalBucket is
        // materialized while invoking the existing sink callback.
        for (source_id, (lower, upper)) in &observed_bounds {
            let mut bucket_start = *lower;
            while bucket_start <= *upper {
                self.emit_bucket(source_id, bucket_start, &mut emit)?;
                let Some(next) = bucket_start.checked_add(BUCKET_SECONDS) else {
                    break;
                };
                bucket_start = next;
            }
        }
        Ok(CsvScanComplete {
            scan_locator: self.scan_locator,
            rejected_rows: self.rejected_rows,
            skipped_bad_column_count: self.skipped_bad_column_count,
            observed_bounds,
        })
    }
}

impl CsvScanAccumulator for UnsortedCsvScanState {
    fn note_skipped_bad_column_count(&mut self) {
        self.skipped_bad_column_count += 1;
    }

    fn note_rejected_row(&mut self) {
        self.rejected_rows += 1;
    }

    fn observe(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        self.connection.execute(
            "INSERT INTO bounds (source_id, lower_bound, upper_bound) VALUES (?1, ?2, ?2) \
             ON CONFLICT(source_id) DO UPDATE SET \
                 lower_bound = MIN(lower_bound, excluded.lower_bound), \
                 upper_bound = MAX(upper_bound, excluded.upper_bound)",
            params![source_id, bucket_start],
        )?;
        Ok(())
    }

    fn mark_valid(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        self.connection.execute(
            "INSERT INTO coverage (source_id, bucket_start, valid) VALUES (?1, ?2, 1) \
             ON CONFLICT(source_id, bucket_start) DO UPDATE SET valid = 1",
            params![source_id, bucket_start],
        )?;
        Ok(())
    }

    fn mark_rejected(&mut self, source_id: &str, bucket_start: i64) -> Result<(), IngestError> {
        self.connection.execute(
            "INSERT INTO coverage (source_id, bucket_start, rejected) VALUES (?1, ?2, 1) \
             ON CONFLICT(source_id, bucket_start) DO UPDATE SET rejected = 1",
            params![source_id, bucket_start],
        )?;
        Ok(())
    }

    fn accept(&mut self, row: crate::normalize::NormalizedRow) -> Result<(), IngestError> {
        let crate::normalize::NormalizedRow {
            source_id,
            bucket_start,
            observation,
            ..
        } = row;
        self.connection.execute(
            "INSERT INTO accepted_rows (\
                 source_id, bucket_start, src_ip, dst_ip, protocol, packets, bytes, src_tos, \
                 time_received_ms, time_end_ms, time_start_ms, src_port, dst_port, dst_tos, \
                 duration_ms, min_ttl, max_ttl, flow_count\
             ) VALUES (\
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18\
             )",
            params![
                source_id,
                bucket_start,
                observation.src_ip.to_string(),
                observation.dst_ip.to_string(),
                i64::from(observation.protocol),
                observation.packets,
                observation.bytes,
                i64::from(observation.src_tos),
                observation.time_received_ms,
                observation.time_end_ms,
                observation.time_start_ms,
                observation.src_port.map(i64::from),
                observation.dst_port.map(i64::from),
                i64::from(observation.dst_tos),
                observation.duration_ms,
                observation.min_ttl.map(i64::from),
                observation.max_ttl.map(i64::from),
                observation.flow_count,
            ],
        )?;
        Ok(())
    }

    fn emit_ordered<E>(
        &mut self,
        _source_id: &str,
        _bucket_start: i64,
        _lag_buckets: u64,
        _emit: &mut impl FnMut(CsvBucketReady) -> Result<(), E>,
    ) -> Result<(), ProducerError<E>> {
        Ok(())
    }
}

fn staged_observation(row: &rusqlite::Row<'_>) -> Result<FlowObservation, IngestError> {
    let src_ip = row
        .get::<_, String>(0)?
        .parse()
        .map_err(|error| IngestError::InvalidInput(format!("invalid staged source IP: {error}")))?;
    let dst_ip = row.get::<_, String>(1)?.parse().map_err(|error| {
        IngestError::InvalidInput(format!("invalid staged destination IP: {error}"))
    })?;
    let protocol = staged_u8(row.get(2)?, "protocol")?;
    let packets = row.get(3)?;
    let bytes = row.get(4)?;
    let src_tos = staged_u8(row.get(5)?, "src_tos")?;
    let time_received_ms = row.get(6)?;
    let time_end_ms = row.get(7)?;
    let time_start_ms = row.get(8)?;
    let src_port = staged_optional_u16(row.get(9)?, "src_port")?;
    let dst_port = staged_optional_u16(row.get(10)?, "dst_port")?;
    let dst_tos = staged_u8(row.get(11)?, "dst_tos")?;
    let duration_ms = row.get(12)?;
    let min_ttl = staged_optional_u8(row.get(13)?, "min_ttl")?;
    let max_ttl = staged_optional_u8(row.get(14)?, "max_ttl")?;
    let flow_count = row.get(15)?;
    Ok(FlowObservation {
        src_ip,
        dst_ip,
        protocol,
        packets,
        bytes,
        src_tos,
        time_received_ms,
        time_end_ms,
        time_start_ms,
        src_port,
        dst_port,
        dst_tos,
        duration_ms,
        min_ttl,
        max_ttl,
        flow_count,
    })
}

fn staged_u8(value: i64, field: &str) -> Result<u8, IngestError> {
    value
        .try_into()
        .map_err(|_| IngestError::InvalidInput(format!("invalid staged {field} value {value}")))
}

fn staged_optional_u8(value: Option<i64>, field: &str) -> Result<Option<u8>, IngestError> {
    value.map(|value| staged_u8(value, field)).transpose()
}

fn staged_optional_u16(value: Option<i64>, field: &str) -> Result<Option<u16>, IngestError> {
    value
        .map(|value| {
            value.try_into().map_err(|_| {
                IngestError::InvalidInput(format!("invalid staged {field} value {value}"))
            })
        })
        .transpose()
}

/// Discover canonical `<root>/<source>/YYYY/MM/DD/nfcapd.YYYYMMDDHHMM` inputs.
pub fn discover_nfcapd_source_paths(
    root: impl AsRef<Path>,
    source_ids: &[String],
    timezone: &str,
) -> Result<Vec<NfcapdInputSpec>, IngestError> {
    let root = root.as_ref();
    let mut specs = Vec::new();
    for source_id in source_ids {
        let source_root = root.join(source_id);
        if !source_root.is_dir() {
            continue;
        }
        for year in read_matching_directories(&source_root, 4)? {
            for month in read_matching_directories(&year, 2)? {
                for day in read_matching_directories(&month, 2)? {
                    let entries = fs::read_dir(&day).map_err(|source| IngestError::Io {
                        path: day.clone(),
                        source,
                    })?;
                    for entry in entries {
                        let entry = entry.map_err(|source| IngestError::Io {
                            path: day.clone(),
                            source,
                        })?;
                        if !entry
                            .file_type()
                            .map_err(|source| IngestError::Io {
                                path: entry.path(),
                                source,
                            })?
                            .is_file()
                        {
                            continue;
                        }
                        let path = entry.path();
                        let Ok(bucket_start) = parse_nfcapd_bucket_start(&path, timezone) else {
                            continue;
                        };
                        specs.push(NfcapdInputSpec {
                            path,
                            source_id: source_id.clone(),
                            bucket_start,
                        });
                    }
                }
            }
        }
    }
    specs.sort_by(|left, right| {
        (&left.source_id, left.bucket_start, &left.path).cmp(&(
            &right.source_id,
            right.bucket_start,
            &right.path,
        ))
    });
    Ok(specs)
}

fn read_matching_directories(root: &Path, digits: usize) -> Result<Vec<PathBuf>, IngestError> {
    let entries = fs::read_dir(root).map_err(|source| IngestError::Io {
        path: root.to_owned(),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| IngestError::Io {
            path: root.to_owned(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() == digits
            && name.bytes().all(|byte| byte.is_ascii_digit())
            && entry
                .file_type()
                .map_err(|source| IngestError::Io {
                    path: entry.path(),
                    source,
                })?
                .is_dir()
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[must_use]
pub fn is_nfcapd_bucket_filename(name: &str) -> bool {
    name.strip_prefix("nfcapd.").is_some_and(|timestamp| {
        timestamp.len() == 12 && timestamp.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub fn parse_nfcapd_bucket_start(
    path: impl AsRef<Path>,
    timezone: &str,
) -> Result<i64, IngestError> {
    let path = path.as_ref();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            IngestError::InvalidInput(format!("invalid nfcapd path: {}", path.display()))
        })?;
    let raw = name.strip_prefix("nfcapd.").filter(|timestamp| {
        timestamp.len() == 12 && timestamp.bytes().all(|byte| byte.is_ascii_digit())
    });
    let raw =
        raw.ok_or_else(|| IngestError::InvalidInput(format!("invalid nfcapd filename: {name}")))?;
    DateTime::strptime("%Y%m%d%H%M", raw)
        .and_then(|datetime| datetime.in_tz(timezone))
        .map(|zoned| zoned.timestamp().as_second())
        .map_err(|error| {
            IngestError::InvalidInput(format!(
                "invalid nfcapd filename {name:?} for timezone {timezone:?}: {error}"
            ))
        })
}

/// Build the Atlantis binary `nfdump` command with safe prefix pushdown.
#[must_use]
pub fn build_nfdump_command(
    path: impl AsRef<Path>,
    selection: &FlowSelection,
    executable: impl AsRef<std::ffi::OsStr>,
) -> Vec<OsString> {
    let mut command = vec![
        executable.as_ref().to_owned(),
        "-r".into(),
        path.as_ref().as_os_str().to_owned(),
        "-q".into(),
        "-o".into(),
        nfdump::OUTPUT_MODE.into(),
    ];
    if let Some(filter) = selection.nfdump_filter() {
        command.push(filter.into());
    }
    command
}

/// Build one nfcapd command for several daily active-source selections.
pub fn build_nfdump_command_for_selections(
    path: impl AsRef<Path>,
    selections: &[FlowSelection],
    executable: impl AsRef<std::ffi::OsStr>,
) -> Result<Vec<OsString>, IngestError> {
    let mut command = vec![
        executable.as_ref().to_owned(),
        "-r".into(),
        path.as_ref().as_os_str().to_owned(),
        "-q".into(),
        "-o".into(),
        nfdump::OUTPUT_MODE.into(),
    ];
    command.push(daily_active_union_filter(selections)?.into());
    Ok(command)
}

fn daily_active_union_filter(selections: &[FlowSelection]) -> Result<String, IngestError> {
    if selections.is_empty() {
        return Err(IngestError::InvalidInput(
            "daily active-source selections are empty".into(),
        ));
    }
    let mut prefixes = selections
        .iter()
        .map(|selection| {
            if !selection.selects_daily_active_sources() {
                return Err(IngestError::InvalidInput(
                    "nfdump subset decoding requires daily_active_sources selections".into(),
                ));
            }
            selection
                .ip_prefix()
                .map(ToString::to_string)
                .ok_or_else(|| {
                    IngestError::InvalidInput(
                        "daily_active_sources selection is missing an ip_prefix".into(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    prefixes.sort_unstable();
    prefixes.dedup();
    let source_filter = |qualifier: &str| {
        if prefixes.len() == 1 {
            format!("{qualifier} net {}", prefixes[0])
        } else {
            format!(
                "({})",
                prefixes
                    .iter()
                    .map(|prefix| format!("{qualifier} net {prefix}"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        }
    };
    let outer_source_filter = source_filter("src");
    let tunnel_source_filter = source_filter("src tun");
    Ok(format!(
        "({outer_source_filter} and ipv4 and (proto tcp or proto udp) and src port > 1023) or ({tunnel_source_filter} and (tun proto tcp or tun proto udp) and src port > 1023)"
    ))
}

/// Create a private nfdump input directory containing exactly the requested captures.
///
/// `nfdump -R first:last` selects every alphabetically intervening file. A manifest
/// directory keeps the pinned nfdump range reader while making the input membership
/// explicit and bounded to the paths discovered by the pipeline.
fn prepare_nfcapd_manifest(paths: &[PathBuf]) -> Result<(tempfile::TempDir, PathBuf), IngestError> {
    let context = paths
        .first()
        .ok_or_else(|| IngestError::InvalidInput("nfcapd day range is empty".into()))?;
    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(IngestError::InvalidInput(
            "nfcapd day range paths must be strictly chronological".into(),
        ));
    }
    let manifest = tempfile::Builder::new()
        .prefix("atlantis-nfcapd-")
        .tempdir()
        .map_err(|source| IngestError::Io {
            path: context.clone(),
            source,
        })?;
    for (index, path) in paths.iter().enumerate() {
        let target = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .map_err(|source| IngestError::Io {
                    path: path.clone(),
                    source,
                })?
                .join(path)
        };
        let link = manifest.path().join(format!("nfcapd.{index:020}"));
        link_nfcapd_manifest_entry(&target, &link).map_err(|source| IngestError::Io {
            path: path.clone(),
            source,
        })?;
    }
    let manifest_path = manifest.path().to_owned();
    Ok((manifest, manifest_path))
}

fn link_nfcapd_manifest_entry(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

fn build_nfdump_manifest_command_for_selections(
    manifest: &Path,
    selections: &[FlowSelection],
    executable: impl AsRef<std::ffi::OsStr>,
) -> Result<Vec<OsString>, IngestError> {
    let mut command = vec![
        executable.as_ref().to_owned(),
        "-R".into(),
        manifest.as_os_str().to_owned(),
        "-q".into(),
        "-o".into(),
        nfdump::OUTPUT_MODE.into(),
    ];
    command.push(daily_active_union_filter(selections)?.into());
    Ok(command)
}

/// Prove that an executable implements the private Atlantis output contract before any pipeline
/// output is created.  An empty `-R` directory makes the probe independent of capture contents,
/// while the normal streaming decoder still enforces the header, terminator, and EOF contract.
pub(crate) fn probe_nfdump_compatibility(
    executable: impl AsRef<std::ffi::OsStr>,
) -> Result<(), IngestError> {
    let manifest = tempfile::tempdir().map_err(|source| IngestError::Io {
        path: PathBuf::from("<nfdump compatibility probe>"),
        source,
    })?;
    let command = vec![
        executable.as_ref().to_owned(),
        "-R".into(),
        manifest.path().as_os_str().to_owned(),
        "-q".into(),
        "-o".into(),
        nfdump::OUTPUT_MODE.into(),
    ];
    let key = BucketKey::new("<probe>", Granularity::FiveMinutes, 0, BUCKET_SECONDS);
    let bucket = run_nfdump(
        command,
        manifest.path(),
        NFDUMP_PROBE_TIMEOUT,
        move |stdout| nfdump::reduce_to_bucket(stdout, key, &FlowSelection::default()),
    )
    .map_err(|error| {
        IngestError::InvalidInput(format!(
            "nfdump compatibility probe for {:?} failed: {error}",
            executable.as_ref()
        ))
    })?;
    if bucket.traffic.iter().any(|scope| scope.metrics.flows != 0) {
        return Err(IngestError::InvalidInput(format!(
            "nfdump compatibility probe for {:?} emitted a non-empty Atlantis stream",
            executable.as_ref()
        )));
    }
    Ok(())
}

/// Decode one canonical nfcapd file into its dense five-minute bucket.
pub fn read_nfcapd_bucket(
    path: impl AsRef<Path>,
    source_id: &str,
    selection: &FlowSelection,
    executable: impl AsRef<std::ffi::OsStr>,
    timezone: &str,
) -> Result<CanonicalBucket, IngestError> {
    read_nfcapd_bucket_with_timeout(
        path,
        source_id,
        selection,
        executable,
        timezone,
        NFDUMP_TIMEOUT,
    )
}

fn read_nfcapd_bucket_with_timeout(
    path: impl AsRef<Path>,
    source_id: &str,
    selection: &FlowSelection,
    executable: impl AsRef<std::ffi::OsStr>,
    timezone: &str,
    timeout: Duration,
) -> Result<CanonicalBucket, IngestError> {
    let path = path.as_ref();
    let bucket_start = parse_nfcapd_bucket_start(path, timezone)?;
    let key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + BUCKET_SECONDS,
    );
    let command = build_nfdump_command(path, selection, executable.as_ref());
    let selection = selection.clone();
    run_nfdump(command, path, timeout, move |stdout| {
        nfdump::reduce_to_bucket(stdout, key, &selection)
    })
}

pub fn read_nfcapd_bucket_with_active_sources(
    path: impl AsRef<Path>,
    source_id: &str,
    selection: &FlowSelection,
    active_sources: Arc<AddressSet>,
    executable: impl AsRef<std::ffi::OsStr>,
    timezone: &str,
) -> Result<CanonicalBucket, IngestError> {
    let path = path.as_ref();
    let bucket_start = parse_nfcapd_bucket_start(path, timezone)?;
    let key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + BUCKET_SECONDS,
    );
    let command = build_nfdump_command(path, selection, executable.as_ref());
    let selection = selection.clone();
    run_nfdump(command, path, NFDUMP_TIMEOUT, move |stdout| {
        nfdump::reduce_to_bucket_with_active_sources(
            stdout,
            key,
            &selection,
            active_sources.as_ref(),
        )
    })
}

pub fn read_nfcapd_buckets_with_active_sources(
    path: impl AsRef<Path>,
    source_id: &str,
    selections_and_active_sources: &[NfcapdSelectionAndActiveSources],
    executable: impl AsRef<std::ffi::OsStr>,
    timezone: &str,
) -> Result<Vec<CanonicalBucket>, IngestError> {
    if selections_and_active_sources.is_empty() {
        return Err(IngestError::InvalidInput(
            "daily active-source selection pairs are empty".into(),
        ));
    }
    let path = path.as_ref();
    let bucket_start = parse_nfcapd_bucket_start(path, timezone)?;
    let key = BucketKey::new(
        source_id,
        Granularity::FiveMinutes,
        bucket_start,
        bucket_start + BUCKET_SECONDS,
    );
    let command = build_nfdump_command_for_selections(
        path,
        &selections_and_active_sources
            .iter()
            .map(|(selection, _)| selection.clone())
            .collect::<Vec<_>>(),
        executable.as_ref(),
    )?;
    let selections_and_active_sources = selections_and_active_sources.to_vec();
    run_nfdump(command, path, NFDUMP_TIMEOUT, move |stdout| {
        nfdump::reduce_to_buckets_with_active_sources(stdout, key, &selections_and_active_sources)
    })
}

pub(crate) fn read_nfcapd_daily_source_activities(
    paths: &[PathBuf],
    selections: &[FlowSelection],
    executable: impl AsRef<std::ffi::OsStr>,
) -> Result<Vec<HashMap<IpAddr, nfdump::SourceActivity>>, IngestError> {
    let context = paths
        .first()
        .ok_or_else(|| IngestError::InvalidInput("nfcapd day range is empty".into()))?;
    let (_manifest, manifest_path) = prepare_nfcapd_manifest(paths)?;
    let command =
        build_nfdump_manifest_command_for_selections(&manifest_path, selections, executable)?;
    let selections = selections.to_vec();
    run_nfdump(command, context, NFDUMP_DAY_TIMEOUT, move |stdout| {
        nfdump::reduce_to_daily_source_activities(stdout, &selections)
    })
}

fn run_nfdump<T, F>(
    command: Vec<OsString>,
    path: &Path,
    timeout: Duration,
    decode: F,
) -> Result<T, IngestError>
where
    T: Send + 'static,
    F: FnOnce(BufReader<std::process::ChildStdout>) -> Result<T, nfdump::NfdumpError>
        + Send
        + 'static,
{
    let executable_name = command[0].clone();
    let stderr_file = tempfile::tempfile().map_err(|source| IngestError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr_file.try_clone().map_err(|source| IngestError::Io {
            path: path.to_owned(),
            source,
        })?)
        .group_spawn()
        .map_err(|source| IngestError::StartNfdump {
            executable: executable_name,
            source,
        })?;
    let stdout = child
        .inner()
        .stdout
        .take()
        .ok_or_else(|| IngestError::InvalidInput("nfdump stdout was not captured".into()))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let result = decode(BufReader::new(stdout));
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + timeout;
    let reduced = match receiver.recv_timeout(timeout) {
        Ok(result) => {
            reader.join().map_err(|_| {
                IngestError::InvalidInput(format!(
                    "nfdump decoder thread panicked for {}",
                    path.display()
                ))
            })?;
            result
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            drop(receiver);
            let _ = reader.join();
            return Err(IngestError::NfdumpTimeout {
                seconds: timeout.as_secs(),
            });
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(IngestError::InvalidInput(format!(
                "nfdump decoder thread stopped unexpectedly for {}",
                path.display()
            )));
        }
    };
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|source| IngestError::Io {
            path: path.to_owned(),
            source,
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            drop(receiver);
            return Err(IngestError::NfdumpTimeout {
                seconds: timeout.as_secs(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return Err(IngestError::NfdumpFailed {
            exit_code: status.code(),
            stderr: read_tail(stderr_file, MAX_DIAGNOSTIC_BYTES)
                .unwrap_or_else(|error| format!("unable to read stderr: {error}")),
        });
    }
    reduced.map_err(|error| {
        IngestError::InvalidInput(format!(
            "malformed nfdump binary stream for {}: {error}",
            path.display()
        ))
    })
}

fn read_tail(mut file: fs::File, limit: usize) -> std::io::Result<String> {
    let length = file.seek(SeekFrom::End(0))?;
    let start = length.saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX));
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let detail = String::from_utf8_lossy(&bytes).trim().to_owned();
    Ok(if start == 0 {
        if detail.is_empty() {
            "no stderr".into()
        } else {
            detail
        }
    } else {
        format!("[truncated to final {limit} bytes] {detail}")
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, io::Write, sync::Arc};

    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::config::{CsvSourceConfig, InputOrder};
    use crate::coverage::CoverageState;
    use crate::domain::{IpVersion, Scope, Visibility};

    const ONE_V4_BINARY_STREAM: [u8; 92] = [
        // Stream header and one-record block count.
        b'A', b'T', b'L', b'N', b'F', b'L', b'O', b'W', 1, 0, 72, 0, 1, 0, 0, 0,
        // Source 192.0.2.1 and destination 198.51.100.1.
        192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 198, 51, 100, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, // packets=2, bytes=128, flows=3, duration=999ms.
        2, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 231, 3, 0, 0, 0,
        0, 0, 0, // src port=443, dst port=55000, TCP, IPv4 literal/literal, TTL 32..64.
        187, 1, 216, 214, 6, 0, 32, 64, // Successful end marker.
        0, 0, 0, 0,
    ];

    fn config() -> CsvSourceConfig {
        CsvSourceConfig {
            delimiter: b',',
            has_header: true,
            timestamp_format: "unix".into(),
            datetime_format: "%Y-%m-%d %H:%M:%S".into(),
            timestamp_timezone: "UTC".into(),
            fieldnames: None,
            columns: BTreeMap::from([
                ("time_received".into(), "received".into()),
                ("src_ip".into(), "src".into()),
                ("dst_ip".into(), "dst".into()),
                ("packets".into(), "packets".into()),
                ("bytes".into(), "bytes".into()),
                ("protocol".into(), "protocol".into()),
            ]),
            protocol_map: BTreeMap::from([
                ("TCP".into(), 6),
                ("UDP".into(), 17),
                ("ICMP".into(), 1),
                ("ICMPV6".into(), 58),
            ]),
            source_id_value: Some("sensor-a".into()),
            source_id_column: None,
            skip_bad_column_count: false,
            archive_member_contains: None,
            discovery_include_contains: vec!["csv".into()],
            discovery_include_suffixes: vec![".tar.gz".into(), ".tgz".into()],
            discovery_exclude_suffixes: vec![".aria2".into(), ".txt".into()],
            input_order: InputOrder::TimestampAscending,
            out_of_order_lag_buckets: 12,
        }
    }

    #[test]
    fn discovery_is_chronological_and_ignores_incomplete_downloads() {
        let directory = tempdir().unwrap();
        for name in [
            "march_week2.csv",
            "january_week4.csv",
            "january_week1.csv",
            "notes.txt",
            "february_week1.csv",
            "february_week1.csv.aria2",
        ] {
            fs::write(directory.path().join(name), "").unwrap();
        }

        let specs = discover_csv_inputs(directory.path(), "mapping.json", &config()).unwrap();
        let names = specs
            .iter()
            .map(|spec| spec.path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            ["january_week1.csv", "january_week4.csv", "march_week2.csv"]
        );
    }

    #[test]
    fn scan_distinguishes_rejected_missing_and_selected_out_buckets() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("flows.csv");
        fs::write(
            &input,
            concat!(
                "received,src,dst,packets,bytes,protocol\n",
                "0,192.0.2.1,198.51.100.1,2,128,TCP\n",
                "300,invalid,198.51.100.1,4,256,UDP\n",
                "900,203.0.113.5,198.51.100.2,8,512,UDP\n",
            ),
        )
        .unwrap();
        let selection = FlowSelection::from_payload(Some(&json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": "192.0.2.0/24",
        })))
        .unwrap();
        let mut buckets = Vec::new();

        let complete = scan_csv(&input, &config(), &selection, |ready| {
            buckets.push(ready);
            Ok::<_, ()>(())
        })
        .unwrap();

        assert_eq!(complete.rejected_rows, 1);
        assert_eq!(complete.observed_bounds["sensor-a"], (0, 900));
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0].bucket.key.bucket_start, 0);
        assert_eq!(buckets[1].bucket.key.bucket_start, 300);
        assert_eq!(buckets[2].bucket.key.bucket_start, 600);
        assert_eq!(buckets[3].bucket.key.bucket_start, 900);
        assert_eq!(buckets[0].bucket.coverage.state(), CoverageState::Complete);
        assert_eq!(buckets[1].bucket.coverage.state(), CoverageState::Partial);
        assert_eq!(buckets[2].bucket.coverage.state(), CoverageState::Unknown);
        assert_eq!(buckets[3].bucket.coverage.state(), CoverageState::Complete);
        assert!(buckets[1].bucket.traffic.is_empty());
        assert!(buckets[2].bucket.traffic.is_empty());
        let scope = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
        assert_eq!(
            buckets[0]
                .bucket
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics
                .flows,
            1
        );
        assert_eq!(
            buckets[3]
                .bucket
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics
                .flows,
            0,
            "selection excludes the valid third row without losing coverage"
        );
    }

    #[test]
    fn unsorted_scan_replays_far_apart_rows_in_order_with_coverage() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("unsorted.csv");
        fs::write(
            &input,
            concat!(
                "received,src,dst,packets,bytes,protocol\n",
                "3600,203.0.113.5,198.51.100.1,2,128,TCP\n",
                "900,invalid,198.51.100.1,4,256,UDP\n",
                "1800,192.0.2.1,198.51.100.1,8,512,UDP\n",
                "0,192.0.2.1,198.51.100.1,1,64,TCP\n",
            ),
        )
        .unwrap();
        let selection = FlowSelection::from_payload(Some(&json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": "192.0.2.0/24",
        })))
        .unwrap();
        let mut config = config();
        config.input_order = InputOrder::Unsorted;
        let mut buckets = Vec::new();

        let complete = scan_csv(&input, &config, &selection, |ready| {
            buckets.push(ready.bucket);
            Ok::<_, ()>(())
        })
        .unwrap();

        let starts = buckets
            .iter()
            .map(|bucket| bucket.key.bucket_start)
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            (0..=3600)
                .step_by(usize::try_from(BUCKET_SECONDS).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(complete.rejected_rows, 1);
        assert_eq!(complete.observed_bounds["sensor-a"], (0, 3600));
        assert_eq!(buckets[0].coverage.state(), CoverageState::Complete);
        assert_eq!(buckets[3].coverage.state(), CoverageState::Partial);
        assert_eq!(buckets[3].traffic.len(), 0);
        assert_eq!(buckets[6].coverage.state(), CoverageState::Complete);
        assert_eq!(buckets[12].coverage.state(), CoverageState::Complete);
        let scope = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
        assert_eq!(
            buckets[12]
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics
                .flows,
            0
        );
    }

    #[test]
    fn scan_reads_selected_members_from_compressed_tar() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("flows.tgz");
        let file = fs::File::create(&input).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        append_tar_member(
            &mut archive,
            "keep/flows.csv",
            b"received,src,dst,packets,bytes,protocol\n0,192.0.2.1,198.51.100.1,2,128,TCP\n",
        );
        append_tar_member(
            &mut archive,
            "ignore/flows.csv",
            b"received,src,dst,packets,bytes,protocol\n300,192.0.2.1,198.51.100.1,2,128,TCP\n",
        );
        archive.into_inner().unwrap().finish().unwrap();
        let mut config = config();
        config.archive_member_contains = Some("keep/".into());
        let mut starts = Vec::new();

        let complete = scan_csv(&input, &config, &FlowSelection::default(), |ready| {
            starts.push(ready.bucket.key.bucket_start);
            Ok::<_, ()>(())
        })
        .unwrap();

        assert_eq!(starts, [0]);
        assert_eq!(complete.observed_bounds["sensor-a"], (0, 0));
    }

    #[test]
    fn ordered_scan_fails_when_a_row_arrives_after_its_bucket_was_emitted() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("late.csv");
        fs::write(
            &input,
            concat!(
                "received,src,dst,packets,bytes,protocol\n",
                "0,192.0.2.1,198.51.100.1,1,1,TCP\n",
                "600,192.0.2.1,198.51.100.1,1,1,TCP\n",
                "0,192.0.2.1,198.51.100.1,1,1,TCP\n",
            ),
        )
        .unwrap();
        let mut config = config();
        config.out_of_order_lag_buckets = 0;

        let error = scan_csv(&input, &config, &FlowSelection::default(), |_| {
            Ok::<_, ()>(())
        })
        .unwrap_err();

        assert!(matches!(
            error,
            ProducerError::Input(IngestError::InvalidInput(message))
                if message.contains("not ordered enough")
        ));
    }

    fn append_tar_member(
        archive: &mut tar::Builder<GzEncoder<fs::File>>,
        path: &str,
        contents: &[u8],
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(contents.len()).unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
    }

    #[test]
    fn nfcapd_discovery_validates_names_and_uses_the_pipeline_timezone() {
        let directory = tempdir().unwrap();
        let day = directory.path().join("edge-a/2025/04/15");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("nfcapd.202504151200"), "").unwrap();
        fs::write(day.join("nfcapd.current"), "").unwrap();
        let specs = discover_nfcapd_source_paths(
            directory.path(),
            &["edge-a".into()],
            "America/Los_Angeles",
        )
        .unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].source_id, "edge-a");
        assert_eq!(specs[0].bucket_start, 1_744_743_600);
        assert!(is_nfcapd_bucket_filename("nfcapd.202504151200"));
        assert!(!is_nfcapd_bucket_filename("nfcapd.20250415120"));
    }

    #[test]
    fn nfdump_command_uses_binary_contract_and_safe_prefix_pushdown() {
        let selection = FlowSelection::from_payload(Some(&json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": "192.0.2.42/24",
        })))
        .unwrap();
        let command = build_nfdump_command("capture", &selection, "nfdump");

        assert_eq!(
            command,
            [
                "nfdump",
                "-r",
                "capture",
                "-q",
                "-o",
                "atlantis",
                "net 192.0.2.0/24",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn multi_daily_activity_command_unions_prefixes_and_keeps_fixed_filter() {
        let selections = [
            FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "198.51.0.0/16",
            })))
            .unwrap(),
            FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "192.0.0.0/16",
            })))
            .unwrap(),
            FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "192.0.0.0/16",
            })))
            .unwrap(),
        ];
        let command =
            build_nfdump_command_for_selections("capture", &selections, "nfdump").unwrap();

        assert_eq!(
            command.last().unwrap().to_string_lossy(),
            "((src net 192.0.0.0/16 or src net 198.51.0.0/16) and ipv4 and (proto tcp or proto udp) and src port > 1023) or ((src tun net 192.0.0.0/16 or src tun net 198.51.0.0/16) and (tun proto tcp or tun proto udp) and src port > 1023)"
        );
    }

    #[test]
    fn multi_nfdump_commands_reject_empty_and_non_daily_selection_sets() {
        assert!(matches!(
            build_nfdump_command_for_selections("capture", &[], "nfdump"),
            Err(IngestError::InvalidInput(message)) if message.contains("empty")
        ));
        assert!(matches!(
            build_nfdump_command_for_selections("capture", &[FlowSelection::default()], "nfdump"),
            Err(IngestError::InvalidInput(message)) if message.contains("daily_active_sources")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn multi_bucket_reader_decodes_one_process_into_one_bucket_per_pair() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        let invocation_log = directory.path().join("invocations.log");
        let mut binary = ONE_V4_BINARY_STREAM.to_vec();
        binary[16 + 32..16 + 40].copy_from_slice(&20_u64.to_le_bytes());
        binary[16 + 40..16 + 48].copy_from_slice(&2_000_u64.to_le_bytes());
        binary[16 + 48..16 + 56].copy_from_slice(&3_u64.to_le_bytes());
        binary[16 + 64..16 + 66].copy_from_slice(&55_000_u16.to_le_bytes());
        binary[16 + 69] = 0b010;
        fs::write(&stream, binary).unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf 'x\\n' >> '{}'\ncat '{}'\n",
                invocation_log.display(),
                stream.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "192.0.0.0/16",
        })))
        .unwrap();
        let source = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
        let pairs = [
            (
                selection.clone(),
                Arc::new([source].into_iter().collect::<AddressSet>()),
            ),
            (
                selection,
                Arc::new([source].into_iter().collect::<AddressSet>()),
            ),
        ];

        let buckets = read_nfcapd_buckets_with_active_sources(
            &capture,
            "edge-a",
            &pairs,
            &executable,
            "America/Los_Angeles",
        )
        .unwrap();

        assert_eq!(buckets.len(), 2);
        for bucket in buckets {
            assert_eq!(
                bucket
                    .traffic
                    .iter()
                    .find(|entry| {
                        entry.scope == Scope::new(IpVersion::V4, Visibility::All, Visibility::All)
                    })
                    .unwrap()
                    .metrics
                    .flows,
                3
            );
        }
        assert_eq!(
            fs::read_to_string(invocation_log).unwrap().lines().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn single_bucket_reader_accepts_shared_active_source_set() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        let mut binary = ONE_V4_BINARY_STREAM.to_vec();
        binary[16 + 64..16 + 66].copy_from_slice(&55_000_u16.to_le_bytes());
        binary[16 + 69] = 0b010;
        fs::write(&stream, binary).unwrap();
        fs::write(
            &executable,
            format!("#!/bin/sh\ncat '{}'\n", stream.display()),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "192.0.0.0/16",
        })))
        .unwrap();
        let source = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
        let active_sources = Arc::new([source].into_iter().collect::<AddressSet>());

        let bucket = read_nfcapd_bucket_with_active_sources(
            &capture,
            "edge-a",
            &selection,
            active_sources,
            &executable,
            "America/Los_Angeles",
        )
        .unwrap();

        assert_eq!(
            bucket
                .traffic
                .iter()
                .find(|entry| {
                    entry.scope == Scope::new(IpVersion::V4, Visibility::All, Visibility::All)
                })
                .unwrap()
                .metrics
                .flows,
            3
        );
    }

    #[cfg(unix)]
    #[test]
    fn multi_daily_activity_reader_decodes_one_range_into_distinct_maps() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        let invocation_log = directory.path().join("invocations.log");
        let mut binary = ONE_V4_BINARY_STREAM.to_vec();
        binary[16 + 32..16 + 40].copy_from_slice(&20_u64.to_le_bytes());
        binary[16 + 40..16 + 48].copy_from_slice(&2_000_u64.to_le_bytes());
        binary[16 + 48..16 + 56].copy_from_slice(&3_u64.to_le_bytes());
        binary[16 + 64..16 + 66].copy_from_slice(&55_000_u16.to_le_bytes());
        binary[16 + 69] = 0b010;
        fs::write(&stream, binary).unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf 'x\\n' >> '{}'\ncat '{}'\n",
                invocation_log.display(),
                stream.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        let selections = [
            FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "192.0.0.0/16",
            })))
            .unwrap(),
            FlowSelection::from_payload(Some(&json!({
                "kind": "daily_active_sources",
                "ip_prefix": "198.51.0.0/16",
            })))
            .unwrap(),
        ];

        let activities = read_nfcapd_daily_source_activities(
            std::slice::from_ref(&capture),
            &selections,
            &executable,
        )
        .unwrap();

        assert_eq!(activities.len(), 2);
        assert_eq!(activities[0].len(), 1);
        assert!(activities[1].is_empty());
        assert_eq!(
            fs::read_to_string(invocation_log).unwrap().lines().count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn daily_activity_reader_uses_only_the_discovered_capture_paths() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        let manifest_members = directory.path().join("manifest-members.log");
        let manifest_path = directory.path().join("manifest-path.log");
        let mut binary = ONE_V4_BINARY_STREAM.to_vec();
        binary[16 + 64..16 + 66].copy_from_slice(&55_000_u16.to_le_bytes());
        binary[16 + 69] = 0b010;
        fs::write(&stream, binary).unwrap();
        let first = directory.path().join("nfcapd.202506010000");
        let second = directory.path().join("nfcapd.202506010010");
        fs::write(&first, "selected first").unwrap();
        fs::write(&second, "selected second").unwrap();
        // These files are alphabetically between the selected captures. A physical
        // -R first:last range would read them even though they were not snapshotted.
        fs::write(
            directory.path().join("nfcapd.202506010005"),
            "untracked valid capture",
        )
        .unwrap();
        fs::write(
            directory.path().join("nfcapd.202506010007.backup"),
            "untracked backup",
        )
        .unwrap();
        fs::write(
            directory.path().join("nfcapd.202506010008.aria2"),
            "untracked sidecar",
        )
        .unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\n\
                 set -eu\n\
                 manifest=\"\"\n\
                 while [ \"$#\" -gt 0 ]; do\n\
                   if [ \"$1\" = \"-R\" ]; then manifest=\"$2\"; shift 2; else shift; fi\n\
                 done\n\
                 test -n \"$manifest\"\n\
                 test -d \"$manifest\"\n\
                 printf '%s\\n' \"$manifest\" > '{}'\n\
                 for member in \"$manifest\"/*; do\n\
                   test -L \"$member\"\n\
                   readlink \"$member\" >> '{}'\n\
                 done\n\
                 cat '{}'\n",
                manifest_path.display(),
                manifest_members.display(),
                stream.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "192.0.0.0/16",
        })))
        .unwrap();
        let mut activities = read_nfcapd_daily_source_activities(
            &[first.clone(), second.clone()],
            std::slice::from_ref(&selection),
            &executable,
        )
        .unwrap();
        let activity = activities.pop().unwrap();

        let source = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
        assert_eq!(activity[&source].flows, 3);
        assert_eq!(
            fs::read_to_string(manifest_members).unwrap(),
            format!("{}\n{}\n", first.display(), second.display())
        );
        let manifest_directory = fs::read_to_string(manifest_path).unwrap();
        assert!(!Path::new(manifest_directory.trim()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn daily_activity_reader_keeps_a_qualifying_synthetic_tunnel_flow_once() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        let invocation_log = directory.path().join("invocation.log");

        let mut synthetic: [u8; 72] = ONE_V4_BINARY_STREAM[16..88].try_into().unwrap();
        synthetic[..16].fill(0);
        synthetic[..4].copy_from_slice(&[10, 0, 0, 1]);
        synthetic[16..32].fill(0);
        synthetic[16..20].copy_from_slice(&[10, 0, 0, 2]);
        synthetic[32..40].copy_from_slice(&210_u64.to_le_bytes());
        synthetic[40..48].copy_from_slice(&4_467_904_u64.to_le_bytes());
        synthetic[48..56].copy_from_slice(&1_u64.to_le_bytes());
        synthetic[64..66].copy_from_slice(&22_222_u16.to_le_bytes());
        synthetic[66..68].copy_from_slice(&80_u16.to_le_bytes());
        synthetic[68] = 6;
        synthetic[69] = 0b010;
        synthetic[70..72].fill(0);

        let mut outer = synthetic;
        outer[..16].fill(0);
        outer[..4].copy_from_slice(&[72, 138, 170, 101]);
        outer[16..32].fill(0);
        outer[16..20].copy_from_slice(&[42, 16, 32, 6]);
        outer[48..56].copy_from_slice(&7_u64.to_le_bytes());
        outer[70..72].copy_from_slice(&[40, 255]);

        let mut binary = Vec::new();
        binary.extend_from_slice(&ONE_V4_BINARY_STREAM[..12]);
        binary.extend_from_slice(&2_u32.to_le_bytes());
        binary.extend_from_slice(&synthetic);
        binary.extend_from_slice(&outer);
        binary.extend_from_slice(&[0, 0, 0, 0]);
        fs::write(&stream, binary).unwrap();
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" > '{}'\ncat '{}'\n",
                invocation_log.display(),
                stream.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        let selection = FlowSelection::from_payload(Some(&json!({
            "kind": "daily_active_sources",
            "ip_prefix": "10.0.0.0/16",
        })))
        .unwrap();

        let mut activities = read_nfcapd_daily_source_activities(
            std::slice::from_ref(&capture),
            std::slice::from_ref(&selection),
            &executable,
        )
        .unwrap();
        let activity = activities.pop().unwrap();

        let source = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(activity.len(), 1);
        assert_eq!(
            activity[&source],
            nfdump::SourceActivity {
                flows: 1,
                packets: 20,
                bytes: 2_000,
            }
        );

        let active_sources = Arc::new([source].into_iter().collect::<AddressSet>());
        let bucket = read_nfcapd_bucket_with_active_sources(
            &capture,
            "edge-a",
            &selection,
            active_sources,
            &executable,
            "America/Los_Angeles",
        )
        .unwrap();
        let metrics = &bucket
            .traffic
            .iter()
            .find(|entry| {
                entry.scope == Scope::new(IpVersion::V4, Visibility::All, Visibility::All)
            })
            .unwrap()
            .metrics;
        assert_eq!(metrics.flows, 1);
        assert_eq!(metrics.packets, 210);
        assert_eq!(metrics.bytes, 4_467_904);

        let invocation = fs::read_to_string(invocation_log).unwrap();
        assert!(invocation.contains("src net 10.0.0.0/16"));
        assert!(invocation.contains("src tun net 10.0.0.0/16"));
        assert!(invocation.contains("tun proto tcp or tun proto udp"));
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_decoder_builds_the_canonical_bucket() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let stream = directory.path().join("stream.bin");
        fs::write(&stream, ONE_V4_BINARY_STREAM).unwrap();
        let mut script = fs::File::create(&executable).unwrap();
        writeln!(script, "#!/bin/sh").unwrap();
        writeln!(script, "cat '{}'", stream.display()).unwrap();
        drop(script);
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        let bucket = read_nfcapd_bucket(
            &capture,
            "edge-a",
            &FlowSelection::default(),
            &executable,
            "America/Los_Angeles",
        )
        .unwrap();
        let scope = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
        assert_eq!(bucket.key.bucket_start, 1_744_743_600);
        assert_eq!(
            bucket
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics
                .flows,
            3
        );
        let metrics = &bucket
            .traffic
            .iter()
            .find(|entry| entry.scope == scope)
            .unwrap()
            .metrics;
        assert_eq!(metrics.duration_sum_ms, 2_997);
        assert_eq!(metrics.duration_count, 3);
        assert_eq!(metrics.min_ttl_sum, 96);
        assert_eq!(metrics.max_ttl_sum, 192);
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_decoder_preserves_process_failure_diagnostics() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' 'decoder exploded' >&2\nexit 9\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let error = read_nfcapd_bucket(
            &capture,
            "edge-a",
            &FlowSelection::default(),
            &executable,
            "America/Los_Angeles",
        )
        .unwrap_err();

        assert!(matches!(
            error,
            IngestError::NfdumpFailed {
                exit_code: Some(9),
                stderr,
            } if stderr == "decoder exploded"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_failure_wins_after_stdout_closes_before_child_exit() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        fs::write(
            &executable,
            "#!/bin/sh\nprintf bad\nexec 1>&-\nsleep 0.1\nprintf '%s\\n' 'decoder exploded later' >&2\nexit 9\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let error = read_nfcapd_bucket(
            &capture,
            "edge-a",
            &FlowSelection::default(),
            &executable,
            "America/Los_Angeles",
        )
        .unwrap_err();

        assert!(matches!(
            error,
            IngestError::NfdumpFailed {
                exit_code: Some(9),
                stderr,
            } if stderr == "decoder exploded later"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn nfdump_decoder_timeout_kills_a_blocked_process_group() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("fake-nfdump");
        let header = directory.path().join("header.bin");
        fs::write(&header, &ONE_V4_BINARY_STREAM[..12]).unwrap();
        let capture = directory.path().join("nfcapd.202504151200");
        fs::write(&capture, "fixture").unwrap();
        fs::write(
            &executable,
            format!("#!/bin/sh\ncat '{}'\nsleep 5\n", header.display()),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let error = read_nfcapd_bucket_with_timeout(
            &capture,
            "edge-a",
            &FlowSelection::default(),
            &executable,
            "America/Los_Angeles",
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(matches!(error, IngestError::NfdumpTimeout { .. }));
    }
}
