//! Verification of canonical SQLite databases against web query assumptions.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use thiserror::Error;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyOptions {
    pub source_id: Option<String>,
    pub dataset_id: Option<String>,
    pub require_data: bool,
    pub require_maad_data: bool,
    pub require_processed: bool,
    pub require_rollup_parity: bool,
    pub require_no_raw_ip: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationReport {
    pub database: PathBuf,
    pub source_id: String,
    pub bucket_start: i64,
    pub bucket_end: i64,
    pub row_counts: BTreeMap<String, i64>,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("database not found: {0}")]
    DatabaseNotFound(PathBuf),
    #[error("web compatibility check failed: {0}")]
    Incompatible(String),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub fn verify_database(
    database: impl AsRef<Path>,
    options: &VerifyOptions,
) -> Result<VerificationReport, VerifyError> {
    let database = database.as_ref();
    if !database.is_file() {
        return Err(VerifyError::DatabaseNotFound(database.to_owned()));
    }
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    assert_schema(&connection)?;
    assert_dataset_metadata(&connection, options.dataset_id.as_deref())?;
    if options.require_no_raw_ip {
        assert_no_raw_ip_persistence(&connection)?;
    }
    let source_id = match &options.source_id {
        Some(source_id) => source_id.clone(),
        None => connection
            .query_row(
                "SELECT source_id FROM bucket_coverage WHERE granularity = '5m' \
                 ORDER BY source_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                VerifyError::Incompatible("no source_id found in bucket_coverage".into())
            })?,
    };
    let row_counts = table_row_counts(&connection)?;
    if options.require_data {
        for table in [
            "bucket_coverage",
            "traffic_stats",
            "protocol_stats",
            "address_count_stats",
            "port_count_stats",
        ] {
            if row_counts[table] == 0 {
                return Err(VerifyError::Incompatible(format!("{table} has no rows")));
            }
        }
        let rollups: i64 = connection.query_row(
            "SELECT COUNT(*) FROM traffic_stats WHERE granularity != '5m'",
            [],
            |row| row.get(0),
        )?;
        if rollups == 0 {
            return Err(VerifyError::Incompatible(
                "traffic_stats has no rollup rows".into(),
            ));
        }
    }
    if options.require_maad_data && row_counts["address_structure_stats"] == 0 {
        return Err(VerifyError::Incompatible(
            "address_structure_stats has no rows".into(),
        ));
    }
    if options.require_processed {
        assert_processed_inputs_complete(&connection)?;
    }
    if options.require_processed || options.require_rollup_parity {
        assert_traffic_rollup_parity(&connection)?;
    }
    let (bucket_start, bucket_end) = select_query_window(&connection, &source_id)?;
    assert_query_returns_row(
        &connection,
        COVERAGE_QUERY,
        params![source_id, bucket_start, bucket_end],
        "web coverage query returned no rows",
    )?;
    let has_metric_data = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM traffic_stats
            WHERE source_id = ?1 AND granularity = '1h'
              AND bucket_start >= ?2 AND bucket_start < ?3
        )",
        params![source_id, bucket_start, bucket_end],
        |row| row.get::<_, bool>(0),
    )?;
    if has_metric_data {
        assert_query_returns_row(
            &connection,
            NETFLOW_QUERY,
            params![source_id, bucket_start, bucket_end],
            "web netflow stats query returned no rows",
        )?;
        assert_query_returns_row(
            &connection,
            ADDRESS_QUERY,
            params![source_id, bucket_start, bucket_end],
            "web IP stats query returned no rows",
        )?;
        assert_query_returns_row(
            &connection,
            PROTOCOL_QUERY,
            params![source_id, bucket_start, bucket_end],
            "web protocol stats query returned no rows",
        )?;
        assert_optional_maad_query(
            &connection,
            STRUCTURE_QUERY,
            &source_id,
            bucket_start,
            bucket_end,
            options.require_maad_data,
            "web structure stats query returned no rows",
        )?;
        assert_optional_maad_query(
            &connection,
            SPECTRUM_QUERY,
            &source_id,
            bucket_start,
            bucket_end,
            options.require_maad_data,
            "web spectrum stats query returned no rows",
        )?;
        let file_bucket_start = connection.query_row(
            "SELECT MIN(bucket_start) FROM traffic_stats
             WHERE source_id = ?1 AND granularity = '5m'
               AND bucket_start >= ?2 AND bucket_start < ?3",
            params![source_id, bucket_start, bucket_end],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        if let Some(file_bucket_start) = file_bucket_start {
            assert_query_returns_row(
                &connection,
                FILE_DETAILS_QUERY,
                params![file_bucket_start, file_bucket_start, file_bucket_start],
                "web file details query returned no rows",
            )?;
        }
    } else if options.require_maad_data {
        return Err(VerifyError::Incompatible(
            "capture coverage exists but no metric data is available".into(),
        ));
    }
    Ok(VerificationReport {
        database: database.to_owned(),
        source_id,
        bucket_start,
        bucket_end,
        row_counts,
    })
}

