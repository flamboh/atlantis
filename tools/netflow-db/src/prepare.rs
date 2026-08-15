//! Prepare immutable archives into the canonical nfcapd tree layout.

use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::{Path, PathBuf},
    process::Command,
};

use flate2::read::GzDecoder;
use jiff::{Timestamp, civil::DateTime};
use tar::Archive;
use tempfile::tempdir_in;
use thiserror::Error;

use crate::registry::is_safe_path_component;

#[derive(Debug, Error)]
pub enum PrepareError {
    #[error("filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("nfdump command failed: {0}")]
    Nfdump(String),
    #[error("invalid nfcapd data: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug)]
pub struct PrepareOptions {
    pub archive: PathBuf,
    pub dataset_root: PathBuf,
    pub source_id: String,
    pub nfdump: String,
    pub timezone: String,
    pub interval_seconds: i64,
    pub max_buckets: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrepareStats {
    pub written: usize,
    pub skipped_existing: usize,
    pub members: usize,
}

pub fn prepare_archive(options: &PrepareOptions) -> Result<PrepareStats, PrepareError> {
    if !is_safe_path_component(&options.source_id) {
        return Err(PrepareError::Invalid(format!(
            "source ID {:?} must be exactly one normal path component",
            options.source_id
        )));
    }
    if options.interval_seconds <= 0 {
        return Err(PrepareError::Invalid(
            "interval_seconds must be positive".into(),
        ));
    }
    if !options.archive.is_file() {
        return Err(PrepareError::Invalid(format!(
            "archive not found: {}",
            options.archive.display()
        )));
    }
    let parent = options.dataset_root.parent().ok_or_else(|| {
        PrepareError::Invalid(format!(
            "dataset root has no parent: {}",
            options.dataset_root.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let temporary = tempdir_in(parent)?;
    let file = File::open(&options.archive)?;
    let input: Box<dyn io::Read> = match options.archive.file_name().and_then(|name| name.to_str())
    {
        Some(name) if name.ends_with(".gz") || name.ends_with(".tgz") => {
            Box::new(GzDecoder::new(BufReader::new(file)))
        }
        _ => Box::new(BufReader::new(file)),
    };
    let mut archive = Archive::new(input);
    let mut stats = PrepareStats::default();
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        stats.members += 1;
        let name = entry
            .path()?
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| PrepareError::Invalid("archive member has no UTF-8 filename".into()))?
            .to_owned();
        let extracted = temporary.path().join(format!("{index}-{name}"));
        entry.unpack(&extracted)?;
        if let Some(bucket_start) = parse_nfcapd_name(&name, &options.timezone)? {
            let output = canonical_bucket_path(
                &options.dataset_root,
                &options.source_id,
                bucket_start,
                &options.timezone,
            )?;
            if publish_file(&extracted, &output)? {
                stats.written += 1;
            } else {
                stats.skipped_existing += 1;
            }
        } else {
            let segmented = segment_file(&extracted, options)?;
            stats.written += segmented.written;
            stats.skipped_existing += segmented.skipped_existing;
        }
    }
    if stats.members == 0 {
        return Err(PrepareError::Invalid(format!(
            "no regular files found in {}",
            options.archive.display()
        )));
    }
    Ok(stats)
}

fn segment_file(
    source_file: &Path,
    options: &PrepareOptions,
) -> Result<PrepareStats, PrepareError> {
    let (first, last) = parse_nfdump_summary(&options.nfdump, source_file)?;
    let first = first.div_euclid(options.interval_seconds) * options.interval_seconds;
    let last = last.div_euclid(options.interval_seconds) * options.interval_seconds;
    let mut stats = PrepareStats::default();
    let mut bucket_start = first;
    while bucket_start <= last
        && options
            .max_buckets
            .is_none_or(|limit| stats.written + stats.skipped_existing < limit)
    {
        let output = canonical_bucket_path(
            &options.dataset_root,
            &options.source_id,
            bucket_start,
            &options.timezone,
        )?;
        if output.exists() {
            stats.skipped_existing += 1;
        } else {
            let end = bucket_start + options.interval_seconds - 1;
            let time_range = format!(
                "{}-{}",
                format_nfdump_time(bucket_start, &options.timezone)?,
                format_nfdump_time(end, &options.timezone)?
            );
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            let staging = tempdir_in(output.parent().expect("output parent created above"))?;
            let staged_output = staging.path().join("nfcapd.bucket");
            checked_command(
                Command::new(&options.nfdump)
                    .args(["-r"])
                    .arg(source_file)
                    .args(["-t", &time_range, "-w"])
                    .arg(&staged_output),
            )?;
            if publish_file(&staged_output, &output)? {
                stats.written += 1;
            } else {
                stats.skipped_existing += 1;
            }
        }
        bucket_start = bucket_start
            .checked_add(options.interval_seconds)
            .ok_or_else(|| PrepareError::Invalid("bucket timestamp overflow".into()))?;
    }
    Ok(stats)
}

pub fn parse_nfdump_summary(nfdump: &str, file: &Path) -> Result<(i64, i64), PrepareError> {
    let output = checked_command(Command::new(nfdump).args(["-I", "-r"]).arg(file))?;
    let stdout = String::from_utf8_lossy(&output);
    let mut first = None;
    let mut last = None;
    for line in stdout.lines() {
        if let Some(raw) = line.strip_prefix("First:") {
            first = raw.trim().parse().ok();
        } else if let Some(raw) = line.strip_prefix("Last:") {
            last = raw.trim().parse().ok();
        }
    }
    first.zip(last).ok_or_else(|| {
        PrepareError::Invalid(format!(
            "could not parse first/last timestamps from {}",
            file.display()
        ))
    })
}

pub fn canonical_bucket_path(
    root: &Path,
    source_id: &str,
    bucket_start: i64,
    timezone: &str,
) -> Result<PathBuf, PrepareError> {
    if !is_safe_path_component(source_id) {
        return Err(PrepareError::Invalid(format!(
            "source ID {source_id:?} must be exactly one normal path component"
        )));
    }
    let zoned = Timestamp::new(bucket_start, 0)
        .and_then(|timestamp| timestamp.in_tz(timezone))
        .map_err(|error| PrepareError::Invalid(error.to_string()))?;
    let day = zoned.strftime("%Y/%m/%d").to_string();
    let name = zoned.strftime("nfcapd.%Y%m%d%H%M").to_string();
    Ok(root.join(source_id).join(day).join(name))
}

fn parse_nfcapd_name(name: &str, timezone: &str) -> Result<Option<i64>, PrepareError> {
    let Some(raw) = name.strip_prefix("nfcapd.") else {
        return Ok(None);
    };
    if raw.len() != 12 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let datetime = DateTime::strptime("%Y%m%d%H%M", raw)
        .and_then(|datetime| datetime.in_tz(timezone))
        .map_err(|error| PrepareError::Invalid(error.to_string()))?;
    Ok(Some(datetime.timestamp().as_second()))
}

fn format_nfdump_time(timestamp: i64, timezone: &str) -> Result<String, PrepareError> {
    Timestamp::new(timestamp, 0)
        .and_then(|timestamp| timestamp.in_tz(timezone))
        .map(|zoned| zoned.strftime("%Y/%m/%d.%H:%M:%S").to_string())
        .map_err(|error| PrepareError::Invalid(error.to_string()))
}

fn checked_command(command: &mut Command) -> Result<Vec<u8>, PrepareError> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(PrepareError::Nfdump(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(output.stdout)
}

fn publish_file(source: &Path, output: &Path) -> Result<bool, PrepareError> {
    if output.exists() {
        return Ok(false);
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::hard_link(source, output) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) if error.raw_os_error() == Some(18) => {
            let staging = tempdir_in(output.parent().expect("output parent created above"))?;
            let copied = staging.path().join("nfcapd.bucket");
            fs::copy(source, &copied)?;
            File::open(&copied)?.sync_all()?;
            match fs::hard_link(copied, output) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_use_the_configured_local_timezone() {
        let path = canonical_bucket_path(
            Path::new("/dataset"),
            "router-a",
            1_744_733_100,
            "America/Los_Angeles",
        )
        .unwrap();
        assert_eq!(
            path,
            Path::new("/dataset/router-a/2025/04/15/nfcapd.202504150905")
        );
    }

    #[test]
    fn canonical_paths_reject_source_ids_that_escape_the_dataset_root() {
        for source in [
            "..",
            "../outside",
            "/outside",
            "nested/source",
            "nested\\source",
        ] {
            assert!(
                canonical_bucket_path(Path::new("/dataset"), source, 0, "UTC")
                    .unwrap_err()
                    .to_string()
                    .contains("one normal path component")
            );
        }
    }

    #[test]
    fn only_exact_nfcapd_names_are_recognized() {
        assert!(parse_nfcapd_name("capture.bin", "UTC").unwrap().is_none());
        assert!(
            parse_nfcapd_name("nfcapd.202504151300", "UTC")
                .unwrap()
                .is_some()
        );
        assert!(
            parse_nfcapd_name("nfcapd.20250415", "UTC")
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_segment_command_never_leaves_a_canonical_bucket() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("capture.nfcapd");
        let nfdump = temporary.path().join("fake-nfdump");
        let root = temporary.path().join("dataset");
        fs::write(&source, "capture").unwrap();
        let mut script = File::create(&nfdump).unwrap();
        writeln!(script, "#!/bin/sh").unwrap();
        writeln!(
            script,
            "case \"$1\" in -I) printf 'First: 0\\nLast: 0\\n'; exit 0;; esac"
        )
        .unwrap();
        writeln!(script, "for last; do :; done").unwrap();
        writeln!(script, "printf partial > \"$last\"").unwrap();
        writeln!(script, "exit 9").unwrap();
        drop(script);
        fs::set_permissions(&nfdump, fs::Permissions::from_mode(0o755)).unwrap();
        let options = PrepareOptions {
            archive: temporary.path().join("unused.tar"),
            dataset_root: root.clone(),
            source_id: "r1".into(),
            nfdump: nfdump.to_string_lossy().into_owned(),
            timezone: "UTC".into(),
            interval_seconds: 300,
            max_buckets: None,
        };

        assert!(segment_file(&source, &options).is_err());
        assert!(
            !canonical_bucket_path(&root, "r1", 0, "UTC")
                .unwrap()
                .exists()
        );
    }
}
