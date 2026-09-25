#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::tempdir;

const FAKE_SSH: &str = r#"#!/usr/bin/env bash
while [ "$1" != "--" ]; do shift; done
shift 2
if [ "$*" = "bash -s" ]; then
  script=$(cat)
  case "$script" in
    *setsid*) echo started ;;
    *.status*) echo "exit 0" ;;
  esac
fi
"#;

const FAKE_SCP: &str = r#"#!/usr/bin/env bash
while [ "$1" != "--" ]; do shift; done
shift
case "$1" in
  *:*) : > "$2" ;;
esac
"#;

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn merge_invocation(extra: &[&str]) -> String {
    let temporary = tempdir().unwrap();
    let bin = temporary.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_executable(&bin.join("ssh"), FAKE_SSH);
    write_executable(&bin.join("scp"), FAKE_SCP);
    let log = temporary.path().join("netflow-db.log");
    let netflow_db = temporary.path().join("netflow-db");
    write_executable(
        &netflow_db,
        &format!("#!/usr/bin/env bash\necho \"$*\" >> '{}'\n", log.display()),
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/netflow-db-cluster.sh")
        .canonicalize()
        .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(&script)
        .args(["--hosts", "nodeA,nodeB", "--dataset", "example"])
        .args(["--start-date", "2025-06-01", "--end-date", "2025-06-02"])
        .arg("--output")
        .arg(temporary.path().join("out.sqlite"))
        .args(["--remote-dir", "/remote/atlantis-cluster"])
        .arg("--netflow-db")
        .arg(&netflow_db)
        .args(extra)
        .env("PATH", path)
        .env("NETFLOW_CLUSTER_POLL_SECONDS", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "launcher failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(&log)
        .unwrap()
        .lines()
        .find(|line| line.starts_with("merge-shards --output"))
        .expect("launcher ran merge-shards")
        .to_owned()
}

#[test]
fn launcher_consumes_shards_by_default() {
    let merge = merge_invocation(&[]);
    assert!(merge.contains(" --consume "), "{merge}");
    assert!(
        merge.contains("example.2025-06-01_2025-06-01.sqlite")
            && merge.contains("example.2025-06-02_2025-06-02.sqlite"),
        "{merge}"
    );
}

#[test]
fn keep_shards_disables_consuming_merge() {
    let merge = merge_invocation(&["--keep-shards"]);
    assert!(!merge.contains("--consume"), "{merge}");
}
