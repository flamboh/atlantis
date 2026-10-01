use std::process::Command;

use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn compare_accepts_candidate_only_keys_and_maad_rounding() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.500_000_1, &[0.5, 1.000_000_1], true);
    create_shared_database(&reference, 42, 0.5, &[0.5, 1.0], false);

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
            "0.000001",
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
    assert_eq!(report["tables"]["address_maad_stats"]["mismatched_rows"], 0);
    assert_eq!(
        report["maad_q_grid"],
        serde_json::json!({"ip_versions": [4], "mismatched_ip_versions": []})
    );
    assert!(
        report["tables"]["address_maad_stats"]["max_maad_absolute_delta"]
            .as_f64()
            .unwrap()
            > 0.0
    );
}

#[test]
fn compare_rejects_maad_curves_outside_the_tolerance_or_of_another_length() {
    for candidate_tau in [[0.5, 1.001].as_slice(), [0.5].as_slice()] {
        let temporary = tempdir().unwrap();
        let candidate = temporary.path().join("candidate.sqlite");
        let reference = temporary.path().join("reference.sqlite");
        create_shared_database(&candidate, 42, 0.5, candidate_tau, false);
        create_shared_database(&reference, 42, 0.5, &[0.5, 1.0], false);

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
                "0.000001",
            ])
            .output()
            .unwrap();

        assert!(!output.status.success());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["tables"]["address_maad_stats"]["mismatched_rows"], 1,
            "{candidate_tau:?}"
        );
    }
}

#[test]
fn compare_rejects_identical_maad_curves_on_a_changed_or_missing_q_grid() {
    for change in [
        "UPDATE maad_q_grid SET q_min = -1.0",
        "UPDATE maad_q_grid SET q_step = 0.25",
        "DELETE FROM maad_q_grid",
        "DROP TABLE maad_q_grid",
    ] {
        let temporary = tempdir().unwrap();
        let candidate = temporary.path().join("candidate.sqlite");
        let reference = temporary.path().join("reference.sqlite");
        create_shared_database(&candidate, 42, 0.5, &[0.5, 1.0], false);
        create_shared_database(&reference, 42, 0.5, &[0.5, 1.0], false);
        Connection::open(&candidate)
            .unwrap()
            .execute_batch(change)
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

        assert!(!output.status.success(), "{change}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["compatible"], false, "{change}");
        assert_eq!(
            report["tables"]["address_maad_stats"]["mismatched_rows"], 0,
            "{change}"
        );
        assert_eq!(
            report["maad_q_grid"],
            serde_json::json!({"ip_versions": [4], "mismatched_ip_versions": [4]}),
            "{change}"
        );
    }
}

#[test]
fn compare_rejects_a_shared_scalar_mismatch() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 43, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);

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
    create_shared_database(&candidate, 42, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);
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
fn compare_rejects_ipv6_maad_rows_missing_from_a_reference_bucket() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);
    Connection::open(&candidate)
        .unwrap()
        .execute(
            "INSERT INTO address_maad_stats VALUES ('r1','5m',0,300,6,'all','all','source','addresses',0,NULL,NULL)",
            [],
        )
        .unwrap();

    let (success, report) = run_compare(&candidate, &reference);
    assert!(!success, "{report}");
    assert_eq!(
        report["tables"]["address_maad_stats"]["unexpected_candidate_only_rows"],
        1
    );
}

#[test]
fn compare_rejects_ipv6_maad_rows_for_an_unrelated_source() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);
    Connection::open(&candidate)
        .unwrap()
        .execute(
            "INSERT INTO address_maad_stats VALUES ('r2','5m',0,300,6,'all','all','source','addresses',0,NULL,NULL)",
            [],
        )
        .unwrap();

    let (success, report) = run_compare(&candidate, &reference);
    assert!(!success, "{report}");
    assert_eq!(
        report["tables"]["address_maad_stats"]["unexpected_candidate_only_rows"],
        1
    );
}

