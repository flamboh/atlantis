use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use netflow_db::storage::ProductIdentity;
use rusqlite::{Connection, params};
use tempfile::{TempDir, tempdir};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args(args)
        .output()
        .unwrap()
}

fn successful(args: &[&str]) {
    let output = run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let directory = tempdir().unwrap();
    let csv = directory.path().join("flows.csv");
    fs::write(
        &csv,
        "received,src,dst,packets,bytes,protocol\n\
        0,192.0.2.1,198.51.100.1,2,100,TCP\n\
        10,192.0.2.2,198.51.100.1,3,200,UDP\n\
        300,192.0.2.2,198.51.100.2,5,300,TCP\n\
        310,192.0.2.3,198.51.100.2,7,0,TCP\n\
        0,2001:db8::1,2001:db9::1,2,100,TCP\n\
        10,2001:db8::2,2001:db9::1,3,200,UDP\n\
        300,2001:db8::2,2001:db9::2,5,300,TCP\n\
        310,2001:db8::3,2001:db9::2,7,0,TCP\n",
    )
    .unwrap();
    let mapping = directory.path().join("mapping.json");
    fs::write(
        &mapping,
        serde_json::to_vec(&serde_json::json!({
            "timestamp_format": "unix", "timestamp_timezone": "UTC",
            "columns": {"time_received": "received", "src_ip": "src", "dst_ip": "dst",
                        "packets": "packets", "bytes": "bytes", "protocol": "protocol"},
            "source_id": {"value": "edge"}
        }))
        .unwrap(),
    )
    .unwrap();
    let database = directory.path().join("product.sqlite");
    let config = directory.path().join("pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database, "timezone": "UTC", "run_maad": true,
            "inputs": [{"input_kind": "csv", "path": csv, "mapping_path": mapping}]
        }))
        .unwrap(),
    )
    .unwrap();
    successful(&["pipeline", "--config", config.to_str().unwrap()]);
    Connection::open(&database)
        .unwrap()
        .execute(
            "INSERT INTO datasets (id, label, default_start_date, maad_internal_side)
         VALUES ('fixture', 'Fixture', '1970-01-01', 0)",
            [],
        )
        .unwrap();
    (directory, database, config)
}

