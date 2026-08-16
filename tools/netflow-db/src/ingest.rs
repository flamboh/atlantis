//! Streaming adapters that turn external CSV inputs into canonical five-minute buckets.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use command_group::CommandGroup;
use csv::ReaderBuilder;
use flate2::read::MultiGzDecoder;
use jiff::civil::DateTime;
use thiserror::Error;

use crate::{
    config::{CsvSourceConfig, InputOrder},
    domain::{
        BucketKey, CanonicalBucket, DomainError, FlowSelection, Granularity, StatisticalBucket,
    },
    nfdump,
    normalize::{NormalizeError, field_indexes, normalize_csv_values},
};

const BUCKET_SECONDS: i64 = 300;
const NFDUMP_TIMEOUT: Duration = Duration::from_secs(300);
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
    let mut state = CsvScanState::new(scan_locator.clone());
    if is_tar_archive(path) {
        scan_tar(path, config, selection, &mut state, &mut emit)?;
    } else {
        let file = fs::File::open(path).map_err(|source| IngestError::Io {
            path: path.to_owned(),
            source,
        })?;
        scan_csv_reader(
            file,
            &scan_locator,
            config,
            selection,
            &mut state,
            &mut emit,
        )?;
    }
    state.finish(emit)
}

fn is_tar_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    name.ends_with(".tar.gz") || name.ends_with(".tgz")
}

fn scan_tar<E>(
    path: &Path,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    state: &mut CsvScanState,
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

fn scan_csv_reader<E>(
    reader: impl Read,
    locator: &str,
    config: &CsvSourceConfig,
    selection: &FlowSelection,
    state: &mut CsvScanState,
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
                state.skipped_bad_column_count += 1;
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
            state.observe(source_id, *bucket_start);
        }
        match normalize_csv_values(&values, config, &indexes) {
            Ok(row) if selection.matches(&row.observation) => state.accept(row)?,
            Ok(_) => {}
            Err(error) => {
                state.rejected_rows += 1;
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

struct CsvScanState {
    scan_locator: String,
    buckets: BTreeMap<(String, i64), StatisticalBucket>,
    bounds: BTreeMap<String, (i64, i64)>,
    next_emit: BTreeMap<String, i64>,
    has_emitted: BTreeSet<String>,
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
            rejected_rows: 0,
            skipped_bad_column_count: 0,
        }
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
            let (bucket, input_locator) = match self.buckets.remove(&(source_id.to_owned(), next)) {
                Some(bucket) => (bucket.finish(), self.scan_locator.clone()),
                None => {
                    let key = BucketKey::new(
                        source_id,
                        Granularity::FiveMinutes,
                        next,
                        next + BUCKET_SECONDS,
                    );
                    (
                        StatisticalBucket::dense(key).finish(),
                        csv_gap_locator(&self.scan_locator, source_id, next),
                    )
                }
            };
            emit(CsvBucketReady {
                scan_locator: self.scan_locator.clone(),
                input_locator,
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

#[must_use]
pub fn csv_gap_locator(scan_locator: &str, source_id: &str, bucket_start: i64) -> String {
    format!(
        "gap://csv/{}/{}/{bucket_start}",
        percent_encode(scan_locator),
        percent_encode(source_id)
    )
}

fn percent_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(*byte).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
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
    if let Some(filter) = selection.nfdump_prefix_filter() {
        command.push(filter.into());
    }
    command
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
    let selection = selection.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let result = nfdump::reduce_to_bucket(BufReader::new(stdout), key, &selection);
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
    use std::{collections::BTreeMap, fs, io::Write};

    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::config::{CsvSourceConfig, InputOrder};
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
    fn scan_emits_selected_rows_and_dense_coverage_for_rejected_rows() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("flows.csv");
        fs::write(
            &input,
            concat!(
                "received,src,dst,packets,bytes,protocol\n",
                "0,192.0.2.1,198.51.100.1,2,128,TCP\n",
                "300,invalid,198.51.100.1,4,256,UDP\n",
                "600,203.0.113.5,198.51.100.2,8,512,UDP\n",
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
        assert_eq!(complete.observed_bounds["sensor-a"], (0, 600));
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].bucket.key.bucket_start, 0);
        assert_eq!(
            buckets[1].input_locator,
            csv_gap_locator(input.to_str().unwrap(), "sensor-a", 300)
        );
        assert_eq!(buckets[2].bucket.key.bucket_start, 600);
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
            buckets[1]
                .bucket
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics
                .flows,
            0
        );
        assert_eq!(
            buckets[2]
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
