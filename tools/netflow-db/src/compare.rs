//! Read-only compatibility comparison between a Rust candidate and a historical database.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use rusqlite::{Connection, OptionalExtension, Rows, params, types::Value as SqlValue};
use serde::Serialize;
use thiserror::Error;

use crate::{
    maad::decode_f32,
    storage::{MaadQGridRow, StorageError, connect_readonly, stored_maad_internal_side},
};

const TABLES: &[TableSpec] = &[
    TableSpec::new(
        "traffic_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "bucket_end",
        ],
        &["processed_at"],
        &[],
        CandidateOnlyPolicy::MissingReferenceBucketOrDenseZero,
    ),
    TableSpec::new(
        "bucket_coverage",
        &["granularity", "bucket_start", "source_id", "bucket_end"],
        &[],
        &[],
        CandidateOnlyPolicy::Never,
    ),
    TableSpec::new(
        "protocol_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "bucket_end",
        ],
        &["processed_at"],
        &[],
        CandidateOnlyPolicy::MissingReferenceBucketOrDenseZero,
    ),
    TableSpec::new(
        "address_count_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "address_side",
            "bucket_end",
        ],
        &["processed_at"],
        &[],
        CandidateOnlyPolicy::MissingReferenceBucketOrDenseZero,
    ),
    TableSpec::new(
        "port_count_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "port_side",
            "port_range",
            "bucket_end",
        ],
        &["processed_at"],
        &[],
        CandidateOnlyPolicy::Always,
    ),
    TableSpec::new(
        "address_maad_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "address_side",
            "measure",
            "bucket_end",
        ],
        &[],
        &["d0", "d1", "d2", "tau", "tau_sd", "spectrum"],
        CandidateOnlyPolicy::MissingReferenceBucket,
    ),
    TableSpec::new(
        "address_concentration_stats",
        &[
            "granularity",
            "bucket_start",
            "source_id",
            "ip_version",
            "src_locality",
            "dst_locality",
            "address_side",
            "measure",
            "bucket_end",
        ],
        &[],
        &[
            "hhi",
            "top1_share",
            "top10_share",
            "top100_share",
            "entropy_p8",
            "entropy_p16",
            "entropy_p24",
            "entropy_p32",
            "entropy_p48",
            "entropy_p64",
            "entropy_p128",
            "weight_total",
        ],
        CandidateOnlyPolicy::MissingReferenceBucket,
    ),
    TableSpec::new(
        "processed_inputs",
        &[
            "input_kind",
            "input_locator",
            "source_id",
            "bucket_start",
            "bucket_end",
        ],
        &["discovered_at", "processed_at"],
        &[],
        CandidateOnlyPolicy::Never,
    ),
];

