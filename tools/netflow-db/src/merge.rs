use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, backup::Backup, params};
use thiserror::Error;

use crate::{
    provenance::canonical_json,
    storage::{
        DatabaseOperationLock, StorageError, atomic_replace_sqlite, canonical_path,
        connect_local_writer, connect_readonly, database_related_paths,
        optimize_all_query_planner_statistics, validate_database_path_separation,
    },
};

pub const MAX_SHARDS: usize = 11;

const DAY_OWNED_TABLES: [&str; 8] = [
    "traffic_stats",
    "protocol_stats",
    "address_count_stats",
    "port_count_stats",
    "address_structure_stats",
    "bucket_coverage",
    "input_evidence",
    "processed_inputs",
];

const SHARED_TABLES: [&str; 5] = [
    "pipeline_product",
    "nfcapd_source_layout",
    "datasets",
    "source_members",
    "processed_input_scans",
];

const MARKER_TABLE: &str = "daily_product_completion";

#[derive(Debug, Error)]
pub enum MergeError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Provenance(#[from] crate::provenance::ProvenanceError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot merge shards: {0}")]
    Refused(String),
}

#[derive(Clone, Debug)]
pub struct MergeRequest {
    pub output: PathBuf,
    pub shards: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct MergeReport {
    pub shards: usize,
    pub completed_days: usize,
    pub first_day_start: Option<i64>,
    pub last_day_end: Option<i64>,
    pub table_rows: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProductRow {
    schema_json: String,
    selection_json: String,
    config_json: String,
    product_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DatasetRow {
    id: String,
    label: String,
    source_mode: String,
    discovery_mode: String,
    sort_order: i64,
}

#[derive(Debug)]
struct ShardSummary {
    path: PathBuf,
    schema: Vec<(String, String, Option<String>)>,
    product: ProductRow,
    layout_fingerprint: String,
    datasets: BTreeSet<DatasetRow>,
    default_start_dates: BTreeMap<String, String>,
    source_members: BTreeSet<(String, String, String)>,
    run_maad: BTreeSet<i64>,
    days: Vec<(i64, i64)>,
    table_rows: BTreeMap<String, i64>,
}

pub fn merge_shards(request: &MergeRequest) -> Result<MergeReport, MergeError> {
    if request.shards.len() < 2 {
        return Err(refused("at least two shard databases are required"));
    }
    if request.shards.len() > MAX_SHARDS {
        return Err(refused(format!(
            "{} shards exceed the limit of {MAX_SHARDS} per merge; merge subsets first, since a merged product is itself a valid shard",
            request.shards.len()
        )));
    }
    let output = canonical_path(&request.output)?;
    let shards = request
        .shards
        .iter()
        .map(canonical_path)
        .collect::<Result<Vec<_>, _>>()?;
    let mut all_paths = shards.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    all_paths.push(output.as_path());
    validate_database_path_separation(&all_paths)?;
    for shard in &shards {
        if !shard.is_file() {
            return Err(StorageError::DatabaseNotFound(shard.clone()).into());
        }
    }
    reject_existing_output(&output)?;

    let mut locks = Vec::with_capacity(shards.len() + 1);
    for path in shards.iter().chain([&output]) {
        locks.push(DatabaseOperationLock::acquire(path, "merge-shards")?);
    }
    reject_existing_output(&output)?;

    let summaries = shards
        .iter()
        .map(|path| summarize_shard(path))
        .collect::<Result<Vec<_>, _>>()?;
    validate_compatible(&summaries)?;
    let days = validate_disjoint_days(&summaries)?;
    let default_start_dates = merged_default_start_dates(&summaries);

    let parent = output
        .parent()
        .ok_or_else(|| refused(format!("output path has no parent: {}", output.display())))?;
    std::fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(&format!(
            ".{}.",
            output.file_name().unwrap_or_default().to_string_lossy()
        ))
        .suffix(".merge.tmp")
        .tempfile_in(parent)?;
    let (_, temporary_path) = temporary.keep().map_err(|error| error.error)?;
    let result =
        write_merged(&temporary_path, &summaries, &default_start_dates).and_then(|table_rows| {
            reject_existing_output(&output)?;
            atomic_replace_sqlite(&temporary_path, &output)?;
            Ok(table_rows)
        });
    let table_rows = match result {
        Ok(table_rows) => table_rows,
        Err(error) => {
            for path in database_related_paths(&temporary_path)? {
                if path.extension().is_none_or(|extension| extension != "lock") {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(error);
        }
    };
    drop(locks);
    Ok(MergeReport {
        shards: summaries.len(),
        completed_days: days.len(),
        first_day_start: days.first().map(|day| day.0),
        last_day_end: days.last().map(|day| day.1),
        table_rows,
    })
}

fn refused(message: impl Into<String>) -> MergeError {
    MergeError::Refused(message.into())
}

fn reject_existing_output(output: &Path) -> Result<(), MergeError> {
    if output.exists() {
        return Err(refused(format!(
            "output {} already exists; merge into a new product database",
            output.display()
        )));
    }
    Ok(())
}

fn summarize_shard(path: &Path) -> Result<ShardSummary, MergeError> {
    let connection = connect_readonly(path)?;
    let label = path.display().to_string();
    let schema = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()?;
    let tables = schema
        .iter()
        .filter(|(kind, _, _)| kind == "table")
        .map(|(_, name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    let expected = DAY_OWNED_TABLES
        .iter()
        .chain(&SHARED_TABLES)
        .chain(&[MARKER_TABLE])
        .copied()
        .collect::<BTreeSet<_>>();
    if tables != expected {
        let missing = expected.difference(&tables).copied().collect::<Vec<_>>();
        let unknown = tables.difference(&expected).copied().collect::<Vec<_>>();
        return Err(refused(format!(
            "{label} is not an nfcapd-tree pipeline product (missing tables: [{}], unknown tables: [{}])",
            missing.join(", "),
            unknown.join(", ")
        )));
    }

    let product = connection
        .query_row(
            "SELECT schema_json, selection_json, config_json, product_fingerprint
             FROM pipeline_product WHERE singleton = 1",
            [],
            |row| {
                Ok(ProductRow {
                    schema_json: row.get(0)?,
                    selection_json: row.get(1)?,
                    config_json: row.get(2)?,
                    product_fingerprint: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| refused(format!("{label} has no bound product identity")))?;
    let expected_schema = canonical_json(&crate::pipeline::product_schema())?;
    if product.schema_json != expected_schema {
        return Err(refused(format!(
            "{label} records table contract {} but this build writes {expected_schema}",
            product.schema_json
        )));
    }
    let layout_fingerprint = connection
        .query_row(
            "SELECT layout_fingerprint FROM nfcapd_source_layout WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| refused(format!("{label} has no bound nfcapd source layout")))?;

    let mut datasets = BTreeSet::new();
    let mut default_start_dates = BTreeMap::new();
    let mut statement = connection.prepare(
        "SELECT id, label, default_start_date, source_mode, discovery_mode, sort_order FROM datasets",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        default_start_dates.insert(id.clone(), row.get(2)?);
        datasets.insert(DatasetRow {
            id,
            label: row.get(1)?,
            source_mode: row.get(3)?,
            discovery_mode: row.get(4)?,
            sort_order: row.get(5)?,
        });
    }
    drop(rows);
    drop(statement);
    let source_members = connection
        .prepare("SELECT dataset_id, source_id, member_id FROM source_members")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;

    let csv_scans: i64 =
        connection.query_row("SELECT COUNT(*) FROM processed_input_scans", [], |row| {
            row.get(0)
        })?;
    let csv_inputs: i64 = connection.query_row(
        "SELECT COUNT(*) FROM processed_inputs WHERE input_kind != 'nfcapd'",
        [],
        |row| row.get(0),
    )?;
    if csv_scans != 0 || csv_inputs != 0 {
        return Err(refused(format!(
            "{label} contains CSV inputs; only nfcapd-tree products carry daily completion markers"
        )));
    }

    let mut run_maad = BTreeSet::new();
    let mut days = BTreeSet::new();
    let mut statement = connection.prepare(
        "SELECT source_id, day_start, day_end, product_fingerprint, run_maad
         FROM daily_product_completion",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let source_id: String = row.get(0)?;
        let fingerprint: String = row.get(3)?;
        if fingerprint != product.product_fingerprint {
            return Err(refused(format!(
                "{label} has a completion marker for source {source_id:?} from a different product identity"
            )));
        }
        run_maad.insert(row.get::<_, i64>(4)?);
        days.insert((row.get::<_, i64>(1)?, row.get::<_, i64>(2)?));
    }
    drop(rows);
    drop(statement);
    let days = days.into_iter().collect::<Vec<_>>();
    if let Some(pair) = days.windows(2).find(|pair| pair[0].1 > pair[1].0) {
        return Err(refused(format!(
            "{label} has completion markers with inconsistent day bounds [{}, {}) and [{}, {})",
            pair[0].0, pair[0].1, pair[1].0, pair[1].1
        )));
    }

    let mut table_rows = BTreeMap::new();
    for table in DAY_OWNED_TABLES {
        let (rows, outside) = scan_day_ownership(&connection, table, &days)?;
        if let Some(bucket_start) = outside {
            return Err(refused(format!(
                "{label} has {table} rows at bucket_start {bucket_start} outside its completed days"
            )));
        }
        table_rows.insert(table.to_owned(), rows);
    }
    let markers: i64 =
        connection.query_row(&format!("SELECT COUNT(*) FROM {MARKER_TABLE}"), [], |row| {
            row.get(0)
        })?;
    table_rows.insert(MARKER_TABLE.to_owned(), markers);

    Ok(ShardSummary {
        path: path.to_owned(),
        schema,
        product,
        layout_fingerprint,
        datasets,
        default_start_dates,
        source_members,
        run_maad,
        days,
        table_rows,
    })
}

fn scan_day_ownership(
    connection: &Connection,
    table: &str,
    days: &[(i64, i64)],
) -> Result<(i64, Option<i64>), MergeError> {
    let mut statement = connection.prepare(&format!("SELECT bucket_start FROM {table}"))?;
    let mut rows = statement.query([])?;
    let mut count = 0_i64;
    while let Some(row) = rows.next()? {
        let bucket_start: i64 = row.get(0)?;
        count += 1;
        let index = days.partition_point(|day| day.0 <= bucket_start);
        if index == 0 || bucket_start >= days[index - 1].1 {
            return Ok((count, Some(bucket_start)));
        }
    }
    Ok((count, None))
}

fn validate_compatible(summaries: &[ShardSummary]) -> Result<(), MergeError> {
    let first = &summaries[0];
    let first_label = first.path.display();
    for shard in &summaries[1..] {
        let label = shard.path.display();
        if shard.schema != first.schema {
            return Err(refused(format!(
                "SQLite schema of {label} differs from {first_label}"
            )));
        }
        if shard.product != first.product {
            let mut components = Vec::new();
            for (name, left, right) in [
                (
                    "schema",
                    &first.product.schema_json,
                    &shard.product.schema_json,
                ),
                (
                    "selection",
                    &first.product.selection_json,
                    &shard.product.selection_json,
                ),
                (
                    "config",
                    &first.product.config_json,
                    &shard.product.config_json,
                ),
            ] {
                if left != right {
                    components.push(format!("{name}: {first_label}={left} {label}={right}"));
                }
            }
            if components.is_empty() {
                components.push("product fingerprint".to_owned());
            }
            return Err(refused(format!(
                "product identity of {label} differs from {first_label}: {}",
                components.join("; ")
            )));
        }
        if shard.layout_fingerprint != first.layout_fingerprint {
            return Err(refused(format!(
                "nfcapd source layout of {label} differs from {first_label}"
            )));
        }
        if shard.datasets != first.datasets || shard.source_members != first.source_members {
            return Err(refused(format!(
                "dataset metadata of {label} differs from {first_label}"
            )));
        }
    }
    let run_maad = summaries
        .iter()
        .flat_map(|shard| shard.run_maad.iter().copied())
        .collect::<BTreeSet<_>>();
    if run_maad.len() > 1 {
        return Err(refused(
            "completion markers disagree on whether MAAD was computed",
        ));
    }
    Ok(())
}

fn validate_disjoint_days(summaries: &[ShardSummary]) -> Result<Vec<(i64, i64)>, MergeError> {
    let mut owned = summaries
        .iter()
        .enumerate()
        .flat_map(|(index, shard)| shard.days.iter().map(move |day| (*day, index)))
        .collect::<Vec<_>>();
    owned.sort_unstable();
    for pair in owned.windows(2) {
        let ((left, left_shard), (right, right_shard)) = (pair[0], pair[1]);
        if left.1 > right.0 {
            return Err(refused(format!(
                "completed days overlap: [{}, {}) in {} and [{}, {}) in {}",
                left.0,
                left.1,
                summaries[left_shard].path.display(),
                right.0,
                right.1,
                summaries[right_shard].path.display()
            )));
        }
    }
    Ok(owned.into_iter().map(|(day, _)| day).collect())
}

fn merged_default_start_dates(summaries: &[ShardSummary]) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::<String, String>::new();
    for shard in summaries {
        for (id, date) in &shard.default_start_dates {
            merged
                .entry(id.clone())
                .and_modify(|current| {
                    if date < current {
                        current.clone_from(date);
                    }
                })
                .or_insert_with(|| date.clone());
        }
    }
    merged
}

fn write_merged(
    path: &Path,
    summaries: &[ShardSummary],
    default_start_dates: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, i64>, MergeError> {
    {
        let base = connect_readonly(&summaries[0].path)?;
        let mut target = connect_local_writer(path)?;
        let backup = Backup::new(&base, &mut target)?;
        backup.run_to_completion(1_024, Duration::ZERO, None)?;
    }
    let mut connection = connect_local_writer(path)?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "cache_size", -262_144)?;
    for (index, shard) in summaries.iter().enumerate().skip(1) {
        connection.execute(
            "ATTACH DATABASE ?1 AS ?2",
            params![shard.path.to_string_lossy(), format!("shard{index}")],
        )?;
    }
    let tables = DAY_OWNED_TABLES
        .iter()
        .chain(&[MARKER_TABLE])
        .copied()
        .collect::<Vec<_>>();
    let transaction = connection.transaction()?;
    for (index, shard) in summaries.iter().enumerate().skip(1) {
        for table in &tables {
            let inserted = transaction.execute(
                &format!("INSERT INTO main.{table} SELECT * FROM shard{index}.{table}"),
                [],
            )?;
            let expected = shard.table_rows[*table];
            if i64::try_from(inserted).ok() != Some(expected) {
                return Err(refused(format!(
                    "{table} copied {inserted} rows from {} but validation counted {expected}",
                    shard.path.display()
                )));
            }
        }
    }
    for (id, date) in default_start_dates {
        transaction.execute(
            "UPDATE main.datasets SET default_start_date = ?2 WHERE id = ?1",
            params![id, date],
        )?;
    }
    transaction.commit()?;
    for index in 1..summaries.len() {
        connection.execute("DETACH DATABASE ?1", [format!("shard{index}")])?;
    }

    let mut table_rows = BTreeMap::new();
    for table in &tables {
        let expected = summaries
            .iter()
            .map(|shard| shard.table_rows[*table])
            .sum::<i64>();
        let actual: i64 =
            connection.query_row(&format!("SELECT COUNT(*) FROM main.{table}"), [], |row| {
                row.get(0)
            })?;
        if actual != expected {
            return Err(refused(format!(
                "merged {table} has {actual} rows but the shards hold {expected}"
            )));
        }
        table_rows.insert((*table).to_owned(), actual);
    }
    optimize_all_query_planner_statistics(&connection)?;
    let quick_check =
        connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    if quick_check != "ok" {
        return Err(refused(format!(
            "merged quick_check failed: {quick_check:?}"
        )));
    }
    Ok(table_rows)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use rusqlite::{Connection, types::Value as SqlValue};
    use serde_json::{Value, json};
    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::{
        pipeline::{
            PipelineRequest, run,
            test_support::{write_fake_nfdump, write_nfcapd_day},
        },
        verify::{VerifyOptions, verify_database},
    };

    const TIMESTAMP_COLUMNS: [&str; 4] =
        ["processed_at", "discovered_at", "completed_at", "bound_at"];

    struct Fixture {
        directory: TempDir,
        registry: PathBuf,
        executable: PathBuf,
        invocations: PathBuf,
    }

    impl Fixture {
        fn new(days: &[&str]) -> Self {
            let directory = tempdir().unwrap();
            let root = directory.path().join("captures");
            for day in days {
                write_nfcapd_day(&root, "edge", day);
            }
            let executable = directory.path().join("fake-nfdump");
            let invocations = directory.path().join("invocations");
            write_fake_nfdump(&executable, &invocations);
            let registry = directory.path().join("datasets.json");
            fs::write(
                &registry,
                serde_json::to_vec(&json!([{
                    "dataset_id": "edge",
                    "root_path": root,
                    "db_path": directory.path().join("unused.sqlite"),
                    "source_ids": ["edge"],
                }]))
                .unwrap(),
            )
            .unwrap();
            Self {
                directory,
                registry,
                executable,
                invocations,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().join(name)
        }

        fn run(&self, database: &Path, start: &str, end: &str, run_maad: bool) -> usize {
            run(PipelineRequest {
                config_path: None,
                dataset_id: Some("edge".into()),
                datasets_path: Some(self.registry.clone()),
                start_date: Some(start.into()),
                end_date: Some(end.into()),
                start_time: None,
                end_time: None,
                database_path: Some(database.to_owned()),
                selection: Value::Null,
                nfdump: self.executable.to_string_lossy().into_owned(),
                force: false,
                run_maad,
                require_complete: false,
            })
            .unwrap()
            .five_minute_buckets
        }

        fn invocation_count(&self) -> usize {
            fs::read_to_string(&self.invocations)
                .map(|log| log.lines().count())
                .unwrap_or(0)
        }

        fn merge(&self, output: &Path, shards: &[&Path]) -> Result<MergeReport, MergeError> {
            merge_shards(&MergeRequest {
                output: output.to_owned(),
                shards: shards.iter().map(|shard| (*shard).to_owned()).collect(),
            })
        }
    }

    fn table_contents(database: &Path, table: &str) -> Vec<Vec<SqlValue>> {
        let connection = Connection::open(database).unwrap();
        let columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .filter(|column| !TIMESTAMP_COLUMNS.contains(&column.as_str()))
            .collect::<Vec<_>>();
        let list = columns.join(", ");
        let mut statement = connection
            .prepare(&format!("SELECT {list} FROM {table} ORDER BY {list}"))
            .unwrap();
        statement
            .query_map([], |row| {
                (0..columns.len())
                    .map(|index| row.get::<_, SqlValue>(index))
                    .collect()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn merged_shards_match_a_single_run_and_resume_as_a_no_op() {
        let fixture = Fixture::new(&["2025-06-01", "2025-06-02", "2025-06-03"]);
        let first = fixture.path("first.sqlite");
        let second = fixture.path("second.sqlite");
        let single = fixture.path("single.sqlite");
        let merged = fixture.path("merged.sqlite");
        assert_eq!(fixture.run(&first, "2025-06-01", "2025-06-01", true), 288);
        assert_eq!(fixture.run(&second, "2025-06-02", "2025-06-03", true), 576);
        assert_eq!(fixture.run(&single, "2025-06-01", "2025-06-03", true), 864);

        let report = fixture.merge(&merged, &[&second, &first]).unwrap();
        assert_eq!(report.shards, 2);
        assert_eq!(report.completed_days, 3);

        for table in DAY_OWNED_TABLES
            .iter()
            .chain(&SHARED_TABLES)
            .chain(&[MARKER_TABLE])
        {
            assert_eq!(
                table_contents(&merged, table),
                table_contents(&single, table),
                "{table} differs from the single-process product"
            );
        }
        verify_database(
            &merged,
            &VerifyOptions {
                source_id: None,
                dataset_id: Some("edge".into()),
                require_data: true,
                require_maad_data: true,
                require_processed: true,
                require_rollup_parity: true,
                require_no_raw_ip: true,
            },
        )
        .unwrap();

        let invocations = fixture.invocation_count();
        assert_eq!(fixture.run(&merged, "2025-06-01", "2025-06-03", true), 0);
        assert_eq!(fixture.invocation_count(), invocations);
        assert_eq!(
            table_contents(&merged, "traffic_stats"),
            table_contents(&single, "traffic_stats")
        );
    }

    #[test]
    fn mismatched_product_identity_is_refused_before_writing() {
        let fixture = Fixture::new(&["2025-06-01", "2025-06-02"]);
        let first = fixture.path("first.sqlite");
        let second = fixture.path("second.sqlite");
        let merged = fixture.path("merged.sqlite");
        fixture.run(&first, "2025-06-01", "2025-06-01", true);
        fixture.run(&second, "2025-06-02", "2025-06-02", false);

        let error = fixture.merge(&merged, &[&first, &second]).unwrap_err();
        assert!(
            matches!(&error, MergeError::Refused(message) if message.contains("product identity") && message.contains("config")),
            "{error}"
        );
        assert!(!merged.exists());
    }

    #[test]
    fn overlapping_days_are_refused_before_writing() {
        let fixture = Fixture::new(&["2025-06-01", "2025-06-02", "2025-06-03"]);
        let first = fixture.path("first.sqlite");
        let second = fixture.path("second.sqlite");
        let merged = fixture.path("merged.sqlite");
        fixture.run(&first, "2025-06-01", "2025-06-02", false);
        fixture.run(&second, "2025-06-02", "2025-06-03", false);

        let error = fixture.merge(&merged, &[&first, &second]).unwrap_err();
        assert!(
            matches!(&error, MergeError::Refused(message) if message.contains("overlap")),
            "{error}"
        );
        assert!(!merged.exists());
    }

    #[test]
    fn rows_outside_completed_days_are_refused() {
        let fixture = Fixture::new(&["2025-06-01", "2025-06-02"]);
        let first = fixture.path("first.sqlite");
        let second = fixture.path("second.sqlite");
        let merged = fixture.path("merged.sqlite");
        fixture.run(&first, "2025-06-01", "2025-06-01", false);
        fixture.run(&second, "2025-06-02", "2025-06-02", false);
        Connection::open(&second)
            .unwrap()
            .execute("DELETE FROM daily_product_completion", [])
            .unwrap();

        let error = fixture.merge(&merged, &[&first, &second]).unwrap_err();
        assert!(
            matches!(&error, MergeError::Refused(message) if message.contains("outside its completed days")),
            "{error}"
        );
        assert!(!merged.exists());
    }

    #[test]
    fn bundled_sqlite_attaches_every_shard_after_the_first() {
        let connection = Connection::open_in_memory().unwrap();
        for index in 1..MAX_SHARDS {
            connection
                .execute(
                    "ATTACH DATABASE ':memory:' AS ?1",
                    [format!("shard{index}")],
                )
                .unwrap();
        }
        assert!(
            connection
                .execute("ATTACH DATABASE ':memory:' AS overflow", [])
                .is_err()
        );
    }

    #[test]
    fn existing_output_is_refused() {
        let fixture = Fixture::new(&["2025-06-01", "2025-06-02"]);
        let first = fixture.path("first.sqlite");
        let second = fixture.path("second.sqlite");
        fixture.run(&first, "2025-06-01", "2025-06-01", false);
        fixture.run(&second, "2025-06-02", "2025-06-02", false);
        let merged = fixture.path("merged.sqlite");
        fs::write(&merged, b"").unwrap();

        let error = fixture.merge(&merged, &[&first, &second]).unwrap_err();
        assert!(
            matches!(&error, MergeError::Refused(message) if message.contains("already exists")),
            "{error}"
        );
    }
}
