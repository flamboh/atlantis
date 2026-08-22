//! Portable, bounded exports of canonical pipeline statistics.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow_array::{
    ArrayRef, BinaryArray, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::{
    arrow::ArrowWriter, basic::Compression, errors::ParquetError,
    file::properties::WriterProperties,
};
use rusqlite::{Connection, OpenFlags, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    domain::FlowSelection,
    storage::{ProductIdentity, STATS_TABLE_NAMES, StorageError, backup_database},
};

pub const SQLITE_FILENAME: &str = "netflow.sqlite";
pub const MANIFEST_FILENAME: &str = "manifest.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractRequest {
    pub dataset_id: String,
    pub source_db: PathBuf,
    pub output_dir: PathBuf,
    /// Whether the caller explicitly selected this output directory.
    pub output_dir_explicit: bool,
    pub start_ts: i64,
    pub end_exclusive_ts: i64,
    pub start_input: String,
    pub end_exclusive_input: String,
    pub timezone: String,
    pub source_id: Option<String>,
    pub granularities: Option<Vec<String>>,
    pub write_sqlite: bool,
    pub write_parquet: bool,
    pub parquet_dir: Option<PathBuf>,
    pub batch_size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractResult {
    pub manifest_path: PathBuf,
    pub sqlite_path: Option<PathBuf>,
    pub parquet_dir: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("invalid extraction request: {0}")]
    InvalidRequest(String),
    #[error("source database is missing required table {0}")]
    MissingTable(String),
    #[error("source table {table} is missing required column {column}")]
    MissingColumn { table: String, column: String },
    #[error("source pipeline_product contract is invalid: {0}")]
    InvalidProduct(String),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("storage operation failed: {0}")]
    Storage(#[from] StorageError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Parquet operation failed: {0}")]
    Parquet(#[from] ParquetError),
    #[error("Arrow operation failed: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("time conversion failed: {0}")]
    Time(#[from] jiff::Error),
}

/// Create portable artifacts from one immutable snapshot of a canonical database.
pub fn extract_window(request: &ExtractRequest) -> Result<ExtractResult, ExportError> {
    let product = source_product_for_extract(request)?;

    fs::create_dir_all(&request.output_dir)?;
    let work = tempfile::Builder::new()
        .prefix(".extract-window-")
        .tempdir_in(&request.output_dir)?;
    let snapshot_path = work.path().join("source-snapshot.sqlite");
    backup_database(&request.source_db, &snapshot_path)?;
    let snapshot = open_readonly(&snapshot_path)?;
    validate_source(&snapshot)?;
    let snapshot_product = read_pipeline_product(&snapshot)?;
    if snapshot_product.product_fingerprint != product.product_fingerprint {
        return Err(ExportError::InvalidProduct(
            "pipeline product changed while its snapshot was created".into(),
        ));
    }
    require_explicit_selected_output(&snapshot_product, request.output_dir_explicit)?;

    let temporary_sqlite = request
        .write_sqlite
        .then(|| work.path().join(SQLITE_FILENAME));
    let sqlite_target = request
        .write_sqlite
        .then(|| request.output_dir.join(SQLITE_FILENAME));
    let destination = temporary_sqlite
        .as_ref()
        .map(Connection::open)
        .transpose()?;
    let parquet_target = request.write_parquet.then(|| {
        request
            .parquet_dir
            .clone()
            .unwrap_or_else(|| request.output_dir.join("parquet"))
    });
    let temporary_parquet = request.write_parquet.then(|| work.path().join("parquet"));
    if let Some(directory) = &temporary_parquet {
        fs::create_dir(directory)?;
    }
    let mut summaries = BTreeMap::new();
    for table in STATS_TABLE_NAMES {
        let mut summary = extract_sqlite_table(&snapshot, destination.as_ref(), table, request)?;
        if let (Some(temporary), Some(target)) = (&temporary_parquet, &parquet_target) {
            summary.parquet_row_count = Some(export_table_to_parquet(
                &snapshot,
                &temporary.join(format!("{table}.parquet")),
                table,
                request,
            )?);
            summary.parquet_path = Some(target.join(format!("{table}.parquet")));
        }
        summaries.insert(table.to_owned(), summary);
    }
    drop(destination);
    drop(snapshot);

    // Manifest publication is the commit marker for the complete extraction.
    let manifest_path = request.output_dir.join(MANIFEST_FILENAME);
    let temporary_manifest = work.path().join(MANIFEST_FILENAME);
    let manifest = build_manifest(
        request,
        sqlite_target.as_deref(),
        parquet_target.as_deref(),
        &product,
        summaries,
    )?;
    fs::write(
        &temporary_manifest,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    publish_artifacts(
        temporary_sqlite.as_deref(),
        sqlite_target.as_deref(),
        temporary_parquet.as_deref(),
        parquet_target.as_deref(),
        &temporary_manifest,
        &manifest_path,
        work.path(),
    )?;
    Ok(ExtractResult {
        manifest_path,
        sqlite_path: sqlite_target,
        parquet_dir: parquet_target,
    })
}

/// Validate a dry-run extraction plan without creating output files.
pub fn validate_extract_plan(request: &ExtractRequest) -> Result<(), ExportError> {
    source_product_for_extract(request).map(drop)
}

fn source_product_for_extract(request: &ExtractRequest) -> Result<PipelineProduct, ExportError> {
    validate_request(request)?;
    let source = open_readonly(&request.source_db)?;
    validate_source(&source)?;
    let product = read_pipeline_product(&source)?;
    require_explicit_selected_output(&product, request.output_dir_explicit)?;
    Ok(product)
}

fn require_explicit_selected_output(
    product: &PipelineProduct,
    output_dir_explicit: bool,
) -> Result<(), ExportError> {
    if !output_dir_explicit && product.selection.get("kind").and_then(Value::as_str) != Some("all")
    {
        return Err(ExportError::InvalidRequest(
            "selected pipeline products require an explicit output directory to prevent collisions with other database products".into(),
        ));
    }
    Ok(())
}

const COMMON_COLUMNS: &[&str] = &[
    "source_id",
    "granularity",
    "bucket_start",
    "bucket_end",
    "ip_version",
    "src_visibility",
    "dst_visibility",
    "processed_at",
];

fn validate_request(request: &ExtractRequest) -> Result<(), ExportError> {
    if !request.source_db.is_file() {
        return Err(ExportError::InvalidRequest(format!(
            "source database not found: {}",
            request.source_db.display()
        )));
    }
    if request.end_exclusive_ts <= request.start_ts {
        return Err(ExportError::InvalidRequest(
            "end_exclusive_ts must be after start_ts".into(),
        ));
    }
    for timestamp in [request.start_ts, request.end_exclusive_ts] {
        jiff::Timestamp::from_second(timestamp)?.in_tz(&request.timezone)?;
    }
    if request.batch_size == 0 {
        return Err(ExportError::InvalidRequest(
            "batch_size must be positive".into(),
        ));
    }
    if !request.write_sqlite && !request.write_parquet {
        return Err(ExportError::InvalidRequest(
            "at least one output must be enabled".into(),
        ));
    }
    let source_db = normalized_absolute(&request.source_db)?;
    let output_dir = normalized_absolute(&request.output_dir)?;
    let sqlite_output = output_dir.join(SQLITE_FILENAME);
    let mut managed = vec![sqlite_output.clone(), output_dir.join(MANIFEST_FILENAME)];
    managed.extend(
        ["-journal", "-wal", "-shm"]
            .map(|suffix| sqlite_output.with_file_name(format!("{SQLITE_FILENAME}{suffix}"))),
    );
    if managed.iter().any(|path| path == &source_db) {
        return Err(ExportError::InvalidRequest(
            "source database overlaps a managed output".into(),
        ));
    }
    if request.write_parquet {
        let parquet_dir = if let Some(path) = request.parquet_dir.as_deref() {
            normalized_absolute(path)?
        } else {
            output_dir.join("parquet")
        };
        if parquet_dir == source_db || source_db.starts_with(&parquet_dir) {
            return Err(ExportError::InvalidRequest(
                "Parquet directory overlaps the source database".into(),
            ));
        }
        if managed.iter().any(|path| {
            path == &parquet_dir || path.starts_with(&parquet_dir) || parquet_dir.starts_with(path)
        }) {
            return Err(ExportError::InvalidRequest(
                "Parquet directory overlaps a managed output".into(),
            ));
        }
    }
    if let Some(granularities) = &request.granularities {
        let supported = ["5m", "30m", "1h", "1d"];
        if granularities
            .iter()
            .any(|value| !supported.contains(&value.as_str()))
        {
            return Err(ExportError::InvalidRequest(
                "unsupported granularity filter".into(),
            ));
        }
    }
    Ok(())
}

fn normalized_absolute(path: &Path) -> Result<PathBuf, ExportError> {
    use std::path::Component;

    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            ExportError::InvalidRequest(format!("cannot resolve path {}", path.display()))
        })?;
        suffix.push(name.to_owned());
        existing = existing.parent().ok_or_else(|| {
            ExportError::InvalidRequest(format!("cannot resolve path {}", path.display()))
        })?;
    }
    let mut resolved = existing.canonicalize()?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn open_readonly(path: &Path) -> Result<Connection, rusqlite::Error> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

fn validate_source(connection: &Connection) -> Result<(), ExportError> {
    for table in STATS_TABLE_NAMES {
        let columns = table_columns(connection, table)?;
        for column in required_columns(table) {
            if !columns.contains(column) {
                return Err(ExportError::MissingColumn {
                    table: table.into(),
                    column: (*column).into(),
                });
            }
        }
    }
    Ok(())
}

fn required_columns(table: &str) -> Vec<&'static str> {
    if table == "bucket_coverage" {
        return vec![
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "coverage_state",
            "observed_units",
            "expected_units",
            "rejected_units",
        ];
    }
    let mut columns = COMMON_COLUMNS.to_vec();
    columns.extend(match table {
        "traffic_stats" => vec![
            "flows",
            "flows_tcp",
            "flows_udp",
            "flows_icmp",
            "flows_other",
            "packets",
            "packets_tcp",
            "packets_udp",
            "packets_icmp",
            "packets_other",
            "bytes",
            "bytes_tcp",
            "bytes_udp",
            "bytes_icmp",
            "bytes_other",
            "duration_sum_ms",
            "duration_count",
            "average_duration_ms",
            "min_ttl_sum",
            "min_ttl_count",
            "average_min_ttl",
            "max_ttl_sum",
            "max_ttl_count",
            "average_max_ttl",
        ],
        "protocol_stats" => vec!["unique_protocols_count", "protocols_list"],
        "address_count_stats" => vec!["address_side", "unique_address_count"],
        "port_count_stats" => vec!["port_side", "port_range", "unique_port_count"],
        "address_structure_stats" => vec![
            "address_side",
            "structure_kind",
            "values_json",
            "metadata_json",
        ],
        _ => Vec::new(),
    });
    columns
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>, ExportError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if columns.is_empty() {
        return Err(ExportError::MissingTable(table.into()));
    }
    Ok(columns)
}

