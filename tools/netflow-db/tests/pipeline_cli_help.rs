use std::{fs, process::Command};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn pipeline_help_explains_repeated_dataset_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args(["pipeline", "--help"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(
        help.contains("Repeat --dataset for two or more values"),
        "{help}"
    );
    assert!(
        help.contains("coordinated fixed daily-active subset run"),
        "{help}"
    );
    assert!(
        help.contains("one value keeps the normal single-dataset path"),
        "{help}"
    );
}

#[test]
fn pipeline_repeated_dataset_uses_isolated_registry_and_outputs() {
    let temporary = tempdir().unwrap();
    let capture_root = temporary.path().join("captures");
    fs::create_dir_all(capture_root.join("shared")).unwrap();
    let registry_path = temporary.path().join("registry.json");
    let first_database = temporary.path().join("first.sqlite");
    let second_database = temporary.path().join("second.sqlite");
    let nfdump = temporary.path().join("nfdump");
    let empty_stream = temporary.path().join("empty.stream");
    fs::write(
        &empty_stream,
        [65_u8, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0],
    )
    .unwrap();
    fs::write(
        &nfdump,
        format!("#!/bin/sh\ncat '{}'\n", empty_stream.display()),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&nfdump, fs::Permissions::from_mode(0o755)).unwrap();
    let registry = serde_json::json!({
        "datasets": [
            {
                "dataset_id": "first",
                "root_path": capture_root,
                "db_path": first_database,
                "source_ids": ["shared"],
                "selection": {
                    "kind": "daily_active_sources",
                    "ip_prefix": "10.0.0.0/16"
                }
            },
            {
                "dataset_id": "second",
                "root_path": capture_root,
                "db_path": second_database,
                "source_ids": ["shared"],
                "selection": {
                    "kind": "daily_active_sources",
                    "ip_prefix": "10.0.0.0/16"
                }
            }
        ]
    });
    fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "pipeline",
            "--dataset",
            "first",
            "--dataset",
            "second",
            "--start-date",
            "2025-01-01",
            "--end-date",
            "2025-01-02",
            "--datasets",
            registry_path.to_str().unwrap(),
            "--no-maad",
            "--nfdump",
            nfdump.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for database in [&first_database, &second_database] {
        assert!(
            database.is_file(),
            "missing coordinated output {database:?}"
        );
        let connection = Connection::open(database).unwrap();
        let selection: String = connection
            .query_row(
                "SELECT selection_json FROM pipeline_product WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(selection.contains("daily_active_sources"), "{selection}");
    }
}

#[cfg(unix)]
#[test]
fn pipeline_rejects_an_inaccessible_path_candidate_before_output_setup() {
    let temporary = tempdir().unwrap();
    let first_path = temporary.path().join("first-path");
    let second_path = temporary.path().join("second-path");
    fs::create_dir_all(&first_path).unwrap();
    fs::create_dir_all(&second_path).unwrap();

    // The owner has no execute permission, while another class does. Checking mode bits alone
    // incorrectly treats this as the first PATH candidate for the current process.
    let inaccessible = first_path.join("nfdump");
    fs::write(&inaccessible, b"inaccessible candidate").unwrap();
    fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o001)).unwrap();

    let database = temporary.path().join("pipeline.sqlite");
    let sidecar = database.with_file_name("pipeline.sqlite-wal");
    let sentinel = b"do not overwrite this executable-related sentinel";
    fs::write(&sidecar, sentinel).unwrap();
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&sidecar, second_path.join("nfdump")).unwrap();

    let capture_root = temporary.path().join("captures");
    fs::create_dir_all(capture_root.join("edge")).unwrap();
    let config = temporary.path().join("pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "run_maad": false,
            "nfdump": "nfdump",
            "inputs": [{
                "input_kind": "nfcapd_tree",
                "root_path": capture_root,
                "source_ids": ["edge"],
                "start_date": "2025-01-01",
                "end_date": "2025-01-02"
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let path = std::env::join_paths([&first_path, &second_path]).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "pipeline",
            "--config",
            config.to_str().unwrap(),
            "--no-maad",
        ])
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(!output.status.success(), "pipeline unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aliases pipeline control path"),
        "stderr={stderr}"
    );
    assert_eq!(fs::read(&sidecar).unwrap(), sentinel);
    assert!(
        !database.exists(),
        "output database was created after rejection"
    );
    assert!(
        !temporary
            .path()
            .join(".pipeline.sqlite.operation.lock")
            .exists(),
        "output operation lock was created after rejection"
    );
}

#[test]
fn csv_pipeline_does_not_require_nfdump_from_path() {
    let temporary = tempdir().unwrap();
    let empty_path = temporary.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let csv = temporary.path().join("flows.csv");
    let mapping = temporary.path().join("mapping.json");
    let database = temporary.path().join("csv.sqlite");
    fs::write(&csv, "received,src,dst\n0,192.0.2.1,198.51.100.1\n").unwrap();
    fs::write(
        &mapping,
        serde_json::to_vec(&serde_json::json!({
            "timestamp_format": "unix",
            "timestamp_timezone": "UTC",
            "columns": {
                "time_received": "received",
                "src_ip": "src",
                "dst_ip": "dst"
            },
            "source_id": {"value": "edge"}
        }))
        .unwrap(),
    )
    .unwrap();
    let config = temporary.path().join("csv-pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "run_maad": false,
            "inputs": [{
                "input_kind": "csv",
                "path": csv,
                "mapping_path": mapping
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "pipeline",
            "--config",
            config.to_str().unwrap(),
            "--no-maad",
        ])
        .env("PATH", empty_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Published five-minute buckets: 1\n"),
        "stdout={stdout}"
    );
    assert!(database.is_file());
}

#[test]
fn native_pipeline_requires_nfdump_before_output_setup() {
    let temporary = tempdir().unwrap();
    let empty_path = temporary.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let capture_root = temporary.path().join("captures");
    fs::create_dir_all(capture_root.join("edge")).unwrap();
    let database = temporary.path().join("native.sqlite");
    let config = temporary.path().join("native-pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "run_maad": false,
            "inputs": [{
                "input_kind": "nfcapd_tree",
                "root_path": capture_root,
                "source_ids": ["edge"],
                "start_date": "2025-01-01",
                "end_date": "2025-01-02"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args([
            "pipeline",
            "--config",
            config.to_str().unwrap(),
            "--no-maad",
        ])
        .env("PATH", empty_path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot resolve bare nfdump executable"),
        "stderr={stderr}"
    );
    assert!(!database.exists());
}