const DATASET_REQUIRED_COLUMNS: &[(&str, &[&str])] = &[(
    "datasets",
    &[
        "id",
        "label",
        "default_start_date",
        "source_mode",
        "discovery_mode",
        "sort_order",
    ],
)];

const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
    (
        "processed_inputs",
        &[
            "input_kind",
            "input_locator",
            "scan_locator",
            "source_id",
            "bucket_start",
            "bucket_end",
            "status",
            "error_message",
            "discovered_at",
            "processed_at",
        ],
    ),
    (
        "processed_input_scans",
        &[
            "input_kind",
            "input_locator",
            "status",
            "rejected_rows",
            "skipped_bad_column_count",
            "processed_at",
        ],
    ),
    (
        "bucket_coverage",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "coverage_state",
            "observed_units",
            "expected_units",
            "rejected_units",
        ],
    ),
    (
        "traffic_stats",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "ip_version",
            "src_visibility",
            "dst_visibility",
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
            "processed_at",
        ],
    ),
    (
        "protocol_stats",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "ip_version",
            "src_visibility",
            "dst_visibility",
            "unique_protocols_count",
            "protocols_list",
            "processed_at",
        ],
    ),
    (
        "address_count_stats",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "ip_version",
            "src_visibility",
            "dst_visibility",
            "address_side",
            "unique_address_count",
            "processed_at",
        ],
    ),
    (
        "port_count_stats",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "ip_version",
            "src_visibility",
            "dst_visibility",
            "port_side",
            "port_range",
            "unique_port_count",
            "processed_at",
        ],
    ),
    (
        "address_structure_stats",
        &[
            "source_id",
            "granularity",
            "bucket_start",
            "bucket_end",
            "ip_version",
            "src_visibility",
            "dst_visibility",
            "address_side",
            "structure_kind",
            "values_json",
            "metadata_json",
            "processed_at",
        ],
    ),
];

const LEGACY_TABLES: &[&str] = &[
    "netflow_stats_v2",
    "ip_stats_v2",
    "protocol_stats_v2",
    "structure_stats_v2",
    "spectrum_stats_v2",
    "dimension_stats_v2",
    "processed_inputs_v2",
];

const RAW_IP_COLUMN_NAMES: &[&str] = &[
    "address",
    "client_ip",
    "da_ip",
    "destination_address",
    "destination_ip",
    "dst_addr",
    "dst_ip",
    "ip",
    "ip_addr",
    "ip_address",
    "sa_ip",
    "server_ip",
    "source_address",
    "source_ip",
    "src_addr",
    "src_ip",
];

