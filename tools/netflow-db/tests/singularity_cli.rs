use std::{fs, process::Command};

const EPSILON: f64 = 1e-9;
const FIXTURE: &str = "0.0.0.0\n32.0.0.0\n128.0.0.0\n192.0.0.0\n";

#[test]
fn singularity_file_input_emits_scores_as_csv() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("addresses.txt");
    fs::write(&input, FIXTURE).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_netflow-db"))
        .args(["singularity"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("addr,alpha,intercept,r2,n_levels"));

    let rows: Vec<_> = lines.collect();
    assert_eq!(rows.len(), 4);
    let first: Vec<_> = rows[0].split(',').collect();
    assert_eq!(first.len(), 5);
    assert_eq!(first[0], "0.0.0.0");
    assert_close(first[1].parse().unwrap(), 0.5);
    assert_close(first[2].parse().unwrap(), 1.0 / 6.0);
    assert_close(first[3].parse().unwrap(), 0.75);
    assert_eq!(first[4], "3");
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "actual={actual:?}, expected={expected:?}"
    );
}