#[test]
fn dual_stack_pipeline_stores_concentration_for_every_maad_set_and_rollup() {
    let (_directory, database, _config) = fixture();
    successful(&[
        "verify",
        database.to_str().unwrap(),
        "--require-data",
        "--require-maad-data",
        "--require-rollup-parity",
    ]);
    let connection = Connection::open(&database).unwrap();
    let cardinalities: (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM address_maad_stats),
                (SELECT COUNT(*) FROM address_concentration_stats)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cardinalities.0, cardinalities.1);
    for ip in [4, 6] {
        for (measure, expected_hhi, expected_top, expected_total, count) in [
            ("addresses", 1.0 / 3.0, 1.0 / 3.0, 3.0, 3),
            ("packets", 117.0 / 289.0, 8.0 / 17.0, 17.0, 3),
            ("bytes", 26.0 / 36.0, 5.0 / 6.0, 600.0, 2),
        ] {
            let (hhi, top, total, entries, unused): (f64, f64, f64, i64, Option<f64>) = connection
                .query_row(
                    "SELECT hhi, top1_share, weight_total, entry_count,
                        CASE WHEN ip_version = 4 THEN entropy_p128 ELSE entropy_p8 END
                 FROM address_concentration_stats WHERE granularity = '10m' AND bucket_start = 0
                   AND ip_version = ?1 AND src_locality = 'all' AND dst_locality = 'all'
                   AND address_side = 'source' AND measure = ?2",
                    params![ip, measure],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
            assert!((hhi - expected_hhi).abs() < 1e-12);
            assert!((top - expected_top).abs() < 1e-12);
            assert_eq!((total, entries, unused), (expected_total, count, None));
        }
    }
    let singleton: (i64, f64, Option<f64>) = connection.query_row(
        "SELECT entry_count, weight_total, hhi FROM address_concentration_stats
         WHERE granularity = '5m' AND bucket_start = 300 AND ip_version = 6
           AND src_locality = 'all' AND dst_locality = 'all' AND address_side = 'source' AND measure = 'bytes'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(singleton, (1, 300.0, None));
}

#[test]
fn verify_requires_concentration_coverage_and_checks_retained_counts() {
    let (_directory, database, _config) = fixture();
    let connection = Connection::open(&database).unwrap();
    connection.execute("UPDATE address_concentration_stats SET entry_count = entry_count + 1 WHERE hhi IS NOT NULL AND measure = 'packets'", []).unwrap();
    let result = run(&["verify", database.to_str().unwrap()]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("count"));
    connection.execute("UPDATE address_concentration_stats SET entry_count = entry_count - 1 WHERE hhi IS NOT NULL AND measure = 'packets'", []).unwrap();
    connection.execute("DELETE FROM address_concentration_stats WHERE measure = 'bytes' AND granularity = '10m'", []).unwrap();
    let result = run(&["verify", database.to_str().unwrap(), "--require-maad-data"]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("address_concentration_stats is missing")
    );
}

#[test]
fn old_product_identity_rejects_the_new_result_contract() {
    let (_directory, database, config) = fixture();
    let connection = Connection::open(&database).unwrap();
    let (schema, selection, config_json): (String, String, String) = connection
        .query_row(
            "SELECT schema_json, selection_json, config_json FROM pipeline_product",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let mut schema: serde_json::Value = serde_json::from_str(&schema).unwrap();
    schema["version"] = 7.into();
    schema["tables"]
        .as_array_mut()
        .unwrap()
        .retain(|table| table["name"] != "address_concentration_stats");
    let selection: serde_json::Value = serde_json::from_str(&selection).unwrap();
    let mut result: serde_json::Value = serde_json::from_str(&config_json).unwrap();
    result["version"] = 6.into();
    result.as_object_mut().unwrap().remove("concentration");
    let old = ProductIdentity::create(&schema, &selection, &result).unwrap();
    connection
        .execute(
            "UPDATE pipeline_product SET schema_json = ?1, schema_fingerprint = ?2,
         config_json = ?3, config_fingerprint = ?4, product_fingerprint = ?5",
            params![
                old.schema_json,
                old.schema_fingerprint,
                old.config_json,
                old.config_fingerprint,
                old.fingerprint
            ],
        )
        .unwrap();
    let output = run(&["pipeline", "--config", config.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity mismatch"));
}

#[test]
fn compare_concentration_uses_maad_tolerance_and_exact_counts() {
    let (directory, database, _config) = fixture();
    let candidate = directory.path().join("candidate.sqlite");
    fs::copy(&database, &candidate).unwrap();
    let connection = Connection::open(&candidate).unwrap();
    let compare = || {
        let output = run(&[
            "compare",
            candidate.to_str().unwrap(),
            database.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
            "--maad-absolute-tolerance",
            "0.000001",
        ]);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.success(), report)
    };
    connection.execute("UPDATE address_concentration_stats SET hhi = hhi + 0.0000005 WHERE measure = 'packets' AND hhi IS NOT NULL", []).unwrap();
    assert!(compare().0);
    connection.execute("UPDATE address_concentration_stats SET hhi = hhi + 0.001 WHERE measure = 'packets' AND hhi IS NOT NULL", []).unwrap();
    let (success, report) = compare();
    assert!(!success);
    assert!(
        report["tables"]["address_concentration_stats"]["mismatched_rows"]
            .as_i64()
            .unwrap()
            > 0
    );
    connection.execute("UPDATE address_concentration_stats SET hhi = hhi - 0.0010005 WHERE measure = 'packets' AND hhi IS NOT NULL", []).unwrap();
    connection.execute("UPDATE address_concentration_stats SET entry_count = entry_count + 1 WHERE measure = 'packets' AND hhi IS NOT NULL", []).unwrap();
    assert!(!compare().0);
}

#[test]
fn compare_skips_reference_internal_concentration_and_rejects_candidate_rows() {
    let (directory, candidate, _config) = fixture();
    let reference = directory.path().join("reference.sqlite");
    fs::copy(&candidate, &reference).unwrap();
    let internal_row = "INSERT INTO address_concentration_stats
        SELECT source_id, granularity, bucket_start, bucket_end, ip_version,
               'internal', 'external', address_side, measure, weight_total, entry_count,
               hhi, top1_share, top10_share, top100_share, entropy_p8, entropy_p16,
               entropy_p24, entropy_p32, entropy_p48, entropy_p64, entropy_p128
        FROM address_concentration_stats
        WHERE granularity = '5m' AND ip_version = 4 AND bucket_start = 0
          AND src_locality = 'all' AND dst_locality = 'all'
          AND address_side = 'source' AND measure = 'addresses'";
    Connection::open(&reference)
        .unwrap()
        .execute(internal_row, [])
        .unwrap();
    let compare = || {
        let output = run(&[
            "compare",
            candidate.to_str().unwrap(),
            reference.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
        ]);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        (output.status.success(), report)
    };
    let (success, report) = compare();
    assert!(success);
    assert_eq!(
        report["tables"]["address_concentration_stats"]["skipped_reference_rows"],
        1
    );
    let connection = Connection::open(&candidate).unwrap();
    connection.execute(internal_row, []).unwrap();
    let (success, report) = compare();
    assert!(!success);
    assert_eq!(
        report["tables"]["address_concentration_stats"]["unexpected_candidate_only_rows"],
        1
    );
    let output = run(&["verify", candidate.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("internal-side"));
}