fn assert_schema(connection: &Connection) -> Result<(), VerifyError> {
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let required = DATASET_REQUIRED_COLUMNS.iter().chain(REQUIRED_COLUMNS);
    let missing = required
        .clone()
        .filter_map(|(table, _)| (!tables.contains(*table)).then_some(*table))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(VerifyError::Incompatible(format!(
            "missing canonical tables: {}",
            missing.join(", ")
        )));
    }
    let legacy = LEGACY_TABLES
        .iter()
        .copied()
        .filter(|table| tables.contains(*table))
        .collect::<Vec<_>>();
    if !legacy.is_empty() {
        return Err(VerifyError::Incompatible(format!(
            "legacy tables still present: {}",
            legacy.join(", ")
        )));
    }
    for (table, expected_columns) in required {
        let columns = table_columns(connection, table)?;
        let missing = expected_columns
            .iter()
            .filter(|column| !columns.contains_key(**column))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(VerifyError::Incompatible(format!(
                "{table} missing columns: {}",
                missing.join(", ")
            )));
        }
    }
    Ok(())
}

fn assert_dataset_metadata(
    connection: &Connection,
    dataset_id: Option<&str>,
) -> Result<(), VerifyError> {
    let row = if let Some(dataset_id) = dataset_id {
        connection
            .query_row("SELECT 1 FROM datasets WHERE id = ?", [dataset_id], |_| {
                Ok(())
            })
            .optional()?
    } else {
        connection
            .query_row("SELECT 1 FROM datasets ORDER BY id LIMIT 1", [], |_| Ok(()))
            .optional()?
    };
    if row.is_none() {
        let message = dataset_id.map_or_else(
            || "datasets has no metadata rows".to_owned(),
            |value| format!("datasets has no metadata row for {value:?}"),
        );
        return Err(VerifyError::Incompatible(message));
    }
    Ok(())
}

fn table_columns(
    connection: &Connection,
    table: &str,
) -> Result<HashMap<String, String>, VerifyError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({})", quote(table)))?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn table_row_counts(connection: &Connection) -> Result<BTreeMap<String, i64>, VerifyError> {
    REQUIRED_COLUMNS
        .iter()
        .map(|(table, _)| {
            let count = connection.query_row(
                &format!("SELECT COUNT(*) FROM {}", quote(table)),
                [],
                |row| row.get(0),
            )?;
            Ok(((*table).to_owned(), count))
        })
        .collect()
}

