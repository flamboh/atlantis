use std::{fs, path::Path, process::Command};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use rusqlite::Connection;
use tempfile::tempdir;

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
                "locality": [{"type": "tos_anonymized"}],
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
                "locality": [{"type": "tos_anonymized"}],
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

#[test]
fn csv_pipeline_stores_locality_pairs_and_binds_rules_to_product_identity() {
    let temporary = tempdir().unwrap();
    let csv = temporary.path().join("flows.csv");
    let mapping = temporary.path().join("mapping.json");
    let addresses = temporary.path().join("internal.txt");
    let database = temporary.path().join("csv.sqlite");
    fs::write(
        &csv,
        concat!(
            "received,src,dst\n",
            "0,192.0.2.1,203.0.113.1\n",
            "1,203.0.113.1,192.0.2.1\n",
            "2,192.0.2.1,198.51.100.53\n",
            "3,203.0.113.1,203.0.113.2\n",
            "4,2001:db8::1,2001:db8:ffff::1\n",
        ),
    )
    .unwrap();
    fs::write(&addresses, "198.51.100.53\n").unwrap();
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
            "locality": [
                {"type": "prefixes", "prefixes": ["192.0.2.0/24", "2001:db8::/48"]},
                {"type": "addresses", "path": addresses}
            ],
            "inputs": [{
                "input_kind": "csv",
                "path": csv,
                "mapping_path": mapping
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_netflow-db"))
            .args([
                "pipeline",
                "--config",
                config.to_str().unwrap(),
                "--no-maad",
            ])
            .output()
            .unwrap()
    };

    let output = run();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let connection = Connection::open(&database).unwrap();
    let flows = |ip_version: i64, src: &str, dst: &str| -> i64 {
        connection
            .query_row(
                "SELECT flows FROM traffic_stats WHERE granularity = '5m' AND ip_version = ?1 \
                 AND src_locality = ?2 AND dst_locality = ?3",
                rusqlite::params![ip_version, src, dst],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(flows(4, "all", "all"), 4);
    assert_eq!(flows(4, "internal", "external"), 1, "egress");
    assert_eq!(flows(4, "external", "internal"), 1, "ingress");
    assert_eq!(flows(4, "internal", "internal"), 1, "lateral");
    assert_eq!(flows(4, "external", "external"), 1, "transit");
    assert_eq!(flows(6, "internal", "external"), 1);
    for granularity in ["10m", "30m", "1h", "1d"] {
        let (exact, all): (i64, i64) = connection
            .query_row(
                "SELECT SUM(CASE WHEN src_locality <> 'all' THEN flows ELSE 0 END), \
                 SUM(CASE WHEN src_locality = 'all' THEN flows ELSE 0 END) \
                 FROM traffic_stats WHERE granularity = ?1",
                [granularity],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (exact, all),
            (5, 5),
            "{granularity} rollup keeps exact locality pairs additive"
        );
    }
    let config_json: String = connection
        .query_row(
            "SELECT config_json FROM pipeline_product WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(config_json.contains("\"locality\""), "{config_json}");
    assert!(
        !config_json.contains("198.51.100.53") && !config_json.contains("192.0.2.0/24"),
        "{config_json}"
    );
    drop(connection);

    fs::write(&addresses, "198.51.100.53\n198.51.100.54\n").unwrap();
    let output = run();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config") && stderr.contains("identity"),
        "stderr={stderr}"
    );
}

#[test]
fn config_pipeline_resolves_locality_files_from_config_directory_and_marks_datasets() {
    let temporary = tempdir().unwrap();
    let config_directory = temporary.path().join("config");
    let working_directory = temporary.path().join("elsewhere");
    fs::create_dir_all(&config_directory).unwrap();
    fs::create_dir_all(&working_directory).unwrap();
    let capture_root = temporary.path().join("captures");
    fs::create_dir_all(capture_root.join("edge")).unwrap();
    let database = temporary.path().join("native.sqlite");
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
    fs::write(config_directory.join("internal.txt"), "198.51.100.53\n").unwrap();
    let config = config_directory.join("pipeline.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "run_maad": false,
            "nfdump": nfdump,
            "locality": [{"type": "addresses", "path": "internal.txt"}],
            "inputs": [{
                "input_kind": "nfcapd_tree",
                "root_path": capture_root,
                "source_ids": ["edge"],
                "start_date": "2025-01-01",
                "end_date": "2025-01-01"
            }],
            "datasets": [{
                "dataset_id": "edge",
                "root_path": capture_root,
                "source_ids": ["edge"]
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
        .current_dir(&working_directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let connection = Connection::open(&database).unwrap();
    let has_locality: i64 = connection
        .query_row(
            "SELECT has_locality FROM datasets WHERE id = 'edge'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_locality, 1);
}

#[test]
fn csv_pipeline_drops_and_logs_zero_packet_flows_but_keeps_coverage() {
    let temporary = tempdir().unwrap();
    let csv = temporary.path().join("flows.csv");
    let mapping = temporary.path().join("mapping.json");
    let database = temporary.path().join("zero-packets.sqlite");
    fs::write(
        &csv,
        concat!(
            "received,src,dst,packets,bytes\n",
            "0,192.0.2.1,198.51.100.1,4,400\n",
            "10,192.0.2.1,198.51.100.9,0,0\n",
            "300,192.0.2.3,198.51.100.3,0,40\n",
        ),
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
                "bytes": "bytes"
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
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr={stderr}");
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.contains("dropped zero-packet flows")
                && line.contains("dropped_zero_packet_flows=1"))
            .count(),
        2,
        "stderr={stderr}"
    );

    let connection = Connection::open(&database).unwrap();
    let rows = connection
        .prepare(
            "SELECT t.bucket_start, t.flows, t.packets, c.observed_units
             FROM traffic_stats t JOIN bucket_coverage c
               ON c.source_id = t.source_id AND c.granularity = t.granularity
              AND c.bucket_start = t.bucket_start
             WHERE t.granularity = '5m' AND t.ip_version = 4
               AND t.src_locality = 'all' AND t.dst_locality = 'all'
             ORDER BY t.bucket_start",
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
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows, [(0, 1, 4, 1), (300, 0, 0, 1)]);
}

#[test]
fn csv_pipeline_skips_internal_side_maad_unless_the_config_opts_in() {
    let temporary = tempdir().unwrap();
    let csv = temporary.path().join("flows.csv");
    let mapping = temporary.path().join("mapping.json");
    fs::write(
        &csv,
        "received,src,dst\n0,192.0.2.1,203.0.113.1\n1,203.0.113.1,192.0.2.2\n",
    )
    .unwrap();
    fs::write(
        &mapping,
        serde_json::to_vec(&serde_json::json!({
            "timestamp_format": "unix",
            "timestamp_timezone": "UTC",
            "columns": {"time_received": "received", "src_ip": "src", "dst_ip": "dst"},
            "source_id": {"value": "edge"}
        }))
        .unwrap(),
    )
    .unwrap();
    let netflow_db = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_netflow-db"))
            .args(args)
            .output()
            .unwrap()
    };
    let build = |database: &Path, settings: serde_json::Value| {
        let mut config = serde_json::json!({
            "database_path": database,
            "timezone": "UTC",
            "locality": [{"type": "prefixes", "prefixes": ["192.0.2.0/24"]}],
            "inputs": [{"input_kind": "csv", "path": csv, "mapping_path": mapping}],
            "datasets": [{"dataset_id": "edge", "root_path": temporary.path()}]
        });
        for (key, value) in settings.as_object().unwrap() {
            config[key] = value.clone();
        }
        let path = database.with_extension("json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        netflow_db(&["pipeline", "--config", path.to_str().unwrap()])
    };
    let verified = |database: &Path, flags: &[&str]| {
        let mut args = vec!["verify", database.to_str().unwrap(), "--require-data"];
        args.extend(flags);
        netflow_db(&args).status.success()
    };
    let scopes = |database: &Path| -> Vec<String> {
        Connection::open(database)
            .unwrap()
            .prepare(
                "SELECT DISTINCT src_locality || '->' || dst_locality || ':' || address_side
                 FROM address_maad_stats WHERE ip_version = 4 AND measure = 'bytes' ORDER BY 1",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };

    let skipped = temporary.path().join("skipped.sqlite");
    assert!(build(&skipped, serde_json::json!({})).status.success());
    assert_eq!(
        scopes(&skipped),
        [
            "all->all:destination",
            "all->all:source",
            "external->external:destination",
            "external->external:source",
            "external->internal:source",
            "internal->external:destination",
        ]
    );
    let config_json: String = Connection::open(&skipped)
        .unwrap()
        .query_row("SELECT config_json FROM pipeline_product", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        config_json.contains("\"internal_side\":false"),
        "{config_json}"
    );
    assert!(verified(&skipped, &["--require-maad-data"]));
    let output = build(&skipped, serde_json::json!({"maad_internal_side": true}));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity"));

    let computed = temporary.path().join("computed.sqlite");
    assert!(
        build(&computed, serde_json::json!({"maad_internal_side": true}))
            .status
            .success()
    );
    assert_eq!(scopes(&computed).len(), 10);
    assert!(verified(&computed, &["--require-maad-data"]));

    let disabled = temporary.path().join("disabled.sqlite");
    assert!(
        build(&disabled, serde_json::json!({"run_maad": false}))
            .status
            .success()
    );
    assert!(verified(&disabled, &[]));
}
