//! Exact, canonical input revisions used to make pipeline retries safe.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::CsvSourceConfig;

pub const CSV_DECODER_VERSION: u32 = 1;
pub const NFCAPD_DECODER_VERSION: u32 = 3;
pub const GAP_DECODER_VERSION: u32 = 1;
pub const NFDUMP_REDUCER_CONTRACT_VERSION: u32 = 1;
pub const NFDUMP_REDUCER_INPUT_CONTRACT: &str = "nfdump-csv-15-v1";
pub const NFDUMP_REDUCER_OUTPUT_CONTRACT: &str = "canonical-scopes-v1";

#[derive(Debug, Error)]
pub enum ProvenanceError {
    #[error("failed to serialize canonical JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    InputContentChanged(String),
    #[error("file timestamp cannot be represented as nanoseconds: {0}")]
    TimestampOverflow(PathBuf),
}

/// Encode a serializable value exactly like Python's canonical pipeline JSON.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<String, ProvenanceError> {
    let value = serde_json::to_value(value)?;
    let mut output = String::new();
    write_canonical_value(&value, &mut output);
    Ok(output)
}

/// Return the SHA-256 digest of a value's canonical JSON representation.
pub fn fingerprint<T: Serialize + ?Sized>(value: &T) -> Result<String, ProvenanceError> {
    Ok(hex_digest(canonical_json(value)?.as_bytes()))
}

fn write_canonical_value(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => write_python_ascii_string(value, output),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_canonical_value(value, output);
            }
            output.push(']');
        }
        Value::Object(values) => write_canonical_object(values, output),
    }
}

fn write_canonical_object(values: &Map<String, Value>, output: &mut String) {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(key, _)| *key);
    output.push('{');
    for (index, (key, value)) in entries.into_iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        write_python_ascii_string(key, output);
        output.push(':');
        write_canonical_value(value, output);
    }
    output.push('}');
}

fn write_python_ascii_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{000c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            ' '..='~' => output.push(character),
            character if (character as u32) <= 0xffff => {
                use std::fmt::Write;
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => {
                use std::fmt::Write;
                let scalar = character as u32 - 0x1_0000;
                let high = 0xd800 + (scalar >> 10);
                let low = 0xdc00 + (scalar & 0x3ff);
                write!(output, "\\u{high:04x}\\u{low:04x}")
                    .expect("writing to a String cannot fail");
            }
        }
    }
    output.push('"');
}

/// Hash a file exactly while using bounded memory.
pub fn file_sha256(path: impl AsRef<Path>) -> Result<String, ProvenanceError> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|source| ProvenanceError::Io {
        context: format!("failed to open input for hashing: {}", path.display()),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| ProvenanceError::Io {
                context: format!("failed to hash input: {}", path.display()),
                source,
            })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Cheap file identity captured alongside an exact content digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

impl FileSnapshot {
    pub fn capture(path: impl AsRef<Path>) -> Result<Self, ProvenanceError> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).map_err(|source| ProvenanceError::Io {
            context: format!("failed to stat input: {}", path.display()),
            source,
        })?;
        snapshot_from_metadata(path, &metadata)
    }
}

#[cfg(unix)]
fn snapshot_from_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<FileSnapshot, ProvenanceError> {
    use std::os::unix::fs::MetadataExt;

    let mtime_ns = timestamp_ns(metadata.mtime(), metadata.mtime_nsec(), path)?;
    let ctime_ns = timestamp_ns(metadata.ctime(), metadata.ctime_nsec(), path)?;
    Ok(FileSnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        mtime_ns,
        ctime_ns,
    })
}

#[cfg(not(unix))]
fn snapshot_from_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<FileSnapshot, ProvenanceError> {
    use std::time::UNIX_EPOCH;

    let modified = metadata
        .modified()
        .map_err(|source| ProvenanceError::Io {
            context: format!("failed to read input timestamp: {}", path.display()),
            source,
        })?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProvenanceError::TimestampOverflow(path.to_path_buf()))?;
    let mtime_ns = i64::try_from(modified.as_nanos())
        .map_err(|_| ProvenanceError::TimestampOverflow(path.to_path_buf()))?;
    Ok(FileSnapshot {
        device: 0,
        inode: 0,
        size: metadata.len(),
        mtime_ns,
        ctime_ns: mtime_ns,
    })
}

fn timestamp_ns(seconds: i64, nanoseconds: i64, path: &Path) -> Result<i64, ProvenanceError> {
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanoseconds))
        .ok_or_else(|| ProvenanceError::TimestampOverflow(path.to_path_buf()))
}

