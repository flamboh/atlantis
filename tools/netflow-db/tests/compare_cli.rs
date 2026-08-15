use std::process::Command;

use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn compare_accepts_candidate_only_keys_and_maad_rounding() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.500_000_000_01, true);
    create_shared_database(&reference, 42, 0.5, false);

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "compare",
            candidate.to_str().unwrap(),
            reference.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
            "--maad-absolute-tolerance",
            "0.000000001",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["compatible"], true);
    assert_eq!(report["tables"]["traffic_stats"]["candidate_only_rows"], 1);
    assert_eq!(
        report["tables"]["address_structure_stats"]["mismatched_rows"],
        0
    );
}

#[test]
fn compare_rejects_a_shared_scalar_mismatch() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 43, 0.5, false);
    create_shared_database(&reference, 42, 0.5, false);

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "compare",
            candidate.to_str().unwrap(),
            reference.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["compatible"], false);
    assert_eq!(report["tables"]["traffic_stats"]["mismatched_rows"], 1);
}

#[test]
fn compare_rejects_a_candidate_only_scope_inside_a_reference_bucket() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, false);
    create_shared_database(&reference, 42, 0.5, false);
    Connection::open(&candidate)
        .unwrap()
        .execute(
            "INSERT INTO traffic_stats VALUES ('r1','5m',0,300,6,'all','all',1)",
            [],
        )
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "compare",
            candidate.to_str().unwrap(),
            reference.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["tables"]["traffic_stats"]["unexpected_candidate_only_rows"],
        1
    );
}

#[test]
fn compare_accepts_a_dense_zero_scope_missing_from_the_reference() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, false);
    create_shared_database(&reference, 42, 0.5, false);
    Connection::open(&candidate)
        .unwrap()
        .execute(
            "INSERT INTO traffic_stats VALUES ('r1','5m',0,300,6,'all','all',0)",
            [],
        )
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "compare",
            candidate.to_str().unwrap(),
            reference.to_str().unwrap(),
            "--start",
            "0",
            "--end",
            "600",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["tables"]["traffic_stats"]["candidate_only_rows"], 1);
    assert_eq!(
        report["tables"]["traffic_stats"]["unexpected_candidate_only_rows"],
        0
    );
}

fn create_shared_database(path: &std::path::Path, flows: i64, dimension: f64, extra: bool) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE traffic_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_visibility TEXT NOT NULL,
                dst_visibility TEXT NOT NULL, flows INTEGER NOT NULL
            );
            CREATE TABLE protocol_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_visibility TEXT NOT NULL,
                dst_visibility TEXT NOT NULL, unique_protocols_count INTEGER NOT NULL,
                protocols_list TEXT NOT NULL
            );
            CREATE TABLE address_count_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_visibility TEXT NOT NULL,
                dst_visibility TEXT NOT NULL, address_side TEXT NOT NULL,
                unique_address_count INTEGER NOT NULL
            );
            CREATE TABLE address_structure_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_visibility TEXT NOT NULL,
                dst_visibility TEXT NOT NULL, address_side TEXT NOT NULL,
                structure_kind TEXT NOT NULL, values_json TEXT NOT NULL,
                metadata_json TEXT NOT NULL
            );
            CREATE TABLE processed_inputs (
                input_kind TEXT NOT NULL, input_locator TEXT NOT NULL,
                source_id TEXT NOT NULL, bucket_start INTEGER NOT NULL,
                bucket_end INTEGER NOT NULL, status TEXT NOT NULL,
                error_message TEXT
            );
            ",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO traffic_stats VALUES ('r1','5m',0,300,4,'all','all',?1)",
            [flows],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO protocol_stats VALUES ('r1','5m',0,300,4,'all','all',1,'6')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO address_count_stats VALUES ('r1','5m',0,300,4,'all','all','source',2)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO address_structure_stats VALUES ('r1','5m',0,300,4,'all','all','source','dimension',?1,'{\"totalAddrs\":2}')",
            [format!("[{{\"q\":1,\"dim\":{dimension}}}]")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO traffic_stats VALUES ('r1','30m',3600,5400,4,'all','all',0)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO address_structure_stats VALUES ('r1','30m',3600,5400,4,'all','all','source','dimension','[]','{\"totalAddrs\":0}')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO processed_inputs VALUES ('nfcapd','capture','r1',0,300,'processed',NULL)",
            [],
        )
        .unwrap();
    if extra {
        connection
            .execute(
                "INSERT INTO traffic_stats VALUES ('r1','30m',0,1800,4,'all','all',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO address_structure_stats VALUES ('r1','30m',0,1800,4,'all','all','source','dimension','[]','{\"totalAddrs\":0}')",
                [],
            )
            .unwrap();
    }
}