#[derive(Clone, Debug, Serialize)]
struct TableSummary {
    source_row_count: usize,
    source_min_time: Option<i64>,
    source_max_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sqlite_row_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parquet_row_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parquet_path: Option<PathBuf>,
}

fn extract_sqlite_table(
    source: &Connection,
    destination: Option<&Connection>,
    table: &str,
    request: &ExtractRequest,
) -> Result<TableSummary, ExportError> {
    let (where_sql, parameters) = table_filter(request);
    let summary = source.query_row(
        &format!(
            "SELECT COUNT(*), MIN(bucket_start), MAX(bucket_start) FROM {} {where_sql}",
            quote(table)
        ),
        params_from_iter(parameters.iter()),
        |row| Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let copied = if let Some(destination) = destination {
        create_table_schema(source, destination, table)?;
        Some(copy_filtered_rows(
            source,
            destination,
            table,
            &where_sql,
            &parameters,
            request.batch_size,
        )?)
    } else {
        None
    };
    Ok(TableSummary {
        source_row_count: usize::try_from(summary.0).map_err(|_| {
            ExportError::InvalidRequest(format!("negative row count reported for {table}"))
        })?,
        source_min_time: summary.1,
        source_max_time: summary.2,
        sqlite_row_count: copied,
        parquet_row_count: None,
        parquet_path: None,
    })
}

fn table_filter(request: &ExtractRequest) -> (String, Vec<SqlValue>) {
    let mut clauses = vec![
        "bucket_start >= ?".to_owned(),
        "bucket_start < ?".to_owned(),
    ];
    let mut values = vec![
        SqlValue::Integer(request.start_ts),
        SqlValue::Integer(request.end_exclusive_ts),
    ];
    if let Some(source_id) = &request.source_id {
        clauses.push("source_id = ?".into());
        values.push(SqlValue::Text(source_id.clone()));
    }
    if let Some(granularities) = &request.granularities {
        clauses.push(format!(
            "granularity IN ({})",
            vec!["?"; granularities.len()].join(", ")
        ));
        values.extend(granularities.iter().cloned().map(SqlValue::Text));
    }
    (format!("WHERE {}", clauses.join(" AND ")), values)
}

fn create_table_schema(
    source: &Connection,
    destination: &Connection,
    table: &str,
) -> Result<(), ExportError> {
    let mut statement = source.prepare(
        "SELECT sql FROM sqlite_master WHERE tbl_name = ? AND type IN ('table', 'index') \
         AND sql IS NOT NULL ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name",
    )?;
    let schema = statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if schema.is_empty() {
        return Err(ExportError::MissingTable(table.into()));
    }
    for sql in schema {
        destination.execute_batch(&sql)?;
    }
    Ok(())
}

fn copy_filtered_rows(
    source: &Connection,
    destination: &Connection,
    table: &str,
    where_sql: &str,
    parameters: &[SqlValue],
    _batch_size: usize,
) -> Result<usize, ExportError> {
    let mut table_info = source.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
    let columns = table_info
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let column_sql = columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO {} ({column_sql}) VALUES ({})",
        quote(table),
        vec!["?"; columns.len()].join(", ")
    );
    let mut select = source.prepare(&format!("SELECT * FROM {} {where_sql}", quote(table)))?;
    let mut rows = select.query(params_from_iter(parameters.iter()))?;
    let transaction = destination.unchecked_transaction()?;
    let mut insert = transaction.prepare_cached(&insert_sql)?;
    let mut copied = 0;
    while let Some(row) = rows.next()? {
        let values = (0..columns.len())
            .map(|index| row.get::<_, SqlValue>(index))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        insert.execute(params_from_iter(values.iter()))?;
        copied += 1;
    }
    drop(insert);
    transaction.commit()?;
    Ok(copied)
}