#[derive(Clone, Debug)]
pub struct CompareOptions {
    pub candidate: PathBuf,
    pub reference: PathBuf,
    pub start_ts: i64,
    pub end_exclusive_ts: i64,
    pub maad_absolute_tolerance: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComparisonReport {
    pub compatible: bool,
    pub candidate: PathBuf,
    pub reference: PathBuf,
    pub start_ts: i64,
    pub end_exclusive_ts: i64,
    pub maad_absolute_tolerance: f64,
    pub tables: BTreeMap<String, TableComparison>,
    pub maad_q_grid: QGridComparison,
}

/// The q grids behind the IP versions whose MAAD curves both databases store in the window.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct QGridComparison {
    pub ip_versions: Vec<i64>,
    pub mismatched_ip_versions: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TableComparison {
    pub comparable: bool,
    pub candidate_rows: i64,
    pub reference_rows: i64,
    pub shared_rows: i64,
    pub candidate_only_rows: i64,
    pub unexpected_candidate_only_rows: i64,
    pub reference_only_rows: i64,
    /// Reference rows for internal-side MAAD scopes that the candidate does not compute.
    pub skipped_reference_rows: i64,
    pub mismatched_rows: i64,
    pub max_maad_absolute_delta: f64,
}

#[derive(Debug, Error)]
pub enum CompareError {
    #[error("candidate database not found: {0}")]
    CandidateNotFound(PathBuf),
    #[error("reference database not found: {0}")]
    ReferenceNotFound(PathBuf),
    #[error("comparison end must be after start")]
    InvalidWindow,
    #[error("MAAD absolute tolerance must be finite and nonnegative")]
    InvalidTolerance,
    #[error("{table} is missing comparison key column {column} in the {database} database")]
    MissingKeyColumn {
        table: &'static str,
        column: &'static str,
        database: &'static str,
    },
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("storage operation failed: {0}")]
    Storage(#[from] StorageError),
}

pub fn compare_databases(options: &CompareOptions) -> Result<ComparisonReport, CompareError> {
    validate_options(options)?;
    let candidate = connect_readonly(&options.candidate)?;
    let reference = connect_readonly(&options.reference)?;
    let candidate_skips_internal_side = !stored_maad_internal_side(&candidate)?;
    let mut compatible = true;
    let mut tables = BTreeMap::new();
    for spec in TABLES {
        let skip_internal_side = candidate_skips_internal_side
            && matches!(
                spec.name,
                "address_maad_stats" | "address_concentration_stats"
            );
        let comparison = compare_table(&candidate, &reference, spec, skip_internal_side, options)?;
        compatible &= comparison.reference_only_rows == 0
            && comparison.unexpected_candidate_only_rows == 0
            && comparison.mismatched_rows == 0;
        tables.insert(spec.name.to_owned(), comparison);
    }
    let maad_q_grid = compare_q_grids(&candidate, &reference, options)?;
    compatible &= maad_q_grid.mismatched_ip_versions.is_empty();
    Ok(ComparisonReport {
        compatible,
        candidate: options.candidate.clone(),
        reference: options.reference.clone(),
        start_ts: options.start_ts,
        end_exclusive_ts: options.end_exclusive_ts,
        maad_absolute_tolerance: options.maad_absolute_tolerance,
        tables,
        maad_q_grid,
    })
}

fn compare_q_grids(
    candidate: &Connection,
    reference: &Connection,
    options: &CompareOptions,
) -> Result<QGridComparison, rusqlite::Error> {
    let reference_versions = curve_ip_versions(reference, options)?;
    let ip_versions = curve_ip_versions(candidate, options)?
        .intersection(&reference_versions)
        .copied()
        .collect::<Vec<_>>();
    let candidate_grids = load_q_grids(candidate)?;
    let reference_grids = load_q_grids(reference)?;
    let mismatched_ip_versions = ip_versions
        .iter()
        .copied()
        .filter(|ip_version| {
            let candidate_grid = candidate_grids.get(ip_version);
            candidate_grid.is_none() || candidate_grid != reference_grids.get(ip_version)
        })
        .collect();
    Ok(QGridComparison {
        ip_versions,
        mismatched_ip_versions,
    })
}

fn curve_ip_versions(
    connection: &Connection,
    options: &CompareOptions,
) -> Result<BTreeSet<i64>, rusqlite::Error> {
    if !table_columns(connection, "address_maad_stats")?
        .iter()
        .any(|column| column == "tau")
    {
        return Ok(BTreeSet::new());
    }
    let mut statement = connection.prepare(
        "SELECT DISTINCT ip_version FROM address_maad_stats \
         WHERE tau IS NOT NULL AND bucket_start >= ?1 AND bucket_start < ?2",
    )?;
    statement
        .query_map(params![options.start_ts, options.end_exclusive_ts], |row| {
            row.get(0)
        })?
        .collect()
}

fn load_q_grids(connection: &Connection) -> Result<BTreeMap<i64, MaadQGridRow>, rusqlite::Error> {
    if table_columns(connection, "maad_q_grid")?.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut statement =
        connection.prepare("SELECT ip_version, q_min, q_step, q_count FROM maad_q_grid")?;
    statement
        .query_map([], |row| {
            let grid = MaadQGridRow {
                ip_version: row.get(0)?,
                q_min: row.get(1)?,
                q_step: row.get(2)?,
                q_count: row.get(3)?,
            };
            Ok((grid.ip_version, grid))
        })?
        .collect()
}

fn validate_options(options: &CompareOptions) -> Result<(), CompareError> {
    if !options.candidate.is_file() {
        return Err(CompareError::CandidateNotFound(options.candidate.clone()));
    }
    if !options.reference.is_file() {
        return Err(CompareError::ReferenceNotFound(options.reference.clone()));
    }
    if options.end_exclusive_ts <= options.start_ts {
        return Err(CompareError::InvalidWindow);
    }
    if !options.maad_absolute_tolerance.is_finite() || options.maad_absolute_tolerance < 0.0 {
        return Err(CompareError::InvalidTolerance);
    }
    Ok(())
}

fn compare_table(
    candidate: &Connection,
    reference: &Connection,
    spec: &TableSpec,
    skip_internal_side: bool,
    options: &CompareOptions,
) -> Result<TableComparison, CompareError> {
    let candidate_columns = table_columns(candidate, spec.name)?;
    let reference_columns = table_columns(reference, spec.name)?;
    match (candidate_columns.is_empty(), reference_columns.is_empty()) {
        (true, true) => return Ok(TableComparison::default()),
        (false, true) => {
            let candidate_rows = count_rows(candidate, spec.name, options)?;
            return Ok(TableComparison {
                candidate_rows,
                candidate_only_rows: candidate_rows,
                unexpected_candidate_only_rows: if matches!(
                    spec.candidate_only_policy,
                    CandidateOnlyPolicy::Always
                ) {
                    0
                } else {
                    candidate_rows
                },
                ..TableComparison::default()
            });
        }
        (true, false) => {
            let reference_rows = count_rows(reference, spec.name, options)?;
            return Ok(TableComparison {
                reference_rows,
                reference_only_rows: reference_rows,
                ..TableComparison::default()
            });
        }
        (false, false) => {}
    }

    require_keys(spec, &candidate_columns, "candidate")?;
    require_keys(spec, &reference_columns, "reference")?;
    let ignored = spec
        .ignored_columns
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let reference_names = reference_columns
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let common = candidate_columns
        .iter()
        .filter(|column| reference_names.contains(column.as_str()))
        .filter(|column| !ignored.contains(column.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let key_names = spec.key_columns.iter().copied().collect::<BTreeSet<_>>();
    let mut columns = spec
        .key_columns
        .iter()
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    columns.extend(
        common
            .into_iter()
            .filter(|column| !key_names.contains(column.as_str())),
    );
    let reference_coverage = load_reference_coverage(reference, spec, options)?;
    compare_shared_rows(
        candidate,
        reference,
        spec,
        &columns,
        &reference_coverage,
        skip_internal_side,
        options,
    )
}

fn compare_shared_rows(
    candidate: &Connection,
    reference: &Connection,
    spec: &TableSpec,
    columns: &[String],
    reference_coverage: &ReferenceCoverage,
    skip_internal_side: bool,
    options: &CompareOptions,
) -> Result<TableComparison, CompareError> {
    let query = ordered_query(spec, columns);
    let mut candidate_statement = candidate.prepare(&query)?;
    let mut reference_statement = reference.prepare(&query)?;
    let mut candidate_rows =
        candidate_statement.query(params![options.start_ts, options.end_exclusive_ts])?;
    let mut reference_rows =
        reference_statement.query(params![options.start_ts, options.end_exclusive_ts])?;
    let mut candidate_row = next_row(&mut candidate_rows, columns.len(), spec.key_columns.len())?;
    let mut reference_row = next_row(&mut reference_rows, columns.len(), spec.key_columns.len())?;
    let mut report = TableComparison {
        comparable: true,
        ..TableComparison::default()
    };
    while candidate_row.is_some() || reference_row.is_some() {
        if skip_internal_side {
            if candidate_row
                .as_ref()
                .is_some_and(|row| is_internal_side_row(spec, row))
            {
                report.candidate_rows += 1;
                report.candidate_only_rows += 1;
                report.unexpected_candidate_only_rows += 1;
                candidate_row =
                    next_row(&mut candidate_rows, columns.len(), spec.key_columns.len())?;
                continue;
            }
            if reference_row
                .as_ref()
                .is_some_and(|row| is_internal_side_row(spec, row))
            {
                report.reference_rows += 1;
                report.skipped_reference_rows += 1;
                reference_row =
                    next_row(&mut reference_rows, columns.len(), spec.key_columns.len())?;
                continue;
            }
        }
        match (&candidate_row, &reference_row) {
            (Some(candidate_value), Some(reference_value)) => {
                match candidate_value.key.cmp(&reference_value.key) {
                    Ordering::Less => {
                        report.candidate_rows += 1;
                        report.candidate_only_rows += 1;
                        report.unexpected_candidate_only_rows += i64::from(
                            !candidate_only_allowed(spec, candidate_value, reference_coverage),
                        );
                        candidate_row =
                            next_row(&mut candidate_rows, columns.len(), spec.key_columns.len())?;
                    }
                    Ordering::Greater => {
                        report.reference_rows += 1;
                        report.reference_only_rows += 1;
                        reference_row =
                            next_row(&mut reference_rows, columns.len(), spec.key_columns.len())?;
                    }
                    Ordering::Equal => {
                        report.candidate_rows += 1;
                        report.reference_rows += 1;
                        report.shared_rows += 1;
                        if !rows_match(
                            candidate_value,
                            reference_value,
                            spec,
                            columns,
                            options.maad_absolute_tolerance,
                            &mut report.max_maad_absolute_delta,
                        ) {
                            report.mismatched_rows += 1;
                        }
                        candidate_row =
                            next_row(&mut candidate_rows, columns.len(), spec.key_columns.len())?;
                        reference_row =
                            next_row(&mut reference_rows, columns.len(), spec.key_columns.len())?;
                    }
                }
            }
            (Some(_), None) => {
                report.candidate_rows += 1;
                report.candidate_only_rows += 1;
                report.unexpected_candidate_only_rows += i64::from(!candidate_only_allowed(
                    spec,
                    candidate_row.as_ref().expect("candidate row is present"),
                    reference_coverage,
                ));
                candidate_row =
                    next_row(&mut candidate_rows, columns.len(), spec.key_columns.len())?;
            }
            (None, Some(_)) => {
                report.reference_rows += 1;
                report.reference_only_rows += 1;
                reference_row =
                    next_row(&mut reference_rows, columns.len(), spec.key_columns.len())?;
            }
            (None, None) => break,
        }
    }
    Ok(report)
}

fn rows_match(
    candidate: &ComparableRow,
    reference: &ComparableRow,
    spec: &TableSpec,
    columns: &[String],
    tolerance: f64,
    max_delta: &mut f64,
) -> bool {
    let mut matches = true;
    for ((column, candidate_value), reference_value) in
        columns.iter().zip(&candidate.values).zip(&reference.values)
    {
        if candidate_value == reference_value {
            continue;
        }
        matches &= spec.maad_columns.contains(&column.as_str())
            && maad_values_match(candidate_value, reference_value, tolerance, max_delta);
    }
    matches
}

/// Compare MAAD REAL values, or f32 BLOB arrays element-wise, within the absolute tolerance.
fn maad_values_match(
    candidate: &SqlValue,
    reference: &SqlValue,
    tolerance: f64,
    max_delta: &mut f64,
) -> bool {
    let mut within = |candidate: f64, reference: f64| {
        let delta = (candidate - reference).abs();
        *max_delta = max_delta.max(delta);
        delta <= tolerance
    };
    match (candidate, reference) {
        (SqlValue::Real(candidate), SqlValue::Real(reference)) => within(*candidate, *reference),
        (SqlValue::Blob(candidate), SqlValue::Blob(reference)) => {
            match (decode_f32(candidate), decode_f32(reference)) {
                (Some(candidate), Some(reference)) if candidate.len() == reference.len() => {
                    candidate.iter().zip(&reference).fold(
                        true,
                        |matches, (candidate, reference)| {
                            within(f64::from(*candidate), f64::from(*reference)) && matches
                        },
                    )
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn table_columns(
    connection: &Connection,
    table: &'static str,
) -> Result<Vec<String>, rusqlite::Error> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
    statement
        .query_map([], |row| row.get(1))?
        .collect::<Result<Vec<_>, _>>()
}

fn load_reference_coverage(
    reference: &Connection,
    spec: &TableSpec,
    options: &CompareOptions,
) -> Result<ReferenceCoverage, rusqlite::Error> {
    if !matches!(
        spec.candidate_only_policy,
        CandidateOnlyPolicy::MissingReferenceBucket
            | CandidateOnlyPolicy::MissingReferenceBucketOrDenseZero
    ) {
        return Ok(ReferenceCoverage::default());
    }
    let mut coverage_statement = reference.prepare(&format!(
        "SELECT DISTINCT source_id, granularity, bucket_start FROM {} \
         WHERE bucket_start >= ?1 AND bucket_start < ?2",
        quote(spec.name)
    ))?;
    let bucket_keys = coverage_statement
        .query_map(params![options.start_ts, options.end_exclusive_ts], |row| {
            Ok(vec![
                KeyValue::from(row.get::<_, SqlValue>(0)?),
                KeyValue::from(row.get::<_, SqlValue>(1)?),
                KeyValue::from(row.get::<_, SqlValue>(2)?),
            ])
        })?
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut source_statement = reference.prepare(&format!(
        "SELECT DISTINCT source_id FROM {} \
         WHERE bucket_start >= ?1 AND bucket_start < ?2",
        quote(spec.name)
    ))?;
    let known_sources = source_statement
        .query_map(params![options.start_ts, options.end_exclusive_ts], |row| {
            Ok(KeyValue::from(row.get::<_, SqlValue>(0)?))
        })?
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(ReferenceCoverage {
        bucket_keys,
        known_sources,
    })
}

fn candidate_only_allowed(
    spec: &TableSpec,
    candidate: &ComparableRow,
    reference: &ReferenceCoverage,
) -> bool {
    match spec.candidate_only_policy {
        CandidateOnlyPolicy::Always => true,
        CandidateOnlyPolicy::Never => false,
        CandidateOnlyPolicy::MissingReferenceBucket => {
            reference_bucket_missing(spec, candidate, reference)
        }
        CandidateOnlyPolicy::MissingReferenceBucketOrDenseZero => {
            reference_bucket_missing(spec, candidate, reference)
                || candidate.values[spec.key_columns.len()..]
                    .iter()
                    .all(is_dense_zero_value)
        }
    }
}

fn reference_bucket_missing(
    spec: &TableSpec,
    candidate: &ComparableRow,
    reference: &ReferenceCoverage,
) -> bool {
    let source = key_component(spec, candidate, "source_id");
    let granularity = key_component(spec, candidate, "granularity");
    let bucket_start = key_component(spec, candidate, "bucket_start");
    reference.known_sources.contains(source)
        && matches!(
            granularity,
            KeyValue::Text(granularity)
                if matches!(granularity.as_str(), "5m" | "10m" | "30m" | "1h" | "1d")
        )
        && !reference.bucket_keys.contains(&vec![
            source.clone(),
            granularity.clone(),
            bucket_start.clone(),
        ])
}

fn is_internal_side_row(spec: &TableSpec, row: &ComparableRow) -> bool {
    let internal = |column| matches!(key_component(spec, row, column), KeyValue::Text(locality) if locality == "internal");
    match key_component(spec, row, "address_side") {
        KeyValue::Text(side) if side == "source" => internal("src_locality"),
        KeyValue::Text(side) if side == "destination" => internal("dst_locality"),
        _ => false,
    }
}

fn is_dense_zero_value(value: &SqlValue) -> bool {
    match value {
        SqlValue::Null => true,
        SqlValue::Integer(value) => *value == 0,
        SqlValue::Real(value) => *value == 0.0,
        SqlValue::Text(value) => value.is_empty(),
        SqlValue::Blob(value) => value.is_empty(),
    }
}

fn key_component<'a>(spec: &TableSpec, row: &'a ComparableRow, column: &str) -> &'a KeyValue {
    let index = spec
        .key_columns
        .iter()
        .position(|candidate| *candidate == column)
        .expect("comparison coverage columns are part of the key");
    &row.key[index]
}

fn require_keys(
    spec: &TableSpec,
    columns: &[String],
    database: &'static str,
) -> Result<(), CompareError> {
    for key in spec.key_columns {
        if !columns.iter().any(|column| column == key) {
            return Err(CompareError::MissingKeyColumn {
                table: spec.name,
                column: key,
                database,
            });
        }
    }
    Ok(())
}

fn count_rows(
    connection: &Connection,
    table: &'static str,
    options: &CompareOptions,
) -> Result<i64, rusqlite::Error> {
    connection.query_row(
        &format!(
            "SELECT COUNT(*) FROM {} WHERE bucket_start >= ?1 AND bucket_start < ?2",
            quote(table)
        ),
        params![options.start_ts, options.end_exclusive_ts],
        |row| row.get(0),
    )
}

fn ordered_query(spec: &TableSpec, columns: &[String]) -> String {
    let selected = columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ");
    let ordered = spec
        .key_columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ");
    let granularity_filter = if spec.key_columns.contains(&"granularity") {
        "granularity IN ('1d', '1h', '30m', '10m', '5m') AND "
    } else {
        ""
    };
    format!(
        "SELECT {selected} FROM {} WHERE {granularity_filter}bucket_start >= ?1 AND bucket_start < ?2 ORDER BY {ordered}",
        quote(spec.name),
    )
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn next_row(
    rows: &mut Rows<'_>,
    column_count: usize,
    key_count: usize,
) -> Result<Option<ComparableRow>, rusqlite::Error> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let values = (0..column_count)
        .map(|index| row.get(index))
        .collect::<Result<Vec<SqlValue>, _>>()?;
    let key = values[..key_count]
        .iter()
        .cloned()
        .map(KeyValue::from)
        .collect();
    Ok(Some(ComparableRow { key, values }))
}

struct TableSpec {
    name: &'static str,
    key_columns: &'static [&'static str],
    ignored_columns: &'static [&'static str],
    maad_columns: &'static [&'static str],
    candidate_only_policy: CandidateOnlyPolicy,
}

impl TableSpec {
    const fn new(
        name: &'static str,
        key_columns: &'static [&'static str],
        ignored_columns: &'static [&'static str],
        maad_columns: &'static [&'static str],
        candidate_only_policy: CandidateOnlyPolicy,
    ) -> Self {
        Self {
            name,
            key_columns,
            ignored_columns,
            maad_columns,
            candidate_only_policy,
        }
    }
}

#[derive(Clone, Copy)]
enum CandidateOnlyPolicy {
    Always,
    MissingReferenceBucket,
    MissingReferenceBucketOrDenseZero,
    Never,
}

#[derive(Default)]
struct ReferenceCoverage {
    bucket_keys: BTreeSet<Vec<KeyValue>>,
    known_sources: BTreeSet<KeyValue>,
}

struct ComparableRow {
    key: Vec<KeyValue>,
    values: Vec<SqlValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum KeyValue {
    Null,
    Integer(i64),
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<SqlValue> for KeyValue {
    fn from(value: SqlValue) -> Self {
        match value {
            SqlValue::Null => Self::Null,
            SqlValue::Integer(value) => Self::Integer(value),
            SqlValue::Real(value) => Self::Real(value.to_bits()),
            SqlValue::Text(value) => Self::Text(value),
            SqlValue::Blob(value) => Self::Blob(value),
        }
    }
}