/// Hash a file only if its inexpensive identity remains stable throughout hashing.
pub fn capture_file_revision(
    path: impl AsRef<Path>,
) -> Result<(String, FileSnapshot), ProvenanceError> {
    let path = path.as_ref();
    let before = FileSnapshot::capture(path)?;
    let content_fingerprint = file_sha256(path)?;
    let after = FileSnapshot::capture(path)?;
    if before != after {
        return Err(ProvenanceError::InputContentChanged(format!(
            "Input changed while its revision was being captured: {:?}",
            path
        )));
    }
    Ok((content_fingerprint, after))
}

/// Verify that a file still has the identity captured after exact hashing.
pub fn verify_file_snapshot(
    path: impl AsRef<Path>,
    expected: &FileSnapshot,
) -> Result<(), ProvenanceError> {
    let path = path.as_ref();
    if &FileSnapshot::capture(path)? != expected {
        return Err(ProvenanceError::InputContentChanged(format!(
            "Input changed while it was being decoded: {:?}",
            path
        )));
    }
    Ok(())
}

/// A path whose continued absence is required before publishing a synthetic gap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedAbsence {
    path: PathBuf,
}

impl ExpectedAbsence {
    pub fn capture(path: impl AsRef<Path>) -> Result<Self, ProvenanceError> {
        let expected = Self {
            path: path.as_ref().to_path_buf(),
        };
        expected.verify()?;
        Ok(expected)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(&self) -> Result<(), ProvenanceError> {
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ProvenanceError::Io {
                context: format!("failed to inspect expected input: {}", self.path.display()),
                source,
            }),
            Ok(_) => Err(ProvenanceError::InputContentChanged(format!(
                "Expected absent input appeared before gap publication: {:?}",
                self.path
            ))),
        }
    }
}

/// Exact content and decoder identity for one input locator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRevision {
    pub input_kind: String,
    pub locator: String,
    pub content_fingerprint: String,
    pub decoder_fingerprint: String,
    pub fingerprint: String,
}

impl InputRevision {
    pub fn create(
        input_kind: impl Into<String>,
        locator: impl Into<String>,
        content_fingerprint: impl Into<String>,
        decoder_fingerprint: impl Into<String>,
    ) -> Result<Self, ProvenanceError> {
        let input_kind = input_kind.into();
        let locator = locator.into();
        let content_fingerprint = content_fingerprint.into();
        let decoder_fingerprint = decoder_fingerprint.into();
        let revision_fingerprint = fingerprint(&json!({
            "version": 1,
            "input_kind": input_kind,
            "locator": locator,
            "content_fingerprint": content_fingerprint,
            "decoder_fingerprint": decoder_fingerprint,
        }))?;
        Ok(Self {
            input_kind,
            locator,
            content_fingerprint,
            decoder_fingerprint,
            fingerprint: revision_fingerprint,
        })
    }
}

/// Fingerprint validated CSV decoding semantics, excluding discovery-only settings.
pub fn csv_decoder_fingerprint(config: &CsvSourceConfig) -> Result<String, ProvenanceError> {
    fingerprint(&json!({
        "version": CSV_DECODER_VERSION,
        "kind": "csv",
        "config": {
            "delimiter": char::from(config.delimiter).to_string(),
            "has_header": config.has_header,
            "timestamp_format": config.timestamp_format,
            "datetime_format": config.datetime_format,
            "timestamp_timezone": config.timestamp_timezone,
            "fieldnames": config.fieldnames,
            "columns": config.columns,
            "protocol_map": config.protocol_map,
            "source_id_value": config.source_id_value,
            "source_id_column": config.source_id_column,
            "skip_bad_column_count": config.skip_bad_column_count,
            "archive_member_contains": config.archive_member_contains,
        },
    }))
}

/// Fingerprint the fixed nfdump CSV reducer contract.
pub fn nfcapd_decoder_fingerprint() -> Result<String, ProvenanceError> {
    fingerprint(&json!({
        "version": NFCAPD_DECODER_VERSION,
        "kind": "nfcapd-compiled-csv-reducer",
        "reducer_contract_version": NFDUMP_REDUCER_CONTRACT_VERSION,
        "input_contract": NFDUMP_REDUCER_INPUT_CONTRACT,
        "output_contract": NFDUMP_REDUCER_OUTPUT_CONTRACT,
        "ttl_missing_semantics": "zero-or-blank",
        "fields": [
            "timestamps", "addresses", "ports", "protocol", "packets", "bytes", "tos",
            "flow-count", "min-ttl", "max-ttl"
        ],
    }))
}

