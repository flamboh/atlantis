use std::{fs, path::Path, process::Command};

use rusqlite::{Connection, params};
use tempfile::tempdir;

fn run(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "args={args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn build_csv_product(directory: &Path) -> std::path::PathBuf {
    let csv = directory.join("flows.csv");
    let mapping = directory.join("mapping.json");
    let database = directory.join("csv.sqlite");
    fs::write(
        &csv,
        "received,src,dst,packets,bytes,protocol\n\
         0,192.0.2.1,198.51.100.1,2,100,TCP\n\
         10,192.0.2.2,198.51.100.1,3,200,UDP\n\
         300,192.0.2.2,198.51.100.2,5,300,TCP\n\
         310,192.0.2.3,198.51.100.2,7,400,TCP\n\
         600,192.0.2.4,198.51.100.3,11,500,UDP\n\
         900,192.0.2.4,198.51.100.3,13,600,UDP\n\
         1210,192.0.2.5,198.51.100.4,17,700,TCP\n",
    )
    .unwrap();
    fs::write(
        &mapping,
        serde_json::to_vec(&serde_json::json!({
            "timestamp_format": "unix",
            "timestamp_timezone": "UTC",
            "columns": {
                "time_received": "received",
                "src_ip": "src",
                "dst_ip": "dst",
                "packets": "packets",
                "bytes": "bytes",
                "protocol": "protocol"
            },
            "source_id": {"value": "edge"}
        }))
        .unwrap(),
    )
    .unwrap();
    let config = directory.join("csv-pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "run_maad": true,
            "inputs": [{
                "input_kind": "csv",
                "path": csv,
                "mapping_path": mapping
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    run(&["pipeline", "--config", config.to_str().unwrap()]);
    database
}

#[test]
fn ten_minute_rollups_match_their_five_minute_children() {
    let temporary = tempdir().unwrap();
    let database = build_csv_product(temporary.path());
    Connection::open(&database)
        .unwrap()
        .execute(
            "INSERT INTO datasets (id, label, default_start_date) VALUES ('csv', 'CSV', '1970-01-01')",
            [],
        )
        .unwrap();

    run(&[
        "verify",
        database.to_str().unwrap(),
        "--require-data",
        "--require-maad-data",
        "--require-rollup-parity",
    ]);

    let connection = Connection::open(&database).unwrap();
    let ten_minute_buckets = connection
        .prepare(
            "SELECT bucket_start, bucket_end, observed_units, expected_units
             FROM bucket_coverage WHERE source_id = 'edge' AND granularity = '10m'
             ORDER BY bucket_start",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        ten_minute_buckets,
        vec![(0, 600, 2, 2), (600, 1_200, 2, 2), (1_200, 1_800, 1, 1)]
    );

    for table in [
        "traffic_stats",
        "protocol_stats",
        "address_count_stats",
        "port_count_stats",
        "address_structure_stats",
    ] {
        let rows: i64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE granularity = '10m'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(rows > 0, "{table} has no 10m rows");
    }

    let parity_mismatches: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM traffic_stats parent
             WHERE parent.granularity = '10m'
               AND (parent.flows, parent.packets, parent.bytes) IS NOT (
                   SELECT SUM(child.flows), SUM(child.packets), SUM(child.bytes)
                   FROM traffic_stats child
                   WHERE child.granularity = '5m'
                     AND child.source_id = parent.source_id
                     AND child.ip_version = parent.ip_version
                     AND child.src_locality = parent.src_locality
                     AND child.dst_locality = parent.dst_locality
                     AND child.bucket_start >= parent.bucket_start
                     AND child.bucket_start < parent.bucket_end
               )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parity_mismatches, 0);

    let unique_sources = |granularity: &str, bucket_start: i64| -> i64 {
        connection
            .query_row(
                "SELECT unique_address_count FROM address_count_stats
                 WHERE source_id = 'edge' AND granularity = ?1 AND bucket_start = ?2
                   AND ip_version = 4 AND src_locality = 'all' AND dst_locality = 'all'
                   AND address_side = 'source'",
                params![granularity, bucket_start],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(unique_sources("5m", 0), 2);
    assert_eq!(unique_sources("5m", 300), 2);
    assert_eq!(unique_sources("10m", 0), 3);
    assert_eq!(unique_sources("10m", 600), 1);

    let maad_total_addrs = |granularity: &str, bucket_start: i64| -> i64 {
        connection
            .query_row(
                "SELECT json_extract(metadata_json, '$.totalAddrs') FROM address_structure_stats
                 WHERE source_id = 'edge' AND granularity = ?1 AND bucket_start = ?2
                   AND ip_version = 4 AND src_locality = 'all' AND dst_locality = 'all'
                   AND address_side = 'source' AND structure_kind = 'structure'",
                params![granularity, bucket_start],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(maad_total_addrs("10m", 0), 3);
    assert_eq!(maad_total_addrs("10m", 600), 1);
}