fn export_table_to_parquet(
    source: &Connection,
    output_path: &Path,
    table: &str,
    request: &ExtractRequest,
) -> Result<usize, ExportError> {
    let columns = table_column_declarations(source, table)?;
    let schema: SchemaRef = Arc::new(Schema::new(
        columns
            .iter()
            .map(|(name, declaration)| Field::new(name, arrow_type(declaration), true))
            .collect::<Vec<_>>(),
    ));
    let file = fs::File::create(output_path)?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();
    let mut writer = ArrowWriter::try_new(file, Arc::clone(&schema), Some(properties))?;
    let (where_sql, parameters) = table_filter(request);
    let mut statement = source.prepare(&format!("SELECT * FROM {} {where_sql}", quote(table)))?;
    let mut rows = statement.query(params_from_iter(parameters.iter()))?;
    let mut written = 0;
    loop {
        let mut batch_rows = Vec::with_capacity(request.batch_size);
        while batch_rows.len() < request.batch_size {
            let Some(row) = rows.next()? else { break };
            batch_rows.push(
                (0..columns.len())
                    .map(|index| row.get::<_, SqlValue>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        if batch_rows.is_empty() {
            break;
        }
        let arrays = columns
            .iter()
            .enumerate()
            .map(|(index, (_, declaration))| {
                arrow_array(&batch_rows, index, &arrow_type(declaration))
            })
            .collect::<Result<Vec<_>, _>>()?;
        written += batch_rows.len();
        writer.write(&RecordBatch::try_new(Arc::clone(&schema), arrays)?)?;
    }
    writer.close()?;
    Ok(written)
}

fn table_column_declarations(
    source: &Connection,
    table: &str,
) -> Result<Vec<(String, String)>, ExportError> {
    let mut statement = source.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn arrow_type(declaration: &str) -> DataType {
    let declaration = declaration.to_ascii_uppercase();
    if declaration.contains("BOOL") {
        DataType::Boolean
    } else if declaration.contains("INT") {
        DataType::Int64
    } else if ["REAL", "FLOA", "DOUB", "NUM", "DEC"]
        .iter()
        .any(|token| declaration.contains(token))
    {
        DataType::Float64
    } else if declaration.contains("BLOB") {
        DataType::Binary
    } else {
        DataType::Utf8
    }
}

fn arrow_array(
    rows: &[Vec<SqlValue>],
    column: usize,
    data_type: &DataType,
) -> Result<ArrayRef, ExportError> {
    let array: ArrayRef = match data_type {
        DataType::Int64 => Arc::new(Int64Array::from(
            rows.iter()
                .map(|row| match &row[column] {
                    SqlValue::Null => Ok(None),
                    SqlValue::Integer(value) => Ok(Some(*value)),
                    value => Err(type_error(column, "integer", value)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        DataType::Float64 => Arc::new(Float64Array::from(
            rows.iter()
                .map(|row| match &row[column] {
                    SqlValue::Null => Ok(None),
                    SqlValue::Integer(value) => Ok(Some(*value as f64)),
                    SqlValue::Real(value) => Ok(Some(*value)),
                    value => Err(type_error(column, "real", value)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        DataType::Boolean => Arc::new(BooleanArray::from(
            rows.iter()
                .map(|row| match &row[column] {
                    SqlValue::Null => Ok(None),
                    SqlValue::Integer(value) => Ok(Some(*value != 0)),
                    value => Err(type_error(column, "boolean", value)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        DataType::Binary => Arc::new(BinaryArray::from(
            rows.iter()
                .map(|row| match &row[column] {
                    SqlValue::Null => Ok(None),
                    SqlValue::Blob(value) => Ok(Some(value.as_slice())),
                    value => Err(type_error(column, "blob", value)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        DataType::Utf8 => Arc::new(StringArray::from(
            rows.iter()
                .map(|row| match &row[column] {
                    SqlValue::Null => Ok(None),
                    SqlValue::Text(value) => Ok(Some(value.as_str())),
                    value => Err(type_error(column, "text", value)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        data_type => {
            return Err(ExportError::InvalidRequest(format!(
                "unsupported Arrow type {data_type:?}"
            )));
        }
    };
    Ok(array)
}

fn type_error(column: usize, expected: &str, actual: &SqlValue) -> ExportError {
    ExportError::InvalidRequest(format!(
        "column {column} declared {expected} contains incompatible SQLite value {actual:?}"
    ))
}

#[derive(Clone, Debug)]
struct PipelineProduct {
    product_fingerprint: String,
    schema_fingerprint: String,
    selection_fingerprint: String,
    selection: Value,
}

fn read_pipeline_product(connection: &Connection) -> Result<PipelineProduct, ExportError> {
    let columns = table_columns(connection, "pipeline_product")?;
    for column in [
        "singleton",
        "schema_json",
        "schema_fingerprint",
        "selection_json",
        "selection_fingerprint",
        "config_json",
        "config_fingerprint",
        "product_fingerprint",
    ] {
        if !columns.contains(column) {
            return Err(ExportError::MissingColumn {
                table: "pipeline_product".into(),
                column: column.into(),
            });
        }
    }
    let mut statement = connection.prepare(
        "SELECT singleton, schema_json, schema_fingerprint, selection_json, \
         selection_fingerprint, config_json, config_fingerprint, product_fingerprint \
         FROM pipeline_product",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() != 1 || rows[0].0 != 1 {
        return Err(ExportError::InvalidProduct(
            "expected exactly one singleton row".into(),
        ));
    }
    let row = &rows[0];
    let schema: Value = serde_json::from_str(&row.1)?;
    let raw_selection: Value = serde_json::from_str(&row.3)?;
    let selection = FlowSelection::from_payload(Some(&raw_selection))
        .map_err(|error| ExportError::InvalidProduct(error.to_string()))?
        .normalized_payload();
    let config: Value = serde_json::from_str(&row.5)?;
    let expected_schema = json!({"version": 2, "tables": [
        {"name": "traffic_stats", "version": 2},
        {"name": "protocol_stats", "version": 1},
        {"name": "address_count_stats", "version": 1},
        {"name": "port_count_stats", "version": 1},
        {"name": "address_structure_stats", "version": 1},
        {"name": "bucket_coverage", "version": 1}
    ]});
    if schema != expected_schema {
        return Err(ExportError::InvalidProduct(
            "unsupported observation-metrics schema".into(),
        ));
    }
    let expected = ProductIdentity::create(&schema, &selection, &config)?;
    let expected_values = [
        ("schema_json", expected.schema_json.as_str(), row.1.as_str()),
        (
            "schema_fingerprint",
            expected.schema_fingerprint.as_str(),
            row.2.as_str(),
        ),
        (
            "selection_json",
            expected.selection_json.as_str(),
            row.3.as_str(),
        ),
        (
            "selection_fingerprint",
            expected.selection_fingerprint.as_str(),
            row.4.as_str(),
        ),
        ("config_json", expected.config_json.as_str(), row.5.as_str()),
        (
            "config_fingerprint",
            expected.config_fingerprint.as_str(),
            row.6.as_str(),
        ),
        (
            "product_fingerprint",
            expected.fingerprint.as_str(),
            row.7.as_str(),
        ),
    ];
    let mismatches = expected_values
        .into_iter()
        .filter_map(|(name, expected, actual)| (expected != actual).then_some(name))
        .collect::<Vec<_>>();
    if !mismatches.is_empty() {
        return Err(ExportError::InvalidProduct(format!(
            "internally inconsistent fields: {}",
            mismatches.join(", ")
        )));
    }
    Ok(PipelineProduct {
        product_fingerprint: expected.fingerprint,
        schema_fingerprint: expected.schema_fingerprint,
        selection_fingerprint: expected.selection_fingerprint,
        selection,
    })
}

fn build_manifest(
    request: &ExtractRequest,
    sqlite_path: Option<&Path>,
    parquet_dir: Option<&Path>,
    product: &PipelineProduct,
    tables: BTreeMap<String, TableSummary>,
) -> Result<Value, ExportError> {
    let granularities = request.granularities.clone();
    let generated_at = jiff::Timestamp::now().to_string();
    let start = jiff::Timestamp::from_second(request.start_ts)?
        .in_tz(&request.timezone)?
        .to_string();
    let end_exclusive = jiff::Timestamp::from_second(request.end_exclusive_ts)?
        .in_tz(&request.timezone)?
        .to_string();
    Ok(json!({
        "dataset_id": request.dataset_id,
        "source_db": request.source_db,
        "output_dir": request.output_dir,
        "sqlite_path": sqlite_path,
        "parquet_dir": parquet_dir,
        "start": start,
        "start_input": request.start_input,
        "start_ts": request.start_ts,
        "end_exclusive": end_exclusive,
        "end_exclusive_input": request.end_exclusive_input,
        "end_exclusive_ts": request.end_exclusive_ts,
        "timezone": request.timezone,
        "source_id_filter": request.source_id,
        "granularity_filter": granularities,
        "generated_at": generated_at,
        "pipeline_product": {
            "product_fingerprint": product.product_fingerprint,
            "schema_fingerprint": product.schema_fingerprint,
            "selection_fingerprint": product.selection_fingerprint,
            "selection": product.selection,
        },
        "filters": {
            "source_id": request.source_id,
            "granularities": granularities,
        },
        "window": {
            "start": start,
            "start_input": request.start_input,
            "end_exclusive": end_exclusive,
            "end_exclusive_input": request.end_exclusive_input,
            "start_ts": request.start_ts,
            "end_exclusive_ts": request.end_exclusive_ts,
            "timezone": request.timezone,
        },
        "outputs": {
            "source_db": request.source_db,
            "output_dir": request.output_dir,
            "sqlite_path": sqlite_path,
            "parquet_dir": parquet_dir,
        },
        "tables": tables,
    }))
}

fn publish_artifacts(
    temporary_sqlite: Option<&Path>,
    sqlite_target: Option<&Path>,
    temporary_parquet: Option<&Path>,
    parquet_target: Option<&Path>,
    temporary_manifest: &Path,
    manifest_target: &Path,
    work_dir: &Path,
) -> Result<(), ExportError> {
    let mut installed = Vec::new();
    let mut staging = Vec::new();
    let result = (|| {
        for (source, target, is_sqlite) in [
            (temporary_sqlite, sqlite_target, true),
            (temporary_parquet, parquet_target, false),
            (Some(temporary_manifest), Some(manifest_target), false),
        ] {
            if let (Some(source), Some(target)) = (source, target) {
                let publish_source = prepare_publish_source(source, target, work_dir)?;
                staging.push(publish_source.clone());
                let backups = backup_target(target, work_dir, is_sqlite)?;
                if let Err(error) = fs::rename(&publish_source, target) {
                    restore_backups(&backups)?;
                    return Err(error.into());
                }
                installed.push((target.to_owned(), backups));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        for (target, backups) in installed.iter().rev() {
            remove_path(target)?;
            restore_backups(backups)?;
        }
    } else {
        for (_, backups) in &installed {
            for (_, backup) in backups {
                remove_path(backup)?;
            }
        }
    }
    for path in staging {
        remove_path(&path)?;
    }
    result
}

fn prepare_publish_source(
    source: &Path,
    target: &Path,
    work_dir: &Path,
) -> Result<PathBuf, ExportError> {
    let parent = target.parent().ok_or_else(|| {
        ExportError::InvalidRequest(format!("output target has no parent: {}", target.display()))
    })?;
    fs::create_dir_all(parent)?;
    let target_name = target.file_name().ok_or_else(|| {
        ExportError::InvalidRequest(format!(
            "output target has no filename: {}",
            target.display()
        ))
    })?;
    let nonce = work_dir.file_name().unwrap_or_default().to_string_lossy();
    let staging = parent.join(format!(
        ".incoming-{}-{nonce}",
        target_name.to_string_lossy()
    ));
    remove_path(&staging)?;
    copy_path(source, &staging)?;
    Ok(staging)
}

fn copy_path(source: &Path, target: &Path) -> Result<(), ExportError> {
    if source.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, target)?;
        fs::File::open(target)?.sync_all()?;
    }
    Ok(())
}

fn backup_target(
    target: &Path,
    work_dir: &Path,
    include_sqlite_sidecars: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, ExportError> {
    let parent = target.parent().ok_or_else(|| {
        ExportError::InvalidRequest(format!("output target has no parent: {}", target.display()))
    })?;
    fs::create_dir_all(parent)?;
    let nonce = work_dir.file_name().unwrap_or_default().to_string_lossy();
    let mut candidates = vec![target.to_owned()];
    if include_sqlite_sidecars {
        candidates.extend(["-journal", "-wal", "-shm"].map(|suffix| {
            target.with_file_name(format!(
                "{}{suffix}",
                target.file_name().unwrap_or_default().to_string_lossy()
            ))
        }));
    }
    let mut backups = Vec::new();
    for original in candidates.into_iter().filter(|path| path.exists()) {
        let name = original.file_name().ok_or_else(|| {
            ExportError::InvalidRequest(format!(
                "output target has no filename: {}",
                original.display()
            ))
        })?;
        let backup = parent.join(format!(".previous-{}-{nonce}", name.to_string_lossy()));
        remove_path(&backup)?;
        if let Err(error) = fs::rename(&original, &backup) {
            restore_backups(&backups)?;
            return Err(error.into());
        }
        backups.push((original, backup));
    }
    Ok(backups)
}

fn restore_backups(backups: &[(PathBuf, PathBuf)]) -> Result<(), ExportError> {
    for (original, backup) in backups.iter().rev() {
        fs::rename(backup, original)?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<(), std::io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::*;
    use crate::storage::{ProductIdentity, bind_product_identity, init_stats_tables};

    #[test]
    fn sqlite_extract_copies_only_the_requested_half_open_window() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let source_connection = Connection::open(&source).unwrap();
        init_stats_tables(&source_connection).unwrap();
        let product = ProductIdentity::create(
            &serde_json::json!({"version": 2, "tables": [
                {"name": "traffic_stats", "version": 2},
                {"name": "protocol_stats", "version": 1},
                {"name": "address_count_stats", "version": 1},
                {"name": "port_count_stats", "version": 1},
                {"name": "address_structure_stats", "version": 1},
                {"name": "bucket_coverage", "version": 1}
            ]}),
            &serde_json::json!({"version": 1, "kind": "all"}),
            &serde_json::json!({"version": 2}),
        )
        .unwrap();
        bind_product_identity(
            &source_connection,
            &product,
            &crate::storage::STATS_TABLE_NAMES,
        )
        .unwrap();
        for (bucket_start, flows) in [(99, 1), (100, 2), (199, 3), (200, 4)] {
            source_connection
                .execute(
                    "INSERT INTO traffic_stats (
                    source_id, granularity, bucket_start, bucket_end, ip_version,
                    src_visibility, dst_visibility, flows, flows_tcp, flows_udp,
                    flows_icmp, flows_other, packets, packets_tcp, packets_udp,
                    packets_icmp, packets_other, bytes, bytes_tcp, bytes_udp,
                    bytes_icmp, bytes_other, duration_sum_ms, duration_count,
                    average_duration_ms, min_ttl_sum, min_ttl_count, average_min_ttl,
                    max_ttl_sum, max_ttl_count, average_max_ttl
                 ) VALUES (
                    'r1', '5m', ?1, ?1 + 300, 4, 'all', 'all', ?2, ?2, 0, 0, 0,
                    ?2, ?2, 0, 0, 0, ?2, ?2, 0, 0, 0, 0, 0, NULL, 0, 0, NULL,
                    0, 0, NULL
                 )",
                    params![bucket_start, flows],
                )
                .unwrap();
            source_connection
                .execute(
                    "INSERT INTO bucket_coverage (
                        source_id, granularity, bucket_start, bucket_end,
                        coverage_state, observed_units, expected_units, rejected_units
                     ) VALUES ('r1', '5m', ?1, ?1 + 300, 'complete', 1, 1, 0)",
                    [bucket_start],
                )
                .unwrap();
        }
        drop(source_connection);

        let request = ExtractRequest {
            dataset_id: "fixture".into(),
            source_db: source,
            output_dir: directory.path().join("extract"),
            output_dir_explicit: true,
            start_ts: 100,
            end_exclusive_ts: 200,
            start_input: "100".into(),
            end_exclusive_input: "200".into(),
            timezone: "UTC".into(),
            source_id: Some("r1".into()),
            granularities: Some(vec!["5m".into()]),
            write_sqlite: true,
            write_parquet: false,
            parquet_dir: None,
            batch_size: 1,
        };

        fs::create_dir_all(&request.output_dir).unwrap();
        let stale_target = request.output_dir.join(SQLITE_FILENAME);
        fs::write(&stale_target, b"old database").unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            fs::write(
                stale_target.with_file_name(format!("{SQLITE_FILENAME}{suffix}")),
                b"stale",
            )
            .unwrap();
        }

        let result = extract_window(&request).unwrap();
        let output = Connection::open(result.sqlite_path.unwrap()).unwrap();
        let rows = output
            .prepare("SELECT bucket_start, flows FROM traffic_stats ORDER BY bucket_start")
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, [(100, 2), (199, 3)]);
        let coverage = output
            .prepare(
                "SELECT bucket_start, coverage_state, observed_units, expected_units, rejected_units
                 FROM bucket_coverage ORDER BY bucket_start",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            coverage,
            vec![
                (100, "complete".into(), 1, 1, 0),
                (199, "complete".into(), 1, 1, 0)
            ]
        );
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(
                !stale_target
                    .with_file_name(format!("{SQLITE_FILENAME}{suffix}"))
                    .exists()
            );
        }
    }

    #[test]
    fn selected_products_require_an_explicit_output_directory() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("selected.sqlite");
        let connection = Connection::open(&source).unwrap();
        init_stats_tables(&connection).unwrap();
        let product = ProductIdentity::create(
            &serde_json::json!({"version": 2, "tables": [
                {"name": "traffic_stats", "version": 2},
                {"name": "protocol_stats", "version": 1},
                {"name": "address_count_stats", "version": 1},
                {"name": "port_count_stats", "version": 1},
                {"name": "address_structure_stats", "version": 1},
                {"name": "bucket_coverage", "version": 1}
            ]}),
            &FlowSelection::from_payload(Some(&serde_json::json!({
                "version":1,
                "kind":"flows",
                "ip_prefix":"192.0.2.0/24"
            })))
            .unwrap()
            .normalized_payload(),
            &serde_json::json!({"version": 2}),
        )
        .unwrap();
        bind_product_identity(&connection, &product, &crate::storage::STATS_TABLE_NAMES).unwrap();
        drop(connection);
        let output_dir = directory.path().join("implicit-output");
        let request = ExtractRequest {
            dataset_id: "fixture".into(),
            source_db: source,
            output_dir: output_dir.clone(),
            output_dir_explicit: false,
            start_ts: 100,
            end_exclusive_ts: 200,
            start_input: "100".into(),
            end_exclusive_input: "200".into(),
            timezone: "UTC".into(),
            source_id: None,
            granularities: None,
            write_sqlite: true,
            write_parquet: false,
            parquet_dir: None,
            batch_size: 1,
        };

        let error = extract_window(&request).unwrap_err();

        assert!(
            error.to_string().contains("explicit output directory"),
            "{error}"
        );
        assert!(!output_dir.exists());
    }

    #[test]
    fn source_cannot_alias_an_output_sqlite_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let output_dir = directory.path().join("extract");
        fs::create_dir(&output_dir).unwrap();
        let source = output_dir.join(format!("{SQLITE_FILENAME}-wal"));
        fs::write(&source, b"source bytes").unwrap();
        let original = fs::read(&source).unwrap();
        let request = ExtractRequest {
            dataset_id: "fixture".into(),
            source_db: source.clone(),
            output_dir,
            output_dir_explicit: true,
            start_ts: 100,
            end_exclusive_ts: 200,
            start_input: "100".into(),
            end_exclusive_input: "200".into(),
            timezone: "UTC".into(),
            source_id: None,
            granularities: None,
            write_sqlite: true,
            write_parquet: false,
            parquet_dir: None,
            batch_size: 1,
        };

        let error = extract_window(&request).unwrap_err();

        assert!(error.to_string().contains("managed output"));
        assert_eq!(fs::read(source).unwrap(), original);
    }
}