pub fn capture_csv_input_revision(
    path: impl AsRef<Path>,
    config: &CsvSourceConfig,
) -> Result<(InputRevision, FileSnapshot), ProvenanceError> {
    capture_input_revision(path, "csv", csv_decoder_fingerprint(config)?)
}

pub fn capture_nfcapd_input_revision(
    path: impl AsRef<Path>,
) -> Result<(InputRevision, FileSnapshot), ProvenanceError> {
    capture_input_revision(path, "nfcapd", nfcapd_decoder_fingerprint()?)
}

fn capture_input_revision(
    path: impl AsRef<Path>,
    input_kind: &str,
    decoder_fingerprint: String,
) -> Result<(InputRevision, FileSnapshot), ProvenanceError> {
    let path = path.as_ref();
    let locator = path.to_string_lossy().into_owned();
    let (content_fingerprint, snapshot) = capture_file_revision(path)?;
    let revision = InputRevision::create(
        input_kind,
        locator,
        content_fingerprint,
        decoder_fingerprint,
    )?;
    Ok((revision, snapshot))
}

pub fn gap_input_revision(
    input_kind: &str,
    locator: &str,
) -> Result<InputRevision, ProvenanceError> {
    InputRevision::create(
        input_kind,
        locator,
        fingerprint(&json!({"version": 1, "kind": "empty-gap"}))?,
        fingerprint(&json!({
            "version": GAP_DECODER_VERSION,
            "kind": format!("{input_kind}-gap")
        }))?,
    )
}

pub fn revision_for_locator(
    revision: &InputRevision,
    locator: &str,
) -> Result<InputRevision, ProvenanceError> {
    if locator == revision.locator {
        return Ok(revision.clone());
    }
    InputRevision::create(
        &revision.input_kind,
        locator,
        &revision.content_fingerprint,
        &revision.decoder_fingerprint,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn canonical_json_matches_python_sorting_and_ascii_escaping() {
        let value = json!({"z": "café 🌊", "a": [true, null, "\n"]});

        assert_eq!(
            canonical_json(&value).unwrap(),
            r#"{"a":[true,null,"\n"],"z":"caf\u00e9 \ud83c\udf0a"}"#
        );
        assert_eq!(
            fingerprint(&value).unwrap(),
            "39643851f0cfe41833461b7d3ae782dd74d57a24f66450bfd9a088179625a9c9"
        );
    }

    #[test]
    fn file_revision_hashes_exact_bytes_and_detects_later_changes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("flows.csv");
        fs::write(&path, b"one").unwrap();

        let (digest, snapshot) = capture_file_revision(&path).unwrap();
        assert_eq!(
            digest,
            "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed"
        );
        verify_file_snapshot(&path, &snapshot).unwrap();

        fs::write(&path, b"replacement").unwrap();
        assert!(matches!(
            verify_file_snapshot(&path, &snapshot),
            Err(ProvenanceError::InputContentChanged(_))
        ));
    }

    #[test]
    fn expected_absence_rejects_a_new_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("missing");
        let absence = ExpectedAbsence::capture(&path).unwrap();

        fs::write(&path, b"now present").unwrap();

        assert!(matches!(
            absence.verify(),
            Err(ProvenanceError::InputContentChanged(_))
        ));
    }

    #[test]
    fn revision_fingerprint_matches_python_contract() {
        let revision =
            InputRevision::create("csv", "/csv/input.csv", "content", "decoder").unwrap();

        assert_eq!(
            revision.fingerprint,
            "61f221cd6f20d114a5dec276232e9cf5a7528ef805a7c63ffb73aa6521e53dba"
        );
        assert_eq!(
            revision_for_locator(&revision, "/csv/input.csv").unwrap(),
            revision
        );
        assert_ne!(
            revision_for_locator(&revision, "archive://member.csv")
                .unwrap()
                .fingerprint,
            revision.fingerprint
        );
    }

    #[test]
    fn gap_revision_is_stable_and_kind_specific() {
        let first = gap_input_revision("nfcapd", "gap://nfcapd/r1/0").unwrap();
        let second = gap_input_revision("nfcapd", "gap://nfcapd/r1/0").unwrap();
        let csv = gap_input_revision("csv", "gap://nfcapd/r1/0").unwrap();

        assert_eq!(first, second);
        assert_ne!(first.decoder_fingerprint, csv.decoder_fingerprint);
        assert_eq!(
            nfcapd_decoder_fingerprint().unwrap(),
            "495da8d9c808642b6c82b9b74dfc53746e7d368865db7d0035704933e90cac17"
        );
        assert_eq!(
            first.fingerprint,
            "3503016260940c1b05347142ac039f665f8eb2ae603743022ace10f34e0b3696"
        );
    }
}