fn assert_no_raw_ip_persistence(connection: &Connection) -> Result<(), VerifyError> {
    for (table, _) in REQUIRED_COLUMNS {
        for (column, declared_type) in table_columns(connection, table)? {
            if RAW_IP_COLUMN_NAMES.contains(&column.to_ascii_lowercase().as_str()) {
                return Err(VerifyError::Incompatible(format!(
                    "{table}.{column} looks like a raw IP address column"
                )));
            }
            if declared_type.to_ascii_uppercase().contains("TEXT") {
                let mut statement = connection.prepare(&format!(
                    "SELECT {} FROM {} WHERE {} IS NOT NULL",
                    quote(&column),
                    quote(table),
                    quote(&column)
                ))?;
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    let value = row.get::<_, String>(0)?;
                    if let Some(address) = ipv4_literal(&value) {
                        return Err(VerifyError::Incompatible(format!(
                            "{table}.{column} contains raw IPv4 literal {address}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn ipv4_literal(value: &str) -> Option<&str> {
    value
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .find(|candidate| {
            let octets = candidate.split('.').collect::<Vec<_>>();
            octets.len() == 4
                && octets.iter().all(|octet| {
                    !octet.is_empty() && octet.len() <= 3 && octet.parse::<u8>().is_ok()
                })
        })
}

fn assert_processed_inputs_complete(connection: &Connection) -> Result<(), VerifyError> {
    let pending: i64 = connection.query_row(
        "SELECT COUNT(*) FROM processed_inputs WHERE status != 'processed'",
        [],
        |row| row.get(0),
    )?;
    if pending != 0 {
        return Err(VerifyError::Incompatible(format!(
            "processed_inputs has {pending} unprocessed rows"
        )));
    }
    let incomplete: i64 = connection.query_row(
        "SELECT COUNT(DISTINCT buckets.scan_locator)
         FROM processed_inputs AS buckets
         LEFT JOIN processed_input_scans AS scans
           ON scans.input_kind = buckets.input_kind
          AND scans.input_locator = buckets.scan_locator
          AND scans.status = 'processed'
         WHERE buckets.input_kind = 'csv' AND scans.input_locator IS NULL",
        [],
        |row| row.get(0),
    )?;
    if incomplete != 0 {
        return Err(VerifyError::Incompatible(format!(
            "processed_inputs has {incomplete} incomplete CSV scan(s)"
        )));
    }
    Ok(())
}

fn select_query_window(
    connection: &Connection,
    source_id: &str,
) -> Result<(i64, i64), VerifyError> {
    let window = connection.query_row(
        "SELECT MIN(bucket_start), MAX(bucket_end) FROM bucket_coverage
         WHERE source_id = ? AND granularity = '5m'
        ",
        [source_id],
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    window.0.zip(window.1).ok_or_else(|| {
        VerifyError::Incompatible(format!("no coverage rows found for source_id={source_id}"))
    })
}

fn assert_query_returns_row<P: rusqlite::Params>(
    connection: &Connection,
    query: &str,
    parameters: P,
    message: &str,
) -> Result<(), VerifyError> {
    let found = connection
        .query_row(query, parameters, |_| Ok(()))
        .optional()?;
    if found.is_none() {
        return Err(VerifyError::Incompatible(message.into()));
    }
    Ok(())
}

fn assert_optional_maad_query(
    connection: &Connection,
    query: &str,
    source_id: &str,
    bucket_start: i64,
    bucket_end: i64,
    required: bool,
    message: &str,
) -> Result<(), VerifyError> {
    let found = connection
        .query_row(query, params![source_id, bucket_start, bucket_end], |_| {
            Ok(())
        })
        .optional()?;
    if required && found.is_none() {
        return Err(VerifyError::Incompatible(message.into()));
    }
    Ok(())
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn assert_traffic_rollup_parity(connection: &Connection) -> Result<(), VerifyError> {
    let mismatches: i64 = connection.query_row(ROLLUP_PARITY_QUERY, [], |row| row.get(0))?;
    if mismatches != 0 {
        return Err(VerifyError::Incompatible(format!(
            "traffic_stats rollup parity failed with {mismatches} mismatched rows"
        )));
    }
    Ok(())
}

const NETFLOW_QUERY: &str = "
    SELECT bucket_start, SUM(flows), SUM(packets), SUM(bytes),
           CASE WHEN SUM(duration_count) = 0 THEN NULL
                ELSE CAST(SUM(duration_sum_ms) AS REAL) / SUM(duration_count) END,
           CASE WHEN SUM(min_ttl_count) = 0 THEN NULL
                ELSE CAST(SUM(min_ttl_sum) AS REAL) / SUM(min_ttl_count) END,
           CASE WHEN SUM(max_ttl_count) = 0 THEN NULL
                ELSE CAST(SUM(max_ttl_sum) AS REAL) / SUM(max_ttl_count) END,
           SUM(CASE WHEN ip_version = 4 THEN flows ELSE 0 END),
           SUM(CASE WHEN ip_version = 6 THEN flows ELSE 0 END)
    FROM traffic_stats
    WHERE source_id IN (?) AND granularity = '1h'
      AND src_visibility = 'all' AND dst_visibility = 'all'
      AND bucket_start >= ? AND bucket_start < ?
    GROUP BY bucket_start ORDER BY bucket_start LIMIT 1";

const COVERAGE_QUERY: &str = "
    SELECT source_id, bucket_start, bucket_end, coverage_state,
           observed_units, expected_units
    FROM bucket_coverage
    WHERE source_id IN (?) AND granularity = '5m'
      AND bucket_start >= ? AND bucket_start < ?
    ORDER BY source_id, bucket_start LIMIT 1";

const ADDRESS_QUERY: &str = "
    SELECT source_id, bucket_start, bucket_end, granularity,
           SUM(CASE WHEN address_side = 'source' AND ip_version = 4
                    THEN unique_address_count ELSE 0 END),
           SUM(CASE WHEN address_side = 'destination' AND ip_version = 4
                    THEN unique_address_count ELSE 0 END),
           SUM(CASE WHEN address_side = 'source' AND ip_version = 6
                    THEN unique_address_count ELSE 0 END),
           SUM(CASE WHEN address_side = 'destination' AND ip_version = 6
                    THEN unique_address_count ELSE 0 END),
           MAX(processed_at)
    FROM address_count_stats
    WHERE granularity = '1h' AND source_id IN (?)
      AND src_visibility = 'all' AND dst_visibility = 'all'
      AND bucket_start >= ? AND bucket_start < ?
    GROUP BY source_id, bucket_start, bucket_end, granularity
    ORDER BY source_id, bucket_start LIMIT 1";

const PROTOCOL_QUERY: &str = "
    SELECT source_id, bucket_start, bucket_end, granularity,
           SUM(CASE WHEN ip_version = 4 THEN unique_protocols_count ELSE 0 END),
           SUM(CASE WHEN ip_version = 6 THEN unique_protocols_count ELSE 0 END),
           MAX(processed_at)
    FROM protocol_stats
    WHERE granularity = '1h' AND source_id IN (?)
      AND src_visibility = 'all' AND dst_visibility = 'all'
      AND bucket_start >= ? AND bucket_start < ?
    GROUP BY source_id, bucket_start, bucket_end, granularity
    ORDER BY source_id, bucket_start LIMIT 1";

const STRUCTURE_QUERY: &str = "
    SELECT source_id, bucket_start,
           MAX(CASE WHEN address_side = 'source' THEN values_json END),
           MAX(CASE WHEN address_side = 'destination' THEN values_json END)
    FROM address_structure_stats
    WHERE granularity = '1h' AND source_id IN (?)
      AND bucket_start >= ? AND bucket_start < ? AND ip_version = 4
      AND src_visibility = 'all' AND dst_visibility = 'all'
      AND structure_kind = 'structure'
    GROUP BY source_id, bucket_start ORDER BY source_id, bucket_start LIMIT 1";

const SPECTRUM_QUERY: &str = "
    SELECT source_id, bucket_start,
           MAX(CASE WHEN address_side = 'source' THEN values_json END),
           MAX(CASE WHEN address_side = 'destination' THEN values_json END)
    FROM address_structure_stats
    WHERE granularity = '1h' AND source_id IN (?)
      AND bucket_start >= ? AND bucket_start < ? AND ip_version = 4
      AND src_visibility = 'all' AND dst_visibility = 'all'
      AND structure_kind = 'spectrum'
    GROUP BY source_id, bucket_start ORDER BY source_id, bucket_start LIMIT 1";

const FILE_DETAILS_QUERY: &str = "
    SELECT ns.router, ns.bucket_start, ns.flows, pi.input_locator, ip.address_count
    FROM (
        SELECT source_id AS router, bucket_start, SUM(flows) AS flows
        FROM traffic_stats
        WHERE granularity = '5m' AND bucket_start = ?
          AND src_visibility = 'all' AND dst_visibility = 'all'
        GROUP BY source_id, bucket_start
    ) ns
    LEFT JOIN (
        SELECT source_id, bucket_start, MIN(input_locator) AS input_locator
        FROM processed_inputs WHERE bucket_start = ? GROUP BY source_id, bucket_start
    ) pi ON pi.source_id = ns.router AND pi.bucket_start = ns.bucket_start
    LEFT JOIN (
        SELECT source_id, bucket_start,
               SUM(CASE WHEN address_side = 'source' AND ip_version = 4
                        THEN unique_address_count ELSE 0 END) AS address_count
        FROM address_count_stats
        WHERE granularity = '5m' AND bucket_start = ?
          AND src_visibility = 'all' AND dst_visibility = 'all'
        GROUP BY source_id, bucket_start
    ) ip ON ip.source_id = ns.router AND ip.bucket_start = ns.bucket_start
    ORDER BY ns.router LIMIT 1";

const ROLLUP_PARITY_QUERY: &str = "
    WITH expected AS (
        SELECT calendar.source_id, calendar.granularity, calendar.bucket_start,
               calendar.bucket_end, ts.ip_version, ts.src_visibility, ts.dst_visibility,
               SUM(ts.flows), SUM(ts.flows_tcp), SUM(ts.flows_udp), SUM(ts.flows_icmp),
               SUM(ts.flows_other), SUM(ts.packets), SUM(ts.packets_tcp),
               SUM(ts.packets_udp), SUM(ts.packets_icmp), SUM(ts.packets_other),
               SUM(ts.bytes), SUM(ts.bytes_tcp), SUM(ts.bytes_udp), SUM(ts.bytes_icmp),
               SUM(ts.bytes_other), SUM(ts.duration_sum_ms), SUM(ts.duration_count),
               CASE WHEN SUM(ts.duration_count) = 0 THEN NULL
                    ELSE CAST(SUM(ts.duration_sum_ms) AS REAL) / SUM(ts.duration_count) END,
               SUM(ts.min_ttl_sum), SUM(ts.min_ttl_count),
               CASE WHEN SUM(ts.min_ttl_count) = 0 THEN NULL
                    ELSE CAST(SUM(ts.min_ttl_sum) AS REAL) / SUM(ts.min_ttl_count) END,
               SUM(ts.max_ttl_sum), SUM(ts.max_ttl_count),
               CASE WHEN SUM(ts.max_ttl_count) = 0 THEN NULL
                    ELSE CAST(SUM(ts.max_ttl_sum) AS REAL) / SUM(ts.max_ttl_count) END
        FROM (
            SELECT source_id, granularity, bucket_start, bucket_end
            FROM address_count_stats WHERE granularity IN ('30m', '1h', '1d')
            GROUP BY source_id, granularity, bucket_start, bucket_end
        ) calendar
        JOIN traffic_stats ts
          ON ts.source_id = calendar.source_id
         AND ts.bucket_start >= calendar.bucket_start
         AND ts.bucket_start < calendar.bucket_end
         AND ts.granularity = '5m'
        GROUP BY calendar.source_id, calendar.granularity, calendar.bucket_start,
                 calendar.bucket_end, ts.ip_version, ts.src_visibility, ts.dst_visibility
    ), actual AS (
        SELECT source_id, granularity, bucket_start, bucket_end, ip_version,
               src_visibility, dst_visibility, flows, flows_tcp, flows_udp, flows_icmp,
               flows_other, packets, packets_tcp, packets_udp, packets_icmp, packets_other,
               bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other, duration_sum_ms,
               duration_count, average_duration_ms, min_ttl_sum, min_ttl_count,
               average_min_ttl, max_ttl_sum, max_ttl_count, average_max_ttl
        FROM traffic_stats WHERE granularity IN ('30m', '1h', '1d')
    ), missing_or_changed AS (SELECT * FROM expected EXCEPT SELECT * FROM actual),
       extra_or_changed AS (SELECT * FROM actual EXCEPT SELECT * FROM expected)
    SELECT (SELECT COUNT(*) FROM missing_or_changed)
         + (SELECT COUNT(*) FROM extra_or_changed)";

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::*;
    use crate::storage::init_schema;

    #[test]
    fn canonical_database_satisfies_web_query_contract() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("canonical.sqlite");
        let connection = Connection::open(&database).unwrap();
        init_schema(&connection).unwrap();
        connection.execute(
            "INSERT INTO datasets (id, label, default_start_date, source_mode, discovery_mode, sort_order)
             VALUES ('fixture', 'Fixture', '2025-01-01', 'single', 'directory', 0)",
            [],
        ).unwrap();
        for (granularity, end) in [("5m", 400), ("1h", 3_700)] {
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
                 ) VALUES ('r1', ?1, 100, ?2, 4, 'all', 'all', 2, 2, 0, 0, 0,
                    3, 3, 0, 0, 0, 4, 4, 0, 0, 0, 10, 2, 5.0, 62, 2, 31.0,
                    128, 2, 64.0)",
                    params![granularity, end],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO bucket_coverage (
                        source_id, granularity, bucket_start, bucket_end,
                        coverage_state, observed_units, expected_units, rejected_units
                     ) VALUES ('r1', ?1, 100, ?2, 'complete', 1, 1, 0)",
                    params![granularity, end],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO protocol_stats (
                    source_id, granularity, bucket_start, bucket_end, ip_version,
                    src_visibility, dst_visibility, unique_protocols_count, protocols_list
                 ) VALUES ('r1', ?1, 100, ?2, 4, 'all', 'all', 1, '6')",
                    params![granularity, end],
                )
                .unwrap();
            for side in ["source", "destination"] {
                connection
                    .execute(
                        "INSERT INTO address_count_stats (
                        source_id, granularity, bucket_start, bucket_end, ip_version,
                        src_visibility, dst_visibility, address_side, unique_address_count
                     ) VALUES ('r1', ?1, 100, ?2, 4, 'all', 'all', ?3, 1)",
                        params![granularity, end, side],
                    )
                    .unwrap();
            }
            connection
                .execute(
                    "INSERT INTO port_count_stats (
                    source_id, granularity, bucket_start, bucket_end, ip_version,
                    src_visibility, dst_visibility, port_side, port_range, unique_port_count
                 ) VALUES ('r1', ?1, 100, ?2, 4, 'all', 'all', 'source', 'low', 1)",
                    params![granularity, end],
                )
                .unwrap();
        }
        drop(connection);

        let report = verify_database(
            &database,
            &VerifyOptions {
                source_id: Some("r1".into()),
                dataset_id: Some("fixture".into()),
                require_data: true,
                require_no_raw_ip: true,
                ..VerifyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.source_id, "r1");
        assert_eq!((report.bucket_start, report.bucket_end), (100, 400));
    }

    #[test]
    fn coverage_only_database_is_a_valid_inspectable_product() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("unknown.sqlite");
        let connection = Connection::open(&database).unwrap();
        init_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO datasets (
                    id, label, default_start_date, source_mode, discovery_mode, sort_order
                 ) VALUES ('fixture', 'Fixture', '2025-01-01', 'single', 'directory', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO bucket_coverage (
                    source_id, granularity, bucket_start, bucket_end,
                    coverage_state, observed_units, expected_units, rejected_units
                 ) VALUES ('r1', '5m', 100, 400, 'unknown', 0, 1, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let report = verify_database(&database, &VerifyOptions::default()).unwrap();

        assert_eq!(report.source_id, "r1");
        assert_eq!((report.bucket_start, report.bucket_end), (100, 400));
        assert_eq!(report.row_counts["traffic_stats"], 0);
    }

    #[test]
    fn raw_ipv4_literal_in_canonical_text_column_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("raw-ip.sqlite");
        let connection = Connection::open(&database).unwrap();
        init_schema(&connection).unwrap();
        connection.execute(
            "INSERT INTO datasets (id, label, default_start_date, source_mode, discovery_mode, sort_order)
             VALUES ('fixture', 'Fixture', '2025-01-01', 'single', 'directory', 0)",
            [],
        ).unwrap();
        connection
            .execute("ALTER TABLE protocol_stats ADD COLUMN trace TEXT", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO protocol_stats (
                source_id, granularity, bucket_start, bucket_end, ip_version,
                src_visibility, dst_visibility, unique_protocols_count, protocols_list, trace
             ) VALUES ('r1', '5m', 100, 400, 4, 'all', 'all', 1, '6', 'peer=192.0.2.8')",
                [],
            )
            .unwrap();
        drop(connection);

        let error = verify_database(
            &database,
            &VerifyOptions {
                require_no_raw_ip: true,
                ..VerifyOptions::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("192.0.2.8"));
    }
}