#[test]
fn compare_skips_reference_internal_side_maad_the_candidate_does_not_compute() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);
    Connection::open(&candidate)
        .unwrap()
        .execute_batch(
            "CREATE TABLE datasets (id TEXT PRIMARY KEY, maad_internal_side INTEGER NOT NULL);
             INSERT INTO datasets VALUES ('d1', 0);",
        )
        .unwrap();
    let internal_side_row = "INSERT INTO address_maad_stats VALUES ('r1','5m',0,300,4,'internal','external','source','addresses',0,NULL,NULL)";
    Connection::open(&reference)
        .unwrap()
        .execute_batch(&format!(
            "{internal_side_row};
             INSERT INTO address_maad_stats VALUES ('r1','5m',0,300,4,'internal','external','destination','addresses',0,NULL,NULL);"
        ))
        .unwrap();

    let (success, report) = run_compare(&candidate, &reference);
    assert!(!success, "{report}");
    assert_eq!(
        report["tables"]["address_maad_stats"]["skipped_reference_rows"],
        1
    );
    assert_eq!(
        report["tables"]["address_maad_stats"]["reference_only_rows"],
        1
    );

    Connection::open(&candidate)
        .unwrap()
        .execute(
            "INSERT INTO address_maad_stats VALUES ('r1','5m',0,300,4,'internal','external','destination','addresses',0,NULL,NULL)",
            [],
        )
        .unwrap();
    let (success, report) = run_compare(&candidate, &reference);
    assert!(success, "{report}");
    assert_eq!(
        report["tables"]["address_maad_stats"]["skipped_reference_rows"],
        1
    );

    Connection::open(&candidate)
        .unwrap()
        .execute(internal_side_row, [])
        .unwrap();
    let (success, report) = run_compare(&candidate, &reference);
    assert!(!success, "{report}");
    assert_eq!(
        report["tables"]["address_maad_stats"]["unexpected_candidate_only_rows"],
        1
    );
}

fn run_compare(
    candidate: &std::path::Path,
    reference: &std::path::Path,
) -> (bool, serde_json::Value) {
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
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    (output.status.success(), report)
}

#[test]
fn compare_accepts_a_dense_zero_scope_missing_from_the_reference() {
    let temporary = tempdir().unwrap();
    let candidate = temporary.path().join("candidate.sqlite");
    let reference = temporary.path().join("reference.sqlite");
    create_shared_database(&candidate, 42, 0.5, &[0.5], false);
    create_shared_database(&reference, 42, 0.5, &[0.5], false);
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

fn f32_blob(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn create_shared_database(
    path: &std::path::Path,
    flows: i64,
    dimension: f64,
    tau: &[f32],
    extra: bool,
) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE traffic_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_locality TEXT NOT NULL,
                dst_locality TEXT NOT NULL, flows INTEGER NOT NULL
            );
            CREATE TABLE protocol_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_locality TEXT NOT NULL,
                dst_locality TEXT NOT NULL, unique_protocols_count INTEGER NOT NULL,
                protocols_list TEXT NOT NULL
            );
            CREATE TABLE address_count_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_locality TEXT NOT NULL,
                dst_locality TEXT NOT NULL, address_side TEXT NOT NULL,
                unique_address_count INTEGER NOT NULL
            );
            CREATE TABLE address_maad_stats (
                source_id TEXT NOT NULL, granularity TEXT NOT NULL,
                bucket_start INTEGER NOT NULL, bucket_end INTEGER NOT NULL,
                ip_version INTEGER NOT NULL, src_locality TEXT NOT NULL,
                dst_locality TEXT NOT NULL, address_side TEXT NOT NULL,
                measure TEXT NOT NULL, total_addrs INTEGER NOT NULL, d1 REAL, tau BLOB
            );
            CREATE TABLE maad_q_grid (
                ip_version INTEGER PRIMARY KEY, q_min REAL NOT NULL,
                q_step REAL NOT NULL, q_count INTEGER NOT NULL
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
            "INSERT INTO address_maad_stats VALUES ('r1','5m',0,300,4,'all','all','source','addresses',2,?1,?2)",
            rusqlite::params![dimension, f32_blob(tau)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO maad_q_grid VALUES (4, -0.5, 0.125, ?1)",
            [tau.len() as i64],
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
            "INSERT INTO address_maad_stats VALUES ('r1','30m',3600,5400,4,'all','all','source','addresses',0,NULL,NULL)",
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
                "INSERT INTO address_maad_stats VALUES ('r1','30m',0,1800,4,'all','all','source','addresses',0,NULL,NULL)",
                [],
            )
            .unwrap();
    }
}
